//! 查询会话上下文:已注册的表及其 schema / 数据源,以及顶层 `sql` 执行入口。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::driver;
use crate::array::RecordBatch;
use crate::datasource::{CsvSource, DataSource};
use crate::error::Result;
use crate::logical::{Catalog, LogicalPlan, bind, explain};
use crate::optimizer::Optimizer;
use crate::physical::{PlannerConfig, create_physical_plan_with};
use crate::sql::parse;
use crate::types::Schema;

/// 会话上下文:维护表注册表,串联 解析 → 绑定 → 优化 → 物理计划 → 执行。
pub struct SessionContext {
    tables: HashMap<String, Arc<dyn DataSource>>,
    optimizer: Optimizer,
    parallel: bool,
}

impl SessionContext {
    pub fn new() -> Self {
        SessionContext {
            tables: HashMap::new(),
            optimizer: Optimizer::new(),
            parallel: false,
        }
    }

    /// 开启 / 关闭 partitioned 并行执行(影响 `sql`)。
    pub fn set_parallel(&mut self, on: bool) {
        self.parallel = on;
    }

    /// 当前是否启用并行。
    pub fn parallel(&self) -> bool {
        self.parallel
    }

    /// 注册一个 CSV 文件为命名表。
    pub fn register_csv(&mut self, name: &str, path: impl AsRef<Path>) -> Result<()> {
        let source = CsvSource::open(path)?;
        self.tables.insert(name.to_string(), Arc::new(source));
        Ok(())
    }

    /// 查询某表的 Schema。
    pub fn table_schema(&self, name: &str) -> Option<Arc<Schema>> {
        self.tables.get(name).map(|t| t.schema())
    }

    /// 已注册表名(有序)。
    pub fn table_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tables.keys().cloned().collect();
        names.sort();
        names
    }

    /// 解析 + 绑定为(未优化的)逻辑计划。
    pub fn logical_plan(&self, sql: &str) -> Result<LogicalPlan> {
        let stmt = parse(sql)?;
        bind(&stmt, self)
    }

    /// 解析 + 绑定 + 优化后的逻辑计划。
    pub fn optimized_plan(&self, sql: &str) -> Result<LogicalPlan> {
        let plan = self.logical_plan(sql)?;
        self.optimizer.optimize(plan)
    }

    /// 执行一条 SQL(并行性由会话设置决定),返回结果批。
    pub fn sql(&self, sql: &str) -> Result<Vec<RecordBatch>> {
        self.sql_with_parallel(sql, self.parallel)
    }

    /// 执行一条 SQL,可选启用 partitioned 并行(聚合 / 连接)。
    ///
    /// 并行结果与串行**等价**(行集合相同),但分组 / 连接的行顺序可能不同
    /// (无 ORDER BY 时 SQL 不保证顺序)。
    pub fn sql_with_parallel(&self, sql: &str, parallel: bool) -> Result<Vec<RecordBatch>> {
        let plan = self.optimized_plan(sql)?;
        let mut op = create_physical_plan_with(&plan, &PlannerConfig { parallel })?;
        driver::collect(op.as_mut())
    }

    /// 返回逻辑计划的树形文本(优化前后对比,供 `.explain`)。
    pub fn explain(&self, sql: &str) -> Result<String> {
        let raw = self.logical_plan(sql)?;
        let optimized = self.optimizer.optimize(raw.clone())?;
        Ok(format!(
            "== 未优化逻辑计划 ==\n{}\n== 优化后逻辑计划 ==\n{}",
            explain(&raw),
            explain(&optimized)
        ))
    }
}

impl Default for SessionContext {
    fn default() -> Self {
        Self::new()
    }
}

impl Catalog for SessionContext {
    fn get_table(&self, name: &str) -> Option<Arc<dyn DataSource>> {
        self.tables.get(name).map(Arc::clone)
    }
}
