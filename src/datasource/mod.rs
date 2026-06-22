//! 数据源:让数据进来。抽象 `DataSource` trait,CSV 是唯一必做实现。

mod csv;

pub use csv::CsvSource;

use std::sync::Arc;

use crate::array::RecordBatch;
use crate::error::Result;
use crate::types::Schema;

/// 流式 batch 产出迭代器(拥有所有权,可跨线程,供并行 scan 使用)。
pub type BatchIter = Box<dyn Iterator<Item = Result<RecordBatch>> + Send>;

/// 数据源抽象:对外暴露 Schema 与按 batch 流式产出数据的能力。
///
/// `scan` 接受可选的列裁剪(投影下推接口点):仅装载并产出所需列。
pub trait DataSource: Send + Sync {
    /// 数据源的完整 Schema。
    fn schema(&self) -> Arc<Schema>;

    /// 流式扫描。`projection` 为 None 表示读全部列,否则只读指定位置的列。
    fn scan(&self, projection: Option<Vec<usize>>) -> Result<BatchIter>;

    /// 粗略行数估计(供代价优化选择 Hash Join build 端)。未知返回 None。
    fn estimated_rows(&self) -> Option<usize> {
        None
    }
}
