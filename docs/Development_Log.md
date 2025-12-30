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
| Phase 8 | 65 |
| Phase 9 | 67 |
| Phase 10 | 76 |
| Phase 11 | 79 |

All tests passing.

---

## Phase 8: Admin Middleware (Completed)

**Date:** 2024-12-30

### Overview
添加管理员中间件，用于保护管理员端点，确保只有管理员角色的用户才能访问。

### 数据库迁移
- `0018_add_user_role.sql`
  - 创建 `user_role` 枚举类型: `user`, `admin`, `superadmin`
  - 添加 `role` 字段到 `users` 表
  - 添加 `username` 和 `avatar_url` 字段

### 用户角色
```rust
pub enum UserRole {
    User,       // 普通用户
    Admin,      // 管理员
    SuperAdmin, // 超级管理员
}
```

### 中间件链
```
请求 → auth_middleware (验证JWT) → admin_middleware (检查角色) → Handler
```

### 开发模式支持
- `X-Test-Address`: 测试用户地址
- `X-Test-Role`: 测试用户角色 (user/admin/superadmin)

### 错误响应
- `401 Unauthorized` - 未登录或 token 无效
- `403 Forbidden` - 用户没有管理员权限

### 受保护的管理员端点
| Method | Endpoint | 描述 |
|--------|----------|------|
| POST | `/admin/markets` | 创建市场 |
| POST | `/admin/markets/:market_id/close` | 暂停交易 |
| POST | `/admin/markets/:market_id/resolve` | 结算市场 |
| POST | `/admin/markets/:market_id/cancel` | 取消市场 |

### 修改的文件
- `src/auth/middleware.rs` - 添加 `UserRole`, `admin_middleware`, `fetch_user_role`
- `src/api/routes/mod.rs` - 更新管理员路由使用中间件
- `migrations/0018_add_user_role.sql` - 添加用户角色

---

## Phase 9: Price Oracle (Completed)

**Date:** 2024-12-30

### Overview
实现价格预言机服务，支持多种来源更新市场概率：
- **Orderbook**: 从订单簿计算加权中间价
- **External**: 外部预言机（Chainlink, UMA, Pyth）- 预留接口
- **Manual**: 管理员手动设置
- **Trade**: 交易执行后自动更新

### 新增文件
- `src/services/oracle.rs` - Price Oracle 服务

### 新增 API 端点

| Method | Endpoint | 描述 |
|--------|----------|------|
| POST | `/admin/markets/:market_id/probability` | 手动设置概率 |
| POST | `/admin/markets/:market_id/refresh-probability` | 从数据源刷新概率 |

### 核心功能

```rust
// PriceOracle 服务
PriceOracle::new(pool, matching_engine)
PriceOracle::update_from_orderbook(market_id, outcome_id)  // 从订单簿更新
PriceOracle::update_from_trade(market_id, outcome_id, price)  // 从交易更新
PriceOracle::set_probability_manual(market_id, outcome_id, prob)  // 手动设置
PriceOracle::fetch_from_external(market_id, oracle_name)  // 外部预言机
PriceOracle::refresh_all_from_orderbook()  // 批量刷新所有市场
```

### 概率计算逻辑

**从订单簿计算:**
```
weighted_mid = (best_bid × ask_volume + best_ask × bid_volume) / (bid_volume + ask_volume)
```

**概率范围:** 0.01 - 0.99 (自动裁剪)

### 请求/响应格式

**手动更新概率:**
```json
POST /admin/markets/:market_id/probability
{
    "outcome_id": "uuid",
    "probability": 0.65
}

Response:
{
    "market_id": "uuid",
    "outcome_id": "uuid",
    "yes_probability": 0.65,
    "no_probability": 0.35,
    "message": "Probability updated to 0.65"
}
```

**刷新概率:**
```json
POST /admin/markets/:market_id/refresh-probability
{
    "source": "orderbook"  // 或 "chainlink", "uma", "pyth"
}
```

### 支持的外部预言机 (TODO)
- Chainlink
- UMA
- Pyth

### Tests
- 67 tests passing (新增 2 个 oracle 测试)

---

## Phase 10: Performance Optimization - Market Cache (Completed)

**Date:** 2024-12-30

### Overview
实现预测市场专用缓存层，优化市场数据、概率、用户持仓和订单簿的访问性能。

### 新增缓存键前缀
```rust
// src/cache/keys.rs
pub const MARKET: &str = "market";      // 市场数据
pub const OUTCOME: &str = "outcome";    // 结果数据
pub const SHARE: &str = "share";        // 用户持仓
pub const PROBABILITY: &str = "prob";   // 概率数据
```

### 新增 TTL 配置
| 数据类型 | TTL | 说明 |
|----------|-----|------|
| MARKET | 60s | 市场详情 |
| MARKET_LIST | 30s | 市场列表 |
| PROBABILITY | 5s | 概率数据 |
| SHARES | 10s | 用户持仓 |
| MARKET_ORDERBOOK | 2s | 订单簿快照 |

### 新增文件
- `src/cache/market_cache.rs` - MarketCache 服务

### 缓存数据结构
```rust
/// 缓存的市场数据
pub struct CachedMarket {
    pub id: Uuid,
    pub question: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub status: String,
    pub outcomes: Vec<CachedOutcome>,
    pub volume_24h: Decimal,
    pub total_volume: Decimal,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 缓存的用户持仓
pub struct CachedShareHolding {
    pub market_id: Uuid,
    pub outcome_id: Uuid,
    pub share_type: String,
    pub amount: Decimal,
    pub avg_cost: Decimal,
    pub current_price: Decimal,
    pub unrealized_pnl: Decimal,
}

/// 缓存的订单簿快照
pub struct CachedPMOrderbook {
    pub market_id: Uuid,
    pub outcome_id: Uuid,
    pub share_type: String,
    pub bids: Vec<[String; 2]>,
    pub asks: Vec<[String; 2]>,
    pub timestamp: i64,
}
```

### MarketCache 方法
```rust
// 市场数据
get_market(market_id) -> Option<CachedMarket>
set_market(&CachedMarket)
invalidate_market(market_id)

// 市场列表
get_market_list(category) -> Option<Vec<CachedMarket>>
set_market_list(&[CachedMarket], category)
invalidate_market_list(category)

// 概率数据
get_probability(market_id, outcome_id) -> Option<Decimal>
set_probability(market_id, outcome_id, probability)
get_market_probabilities(market_id) -> Option<Vec<(Uuid, Decimal)>>

// 用户持仓
get_user_shares(address, market_id) -> Option<Vec<CachedShareHolding>>
set_user_shares(address, market_id, &[CachedShareHolding])
invalidate_user_shares(address, market_id)

// 订单簿
get_orderbook(market_id, outcome_id, share_type) -> Option<CachedPMOrderbook>
set_orderbook(&CachedPMOrderbook)

// 成交量
incr_volume(market_id, amount)
get_volume(market_id) -> Option<Decimal>
```

### API Handler 集成示例
```rust
// GET /markets/:market_id
pub async fn get_market(state, market_id) {
    // 1. 先查缓存
    if let Some(cache) = state.cache.market_opt() {
        if let Ok(Some(cached)) = cache.get_market(market_id).await {
            return Ok(Json(cached.into()));  // 缓存命中
        }
    }

    // 2. 查数据库
    let market = db.get_market(market_id).await?;

    // 3. 写入缓存
    if let Some(cache) = state.cache.market_opt() {
        cache.set_market(&market.into()).await.ok();
    }

    Ok(Json(market))
}
```

### Pub/Sub 频道
| 频道 | 格式 | 说明 |
|------|------|------|
| channel:pm:trades:{market_id} | 市场交易 | 交易推送 |
| channel:pm:orderbook:{market_id}:{outcome_id}:{share_type} | 订单簿 | 订单簿更新 |
| channel:pm:prob:{market_id} | 概率 | 概率更新推送 |
| channel:pm:shares:{address} | 用户持仓 | 持仓变动推送 |

### Tests
- 76 tests passing (新增 9 个缓存测试)

---

## Phase 11: Prometheus Monitoring Metrics (Completed)

**Date:** 2024-12-30

### Overview
添加 Prometheus 兼容的监控指标系统，用于监控平台的各项性能指标。

### 新增依赖
```toml
metrics = "0.22"
metrics-exporter-prometheus = "0.13"
```

### 新增文件
- `src/metrics/mod.rs` - 监控指标模块

### 指标类型

#### HTTP 请求指标
| 指标名 | 类型 | 说明 |
|--------|------|------|
| `http_requests_total` | Counter | 请求总数 |
| `http_request_duration_seconds` | Histogram | 请求延迟 |
| `http_requests_in_flight` | Gauge | 进行中请求数 |

#### 撮合引擎指标
| 指标名 | 类型 | 说明 |
|--------|------|------|
| `orders_submitted_total` | Counter | 提交订单数 |
| `orders_matched_total` | Counter | 撮合成功数 |
| `orders_cancelled_total` | Counter | 取消订单数 |
| `order_match_duration_seconds` | Histogram | 撮合延迟 |
| `trades_executed_total` | Counter | 交易执行数 |
| `trade_volume_usdc` | Counter | 交易量 (USDC) |
| `mint_operations_total` | Counter | Mint 操作数 |
| `merge_operations_total` | Counter | Merge 操作数 |

#### 市场指标
| 指标名 | 类型 | 说明 |
|--------|------|------|
| `active_markets` | Gauge | 活跃市场数 |
| `market_volume_24h_usdc` | Gauge | 24h 交易量 |
| `market_probability` | Gauge | 市场概率 |
| `orderbook_depth` | Gauge | 订单簿深度 |
| `orderbook_spread` | Gauge | 买卖价差 |

#### 缓存指标
| 指标名 | 类型 | 说明 |
|--------|------|------|
| `cache_hits_total` | Counter | 缓存命中数 |
| `cache_misses_total` | Counter | 缓存未命中数 |
| `cache_operation_duration_seconds` | Histogram | 缓存操作延迟 |

#### 数据库指标
| 指标名 | 类型 | 说明 |
|--------|------|------|
| `db_query_duration_seconds` | Histogram | 查询延迟 |
| `db_connections_active` | Gauge | 活跃连接数 |
| `db_connections_idle` | Gauge | 空闲连接数 |

#### WebSocket 指标
| 指标名 | 类型 | 说明 |
|--------|------|------|
| `ws_connections_active` | Gauge | 活跃连接数 |
| `ws_messages_sent_total` | Counter | 发送消息数 |
| `ws_messages_received_total` | Counter | 接收消息数 |

### API 端点
```
GET /metrics - Prometheus 指标端点
```

### 使用示例
```rust
use crate::metrics;

// 记录 HTTP 请求
metrics::record_http_request("GET", "/api/v1/markets", 200, 0.015);

// 记录订单撮合
metrics::record_order_submitted("buy", "limit");
metrics::record_order_match_duration(0.0005);
metrics::record_trade_executed("normal", 100.0);

// 记录缓存操作
metrics::record_cache_hit("market");
metrics::record_cache_miss("orderbook");

// 设置市场指标
metrics::set_active_markets(42);
metrics::set_market_probability("market-id", "outcome-id", "yes", 0.65);
```

### Histogram Buckets 配置
- HTTP 请求: 1ms, 5ms, 10ms, 25ms, 50ms, 100ms, 250ms, 500ms, 1s, 2.5s, 5s, 10s
- 订单撮合: 0.1ms, 0.5ms, 1ms, 5ms, 10ms, 25ms, 50ms, 100ms, 500ms
- 缓存操作: 0.1ms, 0.5ms, 1ms, 5ms, 10ms, 50ms, 100ms
- 数据库查询: 1ms, 5ms, 10ms, 25ms, 50ms, 100ms, 250ms, 500ms, 1s, 5s

### Tests
- 79 tests passing (新增 3 个 metrics 测试)

---

## Next Steps (TODO)

1. **External Oracle Integration** - Implement Chainlink/UMA/Pyth integrations

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
