//! 来源级只读采集及 GLM/火山纯策略；尚未接入 CLI 或协调器发布。
pub mod glm;
pub mod probe;
pub mod volcano;

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
