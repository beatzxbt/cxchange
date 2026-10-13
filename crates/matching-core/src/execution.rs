//! Bounded, single-owner command execution and reusable response delivery.
//!
//! `ExecutionAdapter` reserves an output credit before admitting each command,
//! processes admitted commands in FIFO order, and returns credits explicitly
//! after consumers finish reading completed responses.
//!
//! The adapter is a synchronous pump: the caller admits commands, calls
//! [`ExecutionAdapter::process_next`], takes results, then recycles them. It
//! starts no thread and does not wait for scheduled timestamps. Queue pressure
//! is an admission outcome; a matching rejection is a completed result and
//! holds its credit until recycling just like a successful command.
//!
//! Credits transition `Free -> Queued -> Completed -> Leased -> Free`. Their
//! fixed total bounds queued, completed, and consumer-held results together,
//! so execution always has room to record an admitted command's completion.

use crate::{Command, MatchingEngine, MatchingError, MatchingResponse};
use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Monotonic identity assigned to one admitted command.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SubmissionId(pub u64);

/// Fixed capacities for the command queue and reusable response pool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionConfig {
    /// Maximum number of commands waiting for execution.
    pub command_capacity: usize,
    /// Number of response buffers reserved before adapter startup completes.
    pub response_capacity: usize,
}

/// Startup failure while reserving bounded adapter storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionInitError {
    /// A queue or pool has zero capacity.
    InvalidConfig,
    /// The underlying engine could not allocate a complete response buffer.
    Matching(MatchingError),
    /// The process could not reserve one of the bounded queues or pool slots.
    AllocationFailed,
}

impl fmt::Display for ExecutionInitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("execution capacities must be positive"),
            Self::Matching(error) => {
                write!(formatter, "could not initialize response storage: {error}")
            }
            Self::AllocationFailed => {
                formatter.write_str("could not reserve bounded execution storage")
            }
        }
    }
}

impl std::error::Error for ExecutionInitError {}

/// Admission failure that does not invoke the matching engine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    /// The bounded command queue is full.
    CommandQueueFull,
    /// No preallocated response buffer is currently available.
    ResponsePoolExhausted,
    /// The monotonic submission identity cannot advance without wrapping.
    SubmissionIdExhausted,
}

impl fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommandQueueFull => formatter.write_str("command queue is full"),
            Self::ResponsePoolExhausted => formatter.write_str("response pool is exhausted"),
            Self::SubmissionIdExhausted => {
                formatter.write_str("submission ID counter is exhausted")
            }
        }
    }
}

impl std::error::Error for AdmissionError {}

/// Timings collected from the scheduled arrival through result delivery.
///
/// Delivery means the consumer calls `take_completed`, not that a network peer
/// receives the event. Scheduled intervals include producer/admission delay;
/// queue wait starts only at admission. Durations saturate at zero when an
/// explicitly supplied scheduled timestamp lies in the future.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExecutionTimings {
    /// Time from adapter admission until execution begins.
    pub queue_wait: Duration,
    /// Time spent inside `MatchingEngine::execute_into`.
    pub execution: Duration,
    /// Time from execution completion until the consumer takes the result.
    pub delivery_wait: Duration,
    /// Time from the original scheduled arrival until execution begins.
    pub scheduled_to_start: Duration,
    /// Time from the original scheduled arrival until the consumer takes the result.
    pub scheduled_to_delivery: Duration,
}

/// An owned completed result and the response-pool credit that produced it.
///
/// The response remains owned by this value until it is passed back to the
/// originating adapter with [`ExecutionAdapter::recycle`]. Dropping an
/// unrecycled result permanently removes that credit from the adapter, so
/// future admissions may fail closed when the remaining pool is exhausted.
#[must_use = "recycle completed responses to return their bounded pool credit"]
#[derive(Debug)]
pub struct CompletedResponse {
    /// Admission identity retained independently of matching order IDs.
    submission_id: SubmissionId,
    /// Matching success or rejection recorded for the admitted command.
    outcome: Result<(), MatchingError>,
    /// Complete payload whose allocations must be recycled to the pool.
    response: MatchingResponse,
    /// Intervals finalized when the consumer takes this result.
    timings: ExecutionTimings,
    /// Originating credit index, meaningful only together with `owner`.
    slot_index: usize,
    /// Pool identity proving which adapter may accept this credit back.
    owner: Arc<PoolOwner>,
}

impl CompletedResponse {
    /// Returns the identity assigned when this command was admitted.
    pub const fn submission_id(&self) -> SubmissionId {
        self.submission_id
    }

    /// Returns the matching outcome, which may be a canonical trading rejection.
    pub const fn outcome(&self) -> Result<(), MatchingError> {
        self.outcome
    }

    /// Returns the complete response data for this command.
    pub const fn response(&self) -> &MatchingResponse {
        &self.response
    }

    /// Returns the measured queue, execution, and delivery intervals.
    pub const fn timings(&self) -> ExecutionTimings {
        self.timings
    }
}

/// One-owner adapter that executes accepted commands in FIFO order.
///
/// Every admitted command owns one response credit from admission through
/// explicit recycling. Queue and event storage are reserved during startup, so
/// reaching configured limits produces an admission error instead of silently
/// dropping an accepted command or growing the queues.
/// The underlying engine's ordered maps can still allocate during matching.
pub struct ExecutionAdapter {
    /// Exclusive mutable matching state; consumers receive only shared queries.
    engine: MatchingEngine,
    /// Admission limits independent of allocator-provided vector capacity.
    config: ExecutionConfig,
    /// FIFO of admitted commands, each owning a reserved response slot.
    commands: VecDeque<QueuedCommand>,
    /// FIFO of completion metadata; payloads remain in their reserved slots.
    completed: VecDeque<CompletedRecord>,
    /// Fixed credit ledger whose buffers leave only while consumer-leased.
    slots: Vec<ResponseSlot>,
    /// Reusable slot indices; a credit appears here only in the free state.
    free_slots: Vec<usize>,
    /// Next unused admission identity; failed admission never advances it.
    next_submission_id: u64,
    /// Allocation identity used to reject recycling from another adapter.
    owner: Arc<PoolOwner>,
}

impl ExecutionAdapter {
    /// Creates an adapter and reserves all bounded queues and response buffers.
    ///
    /// # Arguments
    ///
    /// * `engine` - The single-owner matching engine driven by this adapter.
    /// * `config` - Positive command-queue and response-pool capacities.
    ///
    /// # Returns
    ///
    /// A ready adapter, or a startup error if a capacity is invalid or storage
    /// cannot be reserved.
    ///
    /// Each response credit reserves a complete maximum command result for the
    /// engine's configuration. Memory therefore scales with response capacity
    /// times the configured sweep size, independently of typical result size.
    pub fn new(
        engine: MatchingEngine,
        config: ExecutionConfig,
    ) -> Result<Self, ExecutionInitError> {
        if config.command_capacity == 0 || config.response_capacity == 0 {
            return Err(ExecutionInitError::InvalidConfig);
        }

        let mut commands = VecDeque::new();
        commands
            .try_reserve_exact(config.command_capacity)
            .map_err(|_| ExecutionInitError::AllocationFailed)?;
        let mut completed = VecDeque::new();
        completed
            .try_reserve_exact(config.response_capacity)
            .map_err(|_| ExecutionInitError::AllocationFailed)?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(config.response_capacity)
            .map_err(|_| ExecutionInitError::AllocationFailed)?;
        let mut free_slots = Vec::new();
        free_slots
            .try_reserve_exact(config.response_capacity)
            .map_err(|_| ExecutionInitError::AllocationFailed)?;
        let engine_config = engine.config();

        for index in 0..config.response_capacity {
            let response = MatchingResponse::with_capacity(&engine_config)
                .map_err(ExecutionInitError::Matching)?;
            slots.push(ResponseSlot {
                response: Some(response),
                outcome: None,
                state: SlotState::Free,
            });
            free_slots.push(index);
        }

        Ok(Self {
            engine,
            config,
            commands,
            completed,
            slots,
            free_slots,
            next_submission_id: 1,
            owner: Arc::new(PoolOwner),
        })
    }

    /// Admits a command using the current time as its scheduled arrival.
    ///
    /// Admission reserves both a queue entry and a response credit before
    /// assigning an ID. Failure does not call the engine or change its book.
    ///
    /// # Arguments
    ///
    /// * `command` - Command to enqueue for FIFO execution.
    ///
    /// # Returns
    ///
    /// The monotonic submission ID, or an [`AdmissionError`] when bounded
    /// storage is unavailable.
    pub fn try_submit(&mut self, command: Command) -> Result<SubmissionId, AdmissionError> {
        self.try_submit_at(command, Instant::now())
    }

    /// Admits a command while preserving its independently scheduled arrival.
    ///
    /// This form is intended for open-loop producers and latency harnesses.
    /// A scheduled time may be earlier or later than the call time; it records
    /// measurement metadata and does not defer execution or reorder the FIFO.
    ///
    /// # Arguments
    ///
    /// * `command` - Command to enqueue for FIFO execution.
    /// * `scheduled_at` - Original arrival time used for scheduling metrics.
    ///
    /// # Returns
    ///
    /// The monotonic submission ID, or an [`AdmissionError`] when bounded
    /// storage is unavailable.
    pub fn try_submit_at(
        &mut self,
        command: Command,
        scheduled_at: Instant,
    ) -> Result<SubmissionId, AdmissionError> {
        if self.commands.len() == self.config.command_capacity {
            return Err(AdmissionError::CommandQueueFull);
        }
        if self.free_slots.is_empty() {
            return Err(AdmissionError::ResponsePoolExhausted);
        }
        let next_submission_id = self
            .next_submission_id
            .checked_add(1)
            .ok_or(AdmissionError::SubmissionIdExhausted)?;

        let slot_index = self.free_slots.pop().expect("checked free response credit");
        debug_assert_eq!(self.slots[slot_index].state, SlotState::Free);
        self.slots[slot_index].state = SlotState::Queued;
        let submission_id = SubmissionId(self.next_submission_id);
        self.commands.push_back(QueuedCommand {
            submission_id,
            command,
            slot_index,
            scheduled_at,
            admitted_at: Instant::now(),
        });
        self.next_submission_id = next_submission_id;
        Ok(submission_id)
    }

    /// Executes the oldest admitted command and records its completion.
    ///
    /// A trading rejection is recorded as a completed matching outcome; it is
    /// distinct from admission failure and never removes a later queued command.
    /// This call synchronously runs the entire command, including a legal full
    /// sweep. It neither truncates work nor releases the response credit.
    ///
    /// # Returns
    ///
    /// The processed submission ID, or `None` when the command queue is empty.
    pub fn process_next(&mut self) -> Option<SubmissionId> {
        let queued = self.commands.pop_front()?;
        let started_at = Instant::now();
        let slot = &mut self.slots[queued.slot_index];
        debug_assert_eq!(slot.state, SlotState::Queued);
        let response = slot
            .response
            .as_mut()
            .expect("queued credit has a response buffer");
        let outcome = self.engine.execute_into(&queued.command, response);
        let completed_at = Instant::now();
        slot.outcome = Some(outcome);
        slot.state = SlotState::Completed;
        self.completed.push_back(CompletedRecord {
            submission_id: queued.submission_id,
            slot_index: queued.slot_index,
            scheduled_at: queued.scheduled_at,
            admitted_at: queued.admitted_at,
            started_at,
            completed_at,
        });
        Some(queued.submission_id)
    }

    /// Takes the oldest processed result, transferring its response credit.
    ///
    /// # Returns
    ///
    /// An owned result whose response storage must be returned with
    /// [`ExecutionAdapter::recycle`], or `None` when no result is ready.
    pub fn take_completed(&mut self) -> Option<CompletedResponse> {
        let record = self.completed.pop_front()?;
        let delivered_at = Instant::now();
        let slot = &mut self.slots[record.slot_index];
        debug_assert_eq!(slot.state, SlotState::Completed);
        slot.state = SlotState::Leased;
        Some(CompletedResponse {
            submission_id: record.submission_id,
            outcome: slot
                .outcome
                .take()
                .expect("completed credit has an outcome"),
            response: slot
                .response
                .take()
                .expect("completed credit has a response buffer"),
            timings: ExecutionTimings {
                queue_wait: record
                    .started_at
                    .saturating_duration_since(record.admitted_at),
                execution: record
                    .completed_at
                    .saturating_duration_since(record.started_at),
                delivery_wait: delivered_at.saturating_duration_since(record.completed_at),
                scheduled_to_start: record
                    .started_at
                    .saturating_duration_since(record.scheduled_at),
                scheduled_to_delivery: delivered_at.saturating_duration_since(record.scheduled_at),
            },
            slot_index: record.slot_index,
            owner: Arc::clone(&self.owner),
        })
    }

    /// Returns a completed response's reusable storage to this adapter.
    ///
    /// # Arguments
    ///
    /// * `completed` - Result previously taken from this adapter.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the response credit is restored, or the original value
    /// when it belongs to another adapter or its credit is not leased here.
    /// Recycling retains output allocations; the engine clears their used
    /// entries when this credit next executes a command.
    #[allow(clippy::result_large_err)]
    pub fn recycle(&mut self, completed: CompletedResponse) -> Result<(), CompletedResponse> {
        if !Arc::ptr_eq(&self.owner, &completed.owner)
            || completed.slot_index >= self.slots.len()
            || self.slots[completed.slot_index].state != SlotState::Leased
            || self.slots[completed.slot_index].response.is_some()
        {
            return Err(completed);
        }

        let CompletedResponse {
            response,
            slot_index,
            ..
        } = completed;
        let slot = &mut self.slots[slot_index];
        slot.response = Some(response);
        slot.outcome = None;
        slot.state = SlotState::Free;
        self.free_slots.push(slot_index);
        Ok(())
    }

    /// Returns the immutable matching engine owned by the adapter.
    pub const fn engine(&self) -> &MatchingEngine {
        &self.engine
    }

    /// Returns the configured queue and response-pool capacities.
    pub const fn config(&self) -> ExecutionConfig {
        self.config
    }

    /// Returns the number of commands waiting to be executed.
    pub fn queued_len(&self) -> usize {
        self.commands.len()
    }

    /// Returns the number of processed results waiting to be taken.
    pub fn completed_len(&self) -> usize {
        self.completed.len()
    }

    /// Returns the number of response credits available for new admissions.
    pub fn available_response_credits(&self) -> usize {
        self.free_slots.len()
    }
}

/// Credit lifecycle; only `Free` slots may be selected for admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SlotState {
    /// Buffer is adapter-owned and its index is in `free_slots`.
    Free,
    /// An admitted command owns the adapter-held buffer until it executes.
    Queued,
    /// Matching finished; payload and outcome await consumer delivery.
    Completed,
    /// A `CompletedResponse` owns the buffer outside the adapter.
    Leased,
}

/// One response credit's payload and lifecycle bookkeeping.
struct ResponseSlot {
    /// Present except while a consumer holds a leased response.
    response: Option<MatchingResponse>,
    /// Present only after execution and before `take_completed` transfers it.
    outcome: Option<Result<(), MatchingError>>,
    /// Lifecycle guard used for admission, delivery, and recycling assertions.
    state: SlotState,
}

/// Admitted work with a response reservation and original measurement timestamps.
struct QueuedCommand {
    /// Identity assigned only after admission succeeds.
    submission_id: SubmissionId,
    /// Complete command executed by the sole engine owner.
    command: Command,
    /// Reserved credit that guarantees complete output space.
    slot_index: usize,
    /// Producer-supplied arrival metadata; not an execution deadline.
    scheduled_at: Instant,
    /// Time the command actually entered this adapter.
    admitted_at: Instant,
}

/// FIFO delivery metadata referring to a completed slot's payload.
struct CompletedRecord {
    /// Original admission identity retained through delivery.
    submission_id: SubmissionId,
    /// Completed credit whose payload is still adapter-owned.
    slot_index: usize,
    /// Original producer-supplied arrival time.
    scheduled_at: Instant,
    /// Actual adapter admission time.
    admitted_at: Instant,
    /// Time immediately before invoking the matching engine.
    started_at: Instant,
    /// Time immediately after the matching engine returned.
    completed_at: Instant,
}

/// Unique pool allocation identity retained by every outstanding lease.
///
/// `Arc::ptr_eq` rejects foreign recycling even when slot indices coincide.
/// A lease keeps the token alive after its adapter is dropped, preventing a
/// later adapter from reusing that allocation identity while the lease exists.
#[derive(Debug)]
struct PoolOwner;
