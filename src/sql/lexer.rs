//! 词法分析器:SQL 字符串 → 带位置的 Token 流。跳过空白,记录字节偏移以便报错。

use super::token::{Keyword, SpannedToken, Token};
use crate::error::{QueryError, Result};

/// 把整段 SQL 切分为 Token 序列(以 `Eof` 结尾)。
pub fn tokenize(input: &str) -> Result<Vec<SpannedToken>> {
    Lexer::new(input).run()
}

struct Lexer {
    chars: Vec<char>,
    offsets: Vec<usize>,
    total: usize,
    pos: usize,
}

impl Lexer {
    fn new(input: &str) -> Self {
        let chars: Vec<char> = input.chars().collect();
        let offsets: Vec<usize> = input.char_indices().map(|(i, _)| i).collect();
        Lexer {
            chars,
            offsets,
            total: input.len(),
            pos: 0,
        }
    }

    /// 第 `idx` 个字符的字节偏移(越界返回输入总长)。
    fn byte(&self, idx: usize) -> usize {
        self.offsets.get(idx).copied().unwrap_or(self.total)
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn run(mut self) -> Result<Vec<SpannedToken>> {
        let mut out = Vec::new();
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.pos += 1;
                continue;
            }
            let start = self.byte(self.pos);
            let token = match c {
                '(' => self.single(Token::LParen),
                ')' => self.single(Token::RParen),
                ',' => self.single(Token::Comma),
                ';' => self.single(Token::Semicolon),
                '+' => self.single(Token::Plus),
                '-' => self.single(Token::Minus),
                '*' => self.single(Token::Star),
                '/' => self.single(Token::Slash),
                '.' => self.single(Token::Dot),
                '=' => self.single(Token::Eq),
                '<' => self.less(),
                '>' => self.greater(),
                '!' => self.bang(start)?,
                '\'' => self.string(start)?,
                c if c.is_ascii_digit() => self.number(),
                c if c.is_alphabetic() || c == '_' => self.ident(),
                other => {
                    return Err(QueryError::Lex {
                        pos: start,
                        msg: format!("无法识别的字符 `{other}`"),
                    });
                }
            };
            let end = self.byte(self.pos);
            out.push(SpannedToken { token, start, end });
        }
        let end = self.total;
        out.push(SpannedToken {
            token: Token::Eof,
            start: end,
            end,
        });
        Ok(out)
    }

    fn single(&mut self, token: Token) -> Token {
        self.pos += 1;
        token
    }

    fn less(&mut self) -> Token {
        self.pos += 1;
        match self.peek() {
            Some('=') => {
                self.pos += 1;
                Token::LtEq
            }
            Some('>') => {
                self.pos += 1;
                Token::NotEq
            }
            _ => Token::Lt,
        }
    }

    fn greater(&mut self) -> Token {
        self.pos += 1;
        if self.peek() == Some('=') {
            self.pos += 1;
            Token::GtEq
        } else {
            Token::Gt
        }
    }

    fn bang(&mut self, start: usize) -> Result<Token> {
        self.pos += 1;
        if self.peek() == Some('=') {
            self.pos += 1;
            Ok(Token::NotEq)
        } else {
            Err(QueryError::Lex {
                pos: start,
                msg: "`!` 后应跟 `=`".to_string(),
            })
        }
    }

    fn string(&mut self, start: usize) -> Result<Token> {
        self.pos += 1; // 跳过开引号
        let mut s = String::new();
        loop {
            match self.peek() {
                Some('\'') => {
                    // 连续两个单引号表示一个字面单引号
                    if self.peek2() == Some('\'') {
                        s.push('\'');
                        self.pos += 2;
                    } else {
                        self.pos += 1; // 跳过闭引号
                        return Ok(Token::Str(s));
                    }
                }
                Some(c) => {
                    s.push(c);
                    self.pos += 1;
                }
                None => {
                    return Err(QueryError::Lex {
                        pos: start,
                        msg: "字符串字面量未闭合".to_string(),
                    });
                }
            }
        }
    }

    fn number(&mut self) -> Token {
        let begin = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let mut is_float = false;
        if self.peek() == Some('.') && self.peek2().is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            self.pos += 1; // 小数点
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let text: String = self.chars[begin..self.pos].iter().collect();
        if is_float {
            Token::Float(text.parse().unwrap_or(0.0))
        } else {
            match text.parse::<i64>() {
                Ok(v) => Token::Int(v),
                Err(_) => Token::Float(text.parse().unwrap_or(0.0)),
            }
        }
    }

    fn ident(&mut self) -> Token {
        let begin = self.pos;
        while self.peek().is_some_and(|c| c.is_alphanumeric() || c == '_') {
            self.pos += 1;
        }
        let text: String = self.chars[begin..self.pos].iter().collect();
        match Keyword::from_word(&text) {
            Some(kw) => Token::Keyword(kw),
            None => Token::Ident(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(sql: &str) -> Vec<Token> {
        tokenize(sql)
            .unwrap()
            .into_iter()
            .map(|t| t.token)
            .collect()
    }

    #[test]
    fn basic_select() {
        let toks = kinds("SELECT a, b FROM t WHERE a >= 10");
        assert_eq!(toks[0], Token::Keyword(Keyword::Select));
        assert_eq!(toks[1], Token::Ident("a".into()));
        assert_eq!(toks[2], Token::Comma);
        assert!(toks.contains(&Token::GtEq));
        assert_eq!(*toks.last().unwrap(), Token::Eof);
    }

    #[test]
    fn numbers_and_strings() {
        let toks = kinds("3 3.5 'hi''there'");
        assert_eq!(toks[0], Token::Int(3));
        assert_eq!(toks[1], Token::Float(3.5));
        assert_eq!(toks[2], Token::Str("hi'there".into()));
    }

    #[test]
    fn operators() {
        let toks = kinds("<> != <= >= < > =");
        assert_eq!(
            toks[..7],
            [
                Token::NotEq,
                Token::NotEq,
                Token::LtEq,
                Token::GtEq,
                Token::Lt,
                Token::Gt,
                Token::Eq
            ]
        );
    }
}
