//! 物理表达式与物理算子:火山模型(Volcano)拉取接口 + 向量化执行。

pub mod expr;
pub(crate) mod key;
mod operators;
mod planner;

pub use expr::{PhysicalExpr, compile};
pub use planner::{PlannerConfig, create_physical_plan, create_physical_plan_with};

use std::sync::Arc;

use crate::array::RecordBatch;
use crate::error::Result;
use crate::types::Schema;

/// 火山模型统一拉取接口:每次产出下一个 `RecordBatch`,或 `None` 表示结束。
/// 所有物理算子实现它;执行引擎只面向这一个抽象驱动整棵算子树。
pub trait PhysicalOperator {
    /// 该算子输出的 Schema。
    fn schema(&self) -> Arc<Schema>;

    /// 拉取下一批结果。
    fn next_batch(&mut self) -> Result<Option<RecordBatch>>;
}
