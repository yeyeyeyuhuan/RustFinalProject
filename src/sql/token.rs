//! 词法单元(Token)与关键字定义。

/// 关键字(大小写不敏感)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    Select,
    From,
    Where,
    Group,
    By,
    Having,
    Order,
    Asc,
    Desc,
    Limit,
    Join,
    Inner,
    Left,
    Outer,
    On,
    And,
    Or,
    Not,
    As,
    Is,
    Null,
    True,
    False,
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

impl Keyword {
    /// 将单词(已知为标识符形态)尝试解析为关键字,大小写不敏感。
    pub fn from_word(word: &str) -> Option<Keyword> {
        let kw = match word.to_ascii_uppercase().as_str() {
            "SELECT" => Keyword::Select,
            "FROM" => Keyword::From,
            "WHERE" => Keyword::Where,
            "GROUP" => Keyword::Group,
            "BY" => Keyword::By,
            "HAVING" => Keyword::Having,
            "ORDER" => Keyword::Order,
            "ASC" => Keyword::Asc,
            "DESC" => Keyword::Desc,
            "LIMIT" => Keyword::Limit,
            "JOIN" => Keyword::Join,
            "INNER" => Keyword::Inner,
            "LEFT" => Keyword::Left,
            "OUTER" => Keyword::Outer,
            "ON" => Keyword::On,
            "AND" => Keyword::And,
            "OR" => Keyword::Or,
            "NOT" => Keyword::Not,
            "AS" => Keyword::As,
            "IS" => Keyword::Is,
            "NULL" => Keyword::Null,
            "TRUE" => Keyword::True,
            "FALSE" => Keyword::False,
            "COUNT" => Keyword::Count,
            "SUM" => Keyword::Sum,
            "AVG" => Keyword::Avg,
            "MIN" => Keyword::Min,
            "MAX" => Keyword::Max,
            _ => return None,
        };
        Some(kw)
    }
}

/// 词法单元。
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Keyword(Keyword),
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),
    Star,
    Plus,
    Minus,
    Slash,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    LParen,
    RParen,
    Comma,
    Dot,
    Semicolon,
    Eof,
}

impl Token {
    /// 用于报错的简短描述。
    pub fn describe(&self) -> String {
        match self {
            Token::Keyword(k) => format!("关键字 {k:?}"),
            Token::Ident(s) => format!("标识符 `{s}`"),
            Token::Int(v) => format!("整数 {v}"),
            Token::Float(v) => format!("浮点数 {v}"),
            Token::Str(s) => format!("字符串 '{s}'"),
            Token::Eof => "输入结束".to_string(),
            other => format!("`{other:?}`"),
        }
    }
}

/// 带位置(字节偏移)的 Token。
#[derive(Debug, Clone)]
pub struct SpannedToken {
    pub token: Token,
    pub start: usize,
    /// Token 结束字节偏移(保留用于未来的区间高亮报错)。
    #[allow(dead_code)]
    pub end: usize,
}
