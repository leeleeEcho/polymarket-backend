# Polymarket Backend Development Log

## Project Overview

Refactoring ZTDX (futures trading exchange) backend to support Polymarket-style prediction markets.

### Key Changes from Futures to Prediction Markets

| Futures Concept | Prediction Market Concept |
|----------------|---------------------------|
| Symbol (BTCUSDT) | market_id + outcome_id + share_type |
| Leverage | N/A (removed) |
| Long/Short Positions | Yes/No Shares |
| Mark Price | Probability (0.01 - 0.99) |
| Funding Rate | N/A (removed) |
| Liquidation | N/A (removed) |

---

## Phase 1: Order Model Refactoring (Completed)

**Date:** 2024-12-30

### Changes
- Replaced `symbol` with `market_id`, `outcome_id`, `share_type`
- Removed `leverage` from orders
- Added `ShareType` enum (Yes/No)
- Added `MatchType` enum (Normal/Mint/Merge)
- Updated order table schema
- Added symmetric fee calculation: `fee = base_rate × min(price, 1-price) × amount`

### Files Modified
- `src/models/order.rs`
- `src/models/market.rs`
- `src/services/matching/types.rs`
- `migrations/0013_prediction_market_types.sql`
- `migrations/0014_markets_table.sql`
- `migrations/0015_modify_orders_table.sql`

### Tests
- 61 tests passing

---

## Phase 2: Mint/Merge Matching Logic (Completed)

**Date:** 2024-12-30
**Commit:** `126f791`

### Implementation

#### Mint Matching
When two buy orders for complementary shares exist and `P_yes + P_no >= 1.0`:
- Creates new share pairs from collateral
- Both buyers receive their respective shares
- Market maker effectively creates liquidity

#### Merge Matching
When two sell orders for complementary shares exist and `P_yes + P_no <= 1.0`:
- Redeems share pairs back to collateral
- Both sellers receive USDC
- Shares are burned

### Key Functions Added
```rust
// engine.rs
fn parse_market_key(market_key: &str) -> Option<(Uuid, Uuid, ShareType)>
fn get_complement_market_key(market_key: &str) -> Option<String>
fn get_or_create_complement_orderbook(&self, market_key: &str) -> Option<Arc<Orderbook>>
fn try_mint_match(...) -> (Vec<TradeExecution>, Decimal)
fn try_merge_match(...) -> (Vec<TradeExecution>, Decimal)

// orderbook.rs
pub fn get_matching_buy_orders(&self, min_price: Decimal) -> Vec<OrderEntry>
pub fn get_matching_sell_orders(&self, max_price: Decimal) -> Vec<OrderEntry>
pub fn fill_order(&self, order_id: Uuid, fill_amount: Decimal) -> bool
```

### Tests Added
- `test_mint_matching`
- `test_merge_matching`
- `test_mint_not_triggered_when_prices_too_low`

### Tests
- 64 tests passing

---

## Phase 3: Code Cleanup (Completed)

**Date:** 2024-12-30
**Commit:** `793f002`

### Changes
- Fixed `unused_must_use` warnings in engine.rs
- Added `#[allow(dead_code)]` for future-use code
- Fixed unused variable warnings (prefixed with `_`)
- Cleaned up unused imports

---

## Phase 4: Market Management API (Completed)

**Date:** 2024-12-30
**Commit:** `1a1a7b7`

### New Endpoints

#### Public
| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/markets/:market_id` | Get single market details |

#### Admin (Auth Required)
| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/admin/markets` | Create new prediction market |
| POST | `/admin/markets/:market_id/close` | Pause trading (status: paused) |
| POST | `/admin/markets/:market_id/resolve` | Set winning outcome (status: resolved) |
| POST | `/admin/markets/:market_id/cancel` | Cancel market (status: cancelled) |

### Database Migration
- `0017_add_market_category.sql`
  - Added `category` column to markets
  - Added `volume_24h`, `total_volume` columns
  - Added `probability` column to outcomes

### Market Status Transitions
```
active -> paused -> resolved
active -> paused -> cancelled
active -> resolved (direct resolution)
active -> cancelled (direct cancel)
```

---

## Phase 5: Share Holdings Management (Completed)

**Date:** 2024-12-30
**Commit:** `6d82465`

### New Endpoint

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/account/shares` | Get user's share holdings with PnL |

### Query Parameters
- `market_id` (optional) - Filter by specific market
- `active_only` (optional, default: true) - Only show non-zero positions

### Response Fields
```json
{
  "shares": [
    {
      "id": "uuid",
      "market_id": "uuid",
      "outcome_id": "uuid",
      "share_type": "yes",
      "amount": "100.0",
      "avg_cost": "0.55",
      "current_price": "0.60",
      "unrealized_pnl": "5.0",
      "market_question": "Will BTC reach $100k?",
      "outcome_name": "Yes"
    }
  ],
  "total_value": "60.0",
  "total_cost": "55.0",
  "total_unrealized_pnl": "5.0"
}
```

---

## Phase 6: WebSocket Enhancements (Completed)

**Date:** 2024-12-30
**Commit:** `ba94bcc`

### New Message Types

```rust
// Prediction market trade
MarketTrade {
    id, market_id, outcome_id, share_type,
    match_type, price, amount, side, timestamp
}

// Prediction market orderbook
MarketOrderbook {
    market_id, outcome_id, share_type,
    bids, asks, timestamp
}

// Market status update
MarketUpdate {
    market_id, status, yes_price, no_price,
    volume_24h, timestamp
}

// User share position update
ShareUpdate {
    market_id, outcome_id, share_type,
    amount, avg_cost, unrealized_pnl, event
}
```

### Channel Subscriptions

| Channel | Description |
|---------|-------------|
| `trades:{market_id}` | All trades for a market |
| `orderbook:{market_id}` | All orderbooks for a market |
| `orderbook:{market_id}:{outcome_id}:{share_type}` | Specific orderbook |
| `market:{market_id}` | All updates for a market |
| `trades:*` | All trades (wildcard) |
| `orderbook:*` | All orderbooks (wildcard) |

### Match Type in Trade Events
- `normal` - Standard buy/sell matching
- `mint` - New shares created from collateral
- `merge` - Shares redeemed back to collateral

---

## Phase 7: Settlement Logic (Completed)

**Date:** 2024-12-30

### Overview
Implemented settlement logic for resolved and cancelled markets:
- **Resolved Markets**: Winning share holders receive 1 USDC per share
- **Cancelled Markets**: All share holders receive refunds at cost basis

### New Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/account/settle/:market_id` | Settle user's shares for a resolved/cancelled market |
| GET | `/account/settle/:market_id/status` | Get settlement status and potential payout |

### Settlement Service (`src/services/settlement.rs`)

```rust
// Core functions
SettlementService::settle_user_shares(pool, market_id, user_address)
SettlementService::get_settlement_status(pool, market_id, user_address)

// Settlement types
SettlementType::Resolution  // Market resolved with winning outcome
SettlementType::Cancellation // Market was cancelled
```

### Settlement Logic

#### Resolved Markets
- YES shares for winning outcome: 1.0 USDC per share
- NO shares for losing outcome: 1.0 USDC per share (inverse win)
- Losing shares: 0 USDC (worthless)

#### Cancelled Markets
- All shares refunded at `avg_cost` price
- `payout = amount × avg_cost`

### Database Changes
- Uses `share_changes` table with `change_type = 'redeem'` for audit trail
- Credits USDC to user's `balances` table

### Response Format
```json
{
  "market_id": "uuid",
  "settlement_type": "resolution",
  "shares_settled": [
    {
      "outcome_id": "uuid",
      "share_type": "yes",
      "amount": "100.0",
      "payout_per_share": "1.0",
      "total_payout": "100.0"
    }
  ],
  "total_payout": "100.0",
  "message": "成功结算 100.0 USDC"
}
```

### Error Handling
- `MARKET_NOT_FOUND` - Market doesn't exist
- `MARKET_NOT_SETTLEABLE` - Market not resolved or cancelled
- `NO_WINNING_OUTCOME` - Resolved market missing winning outcome
- `NO_SHARES` - User has no shares in market
- `ALREADY_SETTLED` - Shares already settled

### Tests
- 65 tests passing (added `test_settlement_type_equality`)

---

## Test Summary

| Phase | Tests |
|-------|-------|
| Phase 1 | 61 |
| Phase 2 | 64 |
| Phase 6 | 64 |
| Phase 7 | 65 |

All tests passing.

---

## Next Steps (TODO)

1. **Admin Middleware** - Add proper admin role checking
2. **Price Oracle Integration** - Update probabilities from external sources
3. **Performance Optimization** - Add caching for frequently accessed data
4. **Monitoring** - Add metrics and alerting

---

## Architecture

```
API Handler
  ↓
OrderFlowOrchestrator
  ├→ MatchingEngine (in-memory matching)
  │    └→ Orderbook (per market:outcome:share_type)
  ├→ HistoryManager (in-memory history)
  └→ Database (async persistence)
```

### Market Key Format
```
{market_id}:{outcome_id}:{share_type}
```
Example: `550e8400-e29b-41d4-a716-446655440000:660e8400-e29b-41d4-a716-446655440001:yes`
