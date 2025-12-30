# Polymarket Backend

> High-performance prediction market backend based on ZTDX trading engine architecture.

## Overview

This project aims to build a Polymarket-style prediction market platform with:
- High-performance order matching engine (~50,000 TPS)
- WebSocket real-time data streaming
- EIP-712 signature authentication
- Integration with CTF Exchange smart contracts

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Frontend / SDK                              │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                   Polymarket Backend                            │
├─────────────────────────────────────────────────────────────────┤
│  • Order Management                                             │
│  • Matching Engine (Lock-free DashMap)                         │
│  • WebSocket Real-time Push                                     │
│  • EIP-712 Authentication                                       │
│  • Redis Cache Layer                                            │
│  • Settlement Scheduler                                         │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                   Polygon Blockchain                            │
├─────────────────────────────────────────────────────────────────┤
│  • CTF Exchange (On-chain Settlement)                          │
│  • Conditional Tokens (ERC-1155)                               │
│  • UMA CTF Adapter (Oracle Resolution)                         │
└─────────────────────────────────────────────────────────────────┘
```

## Tech Stack

- **Language**: Rust 2021
- **Web Framework**: Axum 0.7
- **Async Runtime**: Tokio
- **Database**: PostgreSQL + TimescaleDB
- **Cache**: Redis
- **Authentication**: EIP-712 + JWT

## Project Structure

```
src/
├── main.rs              # Application entry point
├── api/                 # REST API handlers
│   ├── handlers/        # Request handlers
│   ├── middleware/      # Auth middleware
│   └── routes/          # Route definitions
├── auth/                # Authentication (EIP-712, JWT)
├── cache/               # Redis cache layer
├── config/              # Configuration
├── db/                  # Database connection
├── models/              # Data models
├── services/            # Business logic
│   ├── matching/        # Order matching engine
│   ├── market/          # Market management
│   └── settlement/      # On-chain settlement (TODO)
├── utils/               # Utilities
└── websocket/           # WebSocket handlers
```

## Development Status

### Completed (from ZTDX)
- [x] Order matching engine
- [x] Order book management
- [x] WebSocket real-time push
- [x] EIP-712 authentication
- [x] Redis cache layer
- [x] REST API framework

### TODO (Prediction Market Adaptation)
- [ ] Modify order model (remove leverage, add outcome_id)
- [ ] Adapt matching logic for Yes/No complementary pairs
- [ ] Implement symmetric fee calculation
- [ ] Add on-chain settlement integration (CTF Exchange)
- [ ] Integrate UMA Oracle for result resolution
- [ ] Create market management API
- [ ] Simplify position system to share-based

## Quick Start

### Prerequisites
- Rust 1.75+
- PostgreSQL 13+
- Redis 6+

### Setup

```bash
# Clone the repository
git clone https://github.com/leelee-echo/polymarket-backend.git
cd polymarket-backend

# Copy environment file
cp .env.example .env

# Edit .env with your configuration

# Run database migrations
sqlx migrate run

# Start the server
cargo run
```

### Environment Variables

```bash
# Database
DATABASE_URL=postgres://user:pass@localhost/polymarket

# Redis
REDIS_URL=redis://localhost:6379

# JWT
JWT_SECRET=your-secret-key
JWT_EXPIRY_SECONDS=86400

# Server
PORT=8080
RUST_LOG=polymarket_backend=info
```

## API Endpoints

### Public Endpoints
- `GET /markets` - List all markets
- `GET /markets/:symbol/orderbook` - Get order book
- `GET /markets/:symbol/trades` - Get recent trades

### Protected Endpoints (JWT Required)
- `POST /orders` - Create order
- `DELETE /orders/:id` - Cancel order
- `GET /account/orders` - Get user orders
- `GET /account/balances` - Get user balances

## Related Repositories

- [ctf-exchange](../refs/ctf-exchange) - On-chain exchange contracts
- [conditional-tokens-contracts](../refs/conditional-tokens-contracts) - ERC-1155 tokens
- [uma-ctf-adapter](../refs/uma-ctf-adapter) - Oracle adapter
- [clob-client](../refs/clob-client) - TypeScript client SDK

## License

MIT

## Credits

Based on [ZTDX Trading Engine](https://github.com/ztdx) architecture.
Inspired by [Polymarket](https://polymarket.com) prediction market protocol.
