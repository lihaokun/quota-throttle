//! NewAPI 内部余额设置；不读取或改写远端余额。
#![cfg_attr(not(test), allow(dead_code))]

use super::control::{
    bump, ensure, failure, ControlError, ControlErrorCode as Code, ControlResult,
};
use super::store::commit;
use super::{types::*, CatalogStore};
use rusqlite::{params, Connection, TransactionBehavior};
use serde::Serialize;

pub const BALANCE_CAP: i64 = 4_000_000_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BalancePolicy {
    pub revision: i64,
    pub auto_refill: bool,
    pub warn_below: i64,
    pub refill_to: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BalancePolicyDraft {
    pub auto_refill: bool,
    pub warn_below: i64,
    pub refill_to: i64,
}

fn valid(draft: &BalancePolicyDraft) -> bool {
    0 < draft.warn_below && draft.warn_below < draft.refill_to && draft.refill_to <= BALANCE_CAP
}

fn read(conn: &Connection) -> rusqlite::Result<BalancePolicy> {
    conn.query_row(
        "SELECT revision,auto_refill,warn_below,refill_to FROM balance_policy WHERE singleton=1",
        [],
        |row| {
            let policy = BalancePolicy {
                revision: row.get(0)?,
                auto_refill: match row.get::<_, i64>(1)? {
                    0 => false,
                    1 => true,
                    _ => return Err(rusqlite::Error::InvalidQuery),
                },
                warn_below: row.get(2)?,
                refill_to: row.get(3)?,
            };
            if !positive_revision(policy.revision)
                || !valid(&BalancePolicyDraft {
                    auto_refill: policy.auto_refill,
                    warn_below: policy.warn_below,
                    refill_to: policy.refill_to,
                })
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
            Ok(policy)
        },
    )
}

// 仅用于明确未提交的事务 Busy；不能使用会统一折叠为 Storage 的 From。
fn write_error(err: CatalogError) -> ControlError {
    failure(if err.kind == ErrorKind::Busy {
        Code::QueueFull
    } else {
        Code::Storage
    })
}

impl CatalogStore {
    pub(crate) fn balance_policy(&self) -> ControlResult<BalancePolicy> {
        // Step B1.balance.load: 失效关闭；已存值必须重新校验，不能补默认值。
        self.usable()?;
        Ok(read(&self.conn)?)
    }

    pub(crate) fn save_balance_policy(
        &mut self,
        expected_revision: i64,
        draft: BalancePolicyDraft,
    ) -> ControlResult<BalancePolicy> {
        // Step B1.balance.save: 校验与版本比较先于写入，Busy 与未知提交分开处理。
        self.usable()?;
        ensure(
            positive_revision(expected_revision) && valid(&draft),
            Code::InvalidInput,
        )?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(CatalogError::from)
            .map_err(write_error)?;
        let old = read(&tx)?;
        ensure(old.revision == expected_revision, Code::RevisionConflict)?;
        if (old.auto_refill, old.warn_below, old.refill_to)
            == (draft.auto_refill, draft.warn_below, draft.refill_to)
        {
            return Ok(old);
        }
        let policy = BalancePolicy {
            revision: bump(old.revision)?,
            auto_refill: draft.auto_refill,
            warn_below: draft.warn_below,
            refill_to: draft.refill_to,
        };
        let changed = tx.execute(
            "UPDATE balance_policy SET revision=?1,auto_refill=?2,warn_below=?3,refill_to=?4 WHERE singleton=1 AND revision=?5",
            params![policy.revision, policy.auto_refill, policy.warn_below, policy.refill_to, old.revision],
        )?;
        ensure(changed == 1, Code::Storage)?;
        commit(tx, &mut self.poisoned).map_err(write_error)?;
        Ok(policy)
    }
}

pub(super) fn verify_balance(conn: &Connection) -> Result<()> {
    // Step B1.balance.verify: 只接受完整的单行策略；损坏时拒绝而非修补。
    let count: i64 = conn
        .query_row("SELECT count(*) FROM balance_policy", [], |row| row.get(0))
        .map_err(|_| error(ErrorKind::CorruptData))?;
    require(count == 1, ErrorKind::CorruptData)?;
    read(conn).map_err(|_| error(ErrorKind::CorruptData))?;
    Ok(())
}
