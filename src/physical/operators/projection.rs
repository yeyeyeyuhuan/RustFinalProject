//! Projection 算子:对每 batch 求值投影表达式,产出新列集。

use std::sync::Arc;

use crate::array::{ArrayRef, RecordBatch};
use crate::error::Result;
use crate::physical::{PhysicalExpr, PhysicalOperator};
use crate::types::Schema;

/// 投影算子。
pub struct ProjectionExec {
    input: Box<dyn PhysicalOperator>,
    exprs: Vec<PhysicalExpr>,
    schema: Arc<Schema>,
}

impl ProjectionExec {
    pub fn new(
        input: Box<dyn PhysicalOperator>,
        exprs: Vec<PhysicalExpr>,
        schema: Arc<Schema>,
    ) -> Self {
        ProjectionExec {
            input,
            exprs,
            schema,
        }
    }
}

impl PhysicalOperator for ProjectionExec {
    fn schema(&self) -> Arc<Schema> {
        Arc::clone(&self.schema)
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        let batch = match self.input.next_batch()? {
            Some(b) => b,
            None => return Ok(None),
        };
        let columns: Vec<ArrayRef> = self
            .exprs
            .iter()
            .map(|e| e.evaluate(&batch))
            .collect::<Result<_>>()?;
        Ok(Some(RecordBatch::try_new(
            Arc::clone(&self.schema),
            columns,
        )?))
    }
}
