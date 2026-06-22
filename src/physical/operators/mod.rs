//! 物理算子:全部实现统一的 `PhysicalOperator` 拉取接口。

mod filter;
mod hash_aggregate;
mod hash_join;
mod limit;
mod projection;
mod scan;
mod sort;

pub use filter::FilterExec;
pub use hash_aggregate::{AggrSpec, HashAggregateExec};
pub use hash_join::HashJoinExec;
pub use limit::LimitExec;
pub use projection::ProjectionExec;
pub use scan::ScanExec;
pub use sort::SortExec;
