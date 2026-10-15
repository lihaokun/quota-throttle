//! 来源级只读 quota 采集及纯 GLM 分组适配；尚未接入 CLI 或协调器发布。
pub mod glm;
pub mod probe;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyError {
    InvalidInput,
    Capacity,
    SequenceExhausted,
}
impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "source policy: {self:?}")
    }
}
impl std::error::Error for PolicyError {}

#[cfg(test)]
mod reference;
#[cfg(test)]
mod tests;
