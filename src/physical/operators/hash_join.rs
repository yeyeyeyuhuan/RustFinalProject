//! HashJoin 算子(阻塞 build 端)。支持 INNER 与 LEFT JOIN。
//!
//! - `build_left` 决定在左还是右侧建哈希表(代价优化:INNER 选较小一侧 build);
//!   probe 另一侧。无论哪侧 build,输出列顺序恒为「左列 ++ 右列」。
//! - LEFT JOIN 固定 build 右侧、probe 左侧,对无匹配的左行内联补 NULL 右列。
//! - `parallel` 为真时 build 阶段用 **rayon** 并行建局部表再合并。
//! - NULL 键不参与匹配(SQL 等值连接语义)。可选残余条件(非等值部分)作连接后过滤。

use std::collections::HashMap;
use std::sync::Arc;

use rayon::prelude::*;

use crate::array::{Array, ArrayRef, RecordBatch};
use crate::error::{QueryError, Result};
use crate::physical::key::{KeyValue, row_key};
use crate::physical::{PhysicalExpr, PhysicalOperator};
use crate::sql::ast::JoinType;
use crate::types::{ScalarValue, Schema};

/// key → 落在该键上的 build 侧行(每行是其全部列值)。
type BuildMap = HashMap<Vec<KeyValue>, Vec<Vec<ScalarValue>>>;

/// 哈希连接算子。
pub struct HashJoinExec {
    left: Box<dyn PhysicalOperator>,
    right: Box<dyn PhysicalOperator>,
    left_keys: Vec<PhysicalExpr>,
    right_keys: Vec<PhysicalExpr>,
    filter: Option<PhysicalExpr>,
    schema: Arc<Schema>,
    join_type: JoinType,
    /// 是否在左侧建哈希表(LEFT JOIN 强制为 false)。
    build_left: bool,
    parallel: bool,
    left_ncols: usize,
    right_ncols: usize,
    built: bool,
    index: BuildMap,
}

impl HashJoinExec {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        left: Box<dyn PhysicalOperator>,
        right: Box<dyn PhysicalOperator>,
        left_keys: Vec<PhysicalExpr>,
        right_keys: Vec<PhysicalExpr>,
        filter: Option<PhysicalExpr>,
        schema: Arc<Schema>,
        join_type: JoinType,
        build_left: bool,
        parallel: bool,
    ) -> Self {
        let left_ncols = left.schema().len();
        let right_ncols = right.schema().len();
        HashJoinExec {
            left,
            right,
            left_keys,
            right_keys,
            filter,
            schema,
            join_type,
            build_left,
            parallel,
            left_ncols,
            right_ncols,
            built: false,
            index: HashMap::new(),
        }
    }

    /// build 阶段:消费完 build 侧,按 key 建哈希表(NULL 键跳过)。
    fn build(&mut self) -> Result<()> {
        let build_keys = if self.build_left {
            &self.left_keys
        } else {
            &self.right_keys
        };

        let mut batches = Vec::new();
        loop {
            let next = if self.build_left {
                self.left.next_batch()?
            } else {
                self.right.next_batch()?
            };
            match next {
                Some(b) => batches.push(b),
                None => break,
            }
        }

        let partials: Vec<BuildMap> = if self.parallel {
            batches
                .par_iter()
                .map(|b| build_partial(b, build_keys))
                .collect::<Result<Vec<_>>>()?
        } else {
            batches
                .iter()
                .map(|b| build_partial(b, build_keys))
                .collect::<Result<Vec<_>>>()?
        };
        for partial in partials {
            for (k, rows) in partial {
                self.index.entry(k).or_default().extend(rows);
            }
        }
        Ok(())
    }

    /// 把 build 行与 probe 行按「左 ++ 右」顺序拼成输出行。
    fn combine(&self, build_row: &[ScalarValue], probe_row: &[ScalarValue]) -> Vec<ScalarValue> {
        let mut row = Vec::with_capacity(self.left_ncols + self.right_ncols);
        if self.build_left {
            row.extend_from_slice(build_row); // build=左
            row.extend_from_slice(probe_row); // probe=右
        } else {
            row.extend_from_slice(probe_row); // probe=左
            row.extend_from_slice(build_row); // build=右
        }
        row
    }

    /// 为无匹配的左行补 NULL 右列(LEFT JOIN 用,此时 probe 即左侧)。
    fn left_outer_row(&self, probe_row: &[ScalarValue]) -> Vec<ScalarValue> {
        let mut row = Vec::with_capacity(self.left_ncols + self.right_ncols);
        row.extend_from_slice(probe_row);
        for c in self.left_ncols..self.schema.len() {
            row.push(ScalarValue::Null(self.schema.field(c).data_type));
        }
        row
    }
}

/// 为单个 batch 构建局部哈希表(NULL 键跳过)。
fn build_partial(batch: &RecordBatch, build_keys: &[PhysicalExpr]) -> Result<BuildMap> {
    let key_cols: Vec<ArrayRef> = build_keys
        .iter()
        .map(|e| e.evaluate(batch))
        .collect::<Result<_>>()?;
    let mut map: BuildMap = HashMap::new();
    for row in 0..batch.num_rows() {
        let kv: Vec<ScalarValue> = key_cols.iter().map(|c| c.value(row)).collect();
        let key = row_key(&kv);
        if key.iter().any(|k| k.is_null()) {
            continue;
        }
        let row_vals: Vec<ScalarValue> = batch.columns.iter().map(|c| c.value(row)).collect();
        map.entry(key).or_default().push(row_vals);
    }
    Ok(map)
}

impl PhysicalOperator for HashJoinExec {
    fn schema(&self) -> Arc<Schema> {
        Arc::clone(&self.schema)
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if !self.built {
            self.build()?;
            self.built = true;
        }

        let left_outer = self.join_type == JoinType::Left; // 此时必然 build 右、probe 左

        loop {
            // probe 侧 = 与 build 相反的一侧
            let pbatch = if self.build_left {
                self.right.next_batch()?
            } else {
                self.left.next_batch()?
            };
            let pbatch = match pbatch {
                Some(b) => b,
                None => return Ok(None),
            };
            let pnrows = pbatch.num_rows();
            let probe_keys = if self.build_left {
                &self.right_keys
            } else {
                &self.left_keys
            };
            let key_cols: Vec<ArrayRef> = probe_keys
                .iter()
                .map(|e| e.evaluate(&pbatch))
                .collect::<Result<_>>()?;

            // 收集匹配产生的候选行及其来源 probe 行下标。
            let mut cand_rows: Vec<Vec<ScalarValue>> = Vec::new();
            let mut cand_src: Vec<usize> = Vec::new();
            for prow in 0..pnrows {
                let kv: Vec<ScalarValue> = key_cols.iter().map(|c| c.value(prow)).collect();
                let key = row_key(&kv);
                if key.iter().any(|k| k.is_null()) {
                    continue;
                }
                if let Some(matches) = self.index.get(&key) {
                    let probe_vals: Vec<ScalarValue> =
                        pbatch.columns.iter().map(|c| c.value(prow)).collect();
                    for build_row in matches {
                        cand_rows.push(self.combine(build_row, &probe_vals));
                        cand_src.push(prow);
                    }
                }
            }

            // 对候选行套残余条件(非等值部分)。
            let keep: Vec<bool> = match &self.filter {
                None => vec![true; cand_rows.len()],
                Some(f) => {
                    let batch = rows_to_batch(&self.schema, &cand_rows)?;
                    let mask = f.evaluate(&batch)?;
                    let Array::Boolean(b) = mask.as_ref() else {
                        return Err(QueryError::exec("JOIN 残余条件未求值为布尔列"));
                    };
                    (0..b.data.len())
                        .map(|i| b.validity.is_valid(i) && b.data[i])
                        .collect()
                }
            };

            let mut out_rows: Vec<Vec<ScalarValue>> = Vec::new();
            let mut probe_matched = vec![false; pnrows];
            for (pos, row) in cand_rows.into_iter().enumerate() {
                if keep[pos] {
                    probe_matched[cand_src[pos]] = true;
                    out_rows.push(row);
                }
            }

            // LEFT JOIN:无匹配的左(probe)行补 NULL 右列。
            if left_outer {
                for (prow, matched) in probe_matched.iter().enumerate() {
                    if !*matched {
                        let probe_vals: Vec<ScalarValue> =
                            pbatch.columns.iter().map(|c| c.value(prow)).collect();
                        out_rows.push(self.left_outer_row(&probe_vals));
                    }
                }
            }

            if out_rows.is_empty() {
                continue;
            }
            return Ok(Some(rows_to_batch(&self.schema, &out_rows)?));
        }
    }
}

/// 行主序的输出行 → 列式 RecordBatch。
fn rows_to_batch(schema: &Arc<Schema>, rows: &[Vec<ScalarValue>]) -> Result<RecordBatch> {
    let ncols = schema.len();
    let mut columns = Vec::with_capacity(ncols);
    for c in 0..ncols {
        let vals: Vec<ScalarValue> = rows.iter().map(|r| r[c].clone()).collect();
        columns.push(Array::from_scalars(schema.field(c).data_type, &vals));
    }
    RecordBatch::try_new(Arc::clone(schema), columns)
}
