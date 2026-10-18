//! Tokenizer for ZDoom-style text lumps (TEXTURES, DECORATE, MAPINFO, ...):
//! identifiers, numbers, quoted strings and punctuation, with `//` and
//! `/* */` comments.

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    /// Identifiers and bare words (`Actor`, `+NOBLOCKMAP` splits into `+`,
    /// `NOBLOCKMAP`).
    Ident(String),
    Str(String),
    Int(i64),
    Float(f64),
    /// Single punctuation character (`{`, `}`, `,`, `;`, `=`, `:`, ...).
    Punct(char),
}

impl Token {
    pub fn is_punct(&self, c: char) -> bool {
        matches!(self, Token::Punct(p) if *p == c)
    }

    /// Identifier or string text, compared case-insensitively by callers.
    pub fn text(&self) -> Option<&str> {
        match self {
            Token::Ident(s) | Token::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn is_word(&self, w: &str) -> bool {
        matches!(self, Token::Ident(s) if s.eq_ignore_ascii_case(w))
    }

    pub fn number(&self) -> Option<f64> {
        match *self {
            Token::Int(i) => Some(i as f64),
            Token::Float(f) => Some(f),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Scanner {
    tokens: Vec<(Token, u32)>,
    pos: usize,
}

impl Scanner {
    pub fn new(src: &str) -> Scanner {
        let b = src.as_bytes();
        let mut tokens = Vec::new();
        let mut i = 0;
        let mut line = 1u32;
        while i < b.len() {
            let c = b[i];
            if c == b'\n' {
                line += 1;
                i += 1;
            } else if c.is_ascii_whitespace() {
                i += 1;
            } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    if b[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 2;
            } else if c == b'"' {
                let mut s = String::new();
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' && i + 1 < b.len() {
                        i += 1;
                        s.push(match b[i] {
                            b'n' => '\n',
                            b't' => '\t',
                            other => other as char,
                        });
                    } else {
                        if b[i] == b'\n' {
                            line += 1;
                        }
                        s.push(b[i] as char);
                    }
                    i += 1;
                }
                i += 1;
                tokens.push((Token::Str(s), line));
            } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
                let start = i;
                if c == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X')) {
                    i += 2;
                    while i < b.len() && b[i].is_ascii_hexdigit() {
                        i += 1;
                    }
                    let v = i64::from_str_radix(&src[start + 2..i], 16).unwrap_or(0);
                    tokens.push((Token::Int(v), line));
                    continue;
                }
                let mut float = false;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.' || ((b[i] == b'e' || b[i] == b'E') && float)) {
                    if b[i] == b'.' {
                        float = true;
                    }
                    i += 1;
                }
                let text = &src[start..i];
                tokens.push((
                    if float { Token::Float(text.parse().unwrap_or(0.0)) } else { Token::Int(text.parse().unwrap_or(0)) },
                    line,
                ));
            } else if c.is_ascii_alphabetic() || c == b'_' {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b'.')) {
                    i += 1;
                }
                tokens.push((Token::Ident(src[start..i].to_string()), line));
            } else {
                tokens.push((Token::Punct(c as char), line));
                i += 1;
            }
        }
        Scanner { tokens, pos: 0 }
    }

    pub fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|t| &t.0)
    }

    pub fn peek_at(&self, n: usize) -> Option<&Token> {
        self.tokens.get(self.pos + n).map(|t| &t.0)
    }

    pub fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).map(|t| t.0.clone());
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    /// Line of the next token (for error messages).
    pub fn line(&self) -> u32 {
        self.tokens.get(self.pos).or(self.tokens.last()).map_or(0, |t| t.1)
    }

    pub fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    /// Consume the punctuation `c` if it is next.
    pub fn eat(&mut self, c: char) -> bool {
        if self.peek().is_some_and(|t| t.is_punct(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    pub fn eat_word(&mut self, w: &str) -> bool {
        if self.peek().is_some_and(|t| t.is_word(w)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// A signed number (`-12`, `0.5`).
    pub fn number(&mut self) -> Option<f64> {
        let neg = self.eat('-');
        if !neg {
            self.eat('+');
        }
        let v = self.peek()?.number()?;
        self.pos += 1;
        Some(if neg { -v } else { v })
    }

    /// Skip a balanced `{ ... }` block (the opening brace must be next).
    pub fn skip_block(&mut self) {
        if !self.eat('{') {
            return;
        }
        let mut depth = 1;
        while let Some(t) = self.next() {
            if t.is_punct('{') {
                depth += 1;
            } else if t.is_punct('}') {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        let mut s = Scanner::new("Actor Foo : Bar 123 // c\n{ /* x\n y */ Radius 8.5 +SOLID \"a b\" 0x10 -3 }");
        assert!(s.next().unwrap().is_word("actor"));
        assert_eq!(s.next(), Some(Token::Ident("Foo".into())));
        assert!(s.eat(':'));
        assert_eq!(s.next(), Some(Token::Ident("Bar".into())));
        assert_eq!(s.next(), Some(Token::Int(123)));
        assert!(s.eat('{'));
        assert_eq!(s.line(), 3);
        assert!(s.eat_word("radius"));
        assert_eq!(s.number(), Some(8.5));
        assert!(s.eat('+'));
        assert_eq!(s.next(), Some(Token::Ident("SOLID".into())));
        assert_eq!(s.next(), Some(Token::Str("a b".into())));
        assert_eq!(s.next(), Some(Token::Int(16)));
        assert_eq!(s.number(), Some(-3.0));
        assert!(s.eat('}'));
        assert!(s.at_end());
    }
}
