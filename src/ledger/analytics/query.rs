use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageGrain {
    Hour,
    Day,
}
impl UsageGrain {
    fn seconds(self) -> i128 {
        match self {
            Self::Hour => 3600,
            Self::Day => 86400,
        }
    }
}
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) enum DimensionFilter<T> {
    #[default]
    Any,
    Unknown,
    Exact(T),
}
impl<T: PartialEq> DimensionFilter<T> {
    fn matches(&self, value: Option<&T>) -> bool {
        match self {
            Self::Any => true,
            Self::Unknown => value.is_none(),
            Self::Exact(want) => value == Some(want),
        }
    }
}
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct UsageFilter {
    pub(crate) source: DimensionFilter<SourceId>,
    pub(crate) group: DimensionFilter<GroupId>,
    pub(crate) token: DimensionFilter<i64>,
    pub(crate) model: DimensionFilter<String>,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct UsageQuery {
    window: ScanWindow,
    utc_offset_minutes: i64,
    grain: UsageGrain,
    filter: UsageFilter,
}
pub(crate) struct MinutePartition {
    pub(crate) whole: Option<ScanWindow>,
    pub(crate) edges: Vec<ScanWindow>,
}
pub(crate) fn partition_window(window: ScanWindow) -> AnalyticsResult<MinutePartition> {
    if window.from < 0 || window.from >= window.to || window.to > JSON_SAFE {
        return Err(AnalyticsError::InvalidQuery);
    }
    Ok(partition_valid_window(window))
}
fn partition_valid_window(window: ScanWindow) -> MinutePartition {
    let ScanWindow { from, to } = window;
    let left = (from + 59) / 60 * 60;
    let right = to / 60 * 60;
    if left >= right {
        return MinutePartition {
            whole: None,
            edges: vec![window],
        };
    }
    let edges = [
        ScanWindow { from, to: left },
        ScanWindow { from: right, to },
    ]
    .into_iter()
    .filter(|w| w.from < w.to)
    .collect();
    MinutePartition {
        whole: Some(ScanWindow {
            from: left,
            to: right,
        }),
        edges,
    }
}
impl UsageQuery {
    pub(crate) fn new(
        window: ScanWindow,
        utc_offset_minutes: i64,
        grain: UsageGrain,
        filter: UsageFilter,
    ) -> AnalyticsResult<Self> {
        // Step B3.1: 所有值域先验；后续分钟与自然时间桶仅使用精确整数。
        if window.from < 0
            || window.to > JSON_SAFE
            || window.from >= window.to
            || window.to - window.from > MAX_QUERY_SECONDS
            || !(-720..=840).contains(&utc_offset_minutes)
            || matches!(filter.token, DimensionFilter::Exact(n) if n <= 0)
            || matches!(&filter.model, DimensionFilter::Exact(s) if !valid_label(s))
        {
            return Err(AnalyticsError::InvalidQuery);
        }
        Ok(Self {
            window,
            utc_offset_minutes,
            grain,
            filter,
        })
    }
    pub(crate) fn window(&self) -> ScanWindow {
        self.window
    }
    pub(crate) fn utc_offset_minutes(&self) -> i64 {
        self.utc_offset_minutes
    }
    pub(crate) fn grain(&self) -> UsageGrain {
        self.grain
    }
    pub(crate) fn filter(&self) -> &UsageFilter {
        &self.filter
    }
    pub(crate) fn partition(&self) -> MinutePartition {
        partition_valid_window(self.window)
    }
    fn anchor(&self) -> i128 {
        let shift = i128::from(self.utc_offset_minutes) * 60;
        (i128::from(self.window.from) + shift).div_euclid(self.grain.seconds())
            * self.grain.seconds()
            - shift
    }
    pub(crate) fn bucket_ranges(&self) -> Vec<ScanWindow> {
        let mut start = self.anchor();
        let end = i128::from(self.window.to);
        let mut ranges = Vec::new();
        while start < end {
            let next = start + self.grain.seconds();
            ranges.push(ScanWindow {
                from: start.max(i128::from(self.window.from)) as i64,
                to: next.min(end) as i64,
            });
            start = next;
        }
        ranges
    }
    pub(crate) fn bucket_index(&self, at: i64) -> AnalyticsResult<usize> {
        if at < self.window.from || at >= self.window.to {
            return Err(AnalyticsError::InvalidQuery);
        }
        Ok(((i128::from(at) - self.anchor()) / self.grain.seconds()) as usize)
    }
    pub(super) fn matches(&self, dims: UsageDimensions<'_>) -> bool {
        self.filter.source.matches(dims.source.as_ref())
            && self.filter.group.matches(dims.group.as_ref())
            && self.filter.token.matches(dims.token.as_ref())
            && match &self.filter.model {
                DimensionFilter::Any => true,
                DimensionFilter::Unknown => dims.model.is_none(),
                DimensionFilter::Exact(want) => dims.model == Some(want.as_str()),
            }
    }
}
