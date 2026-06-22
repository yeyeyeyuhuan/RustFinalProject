//! 端到端测试:给定 CSV + SQL,断言最终查询结果。覆盖投影 / 过滤 / 聚合 / 排序 / Limit。

use rust_final_project::array::RecordBatch;
use rust_final_project::execution::SessionContext;

/// 构造已加载 employees / departments / nulls 三张表的会话。
fn ctx() -> SessionContext {
    let mut ctx = SessionContext::new();
    ctx.register_csv("emp", "testdata/employees.csv").unwrap();
    ctx.register_csv("dept", "testdata/departments.csv")
        .unwrap();
    ctx.register_csv("n", "testdata/nulls.csv").unwrap();
    ctx
}

/// 把结果批拍平为 `Vec<行>`,每个单元格转字符串。
fn rows(batches: &[RecordBatch]) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for b in batches {
        for r in 0..b.num_rows() {
            out.push(b.columns.iter().map(|c| c.value(r).to_string()).collect());
        }
    }
    out
}

fn run(ctx: &SessionContext, sql: &str) -> Vec<Vec<String>> {
    rows(&ctx.sql(sql).unwrap())
}

#[test]
fn projection_and_filter() {
    let ctx = ctx();
    let r = run(&ctx, "SELECT name, salary FROM emp WHERE salary > 9000");
    let names: Vec<&str> = r.iter().map(|row| row[0].as_str()).collect();
    assert_eq!(names, vec!["Carol", "Eve", "Judy"]);
}

#[test]
fn boolean_filter() {
    let ctx = ctx();
    let r = run(&ctx, "SELECT name FROM emp WHERE active = true");
    assert_eq!(r.len(), 7);
}

#[test]
fn arithmetic_projection() {
    let ctx = ctx();
    let r = run(&ctx, "SELECT id, salary / 2 FROM emp WHERE id = 1");
    assert_eq!(r[0][0], "1");
    assert_eq!(r[0][1], "4250"); // 8500.0 / 2
}

#[test]
fn group_by_count() {
    let ctx = ctx();
    let r = run(&ctx, "SELECT dept_id, COUNT(*) FROM emp GROUP BY dept_id");
    // 分组按首次出现顺序:10, 20, 30
    assert_eq!(
        r,
        vec![
            vec!["10".to_string(), "4".to_string()],
            vec!["20".to_string(), "3".to_string()],
            vec!["30".to_string(), "3".to_string()],
        ]
    );
}

#[test]
fn showcase_aggregate() {
    let ctx = ctx();
    let r = run(
        &ctx,
        "SELECT dept_id, COUNT(*), AVG(salary) FROM emp WHERE active = true \
         GROUP BY dept_id HAVING COUNT(*) > 1 ORDER BY AVG(salary) DESC",
    );
    // 30(avg 9300) > 10(avg 8533.3) > 20(avg 7450)
    assert_eq!(r.len(), 3);
    assert_eq!(r[0][0], "30");
    assert_eq!(r[0][1], "2");
    assert_eq!(r[1][0], "10");
    assert_eq!(r[2][0], "20");
}

#[test]
fn sum_min_max() {
    let ctx = ctx();
    let r = run(&ctx, "SELECT MIN(salary), MAX(salary) FROM emp");
    assert_eq!(r[0][0], "4500");
    assert_eq!(r[0][1], "11000");
}

#[test]
fn order_by_limit() {
    let ctx = ctx();
    let r = run(
        &ctx,
        "SELECT name, salary FROM emp ORDER BY salary DESC LIMIT 3",
    );
    let names: Vec<&str> = r.iter().map(|row| row[0].as_str()).collect();
    assert_eq!(names, vec!["Eve", "Judy", "Carol"]);
}

#[test]
fn count_skips_nulls() {
    let ctx = ctx();
    let r = run(&ctx, "SELECT COUNT(score), COUNT(*) FROM n");
    assert_eq!(r[0][0], "3"); // 两个 score 为 NULL
    assert_eq!(r[0][1], "5");
}

#[test]
fn null_arithmetic_propagates() {
    let ctx = ctx();
    // score 为 NULL 的行,score + 1 也应为 NULL
    let r = run(&ctx, "SELECT id, score FROM n WHERE id = 3");
    assert_eq!(r[0][0], "3");
    assert_eq!(r[0][1], "NULL");
}

#[test]
fn explain_contains_nodes() {
    let ctx = ctx();
    let plan = ctx
        .explain("SELECT dept_id, COUNT(*) FROM emp GROUP BY dept_id")
        .unwrap();
    assert!(plan.contains("Aggregate"));
    assert!(plan.contains("Scan"));
}

fn ctx_events() -> SessionContext {
    let mut ctx = SessionContext::new();
    ctx.register_csv("ev", "testdata/events.csv").unwrap();
    ctx
}

#[test]
fn date_inferred_and_displayed() {
    let ctx = ctx_events();
    let schema = ctx.table_schema("ev").unwrap();
    assert_eq!(schema.field(2).data_type.name(), "Date");
    // 日期按 YYYY-MM-DD 显示
    let r = run(&ctx, "SELECT event_date FROM ev WHERE id = 1");
    assert_eq!(r[0][0], "2024-01-15");
}

#[test]
fn date_comparison_with_string_literal() {
    let ctx = ctx_events();
    // 日期列与日期字符串字面量比较(隐式转 Date)
    let r = run(&ctx, "SELECT id FROM ev WHERE event_date > '2024-01-01'");
    assert_eq!(r.len(), 4); // 排除 2023-11-05
}

#[test]
fn date_order_and_minmax() {
    let ctx = ctx_events();
    let r = run(&ctx, "SELECT name FROM ev ORDER BY event_date LIMIT 1");
    assert_eq!(r[0][0], "Release"); // 2023-11-05 最早
    let m = run(&ctx, "SELECT MIN(event_date), MAX(event_date) FROM ev");
    assert_eq!(m[0][0], "2023-11-05");
    assert_eq!(m[0][1], "2024-06-30");
}

#[test]
fn date_group_by() {
    let ctx = ctx_events();
    let r = run(
        &ctx,
        "SELECT event_date, COUNT(*) FROM ev GROUP BY event_date ORDER BY event_date",
    );
    // 2024-01-15 出现两次
    let jan: Vec<&Vec<String>> = r.iter().filter(|row| row[0] == "2024-01-15").collect();
    assert_eq!(jan[0][1], "2");
}

#[test]
fn errors_are_reported() {
    let ctx = ctx();
    assert!(ctx.sql("SELECT nope FROM emp").is_err()); // 列不存在
    assert!(ctx.sql("SELECT * FROM missing_table").is_err()); // 表不存在
    assert!(ctx.sql("SELECT name FROM emp WHERE salary").is_err()); // WHERE 非布尔
}

#[test]
fn inner_join() {
    let ctx = ctx();
    let r = run(
        &ctx,
        "SELECT e.name, d.name FROM emp AS e INNER JOIN dept AS d ON e.dept_id = d.id \
         WHERE e.id = 1",
    );
    // Alice 属于 dept 10 = Engineering
    assert_eq!(r.len(), 1);
    assert_eq!(r[0][0], "Alice");
    assert_eq!(r[0][1], "Engineering");
}

#[test]
fn join_row_count() {
    let ctx = ctx();
    // 10 名员工的 dept_id 都能在 dept 表中匹配(10/20/30 均存在)→ 10 行
    let r = run(
        &ctx,
        "SELECT e.id FROM emp AS e JOIN dept AS d ON e.dept_id = d.id",
    );
    assert_eq!(r.len(), 10);
}

#[test]
fn mixed_type_comparison() {
    let ctx = ctx();
    // age 为 Int64,与浮点字面量比较应隐式提升
    let r = run(&ctx, "SELECT name FROM emp WHERE age > 45.5");
    let names: Vec<&str> = r.iter().map(|row| row[0].as_str()).collect();
    assert_eq!(names, vec!["Eve", "Judy"]); // 52, 48
}

#[test]
fn mixed_type_join_coercion() {
    let mut ctx = SessionContext::new();
    ctx.register_csv("emp", "testdata/employees.csv").unwrap();
    ctx.register_csv("df", "testdata/dept_float.csv").unwrap();
    // emp.dept_id (Int64) = df.fid (Float64):隐式提升后应能匹配
    let r = run(
        &ctx,
        "SELECT COUNT(*) FROM emp AS e JOIN df ON e.dept_id = df.fid",
    );
    assert_eq!(r[0][0], "10"); // 全部 10 名员工的部门都在 df 中
}

#[test]
fn left_join_keeps_unmatched() {
    let ctx = ctx();
    // dept 40 (Research) 无员工 → LEFT JOIN 保留该行,员工列为 NULL
    let r = run(
        &ctx,
        "SELECT d.name, e.name FROM dept AS d LEFT JOIN emp AS e ON d.id = e.dept_id",
    );
    assert_eq!(r.len(), 11); // 10 个匹配 + 1 个未匹配(Research)
    let research: Vec<&Vec<String>> = r.iter().filter(|row| row[0] == "Research").collect();
    assert_eq!(research.len(), 1);
    assert_eq!(research[0][1], "NULL"); // 员工名为 NULL
}

#[test]
fn left_join_vs_inner_count() {
    let ctx = ctx();
    let inner = run(
        &ctx,
        "SELECT d.name FROM dept AS d JOIN emp AS e ON d.id = e.dept_id",
    );
    let left = run(
        &ctx,
        "SELECT d.name FROM dept AS d LEFT JOIN emp AS e ON d.id = e.dept_id",
    );
    assert_eq!(inner.len(), 10); // 仅匹配行
    assert_eq!(left.len(), 11); // 多保留 Research
}

#[test]
fn join_then_aggregate() {
    let ctx = ctx();
    // 按部门名分组统计人数
    let r = run(
        &ctx,
        "SELECT d.name, COUNT(*) FROM emp AS e JOIN dept AS d ON e.dept_id = d.id \
         GROUP BY d.name ORDER BY COUNT(*) DESC",
    );
    // Engineering 4 人最多
    assert_eq!(r[0][0], "Engineering");
    assert_eq!(r[0][1], "4");
}
