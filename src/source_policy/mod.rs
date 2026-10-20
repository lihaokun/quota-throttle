//! 来源级只读采集及 GLM/火山纯策略；GLM 已接协调器显式刷新，尚未接入运行 CLI。
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
