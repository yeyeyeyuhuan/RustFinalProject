//! 语法分析器:Token 流 → AST。
//!
//! 语句结构用递归下降;表达式用 **Pratt parsing**(运算符优先级分析)处理算术 / 比较
//! / 逻辑运算符的优先级与结合性。

use super::ast::*;
use super::token::{Keyword, SpannedToken, Token};
use crate::error::{QueryError, Result};

/// 解析一条 SQL 语句。
pub fn parse(sql: &str) -> Result<Statement> {
    let tokens = super::lexer::tokenize(sql)?;
    let mut parser = Parser::new(tokens);
    let stmt = parser.parse_statement()?;
    parser.expect_end()?;
    Ok(stmt)
}

struct Parser {
    tokens: Vec<SpannedToken>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<SpannedToken>) -> Self {
        Parser { tokens, pos: 0 }
    }

    // ---- 基础游标操作 ----

    fn nth(&self, n: usize) -> &Token {
        self.tokens
            .get(self.pos + n)
            .map(|t| &t.token)
            .unwrap_or(&Token::Eof)
    }

    fn peek(&self) -> &Token {
        self.nth(0)
    }

    fn cur_pos(&self) -> usize {
        self.tokens.get(self.pos).map(|t| t.start).unwrap_or(0)
    }

    fn advance(&mut self) -> Token {
        let tok = self
            .tokens
            .get(self.pos)
            .map(|t| t.token.clone())
            .unwrap_or(Token::Eof);
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn at_keyword(&self, kw: Keyword) -> bool {
        matches!(self.peek(), Token::Keyword(k) if *k == kw)
    }

    fn eat_keyword(&mut self, kw: Keyword) -> bool {
        if self.at_keyword(kw) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect_keyword(&mut self, kw: Keyword) -> Result<()> {
        if self.eat_keyword(kw) {
            Ok(())
        } else {
            Err(self.error(format!(
                "期望关键字 {kw:?},实际遇到 {}",
                self.peek().describe()
            )))
        }
    }

    fn expect_token(&mut self, tok: Token) -> Result<()> {
        if *self.peek() == tok {
            self.advance();
            Ok(())
        } else {
            Err(self.error(format!(
                "期望 {},实际遇到 {}",
                tok.describe(),
                self.peek().describe()
            )))
        }
    }

    fn expect_ident(&mut self) -> Result<String> {
        match self.peek().clone() {
            Token::Ident(name) => {
                self.advance();
                Ok(name)
            }
            other => Err(self.error(format!("期望标识符,实际遇到 {}", other.describe()))),
        }
    }

    fn expect_end(&mut self) -> Result<()> {
        if matches!(self.peek(), Token::Eof) {
            Ok(())
        } else {
            Err(self.error(format!("语句结尾有多余的 {}", self.peek().describe())))
        }
    }

    fn error(&self, msg: impl Into<String>) -> QueryError {
        QueryError::Parse {
            pos: self.cur_pos(),
            msg: msg.into(),
        }
    }

    // ---- 语句 ----

    fn parse_statement(&mut self) -> Result<Statement> {
        if self.at_keyword(Keyword::Select) {
            Ok(Statement::Select(self.parse_select()?))
        } else {
            Err(self.error(format!(
                "仅支持 SELECT 语句,实际遇到 {}",
                self.peek().describe()
            )))
        }
    }

    fn parse_select(&mut self) -> Result<SelectStmt> {
        self.expect_keyword(Keyword::Select)?;

        let mut projections = vec![self.parse_select_item()?];
        while *self.peek() == Token::Comma {
            self.advance();
            projections.push(self.parse_select_item()?);
        }

        self.expect_keyword(Keyword::From)?;
        let from = self.parse_table_ref()?;

        let mut joins = Vec::new();
        while self.at_keyword(Keyword::Inner)
            || self.at_keyword(Keyword::Left)
            || self.at_keyword(Keyword::Join)
        {
            joins.push(self.parse_join()?);
        }

        let filter = if self.eat_keyword(Keyword::Where) {
            Some(self.parse_expr(0)?)
        } else {
            None
        };

        let mut group_by = Vec::new();
        if self.eat_keyword(Keyword::Group) {
            self.expect_keyword(Keyword::By)?;
            group_by.push(self.parse_expr(0)?);
            while *self.peek() == Token::Comma {
                self.advance();
                group_by.push(self.parse_expr(0)?);
            }
        }

        let having = if self.eat_keyword(Keyword::Having) {
            Some(self.parse_expr(0)?)
        } else {
            None
        };

        let mut order_by = Vec::new();
        if self.eat_keyword(Keyword::Order) {
            self.expect_keyword(Keyword::By)?;
            order_by.push(self.parse_order_item()?);
            while *self.peek() == Token::Comma {
                self.advance();
                order_by.push(self.parse_order_item()?);
            }
        }

        let limit = if self.eat_keyword(Keyword::Limit) {
            match self.advance() {
                Token::Int(v) if v >= 0 => Some(v as u64),
                other => {
                    return Err(self.error(format!("LIMIT 期望非负整数,实际 {}", other.describe())));
                }
            }
        } else {
            None
        };

        if *self.peek() == Token::Semicolon {
            self.advance();
        }

        Ok(SelectStmt {
            projections,
            from,
            joins,
            filter,
            group_by,
            having,
            order_by,
            limit,
        })
    }

    fn parse_select_item(&mut self) -> Result<SelectItem> {
        // 裸 `*`:其后紧跟逗号或 FROM 时视为通配,否则当作乘法表达式。
        if *self.peek() == Token::Star
            && matches!(self.nth(1), Token::Comma | Token::Keyword(Keyword::From))
        {
            self.advance();
            return Ok(SelectItem::Wildcard);
        }

        let expr = self.parse_expr(0)?;
        let alias = self.parse_optional_alias()?;
        Ok(SelectItem::Expr { expr, alias })
    }

    fn parse_optional_alias(&mut self) -> Result<Option<String>> {
        if self.eat_keyword(Keyword::As) {
            Ok(Some(self.expect_ident()?))
        } else if let Token::Ident(_) = self.peek() {
            Ok(Some(self.expect_ident()?))
        } else {
            Ok(None)
        }
    }

    fn parse_table_ref(&mut self) -> Result<TableRef> {
        let name = self.expect_ident()?;
        let alias = self.parse_optional_alias()?;
        Ok(TableRef { name, alias })
    }

    fn parse_join(&mut self) -> Result<Join> {
        // [INNER] JOIN  或  LEFT [OUTER] JOIN
        let join_type = if self.eat_keyword(Keyword::Left) {
            self.eat_keyword(Keyword::Outer); // 可选 OUTER
            JoinType::Left
        } else {
            self.eat_keyword(Keyword::Inner); // 可选 INNER
            JoinType::Inner
        };
        self.expect_keyword(Keyword::Join)?;
        let table = self.parse_table_ref()?;
        self.expect_keyword(Keyword::On)?;
        let on = self.parse_expr(0)?;
        Ok(Join {
            table,
            join_type,
            on,
        })
    }

    fn parse_order_item(&mut self) -> Result<OrderByExpr> {
        let expr = self.parse_expr(0)?;
        let asc = if self.eat_keyword(Keyword::Desc) {
            false
        } else {
            self.eat_keyword(Keyword::Asc);
            true
        };
        Ok(OrderByExpr { expr, asc })
    }

    // ---- 表达式(Pratt)----

    fn parse_expr(&mut self, min_bp: u8) -> Result<AstExpr> {
        let mut left = self.parse_prefix()?;

        loop {
            // 后缀 IS [NOT] NULL,优先级与比较同级(5)
            if self.at_keyword(Keyword::Is) {
                if 5 < min_bp {
                    break;
                }
                self.advance();
                let negated = self.eat_keyword(Keyword::Not);
                self.expect_keyword(Keyword::Null)?;
                left = AstExpr::IsNull {
                    expr: Box::new(left),
                    negated,
                };
                continue;
            }

            let (op, l_bp, r_bp) = match infix_binding_power(self.peek()) {
                Some(x) => x,
                None => break,
            };
            if l_bp < min_bp {
                break;
            }
            self.advance();
            let right = self.parse_expr(r_bp)?;
            left = AstExpr::Binary {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn parse_prefix(&mut self) -> Result<AstExpr> {
        let tok = self.peek().clone();
        match tok {
            Token::Int(v) => {
                self.advance();
                Ok(AstExpr::Literal(Literal::Int(v)))
            }
            Token::Float(v) => {
                self.advance();
                Ok(AstExpr::Literal(Literal::Float(v)))
            }
            Token::Str(s) => {
                self.advance();
                Ok(AstExpr::Literal(Literal::Str(s)))
            }
            Token::Keyword(Keyword::True) => {
                self.advance();
                Ok(AstExpr::Literal(Literal::Bool(true)))
            }
            Token::Keyword(Keyword::False) => {
                self.advance();
                Ok(AstExpr::Literal(Literal::Bool(false)))
            }
            Token::Keyword(Keyword::Null) => {
                self.advance();
                Ok(AstExpr::Literal(Literal::Null))
            }
            Token::Keyword(Keyword::Not) => {
                self.advance();
                let expr = self.parse_expr(5)?;
                Ok(AstExpr::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                })
            }
            Token::Minus => {
                self.advance();
                let expr = self.parse_expr(11)?;
                Ok(AstExpr::Unary {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                })
            }
            Token::LParen => {
                self.advance();
                let expr = self.parse_expr(0)?;
                self.expect_token(Token::RParen)?;
                Ok(expr)
            }
            Token::Star => {
                self.advance();
                Ok(AstExpr::Wildcard)
            }
            Token::Keyword(kw) if aggregate_func(kw).is_some() => {
                let func = aggregate_func(kw).unwrap();
                self.advance();
                self.expect_token(Token::LParen)?;
                let arg = if func == AggregateFunc::Count && *self.peek() == Token::Star {
                    self.advance();
                    AstExpr::Wildcard
                } else {
                    self.parse_expr(0)?
                };
                self.expect_token(Token::RParen)?;
                Ok(AstExpr::Aggregate {
                    func,
                    arg: Box::new(arg),
                })
            }
            Token::Ident(name) => {
                self.advance();
                if *self.peek() == Token::Dot {
                    self.advance();
                    let col = self.expect_ident()?;
                    Ok(AstExpr::Column {
                        table: Some(name),
                        name: col,
                    })
                } else {
                    Ok(AstExpr::Column { table: None, name })
                }
            }
            other => Err(self.error(format!("表达式中无法识别的 {}", other.describe()))),
        }
    }
}

/// 中缀运算符的绑定力(left_bp, right_bp)。left_bp 越大优先级越高。
fn infix_binding_power(tok: &Token) -> Option<(BinaryOp, u8, u8)> {
    let r = match tok {
        Token::Keyword(Keyword::Or) => (BinaryOp::Or, 1, 2),
        Token::Keyword(Keyword::And) => (BinaryOp::And, 3, 4),
        Token::Eq => (BinaryOp::Eq, 5, 6),
        Token::NotEq => (BinaryOp::NotEq, 5, 6),
        Token::Lt => (BinaryOp::Lt, 5, 6),
        Token::LtEq => (BinaryOp::LtEq, 5, 6),
        Token::Gt => (BinaryOp::Gt, 5, 6),
        Token::GtEq => (BinaryOp::GtEq, 5, 6),
        Token::Plus => (BinaryOp::Add, 7, 8),
        Token::Minus => (BinaryOp::Sub, 7, 8),
        Token::Star => (BinaryOp::Mul, 9, 10),
        Token::Slash => (BinaryOp::Div, 9, 10),
        _ => return None,
    };
    Some(r)
}

fn aggregate_func(kw: Keyword) -> Option<AggregateFunc> {
    Some(match kw {
        Keyword::Count => AggregateFunc::Count,
        Keyword::Sum => AggregateFunc::Sum,
        Keyword::Avg => AggregateFunc::Avg,
        Keyword::Min => AggregateFunc::Min,
        Keyword::Max => AggregateFunc::Max,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(sql: &str) -> SelectStmt {
        match parse(sql).unwrap() {
            Statement::Select(s) => s,
        }
    }

    #[test]
    fn precedence_arithmetic() {
        let s = sel("SELECT a + b * c FROM t");
        // a + (b * c)
        if let SelectItem::Expr { expr, .. } = &s.projections[0] {
            match expr {
                AstExpr::Binary {
                    op: BinaryOp::Add,
                    right,
                    ..
                } => {
                    assert!(matches!(
                        right.as_ref(),
                        AstExpr::Binary {
                            op: BinaryOp::Mul,
                            ..
                        }
                    ));
                }
                _ => panic!("期望加法在顶层"),
            }
        } else {
            panic!("期望表达式投影项");
        }
    }

    #[test]
    fn precedence_and_or_comparison() {
        let s = sel("SELECT 1 FROM t WHERE a = 1 OR b > 2 AND c < 3");
        // a=1 OR (b>2 AND c<3)
        let f = s.filter.unwrap();
        assert!(matches!(
            f,
            AstExpr::Binary {
                op: BinaryOp::Or,
                ..
            }
        ));
    }

    #[test]
    fn full_clauses() {
        let s = sel(
            "SELECT dept_id, COUNT(*) AS n FROM emp WHERE salary > 1000 \
             GROUP BY dept_id HAVING COUNT(*) > 1 ORDER BY n DESC LIMIT 5",
        );
        assert_eq!(s.projections.len(), 2);
        assert_eq!(s.group_by.len(), 1);
        assert!(s.having.is_some());
        assert_eq!(s.order_by.len(), 1);
        assert!(!s.order_by[0].asc);
        assert_eq!(s.limit, Some(5));
    }

    #[test]
    fn count_star_and_is_null() {
        let s = sel("SELECT COUNT(*) FROM t WHERE a IS NOT NULL");
        if let SelectItem::Expr { expr, .. } = &s.projections[0] {
            assert!(matches!(
                expr,
                AstExpr::Aggregate {
                    func: AggregateFunc::Count,
                    ..
                }
            ));
        }
        assert!(matches!(
            s.filter.unwrap(),
            AstExpr::IsNull { negated: true, .. }
        ));
    }
}
