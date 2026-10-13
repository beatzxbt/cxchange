//! Replay deterministic JSON command traces through the native matching core.
//!
//! Read a trace from stdin or a file argument. Write complete command outcomes
//! and a final versioned snapshot to stdout. Optional restore during execution
//! exercises snapshot continuation without exposing private arena handles.
//!
//! Usage: `cxchange-replay [trace.json]`. Commands use the library's tagged
//! Serde representation. Matching rejections are ordinary outcomes with error
//! codes; malformed input, failed restore, and invariant failures exit nonzero.
//! Results are buffered until all commands and optional serialized recovery
//! succeed. This diagnostic tool allocates copies and is not a latency harness.

use matching_core::{
    Command, EngineConfig, EngineId, EngineSnapshot, InstrumentSpec, MatchingEngine,
    MatchingResponse,
};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::io::{self, Read};

/// Input trace using the same command representations as the public library.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Trace {
    /// Nonzero identity domain for every order issued in this trace.
    engine_id: EngineId,
    /// Exact instrument units and command size bounds.
    spec: InstrumentSpec,
    /// Logical limits; omitted values use the library's baseline defaults.
    #[serde(default)]
    config: EngineConfig,
    /// Commands applied in their supplied order, including rejected commands.
    commands: Vec<Command>,
    /// Restore after this many commands, including zero for an initial restore.
    #[serde(default)]
    roundtrip_after: Option<usize>,
}

/// One complete outcome; a rejection contains a stable machine-readable code.
#[derive(Serialize)]
struct Outcome {
    /// Complete result on success; absent on matching rejection.
    response: Option<MatchingResponse>,
    /// Stable rejection code; absent on success.
    error: Option<&'static str>,
}

/// Direct serialization preserves aggregate integers wider than JSON Value's u64 storage.
#[derive(Serialize)]
struct ReplayOutput {
    /// One success or rejection for each input command, in order.
    results: Vec<Outcome>,
    /// State after the last command and optional restore boundary.
    snapshot: EngineSnapshot,
}

/// Execute a trace and emit JSON, returning input/system failures to the caller.
fn run() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() > 1 {
        return Err("usage: cxchange-replay [trace.json]".into());
    }
    let input = if let Some(path) = arguments.first() {
        std::fs::read_to_string(path)?
    } else {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        input
    };
    let trace: Trace = serde_json::from_str(&input)?;
    if trace
        .roundtrip_after
        .is_some_and(|index| index > trace.commands.len())
    {
        return Err("roundtrip_after exceeds command count".into());
    }
    let mut engine = MatchingEngine::new(trace.engine_id, trace.spec, trace.config)?;
    let mut response = MatchingResponse::with_capacity(&trace.config)?;
    if trace.roundtrip_after == Some(0) {
        let encoded = serde_json::to_vec(&engine.export_snapshot())?;
        engine = MatchingEngine::from_snapshot(serde_json::from_slice(&encoded)?)?;
    }
    let mut results = Vec::with_capacity(trace.commands.len());
    for (index, command) in trace.commands.iter().enumerate() {
        let outcome = match engine.execute_into(command, &mut response) {
            Ok(()) => Outcome {
                response: Some(response.clone()),
                error: None,
            },
            Err(error) => Outcome {
                response: None,
                error: Some(error.code()),
            },
        };
        engine.validate_invariants()?;
        results.push(outcome);
        if trace.roundtrip_after == Some(index + 1) {
            let encoded = serde_json::to_vec(&engine.export_snapshot())?;
            engine = MatchingEngine::from_snapshot(serde_json::from_slice(&encoded)?)?;
        }
    }
    serde_json::to_writer(
        io::stdout().lock(),
        &ReplayOutput {
            results,
            snapshot: engine.export_snapshot(),
        },
    )?;
    println!();
    Ok(())
}

/// Reports input/execution failures on stderr and returns a nonzero exit status.
///
/// Validation and execution finish before JSON emission; an output I/O failure
/// can still interrupt serialization after bytes have been written.
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cxchange-replay: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
