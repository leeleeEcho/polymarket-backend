//! Settlement Service for Prediction Markets
//!
//! Handles settlement logic for resolved and cancelled markets:
//! - Resolved markets: Winners receive 1 USDC per winning share
//! - Cancelled markets: All share holders receive refunds based on cost basis

use rust_decimal::Decimal;
use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use crate::models::market::ShareType;

/// Settlement service errors
#[derive(Debug, thiserror::Error)]
pub enum SettlementError {
    #[error("Market not found: {0}")]
    MarketNotFound(Uuid),

    #[error("Market not resolved or cancelled: {0}")]
    MarketNotSettleable(Uuid),

    #[error("No winning outcome set for market: {0}")]
    NoWinningOutcome(Uuid),

    #[error("User has no shares to settle in market: {0}")]
    NoSharesToSettle(Uuid),

    #[error("Shares already settled for user in market: {0}")]
    AlreadySettled(Uuid),

    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),
}

/// Result of a settlement operation
#[derive(Debug, Clone)]
pub struct SettlementResult {
    pub market_id: Uuid,
    #[allow(dead_code)]
    pub user_address: String,
    pub settlement_type: SettlementType,
    pub shares_settled: Vec<ShareSettlement>,
    pub total_payout: Decimal,
}

/// Type of settlement
#[derive(Debug, Clone, PartialEq)]
pub enum SettlementType {
    /// Market resolved with a winning outcome
    Resolution,
    /// Market was cancelled
    Cancellation,
}

/// Individual share settlement details
#[derive(Debug, Clone)]
pub struct ShareSettlement {
    pub outcome_id: Uuid,
    pub share_type: ShareType,
    pub amount: Decimal,
    pub payout_per_share: Decimal,
    pub total_payout: Decimal,
}

/// Settlement service
pub struct SettlementService;

impl SettlementService {
    /// Settle a user's shares for a resolved or cancelled market
    pub async fn settle_user_shares(
        pool: &PgPool,
        market_id: Uuid,
        user_address: &str,
    ) -> Result<SettlementResult, SettlementError> {
        let user_address = user_address.to_lowercase();

        // 1. Get market status and winning outcome
        let market: Option<(String, Option<Uuid>)> = sqlx::query_as(
            r#"
            SELECT status::text, winning_outcome_id
            FROM markets
            WHERE id = $1
            "#
        )
        .bind(market_id)
        .fetch_optional(pool)
        .await?;

        let (status, winning_outcome_id) = market.ok_or(SettlementError::MarketNotFound(market_id))?;

        // 2. Determine settlement type
        let settlement_type = match status.as_str() {
            "resolved" => {
                if winning_outcome_id.is_none() {
                    return Err(SettlementError::NoWinningOutcome(market_id));
                }
                SettlementType::Resolution
            }
            "cancelled" => SettlementType::Cancellation,
            _ => return Err(SettlementError::MarketNotSettleable(market_id)),
        };

        // 3. Check if user has already settled
        let already_settled: Option<(i64,)> = sqlx::query_as(
            r#"
            SELECT COUNT(*) as count
            FROM share_changes
            WHERE user_address = $1 AND market_id = $2 AND change_type = 'redeem'
            "#
        )
        .bind(&user_address)
        .bind(market_id)
        .fetch_optional(pool)
        .await?;

        if let Some((count,)) = already_settled {
            if count > 0 {
                return Err(SettlementError::AlreadySettled(market_id));
            }
        }

        // 4. Get user's shares for this market
        let shares: Vec<(Uuid, Uuid, String, Decimal, Decimal)> = sqlx::query_as(
            r#"
            SELECT id, outcome_id, share_type::text, amount, avg_cost
            FROM shares
            WHERE user_address = $1 AND market_id = $2 AND amount > 0
            "#
        )
        .bind(&user_address)
        .bind(market_id)
        .fetch_all(pool)
        .await?;

        if shares.is_empty() {
            return Err(SettlementError::NoSharesToSettle(market_id));
        }

        // 5. Calculate payouts and execute settlement
        let mut share_settlements = Vec::new();
        let mut total_payout = Decimal::ZERO;

        // Begin transaction
        let mut tx = pool.begin().await?;

        for (share_id, outcome_id, share_type_str, amount, avg_cost) in shares {
            let share_type: ShareType = share_type_str.parse().unwrap_or(ShareType::Yes);

            let (payout_per_share, share_payout) = match &settlement_type {
                SettlementType::Resolution => {
                    // For resolved markets:
                    // - Winning YES shares pay 1.0 USDC each
                    // - Winning NO shares (when NO wins) pay 1.0 USDC each
                    // - Losing shares pay 0
                    let is_winning = outcome_id == winning_outcome_id.unwrap()
                        && share_type == ShareType::Yes;
                    let is_winning_no = outcome_id != winning_outcome_id.unwrap()
                        && share_type == ShareType::No;

                    if is_winning || is_winning_no {
                        (Decimal::ONE, amount)
                    } else {
                        (Decimal::ZERO, Decimal::ZERO)
                    }
                }
                SettlementType::Cancellation => {
                    // For cancelled markets: refund at avg_cost
                    (avg_cost, amount * avg_cost)
                }
            };

            if amount > Decimal::ZERO {
                // Record share change (redeem)
                sqlx::query(
                    r#"
                    INSERT INTO share_changes (
                        user_address, market_id, outcome_id, share_type,
                        change_type, amount, price, trade_id, order_id
                    )
                    VALUES ($1, $2, $3, $4::share_type, 'redeem', $5, $6, NULL, NULL)
                    "#
                )
                .bind(&user_address)
                .bind(market_id)
                .bind(outcome_id)
                .bind(share_type.to_string())
                .bind(-amount)  // Negative because we're removing shares
                .bind(payout_per_share)
                .execute(&mut *tx)
                .await?;

                // Zero out user's shares
                sqlx::query(
                    r#"
                    UPDATE shares
                    SET amount = 0, updated_at = NOW()
                    WHERE id = $1
                    "#
                )
                .bind(share_id)
                .execute(&mut *tx)
                .await?;

                share_settlements.push(ShareSettlement {
                    outcome_id,
                    share_type,
                    amount,
                    payout_per_share,
                    total_payout: share_payout,
                });

                total_payout += share_payout;
            }
        }

        // 6. Credit user's USDC balance
        if total_payout > Decimal::ZERO {
            sqlx::query(
                r#"
                INSERT INTO balances (user_address, token, available, frozen)
                VALUES ($1, 'USDC', $2, 0)
                ON CONFLICT (user_address, token) DO UPDATE SET
                    available = balances.available + $2,
                    updated_at = NOW()
                "#
            )
            .bind(&user_address)
            .bind(total_payout)
            .execute(&mut *tx)
            .await?;

            info!(
                "Settlement complete: user={}, market={}, payout={}",
                user_address, market_id, total_payout
            );
        }

        // Commit transaction
        tx.commit().await?;

        Ok(SettlementResult {
            market_id,
            user_address,
            settlement_type,
            shares_settled: share_settlements,
            total_payout,
        })
    }

    /// Get settlement status for a user in a market
    pub async fn get_settlement_status(
        pool: &PgPool,
        market_id: Uuid,
        user_address: &str,
    ) -> Result<Option<SettlementStatus>, SettlementError> {
        let user_address = user_address.to_lowercase();

        // Check if market exists and get status
        let market: Option<(String, Option<Uuid>)> = sqlx::query_as(
            r#"
            SELECT status::text, winning_outcome_id
            FROM markets
            WHERE id = $1
            "#
        )
        .bind(market_id)
        .fetch_optional(pool)
        .await?;

        let (status, winning_outcome_id) = match market {
            Some(m) => m,
            None => return Err(SettlementError::MarketNotFound(market_id)),
        };

        // Check if user has shares
        let shares: Vec<(Uuid, String, Decimal, Decimal)> = sqlx::query_as(
            r#"
            SELECT outcome_id, share_type::text, amount, avg_cost
            FROM shares
            WHERE user_address = $1 AND market_id = $2
            "#
        )
        .bind(&user_address)
        .bind(market_id)
        .fetch_all(pool)
        .await?;

        if shares.is_empty() {
            return Ok(None);
        }

        // Check if already settled
        let settled: Option<(i64,)> = sqlx::query_as(
            r#"
            SELECT COUNT(*) as count
            FROM share_changes
            WHERE user_address = $1 AND market_id = $2 AND change_type = 'redeem'
            "#
        )
        .bind(&user_address)
        .bind(market_id)
        .fetch_optional(pool)
        .await?;

        let is_settled = settled.map(|(c,)| c > 0).unwrap_or(false);

        // Calculate potential payout
        let mut potential_payout = Decimal::ZERO;
        let can_settle = matches!(status.as_str(), "resolved" | "cancelled");

        if can_settle && !is_settled {
            for (outcome_id, share_type_str, amount, avg_cost) in &shares {
                let share_type: ShareType = share_type_str.parse().unwrap_or(ShareType::Yes);

                match status.as_str() {
                    "resolved" => {
                        if let Some(winner) = winning_outcome_id {
                            let is_winning = *outcome_id == winner && share_type == ShareType::Yes;
                            let is_winning_no = *outcome_id != winner && share_type == ShareType::No;
                            if is_winning || is_winning_no {
                                potential_payout += amount;
                            }
                        }
                    }
                    "cancelled" => {
                        potential_payout += amount * avg_cost;
                    }
                    _ => {}
                }
            }
        }

        Ok(Some(SettlementStatus {
            market_id,
            user_address,
            market_status: status,
            is_settled,
            can_settle: can_settle && !is_settled,
            potential_payout,
            share_count: shares.len(),
        }))
    }
}

/// Settlement status for a user in a market
#[derive(Debug, Clone)]
pub struct SettlementStatus {
    pub market_id: Uuid,
    #[allow(dead_code)]
    pub user_address: String,
    pub market_status: String,
    pub is_settled: bool,
    pub can_settle: bool,
    pub potential_payout: Decimal,
    pub share_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settlement_type_equality() {
        assert_eq!(SettlementType::Resolution, SettlementType::Resolution);
        assert_eq!(SettlementType::Cancellation, SettlementType::Cancellation);
        assert_ne!(SettlementType::Resolution, SettlementType::Cancellation);
    }
}
