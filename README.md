# CXchange WebSocket Server (aiohttp)

A high-performance WebSocket server built with **aiohttp** that serves as the entry and exit point for the CXchange cryptocurrency exchange. The server handles JSON message validation, authentication, rate limiting, and routing to the exchange's core systems with superior performance and scalability.

## Features

- **High-Performance aiohttp**: Built on aiohttp for superior performance and scalability
- **WebSocket Communication**: Real-time bidirectional communication with clients
- **HTTP Endpoints**: Built-in health checks and statistics endpoints
- **Message Validation**: Comprehensive validation of all incoming messages with predefined error codes
- **Rate Limiting**: Token bucket rate limiting per IP and per connection
- **Authentication**: API key/signature based authentication system
- **Connection Management**: Handles connection lifecycle, cleanup, and broadcasting
- **Error Handling**: Structured error responses using predefined error codes from `exceptions.py`
- **Market Data Subscriptions**: Support for subscribing to various market data channels
- **Order Management**: Support for placing, modifying, and canceling orders
- **CORS Support**: Built-in CORS support for web applications
- **Heartbeat**: Automatic WebSocket heartbeat for connection health

## Architecture

The WebSocket server consists of several key components:

- **`WebSocketServer`**: Main aiohttp-based server class that handles connections and message routing
- **`MessageValidator`**: Validates incoming JSON messages and enforces business rules
- **`MessageHandler`**: Processes validated messages and routes them to appropriate business logic
- **`ConnectionManager`**: Manages connections, rate limiting, and broadcasting capabilities

## Endpoints

### WebSocket Endpoint
- **`/ws`**: Main WebSocket endpoint for trading and market data

### HTTP Endpoints
- **`/health`**: Health check endpoint returning server status
- **`/stats`**: Connection and rate limiting statistics

## Message Types

The server supports the following message types:

### Authentication
```json
{
  "type": "auth",
  "timestamp": 1640995200000,
  "api_key": "your_api_key",
  "signature": "hmac_signature"
}
```

### Ping/Pong
```json
{
  "type": "ping",
  "timestamp": 1640995200000
}
```

### Subscriptions
```json
{
  "type": "subscribe",
  "timestamp": 1640995200000,
  "channels": ["orderbook", "trades", "ticker"]
}
```

### Order Operations
```json
{
  "type": "order_new",
  "timestamp": 1640995200000,
  "symbol": "BTC-USD",
  "side": "buy",
  "type": "limit",
  "size": 0.1,
  "price": 50000.0,
  "tif": "GTC",
  "reduce_only": false,
  "client_order_id": "my_order_123"
}
```

```json
{
  "type": "order_modify",
  "timestamp": 1640995200000,
  "order_id": "uuid-here",
  "price": 51000.0,
  "size": 0.2
}
```

```json
{
  "type": "order_cancel",
  "timestamp": 1640995200000,
  "order_id": "uuid-here"
}
```

## Error Codes

The server uses predefined error codes from `exceptions.py`:

- **0xxx**: General errors (critical)
  - `0000`: Unknown error
  - `0001`: Internal server error
  - `0002`: Service unavailable

- **1xxx**: Core API related errors
  - `1001`: Invalid API key-secret
  - `1002`: IP from unsupported region
  - `1003`: IP rate limits violated
  - `1004`: IP has been banned
  - `1005`: Account rate limits violated
  - `1006`: Account has been banned

- **2xxx**: Trading/business errors
  - `2002`: Invalid margin amount
  - `2007`: Invalid order ID
  - `2008`: Invalid order price
  - `2009`: Invalid order size
  - `2010`: Invalid order side
  - `2011`: Invalid order type
  - `2012`: Invalid order time in force
  - `2013`: Invalid order reduce only

## Installation

```bash
# Install uv (if not already installed)
curl -LsSf https://astral.sh/uv/install.sh | sh

# Install dependencies
uv sync --extra dev
```

## Development

```bash
make test        # Run tests
make format      # Format and lint code
make typecheck   # Run type checking
make sync        # Update dependencies
```

## Running the Server

```bash
uv run python -m src.cxchange.server.websocket_server
```

The server will start on `localhost:8765` by default with the following endpoints:
- WebSocket: `ws://localhost:8765/ws`
- Health: `http://localhost:8765/health`
- Stats: `http://localhost:8765/stats`

## Configuration

The server can be configured with the following parameters:

- `host`: Server host (default: "localhost")
- `port`: Server port (default: 8765)
- `max_connections`: Maximum concurrent connections (default: 1000)
- `message_rate_limit`: Messages per second per connection (default: 100)
- `recv_window_ms`: Message receive window in milliseconds (default: 5000)
- `enable_cors`: Enable CORS support (default: True)
- `heartbeat`: WebSocket heartbeat interval in seconds (default: 30)

## Usage Example

See `examples/websocket_client.py` for a complete example of how to connect and interact with the server.

```python
import asyncio
import aiohttp
from examples.websocket_client import ExchangeClient

async def main():
    client = ExchangeClient("ws://localhost:8765/ws")
    await client.connect()
    
    # Authenticate
    await client.authenticate("api_key", "api_secret")
    
    # Subscribe to market data
    await client.subscribe(["orderbook", "trades"])
    
    # Place an order
    await client.place_order(
        symbol="BTC-USD",
        side="buy", 
        order_type="limit",
        size=0.1,
        price=50000.0
    )
    
    await client.disconnect()

asyncio.run(main())
```

## Testing

Run the example client to test the server:

```bash
python examples/websocket_client.py
```

This will run:
1. HTTP endpoint tests (health and stats)
2. Normal trading session
3. Stress test to verify rate limiting
4. Concurrent connections test

## Performance Benefits of aiohttp

The migration to aiohttp provides several performance benefits:

- **Better Concurrency**: Superior handling of concurrent connections
- **Lower Memory Usage**: More efficient memory management
- **HTTP Integration**: Built-in HTTP server for REST endpoints
- **Middleware Support**: Extensible middleware system
- **Production Ready**: Battle-tested in high-load environments
- **Better Error Handling**: More robust error handling and recovery
- **Heartbeat Support**: Built-in WebSocket heartbeat mechanism

## Rate Limiting

The server implements token bucket rate limiting with the following features:

- **Per-connection limits**: Each WebSocket connection has its own rate limit
- **Per-IP limits**: All connections from the same IP share a rate limit
- **Configurable rates**: Rate limits can be adjusted based on requirements
- **Graceful degradation**: Rate-limited requests receive error responses rather than being dropped

## Security Features

- **IP-based region filtering**: Connections from unsupported regions are rejected
- **IP banning**: Ability to ban problematic IP addresses
- **Authentication required for trading**: Order operations require valid authentication
- **Message size limits**: Maximum message size to prevent abuse
- **Connection limits**: Maximum number of concurrent connections
- **CORS Protection**: Configurable CORS policies
- **Proxy Support**: Proper IP extraction from proxy headers

## Integration Points

The server is designed to integrate with other exchange components:

- **Matching Engine**: Order messages are routed to the matching engine
- **Risk Engine**: Risk checks are performed before order submission  
- **Account Management**: Authentication and account validation
- **Market Data**: Broadcasting of real-time market data to subscribers

## Development

To extend the server:

1. Add new message types in `MessageValidator`
2. Implement handlers in `MessageHandler`
3. Add corresponding error codes in `exceptions.py`
4. Update the client example with new functionality
5. Add new HTTP endpoints in the `create_app` method

## Production Considerations

For production deployment, consider:

- **Load balancing**: Multiple server instances behind a load balancer
- **Persistent storage**: Store rate limiting and ban data in Redis/database
- **Monitoring**: Add metrics and health checks (built-in `/health` endpoint)
- **SSL/TLS**: Use secure WebSocket connections (wss://) and HTTPS
- **Authentication**: Implement proper HMAC signature verification
- **Logging**: Structured logging for audit and debugging
- **Graceful shutdown**: Handle server restarts without dropping connections
- **Reverse Proxy**: Use nginx or similar for SSL termination and load balancing
- **Container Deployment**: Docker/Kubernetes ready architecture

## Monitoring

The server provides built-in monitoring endpoints:

- **Health Check**: `GET /health` - Server health and basic metrics
- **Statistics**: `GET /stats` - Detailed connection and rate limiting statistics

Example health response:
```json
{
  "status": "healthy",
  "timestamp": 1640995200000,
  "active_connections": 42,
  "server_info": {
    "host": "localhost",
    "port": 8765,
    "max_connections": 1000,
    "message_rate_limit": 100
  }
}
```

## License

This project is part of the CXchange cryptocurrency exchange system.

