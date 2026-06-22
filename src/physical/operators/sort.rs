//! Sort 算子(阻塞):收齐全部输入后按多列排序键排序。
//! 支持多列、升降序;NULL 视为最小值(升序时在前,降序时在后)。

use std::cmp::Ordering;
use std::sync::Arc;

use crate::array::{Array, ArrayRef, RecordBatch, concat, take};
use crate::error::Result;
use crate::physical::{PhysicalExpr, PhysicalOperator};
use crate::types::Schema;

/// 排序算子。`keys` 为(排序表达式, 是否升序)。
pub struct SortExec {
    input: Box<dyn PhysicalOperator>,
    keys: Vec<(PhysicalExpr, bool)>,
    schema: Arc<Schema>,
    produced: bool,
}

impl SortExec {
    pub fn new(
        input: Box<dyn PhysicalOperator>,
        keys: Vec<(PhysicalExpr, bool)>,
        schema: Arc<Schema>,
    ) -> Self {
        SortExec {
            input,
            keys,
            schema,
            produced: false,
        }
    }

    fn sort_all(&mut self) -> Result<RecordBatch> {
        // 收集全部非空批
        let mut batches = Vec::new();
        while let Some(b) = self.input.next_batch()? {
            if b.num_rows() > 0 {
                batches.push(b);
            }
        }
        if batches.is_empty() {
            let columns: Vec<ArrayRef> = self
                .schema
                .fields
                .iter()
                .map(|f| Array::from_scalars(f.data_type, &[]))
                .collect();
            return RecordBatch::try_new(Arc::clone(&self.schema), columns);
        }

        // 把各批的同名列拼接成整列
        let ncols = self.schema.len();
        let mut combined: Vec<ArrayRef> = Vec::with_capacity(ncols);
        for c in 0..ncols {
            let parts: Vec<ArrayRef> = batches.iter().map(|b| Arc::clone(b.column(c))).collect();
            combined.push(concat(&parts)?);
        }
        let combined_batch = RecordBatch::try_new(Arc::clone(&self.schema), combined)?;
        let n = combined_batch.num_rows();

        // 求值排序键列
        let key_cols: Vec<ArrayRef> = self
            .keys
            .iter()
            .map(|(e, _)| e.evaluate(&combined_batch))
            .collect::<Result<_>>()?;

        let mut indices: Vec<usize> = (0..n).collect();
        indices.sort_by(|&a, &b| {
            for (k, (_, asc)) in self.keys.iter().enumerate() {
                let va = key_cols[k].value(a);
                let vb = key_cols[k].value(b);
                let mut ord = va.order_cmp(&vb);
                if !asc {
                    ord = ord.reverse();
                }
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            Ordering::Equal
        });

        let sorted: Vec<ArrayRef> = combined_batch
            .columns
            .iter()
            .map(|c| take(c, &indices))
            .collect();
        RecordBatch::try_new(Arc::clone(&self.schema), sorted)
    }
}

impl PhysicalOperator for SortExec {
    fn schema(&self) -> Arc<Schema> {
        Arc::clone(&self.schema)
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.produced {
            return Ok(None);
        }
        self.produced = true;
        Ok(Some(self.sort_all()?))
    }
}
