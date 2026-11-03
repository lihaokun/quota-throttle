//! 内部用户额度的有限读取与原子加额；不维护补足策略或尝试状态。
#![cfg_attr(not(test), allow(dead_code))]
use super::transport::{
    error, DeliveryEvidence, ManagementErrorKind, ManagementRequest, RequestClass,
    RequestSemantics, Result, SINGLE_BODY_LIMIT,
};
use super::NewApiClient;
use crate::catalog::BALANCE_CAP;
use reqwest::Method;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::time::Instant;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuotaUnit {
    pub quota_per_unit: f64,
    pub display_type: String,
}

fn parse_unit(body: &Value) -> Result<QuotaUnit> {
    let invalid =
        || error(ManagementErrorKind::InvalidResponse).with_delivery(DeliveryEvidence::Uncertain);
    let data = body.get("data").ok_or_else(invalid)?;
    let quota_per_unit = data
        .get("quota_per_unit")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(invalid)?;
    let display_type = data
        .get("quota_display_type")
        .and_then(Value::as_str)
        .filter(|value| {
            (1..=16).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_alphanumeric())
        })
        .ok_or_else(invalid)?
        .to_owned();
    Ok(QuotaUnit {
        quota_per_unit,
        display_type,
    })
}

impl NewApiClient {
    pub(crate) async fn quota_unit_until(&self, deadline: Instant) -> Result<QuotaUnit> {
        // Step B2.unit: 换算仅来自有效响应，失败不猜比例或填零。
        let request = ManagementRequest::new(
            Method::GET,
            "/api/status",
            None,
            RequestSemantics::Read,
            SINGLE_BODY_LIMIT,
        )?;
        let body = self
            .transport
            .send_fenced_until(request, RequestClass::Observe, &|| true, deadline)
            .await?
            .envelope()?;
        parse_unit(&body)
    }

    pub(crate) async fn add_user_quota_until(
        &self,
        user_id: i64,
        amount: i64,
        guard: &(dyn Fn() -> bool + Send + Sync),
        deadline: Instant,
    ) -> Result<()> {
        // Step B2.add: id=0 会命中 Native root；非正 ID/越界金额必须本地拒绝。
        if user_id <= 0 || !(1..=BALANCE_CAP).contains(&amount) {
            return Err(error(ManagementErrorKind::InvalidInput));
        }
        let request = ManagementRequest::new(
            Method::POST,
            "/api/user/manage",
            Some(json!({"id":user_id,"action":"add_quota","mode":"add","value":amount})),
            RequestSemantics::Create,
            SINGLE_BODY_LIMIT,
        )?;
        self.transport
            .send_fenced_until(request, RequestClass::Control, guard, deadline)
            .await?;
        Ok(())
    }
}
