//! ZScript tokens.
//!
//! ZScript is case-insensitive: identifiers keep their spelling but are
//! compared with [`Tok::is_word`]. Keywords are not separate tokens; the
//! parser recognises them in context, so engine names that look like
//! keywords elsewhere still work as identifiers.

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    /// `"text"` (escapes resolved).
    Str(String),
    /// `'name'`.
    Name(String),
    Int(i64),
    Float(f64),
    /// A word starting with digits (`02CB`): a sprite name, or a number
    /// glued to what follows (`0A_Jump`), which the parser splits.
    Word(String),
    /// Punctuation and operators (`{`, `>>>=`, `~==`, `..`, ...).
    Op(&'static str),
    Eof,
}

impl Tok {
    pub fn is_word(&self, w: &str) -> bool {
        matches!(self, Tok::Ident(s) if s.eq_ignore_ascii_case(w))
    }

    pub fn is_op(&self, o: &str) -> bool {
        matches!(self, Tok::Op(x) if *x == o)
    }
}

/// Position of a token (1-based).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

/// Longest operators first.
const OPS: &[&str] = &[
    ">>>=", "...", "<<=", ">>=", ">>>", "~==", "<>=", "**=", "==", "!=", "<=", ">=", "&&", "||", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=",
    "|=", "^=", "<<", ">>", "**", "..", "::", "->", "{", "}", "(", ")", "[", "]", ";", ",", ".", ":", "?", "=", "<", ">", "+", "-", "*", "/",
    "%", "&", "|", "^", "!", "~", "#", "@", "$", "\\",
];

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub pos: Pos,
    pub message: String,
}

pub fn lex(src: &str) -> Result<Vec<(Tok, Pos)>, LexError> {
    let b = src.as_bytes();
    let mut out: Vec<(Tok, Pos)> = Vec::new();
    let (mut i, mut line, mut line_start) = (0usize, 1u32, 0usize);
    let pos = |i: usize, line: u32, ls: usize| Pos { line, col: (i - ls) as u32 + 1 };
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
            line_start = i;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                if b[i] == b'\n' {
                    line += 1;
                    line_start = i + 1;
                }
                i += 1;
            }
            i = (i + 2).min(b.len());
            continue;
        }
        let p = pos(i, line, line_start);
        if c == b'"' || c == b'\'' {
            let quote = c;
            i += 1;
            let mut s = String::new();
            while i < b.len() && b[i] != quote {
                if b[i] == b'\\' && i + 1 < b.len() {
                    i += 1;
                    s.push(match b[i] {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'0' => '\0',
                        b'\n' => {
                            line += 1;
                            line_start = i + 1;
                            '\n'
                        }
                        other => other as char,
                    });
                } else {
                    if b[i] == b'\n' {
                        line += 1;
                        line_start = i + 1;
                    }
                    // Strings are mostly ASCII; keep UTF-8 sequences intact.
                    let ch_len = utf8_len(b[i]);
                    s.push_str(std::str::from_utf8(&b[i..(i + ch_len).min(b.len())]).unwrap_or("?"));
                    i += ch_len - 1;
                }
                i += 1;
            }
            if i >= b.len() {
                return Err(LexError { pos: p, message: "unterminated string".into() });
            }
            i += 1;
            // Adjacent strings are joined by the expression parser, not here:
            // state frames use separate sprite and frame strings.
            out.push((if quote == b'\'' { Tok::Name(s) } else { Tok::Str(s) }, p));
            continue;
        }
        if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let start = i;
            if c == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X')) {
                i += 2;
                while i < b.len() && b[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let v = i64::from_str_radix(&src[start + 2..i], 16).map_err(|_| LexError { pos: p, message: "bad hex number".into() })?;
                // Optional u/l suffixes.
                while i < b.len() && matches!(b[i], b'u' | b'U' | b'l' | b'L') {
                    i += 1;
                }
                out.push((Tok::Int(v), p));
                continue;
            }
            let mut float = false;
            while i < b.len() {
                match b[i] {
                    b'0'..=b'9' => i += 1,
                    // `1..2` is a range, not a float.
                    b'.' if b.get(i + 1) != Some(&b'.') && !float => {
                        float = true;
                        i += 1;
                    }
                    b'e' | b'E' if b.get(i + 1).is_some_and(|d| d.is_ascii_digit() || *d == b'-' || *d == b'+') => {
                        float = true;
                        i += 2;
                    }
                    _ => break,
                }
            }
            let text = &src[start..i];
            // A word that starts with digits (`02CB`: sprite names) is a
            // name, unless the letters are only a number suffix.
            if !float && i < b.len() && (b[i].is_ascii_alphabetic() || b[i] == b'_') {
                let mut j = i;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                let suffix = src[i..j].bytes().all(|c| matches!(c, b'u' | b'U' | b'l' | b'L' | b'f' | b'F')) && j - i <= 2;
                if !suffix {
                    out.push((Tok::Word(src[start..j].to_string()), p));
                    i = j;
                    continue;
                }
            }
            // Suffixes: f (float), u/l (integers).
            if i < b.len() && matches!(b[i], b'f' | b'F') {
                float = true;
                i += 1;
            }
            while i < b.len() && matches!(b[i], b'u' | b'U' | b'l' | b'L') {
                i += 1;
            }
            let tok = if float {
                Tok::Float(text.parse().map_err(|_| LexError { pos: p, message: format!("bad number {text}") })?)
            } else {
                Tok::Int(text.parse().map_err(|_| LexError { pos: p, message: format!("bad number {text}") })?)
            };
            out.push((tok, p));
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push((Tok::Ident(src[start..i].to_string()), p));
            continue;
        }
        let Some(op) = OPS.iter().find(|o| b[i..].starts_with(o.as_bytes())) else {
            return Err(LexError { pos: p, message: format!("unexpected character {:?}", c as char) });
        };
        i += op.len();
        out.push((Tok::Op(op), p));
    }
    out.push((Tok::Eof, pos(i, line, line_start)));
    Ok(out)
}

fn utf8_len(first: u8) -> usize {
    match first {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(s).unwrap().into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn tokens() {
        assert_eq!(
            toks("x >>>= 0x1F; 'Name' \"a\" \"b\" 1.5 .5 1e3 2..4 a~==b // c\n/* d */ y"),
            [
                Tok::Ident("x".into()),
                Tok::Op(">>>="),
                Tok::Int(31),
                Tok::Op(";"),
                Tok::Name("Name".into()),
                Tok::Str("a".into()),
                Tok::Str("b".into()),
                Tok::Float(1.5),
                Tok::Float(0.5),
                Tok::Float(1000.0),
                Tok::Int(2),
                Tok::Op(".."),
                Tok::Int(4),
                Tok::Ident("a".into()),
                Tok::Op("~=="),
                Tok::Ident("b".into()),
                Tok::Ident("y".into()),
                Tok::Eof,
            ]
        );
        let l = lex("a\n  b").unwrap();
        assert_eq!(l[1].1, Pos { line: 2, col: 3 });
    }
}
