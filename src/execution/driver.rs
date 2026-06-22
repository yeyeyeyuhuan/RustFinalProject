//! 执行驱动:从物理算子树根节点按火山模型不断拉取 batch 直到结束。

use crate::array::RecordBatch;
use crate::error::Result;
use crate::physical::PhysicalOperator;

/// 驱动整棵算子树,收集全部结果批。
pub fn collect(op: &mut dyn PhysicalOperator) -> Result<Vec<RecordBatch>> {
    let mut out = Vec::new();
    while let Some(batch) = op.next_batch()? {
        out.push(batch);
    }
    Ok(out)
}
