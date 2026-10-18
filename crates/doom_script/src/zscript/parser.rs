//! ZScript parser (recursive descent, precedence climbing for expressions).
//!
//! Errors are collected, not fatal: a declaration or member that fails to
//! parse is skipped up to its closing brace or semicolon and parsing goes
//! on, so one construct this parser doesn't know costs one item, not the
//! whole mod.

use super::ast::*;
use super::lexer::{lex, Pos, Tok};

#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub pos: Pos,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}:{}: {}", self.pos.line, self.pos.col, self.message)
    }
}

type R<T> = Result<T, ParseError>;

/// Parse a ZScript lump. Returns what parsed and the errors met.
pub fn parse(src: &str) -> (File, Vec<ParseError>) {
    let toks = match lex(src) {
        Ok(t) => t,
        Err(e) => return (File::default(), vec![ParseError { pos: e.pos, message: e.message }]),
    };
    let mut p = Parser { toks, i: 0, errors: Vec::new(), decorate: false };
    let file = p.file();
    (file, p.errors)
}

/// Built-in type names (anything else followed by a name is a user type).
const BASIC_TYPES: &[&str] = &[
    "int", "uint", "int8", "uint8", "int16", "uint16", "int32", "uint32", "bool", "float", "double", "float64", "string", "name",
    "sound", "state", "color", "vector2", "vector3", "vector4", "fvector2", "fvector3", "quat", "statelabel", "spriteid",
    "textureid", "voidptr", "void", "let", "char", "byte", "sbyte", "short", "ushort",
];

/// Words that start a member declaration.
const MEMBER_FLAGS: &[&str] = &[
    "native", "static", "virtual", "override", "final", "private", "protected", "public", "meta", "transient", "readonly", "action",
    "const", "ui", "play", "clearscope", "virtualscope", "internal", "vararg", "deprecated", "version", "abstract", "latent",
    "sealed", "nosave",
];

struct Parser {
    toks: Vec<(Tok, Pos)>,
    i: usize,
    errors: Vec<ParseError>,
    /// DECORATE: a frame and its action end at the end of the line.
    decorate: bool,
}

impl Parser {
    // ---- token helpers ----------------------------------------------------------

    fn peek(&self) -> &Tok {
        &self.toks[self.i.min(self.toks.len() - 1)].0
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.i + n).min(self.toks.len() - 1)].0
    }

    fn pos(&self) -> Pos {
        self.toks[self.i.min(self.toks.len() - 1)].1
    }

    fn next(&mut self) -> Tok {
        let t = self.peek().clone();
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
        t
    }

    /// Split a digits-first word (`0A_JumpIf`) into a number and a name,
    /// where a number is expected.
    fn split_word(&mut self) {
        let Tok::Word(w) = self.peek().clone() else { return };
        let digits = w.bytes().take_while(u8::is_ascii_digit).count();
        let pos = self.pos();
        let n: i64 = w[..digits].parse().unwrap_or(0);
        self.toks[self.i] = (Tok::Int(n), pos);
        self.toks.insert(self.i + 1, (Tok::Ident(w[digits..].to_string()), Pos { line: pos.line, col: pos.col + digits as u32 }));
    }

    fn at_end(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }

    fn err<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(ParseError { pos: self.pos(), message: msg.into() })
    }

    fn eat_op(&mut self, o: &str) -> bool {
        if self.peek().is_op(o) {
            self.next();
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, w: &str) -> bool {
        if self.peek().is_word(w) {
            self.next();
            true
        } else {
            false
        }
    }

    fn expect_op(&mut self, o: &str) -> R<()> {
        if self.eat_op(o) {
            Ok(())
        } else {
            self.err(format!("expected '{o}', found {:?}", self.peek()))
        }
    }

    fn ident(&mut self) -> R<String> {
        match self.next() {
            // Class names may start with digits (`50CaseSpawn`).
            Tok::Ident(s) | Tok::Word(s) => Ok(s),
            t => {
                self.i -= 1;
                self.err(format!("expected a name, found {t:?}"))
            }
        }
    }

    /// `A.B.C` as one string.
    fn dotted(&mut self) -> R<String> {
        let mut s = self.ident()?;
        while self.peek().is_op(".") && matches!(self.peek_at(1), Tok::Ident(_) | Tok::Word(_)) {
            self.next();
            s.push('.');
            s.push_str(&self.ident()?);
        }
        Ok(s)
    }

    /// Error recovery: skip to the end of the current item (a `;` at depth 0
    /// or a balanced block).
    fn recover(&mut self) {
        let mut depth = 0;
        while !self.at_end() {
            match self.peek() {
                Tok::Op("{") => depth += 1,
                Tok::Op("}") => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                    if depth == 0 {
                        self.next();
                        self.eat_op(";");
                        return;
                    }
                }
                Tok::Op(";") if depth == 0 => {
                    self.next();
                    return;
                }
                _ => {}
            }
            self.next();
        }
    }

    fn record<T>(&mut self, r: R<T>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.errors.push(e);
                self.recover();
                None
            }
        }
    }

    // ---- top level --------------------------------------------------------------

    fn file(&mut self) -> File {
        let mut f = File::default();
        while !self.at_end() {
            if self.eat_op(";") {
                continue;
            }
            if self.peek().is_word("version") && matches!(self.peek_at(1), Tok::Str(_)) {
                self.next();
                if let Tok::Str(v) = self.next() {
                    f.version = Some(v);
                }
                self.eat_op(";");
                continue;
            }
            if self.peek().is_op("#") && self.peek_at(1).is_word("include") {
                self.next();
                self.next();
                if let Tok::Str(path) = self.next() {
                    f.includes.push(path);
                }
                continue;
            }
            let start = self.i;
            let r = self.decl();
            if let Some(Some(d)) = self.record(r) {
                f.decls.push(d);
            }
            if self.i == start {
                // No progress: drop a token to avoid looping.
                self.next();
            }
        }
        f
    }

    fn decl(&mut self) -> R<Option<Decl>> {
        let extend = self.eat_word("extend");
        let mixin = !extend && self.peek().is_word("mixin") && self.peek_at(1).is_word("class");
        if mixin {
            self.next();
        }
        if self.eat_word("class") {
            return Ok(Some(Decl::Class(self.class(extend, mixin)?)));
        }
        if self.eat_word("struct") {
            return Ok(Some(Decl::Struct(self.structure(extend)?)));
        }
        if self.eat_word("enum") {
            return Ok(Some(Decl::Enum(self.enumeration()?)));
        }
        if self.eat_word("const") {
            return Ok(Some(Decl::Const(self.constant()?)));
        }
        self.err(format!("expected a declaration, found {:?}", self.peek()))
    }

    fn class(&mut self, extend: bool, mixin: bool) -> R<ClassDecl> {
        let pos = self.pos();
        let name = self.ident()?;
        let mut c = ClassDecl { name, extend, mixin, pos, ..Default::default() };
        loop {
            if self.eat_op(":") {
                c.parent = Some(self.dotted()?);
            } else if self.eat_word("replaces") {
                c.replaces = Some(self.dotted()?);
            } else if self.peek().is_word("version") {
                self.next();
                self.expect_op("(")?;
                let v = match self.next() {
                    Tok::Str(s) => s,
                    t => format!("{t:?}"),
                };
                self.expect_op(")")?;
                c.flags.push(format!("version({v})"));
            } else if let Tok::Ident(w) = self.peek().clone() {
                if self.peek_at(1).is_op("{") || matches!(self.peek_at(1), Tok::Ident(_)) || self.peek_at(1).is_op(";") {
                    self.next();
                    c.flags.push(w.to_ascii_lowercase());
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        if self.eat_op(";") {
            // `class X : Y;` - the rest of the file is the body (old style).
            while !self.at_end() {
                if let Some(Some(m)) = { let r = self.member(); self.record(r) } {
                    c.members.push(m);
                }
            }
            return Ok(c);
        }
        c.members = self.members()?;
        Ok(c)
    }

    fn structure(&mut self, extend: bool) -> R<StructDecl> {
        let pos = self.pos();
        let name = self.ident()?;
        let mut s = StructDecl { name, extend, pos, ..Default::default() };
        while let Tok::Ident(w) = self.peek().clone() {
            self.next();
            if w.eq_ignore_ascii_case("version") && self.eat_op("(") {
                while !self.eat_op(")") && !self.at_end() {
                    self.next();
                }
            }
            s.flags.push(w.to_ascii_lowercase());
        }
        s.members = self.members()?;
        self.eat_op(";");
        Ok(s)
    }

    /// `{ member* }`
    fn members(&mut self) -> R<Vec<Member>> {
        self.expect_op("{")?;
        let mut out = Vec::new();
        while !self.eat_op("}") {
            if self.at_end() {
                return self.err("unterminated class body");
            }
            let start = self.i;
            let r = self.member();
            if let Some(Some(m)) = self.record(r) {
                out.push(m);
            }
            if self.i == start {
                self.next();
            }
        }
        Ok(out)
    }

    fn enumeration(&mut self) -> R<EnumDecl> {
        let name = self.ident()?;
        if self.eat_op(":") {
            self.ident()?;
        }
        self.expect_op("{")?;
        let mut e = EnumDecl { name, values: Vec::new() };
        while !self.eat_op("}") {
            let n = self.ident()?;
            let v = if self.eat_op("=") { Some(self.expr()?) } else { None };
            e.values.push((n, v));
            if !self.eat_op(",") {
                self.expect_op("}")?;
                break;
            }
        }
        self.eat_op(";");
        Ok(e)
    }

    fn constant(&mut self) -> R<ConstDecl> {
        let name = self.ident()?;
        self.expect_op("=")?;
        let value = self.expr()?;
        self.expect_op(";")?;
        Ok(ConstDecl { name, value })
    }

    // ---- class members ----------------------------------------------------------

    fn member(&mut self) -> R<Option<Member>> {
        if self.eat_op(";") {
            return Ok(None);
        }
        if self.peek().is_word("default") && self.peek_at(1).is_op("{") {
            self.next();
            return Ok(Some(Member::Default(self.defaults()?)));
        }
        if self.peek().is_word("states") && (self.peek_at(1).is_op("{") || self.peek_at(1).is_op("(")) {
            self.next();
            return Ok(Some(Member::States(self.states()?)));
        }
        if self.eat_word("enum") {
            return Ok(Some(Member::Enum(self.enumeration()?)));
        }
        if self.peek().is_word("const") && matches!(self.peek_at(1), Tok::Ident(_)) && self.peek_at(2).is_op("=") {
            self.next();
            return Ok(Some(Member::Const(self.constant()?)));
        }
        if self.eat_word("struct") {
            return Ok(Some(Member::Struct(self.structure(false)?)));
        }
        if self.peek().is_word("property") && matches!(self.peek_at(1), Tok::Ident(_)) {
            self.next();
            let name = self.ident()?;
            self.expect_op(":")?;
            let mut fields = vec![self.ident()?];
            while self.eat_op(",") {
                fields.push(self.ident()?);
            }
            self.expect_op(";")?;
            return Ok(Some(Member::Property { name, fields }));
        }
        if self.peek().is_word("flagdef") {
            self.next();
            let name = self.ident()?;
            self.expect_op(":")?;
            let field = self.ident()?;
            self.expect_op(",")?;
            let bit = match self.next() {
                Tok::Int(i) => i,
                _ => return self.err("expected a bit number"),
            };
            self.expect_op(";")?;
            return Ok(Some(Member::FlagDef { name, field, bit }));
        }
        if self.peek().is_word("mixin") {
            self.next();
            let n = self.ident()?;
            self.expect_op(";")?;
            return Ok(Some(Member::Mixin(n)));
        }
        if self.peek().is_word("static") && self.peek_at(1).is_word("const") {
            self.next();
            self.next();
            let ty = self.type_ref()?;
            let name = self.ident()?;
            if self.eat_op("[") {
                while !self.eat_op("]") {
                    self.next();
                }
            }
            self.expect_op("=")?;
            self.expect_op("{")?;
            let mut values = Vec::new();
            while !self.eat_op("}") {
                values.push(self.expr()?);
                if !self.eat_op(",") {
                    self.expect_op("}")?;
                    break;
                }
            }
            self.expect_op(";")?;
            return Ok(Some(Member::StaticArray { ty, name, values }));
        }
        // Fields and functions: flags, type(s), name.
        let pos = self.pos();
        let mut flags = Vec::new();
        loop {
            let t = self.peek().clone();
            let Tok::Ident(w) = &t else { break };
            let lw = w.to_ascii_lowercase();
            if !MEMBER_FLAGS.contains(&lw.as_str()) {
                break;
            }
            // `action` / `version("4.0")` / `deprecated("4.0", "msg")` take
            // optional arguments.
            self.next();
            if self.peek().is_op("(") && matches!(lw.as_str(), "action" | "version" | "deprecated") {
                let mut depth = 0;
                loop {
                    match self.next() {
                        Tok::Op("(") => depth += 1,
                        Tok::Op(")") => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        Tok::Eof => break,
                        _ => {}
                    }
                }
            }
            flags.push(lw);
        }
        let mut returns = vec![self.type_ref()?];
        while self.eat_op(",") {
            returns.push(self.type_ref()?);
        }
        let name = self.ident()?;
        if self.eat_op("(") {
            let params = self.params()?;
            let is_const = self.eat_word("const");
            let body = if self.eat_op(";") {
                None
            } else {
                Some(self.block()?)
            };
            return Ok(Some(Member::Function(FuncDecl { flags, returns, name, params, is_const, body, pos })));
        }
        let ty = returns.remove(0);
        let mut names = vec![(name, self.dims()?)];
        while self.eat_op(",") {
            let n = self.ident()?;
            names.push((n, self.dims()?));
        }
        self.expect_op(";")?;
        Ok(Some(Member::Field(FieldDecl { flags, ty, names, pos })))
    }

    fn dims(&mut self) -> R<Vec<Expr>> {
        let mut d = Vec::new();
        while self.eat_op("[") {
            if self.eat_op("]") {
                d.push(Expr::Int(0));
                continue;
            }
            d.push(self.expr()?);
            self.expect_op("]")?;
        }
        Ok(d)
    }

    fn params(&mut self) -> R<Vec<Param>> {
        let mut out = Vec::new();
        if self.eat_op(")") {
            return Ok(out);
        }
        loop {
            if self.eat_op("...") {
                self.expect_op(")")?;
                return Ok(out);
            }
            if self.peek().is_word("void") && self.peek_at(1).is_op(")") {
                self.next();
                self.next();
                return Ok(out);
            }
            let mut flags = Vec::new();
            while self.peek().is_word("out") || self.peek().is_word("in") || self.peek().is_word("readonly") {
                if let Tok::Ident(w) = self.next() {
                    flags.push(w.to_ascii_lowercase());
                }
            }
            let ty = self.type_ref()?;
            let name = self.ident()?;
            let default = if self.eat_op("=") { Some(self.expr()?) } else { None };
            out.push(Param { flags, ty, name, default });
            if self.eat_op(")") {
                return Ok(out);
            }
            self.expect_op(",")?;
        }
    }

    fn type_ref(&mut self) -> R<TypeRef> {
        let t = self.peek().clone();
        let Tok::Ident(w) = t else { return self.err(format!("expected a type, found {t:?}")) };
        let lw = w.to_ascii_lowercase();
        self.next();
        let ty = match lw.as_str() {
            "let" => TypeRef::Let,
            "void" => TypeRef::Void,
            "class" => {
                if self.eat_op("<") {
                    let n = self.dotted()?;
                    self.close_angle()?;
                    TypeRef::Class(Some(n))
                } else {
                    TypeRef::Class(None)
                }
            }
            "array" if self.peek().is_op("<") => {
                self.next();
                let inner = self.type_ref()?;
                self.close_angle()?;
                TypeRef::DynArray(Box::new(inner))
            }
            "readonly" if self.peek().is_op("<") => {
                self.next();
                let inner = self.type_ref()?;
                self.close_angle()?;
                TypeRef::ReadOnly(Box::new(inner))
            }
            "map" | "mapiterator" if self.peek().is_op("<") => {
                self.next();
                let k = self.type_ref()?;
                self.expect_op(",")?;
                let v = self.type_ref()?;
                self.close_angle()?;
                TypeRef::Map(lw, Box::new(k), Box::new(v))
            }
            _ => {
                let mut n = w;
                while self.peek().is_op(".") && matches!(self.peek_at(1), Tok::Ident(_)) {
                    self.next();
                    n.push('.');
                    n.push_str(&self.ident()?);
                }
                TypeRef::Named(n)
            }
        };
        Ok(ty)
    }

    /// `>` closing a type argument list; `>>` closes two.
    fn close_angle(&mut self) -> R<()> {
        if self.eat_op(">") {
            return Ok(());
        }
        if self.peek().is_op(">>") {
            // Split: consume one '>' and leave one.
            let p = self.pos();
            self.toks[self.i] = (Tok::Op(">"), p);
            return Ok(());
        }
        if self.peek().is_op(">>>") {
            let p = self.pos();
            self.toks[self.i] = (Tok::Op(">>"), p);
            return Ok(());
        }
        self.err("expected '>'")
    }

    // ---- default blocks ------------------------------------------------------------

    fn defaults(&mut self) -> R<Vec<DefaultItem>> {
        self.expect_op("{")?;
        let mut out = Vec::new();
        while !self.eat_op("}") {
            if self.at_end() {
                return self.err("unterminated default block");
            }
            if self.eat_op(";") {
                continue;
            }
            if self.peek().is_op("+") || self.peek().is_op("-") {
                let on = self.next().is_op("+");
                out.push(DefaultItem::Flag { on, name: self.dotted()? });
                continue;
            }
            let start = self.i;
            let r = self.property();
            if let Some(item) = self.record(r) {
                out.push(item);
            }
            if self.i == start {
                self.next();
            }
        }
        Ok(out)
    }

    fn property(&mut self) -> R<DefaultItem> {
        let name = self.dotted()?;
        let mut values = Vec::new();
        if !self.eat_op(";") {
            loop {
                values.push(self.expr()?);
                if self.eat_op(";") {
                    break;
                }
                if self.peek().is_op("}") {
                    break;
                }
                self.expect_op(",")?;
            }
        }
        Ok(DefaultItem::Property { name, values })
    }

    // ---- states ----------------------------------------------------------------------

    fn states(&mut self) -> R<StatesDecl> {
        let mut d = StatesDecl::default();
        if self.eat_op("(") {
            while !self.eat_op(")") {
                d.scope.push(self.ident()?);
                self.eat_op(",");
            }
        }
        self.expect_op("{")?;
        while !self.eat_op("}") {
            if self.at_end() {
                return self.err("unterminated states block");
            }
            if self.eat_op(";") {
                continue;
            }
            let start = self.i;
            let r = self.state_item();
            if let Some(item) = self.record(r) {
                d.items.push(item);
            }
            if self.i == start {
                self.next();
            }
        }
        Ok(d)
    }

    fn state_item(&mut self) -> R<StateItem> {
        // Label: name[.name]* ':'
        if let Tok::Ident(_) = self.peek() {
            let mut k = 1;
            while self.peek_at(k).is_op(".") && matches!(self.peek_at(k + 1), Tok::Ident(_)) {
                k += 2;
            }
            if self.peek_at(k).is_op(":") {
                let l = self.dotted()?;
                self.expect_op(":")?;
                return Ok(StateItem::Label(l));
            }
            // A sprite can be called LOOP; the keyword is followed by `;`,
            // a label or (DECORATE) the end of the line.
            let next_on_new_line = self.decorate && self.toks.get(self.i + 1).is_some_and(|t| t.1.line != self.pos().line);
            // ... or `Stop Death:` with a label right after it.
            let next_is_label = matches!(self.peek_at(1), Tok::Ident(_)) && self.peek_at(2).is_op(":") && !self.peek_at(2).is_op("::");
            for (w, item) in [("stop", StateItem::Stop), ("loop", StateItem::Loop), ("wait", StateItem::Wait), ("fail", StateItem::Fail)] {
                if self.peek().is_word(w) && (next_on_new_line || next_is_label || !matches!(self.peek_at(1), Tok::Ident(_) | Tok::Int(_))) {
                    self.next();
                    self.eat_op(";");
                    return Ok(item);
                }
            }
            if self.peek().is_word("goto") {
                self.next();
                let mut target = self.dotted()?;
                if self.eat_op("::") {
                    target.push_str("::");
                    target.push_str(&self.dotted()?);
                }
                let offset = if self.eat_op("+") {
                    match self.next() {
                        Tok::Int(n) => n as i32,
                        _ => return self.err("expected an offset"),
                    }
                } else {
                    0
                };
                self.eat_op(";");
                return Ok(StateItem::Goto { target, offset });
            }
        }
        Ok(StateItem::Frame(self.frame()?))
    }

    fn frame(&mut self) -> R<StateFrame> {
        let pos = self.pos();
        let sprite = self.sprite_name()?;
        // Frame letters, including frames past Z (`[`, `\`, `]`), which lex
        // as operators: take everything up to the duration.
        let mut frames = String::new();
        let mut after_symbol = true;
        loop {
            match self.peek().clone() {
                Tok::Ident(s) | Tok::Str(s) if after_symbol && !(s.eq_ignore_ascii_case("random") && self.peek_at(1).is_op("(")) => {
                    frames.push_str(&s);
                    after_symbol = false;
                }
                Tok::Op(o @ ("#" | "[" | "]" | "\\")) => {
                    frames.push_str(o);
                    after_symbol = true;
                }
                Tok::Op("--") if frames.is_empty() => frames.push('-'),
                _ => break,
            }
            self.next();
        }
        if frames.is_empty() {
            return self.err(format!("expected frame letters, found {:?}", self.peek()));
        }
        let line = self.pos().line;
        self.split_word();
        let tics = match self.next() {
            Tok::Int(n) => Tics::Fixed(n as i32),
            Tok::Op("-") => match self.next() {
                Tok::Int(n) => Tics::Fixed(-(n as i32)),
                _ => return self.err("expected a duration"),
            },
            Tok::Ident(w) if w.eq_ignore_ascii_case("random") => {
                self.expect_op("(")?;
                let a = self.int_const()?;
                self.expect_op(",")?;
                let b = self.int_const()?;
                self.expect_op(")")?;
                Tics::Random(a, b)
            }
            t => return self.err(format!("expected a duration, found {t:?}")),
        };
        let mut f = StateFrame {
            sprite,
            frames,
            tics,
            bright: false,
            fast: false,
            slow: false,
            nodelay: false,
            canraise: false,
            light: Vec::new(),
            offset: None,
            action: None,
            pos,
        };
        // In DECORATE the line ends the frame.
        let same_line = |p: &Parser| !p.decorate || p.pos().line == line;
        loop {
            let w = match self.peek() {
                Tok::Ident(w) if same_line(self) => w.to_ascii_lowercase(),
                _ => break,
            };
            match w.as_str() {
                "bright" => f.bright = true,
                "fast" => f.fast = true,
                "slow" => f.slow = true,
                "nodelay" => f.nodelay = true,
                "canraise" => f.canraise = true,
                "light" if self.peek_at(1).is_op("(") => {
                    self.next();
                    self.next();
                    while !self.eat_op(")") {
                        match self.next() {
                            Tok::Str(s) | Tok::Name(s) | Tok::Ident(s) => f.light.push(s),
                            Tok::Eof => return self.err("unterminated light()"),
                            _ => {}
                        }
                    }
                    continue;
                }
                "offset" if self.peek_at(1).is_op("(") => {
                    self.next();
                    self.next();
                    let x = self.expr()?;
                    self.expect_op(",")?;
                    let y = self.expr()?;
                    self.expect_op(")")?;
                    f.offset = Some((x, y));
                    continue;
                }
                _ => break,
            }
            self.next();
        }
        if self.peek().is_op("{") && same_line(self) {
            f.action = Some(StateAction::Block(self.block()?));
        } else if matches!(self.peek(), Tok::Ident(_)) && same_line(self) {
            // The next frame's sprite can follow without a ';' only after a
            // block; an identifier here is an action call.
            let name = self.dotted()?;
            let args = if self.eat_op("(") { self.args()? } else { Vec::new() };
            f.action = Some(StateAction::Call { name, args });
        }
        self.eat_op(";");
        Ok(f)
    }

    fn int_const(&mut self) -> R<i32> {
        let neg = self.eat_op("-");
        self.split_word();
        match self.next() {
            Tok::Int(n) => Ok(if neg { -n } else { n } as i32),
            _ => self.err("expected a number"),
        }
    }

    /// Sprite names: `TROO`, `"####"`, `####`, `----`.
    fn sprite_name(&mut self) -> R<String> {
        match self.next() {
            Tok::Ident(s) | Tok::Str(s) | Tok::Word(s) => Ok(s),
            Tok::Op("#") => {
                for _ in 0..3 {
                    self.expect_op("#")?;
                }
                Ok("####".into())
            }
            Tok::Op("--") => {
                self.expect_op("--")?;
                Ok("----".into())
            }
            Tok::Int(n) => {
                // Sprite names starting with a digit lex as a number + name.
                let mut s = n.to_string();
                if let Tok::Ident(rest) = self.peek().clone() {
                    if s.len() + rest.len() == 4 || rest.len() > 1 {
                        self.next();
                        s.push_str(&rest);
                    }
                }
                Ok(s)
            }
            t => self.err(format!("expected a sprite name, found {t:?}")),
        }
    }

    // ---- statements -------------------------------------------------------------------

    fn block(&mut self) -> R<Block> {
        self.expect_op("{")?;
        let mut out = Vec::new();
        while !self.eat_op("}") {
            if self.at_end() {
                return self.err("unterminated block");
            }
            let start = self.i;
            match self.stmt() {
                Ok(s) => out.push(s),
                Err(e) => {
                    // Lose the statement, keep the rest of the block.
                    self.errors.push(e);
                    self.recover_stmt();
                }
            }
            if self.i == start {
                self.next();
            }
        }
        Ok(Block(out))
    }

    /// Skip to the end of the current statement: past a `;` or a balanced
    /// block, or up to (not past) the enclosing `}`.
    fn recover_stmt(&mut self) {
        let mut depth = 0;
        while !self.at_end() {
            match self.peek() {
                Tok::Op("{") => depth += 1,
                Tok::Op("}") => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                    if depth == 0 {
                        self.next();
                        return;
                    }
                }
                Tok::Op(";") if depth == 0 => {
                    self.next();
                    return;
                }
                _ => {}
            }
            self.next();
        }
    }

    /// Does a local declaration start here? (`Type name`, `Type<...> name`,
    /// `Type[...] name` patterns.)
    fn at_local_decl(&self) -> bool {
        let Tok::Ident(w) = self.peek() else { return false };
        let lw = w.to_ascii_lowercase();
        if matches!(lw.as_str(), "return" | "if" | "else" | "while" | "for" | "do" | "switch" | "case" | "break" | "continue" | "super" | "self" | "until" | "foreach") {
            return false;
        }
        if BASIC_TYPES.contains(&lw.as_str()) {
            // `name(x)` / `int(x)` are casts, not declarations.
            return !self.peek_at(1).is_op("(") && !self.peek_at(1).is_op(".");
        }
        if matches!(lw.as_str(), "class" | "array" | "map" | "readonly" | "mapiterator") && self.peek_at(1).is_op("<") {
            return true;
        }
        if lw == "static" && self.peek_at(1).is_word("const") {
            return true;
        }
        // User type: `Actor a`, `Thinker.Thing a`.
        let mut k = 1;
        while self.peek_at(k).is_op(".") && matches!(self.peek_at(k + 1), Tok::Ident(_)) {
            k += 2;
        }
        matches!(self.peek_at(k), Tok::Ident(_)) && matches!(self.peek_at(k + 1), Tok::Op("=" | ";" | "," | "["))
    }

    fn stmt(&mut self) -> R<Stmt> {
        if self.eat_op(";") {
            return Ok(Stmt::Empty);
        }
        if self.peek().is_op("{") {
            return Ok(Stmt::Block(self.block()?));
        }
        if self.peek().is_op("[") {
            // [a, b] = f();
            let save = self.i;
            self.next();
            let mut targets = Vec::new();
            let ok = loop {
                match self.expr() {
                    Ok(e) => targets.push(e),
                    Err(_) => break false,
                }
                if self.eat_op("]") {
                    break true;
                }
                if !self.eat_op(",") {
                    break false;
                }
            };
            if ok && self.eat_op("=") {
                let value = self.expr()?;
                self.expect_op(";")?;
                return Ok(Stmt::MultiAssign { targets, value });
            }
            self.i = save;
        }
        let w = match self.peek() {
            Tok::Ident(w) => w.to_ascii_lowercase(),
            _ => String::new(),
        };
        match w.as_str() {
            "if" => {
                self.next();
                self.expect_op("(")?;
                let cond = self.expr()?;
                self.expect_op(")")?;
                let then = Box::new(self.stmt()?);
                let otherwise = if self.eat_word("else") { Some(Box::new(self.stmt()?)) } else { None };
                return Ok(Stmt::If { cond, then, otherwise });
            }
            "while" | "until" => {
                self.next();
                self.expect_op("(")?;
                let cond = self.expr()?;
                self.expect_op(")")?;
                let body = Box::new(self.stmt()?);
                return Ok(if w == "while" { Stmt::While { cond, body } } else { Stmt::Until { cond, body } });
            }
            "do" => {
                self.next();
                let body = Box::new(self.stmt()?);
                let until = if self.eat_word("until") {
                    true
                } else if self.eat_word("while") {
                    false
                } else {
                    return self.err("expected 'while' or 'until' after do");
                };
                self.expect_op("(")?;
                let cond = self.expr()?;
                self.expect_op(")")?;
                self.eat_op(";");
                return Ok(Stmt::DoWhile { body, cond, until });
            }
            "for" => {
                self.next();
                self.expect_op("(")?;
                let mut init = Vec::new();
                if !self.eat_op(";") {
                    if self.at_local_decl() {
                        init.push(self.local()?);
                    } else {
                        loop {
                            init.push(Stmt::Expr(self.expr()?));
                            if !self.eat_op(",") {
                                break;
                            }
                        }
                        self.expect_op(";")?;
                    }
                }
                let cond = if self.peek().is_op(";") { None } else { Some(self.expr()?) };
                self.expect_op(";")?;
                let mut step = Vec::new();
                if !self.peek().is_op(")") {
                    loop {
                        step.push(self.expr()?);
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                }
                self.expect_op(")")?;
                let body = Box::new(self.stmt()?);
                return Ok(Stmt::For { init, cond, step, body });
            }
            "foreach" => {
                self.next();
                self.expect_op("(")?;
                if self.at_local_decl() {
                    self.type_ref()?;
                }
                let var = self.ident()?;
                self.expect_op(":")?;
                let list = self.expr()?;
                self.expect_op(")")?;
                let body = Box::new(self.stmt()?);
                return Ok(Stmt::ForEach { var, list, body });
            }
            "switch" => {
                self.next();
                self.expect_op("(")?;
                let value = self.expr()?;
                self.expect_op(")")?;
                self.expect_op("{")?;
                let mut body = Vec::new();
                while !self.eat_op("}") {
                    if self.at_end() {
                        return self.err("unterminated switch");
                    }
                    if self.eat_word("case") {
                        let e = self.expr()?;
                        self.expect_op(":")?;
                        body.push(SwitchItem::Case(e));
                    } else if self.peek().is_word("default") && self.peek_at(1).is_op(":") {
                        self.next();
                        self.next();
                        body.push(SwitchItem::Default);
                    } else {
                        body.push(SwitchItem::Stmt(self.stmt()?));
                    }
                }
                return Ok(Stmt::Switch { value, body });
            }
            "break" => {
                self.next();
                self.expect_op(";")?;
                return Ok(Stmt::Break);
            }
            "continue" => {
                self.next();
                self.expect_op(";")?;
                return Ok(Stmt::Continue);
            }
            "return" => {
                self.next();
                let mut values = Vec::new();
                if !self.eat_op(";") {
                    loop {
                        values.push(self.expr()?);
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                    self.expect_op(";")?;
                }
                return Ok(Stmt::Return(values));
            }
            _ => {}
        }
        if self.at_local_decl() {
            return self.local();
        }
        let e = self.expr()?;
        self.expect_op(";")?;
        Ok(Stmt::Expr(e))
    }

    fn local(&mut self) -> R<Stmt> {
        if self.peek().is_word("static") {
            self.next();
            self.eat_word("const");
        }
        self.eat_word("const");
        let ty = self.type_ref()?;
        let mut vars = Vec::new();
        loop {
            let name = self.ident()?;
            let dims = self.dims()?;
            let (mut init, mut init_list) = (None, None);
            if self.eat_op("=") {
                if self.eat_op("{") {
                    let mut items = Vec::new();
                    while !self.eat_op("}") {
                        items.push(self.expr()?);
                        if !self.eat_op(",") {
                            self.expect_op("}")?;
                            break;
                        }
                    }
                    init_list = Some(items);
                } else {
                    init = Some(self.expr()?);
                }
            } else if self.peek().is_op("(") {
                // `Vector3 v(1, 2, 3);` constructor-style init.
                self.next();
                let args = self.args()?;
                init = Some(Expr::Vector(args.into_iter().map(|a| a.value).collect()));
            }
            vars.push(LocalVar { name, dims, init, init_list });
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(";")?;
        Ok(Stmt::Local { ty, vars })
    }

    // ---- expressions -----------------------------------------------------------------

    pub fn expr(&mut self) -> R<Expr> {
        self.assign()
    }

    fn assign(&mut self) -> R<Expr> {
        let lhs = self.ternary()?;
        const OPS: &[&str] = &["=", "+=", "-=", "*=", "/=", "%=", "<<=", ">>=", ">>>=", "&=", "|=", "^=", "**="];
        if let Tok::Op(o) = self.peek().clone() {
            if OPS.contains(&o) {
                self.next();
                let rhs = self.assign()?;
                return Ok(Expr::Assign(o, Box::new(lhs), Box::new(rhs)));
            }
        }
        Ok(lhs)
    }

    fn ternary(&mut self) -> R<Expr> {
        let c = self.binary(0)?;
        if self.eat_op("?") {
            let a = self.assign()?;
            self.expect_op(":")?;
            let b = self.assign()?;
            return Ok(Expr::Ternary(Box::new(c), Box::new(a), Box::new(b)));
        }
        Ok(c)
    }

    fn binary(&mut self, level: usize) -> R<Expr> {
        const LEVELS: &[&[&str]] = &[
            &["||"],
            &["&&"],
            &["|"],
            &["^"],
            &["&"],
            &["==", "!=", "~==", "<>="],
            &["<", ">", "<=", ">=", "is"],
            &["<<", ">>", ">>>"],
            &[".."],
            &["+", "-"],
            &["*", "/", "%", "cross", "dot"],
        ];
        if level == LEVELS.len() {
            return self.power();
        }
        let mut lhs = self.binary(level + 1)?;
        loop {
            let op = match self.peek() {
                Tok::Op(o) if LEVELS[level].contains(o) => *o,
                Tok::Ident(w) if LEVELS[level].iter().any(|o| w.eq_ignore_ascii_case(o)) => {
                    LEVELS[level].iter().copied().find(|o| w.eq_ignore_ascii_case(o)).unwrap()
                }
                _ => break,
            };
            self.next();
            let rhs = self.binary(level + 1)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn power(&mut self) -> R<Expr> {
        let base = self.unary()?;
        if self.eat_op("**") {
            let exp = self.power()?;
            return Ok(Expr::Binary("**", Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn unary(&mut self) -> R<Expr> {
        for o in ["-", "+", "!", "~", "++", "--"] {
            if self.peek().is_op(o) {
                self.next();
                let e = self.unary()?;
                return Ok(Expr::Unary(o, Box::new(e)));
            }
        }
        if (self.peek().is_word("sizeof") || self.peek().is_word("alignof")) && self.peek_at(1).is_op("(") {
            self.next();
            self.next();
            let e = self.expr()?;
            self.expect_op(")")?;
            return Ok(Expr::SizeOf(Box::new(e)));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> R<Expr> {
        let mut e = self.primary()?;
        loop {
            if self.eat_op(".") {
                let m = self.ident()?;
                e = Expr::Member(Box::new(e), m);
            } else if self.eat_op("(") {
                let args = self.args()?;
                e = Expr::Call(Box::new(e), args);
            } else if self.eat_op("[") {
                let i = self.expr()?;
                self.expect_op("]")?;
                e = Expr::Index(Box::new(e), Box::new(i));
            } else if self.peek().is_op("++") || self.peek().is_op("--") {
                let o = if self.next().is_op("++") { "++" } else { "--" };
                e = Expr::Postfix(o, Box::new(e));
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn args(&mut self) -> R<Vec<Arg>> {
        let mut out = Vec::new();
        if self.eat_op(")") {
            return Ok(out);
        }
        loop {
            let name = if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1).is_op(":") && !self.peek_at(1).is_op("::") {
                let n = self.ident()?;
                self.next();
                Some(n)
            } else {
                None
            };
            out.push(Arg { name, value: self.expr()? });
            if self.eat_op(")") {
                return Ok(out);
            }
            self.expect_op(",")?;
        }
    }

    fn primary(&mut self) -> R<Expr> {
        self.split_word();
        let t = self.next();
        Ok(match t {
            Tok::Int(n) => Expr::Int(n),
            Tok::Float(f) => Expr::Float(f),
            Tok::Str(mut s) => {
                // "a" "b" is "ab".
                while let Tok::Str(more) = self.peek().clone() {
                    self.next();
                    s.push_str(&more);
                }
                Expr::Str(s)
            }
            Tok::Name(s) => Expr::Name(s),
            Tok::Op("(") => {
                let first = self.expr()?;
                if self.eat_op(",") {
                    let mut v = vec![first];
                    loop {
                        v.push(self.expr()?);
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                    self.expect_op(")")?;
                    Expr::Vector(v)
                } else {
                    self.expect_op(")")?;
                    first
                }
            }
            Tok::Ident(w) => {
                let lw = w.to_ascii_lowercase();
                match lw.as_str() {
                    "true" => Expr::Bool(true),
                    "false" => Expr::Bool(false),
                    "null" => Expr::Null,
                    "self" => Expr::SelfRef,
                    "super" => Expr::Super,
                    "new" if self.peek().is_op("(") => {
                        self.next();
                        let e = if self.peek().is_op(")") { Expr::Null } else { self.expr()? };
                        self.expect_op(")")?;
                        Expr::New(Box::new(e))
                    }
                    "class" if self.peek().is_op("<") => {
                        self.next();
                        let n = self.dotted()?;
                        self.close_angle()?;
                        self.expect_op("(")?;
                        let e = self.expr()?;
                        self.expect_op(")")?;
                        Expr::ClassCast(n, Box::new(e))
                    }
                    _ => {
                        if self.eat_op("::") {
                            let m = self.ident()?;
                            Expr::Scoped(w, m)
                        } else {
                            Expr::Ident(w)
                        }
                    }
                }
            }
            t => {
                self.i -= 1;
                return self.err(format!("expected an expression, found {t:?}"));
            }
        })
    }
}

/// Parse a DECORATE lump: actors (whose `States` use the ZScript state
/// grammar, ended by line breaks), `#include`s and constants.
pub fn parse_decorate(src: &str) -> (DecorateFile, Vec<ParseError>) {
    let toks = match lex(src) {
        Ok(t) => t,
        Err(e) => return (DecorateFile::default(), vec![ParseError { pos: e.pos, message: e.message }]),
    };
    let mut p = Parser { toks, i: 0, errors: Vec::new(), decorate: true };
    let mut f = DecorateFile::default();
    while !p.at_end() {
        let start = p.i;
        if p.peek().is_op("#") && p.peek_at(1).is_word("include") {
            p.next();
            p.next();
            // Quoted, or (ZDoom accepts it) a bare path to the end of the line.
            let line = p.pos().line;
            let mut path = String::new();
            if let Tok::Str(s) = p.peek().clone() {
                p.next();
                path = s;
            } else {
                while p.pos().line == line && !p.at_end() {
                    match p.next() {
                        Tok::Ident(s) => path.push_str(&s),
                        Tok::Int(n) => path.push_str(&n.to_string()),
                        Tok::Op(o) => path.push_str(o),
                        _ => break,
                    }
                }
            }
            if !path.is_empty() {
                f.includes.push(path);
            }
        } else if p.peek().is_word("actor") {
            p.next();
            let r = p.decorate_actor();
            if let Some(a) = p.record(r) {
                f.actors.push(a);
            }
        } else if p.peek().is_word("const") {
            p.next();
            let r = (|| -> R<ConstDecl> {
                p.type_ref()?;
                p.constant()
            })();
            if let Some(c) = p.record(r) {
                f.consts.push(c);
            }
        } else if p.peek().is_word("enum") {
            p.next();
            let r = p.enumeration();
            if let Some(e) = p.record(r) {
                f.enums.push(e);
            }
        } else {
            // damagetype, ...: skip to the end of the statement or block.
            p.next();
            while !p.at_end() && !p.peek().is_op(";") && !p.peek().is_op("{") {
                p.next();
            }
            if p.peek().is_op("{") {
                p.recover();
            } else {
                p.eat_op(";");
            }
        }
        if p.i == start {
            p.next();
        }
    }
    (f, p.errors)
}

impl Parser {
    fn decorate_actor(&mut self) -> R<ActorDecl> {
        let pos = self.pos();
        let name = match self.next() {
            Tok::Ident(s) | Tok::Str(s) | Tok::Word(s) => s,
            t => return self.err(format!("expected an actor name, found {t:?}")),
        };
        let mut a = ActorDecl { name, pos, ..Default::default() };
        loop {
            if self.eat_op(":") {
                a.parent = Some(self.dotted()?);
            } else if self.eat_word("replaces") {
                a.replaces = Some(self.dotted()?);
            } else if let Tok::Int(n) = self.peek() {
                a.doomednum = Some(*n);
                self.next();
            } else if self.eat_word("native") {
            } else {
                break;
            }
        }
        self.expect_op("{")?;
        while !self.eat_op("}") {
            if self.at_end() {
                return self.err("unterminated actor");
            }
            let start = self.i;
            let r = self.decorate_member(&mut a);
            self.record(r);
            if self.i == start {
                self.next();
            }
        }
        Ok(a)
    }

    fn decorate_member(&mut self, a: &mut ActorDecl) -> R<()> {
        if self.eat_op(";") {
            return Ok(());
        }
        if (self.peek().is_op("+") || self.peek().is_op("-")) && matches!(self.peek_at(1), Tok::Ident(_)) {
            let on = self.next().is_op("+");
            a.defaults.push(DefaultItem::Flag { on, name: self.dotted()? });
            return Ok(());
        }
        if self.peek().is_word("states") && (self.peek_at(1).is_op("{") || self.peek_at(1).is_op("(")) {
            self.next();
            a.states.push(self.states()?);
            return Ok(());
        }
        if self.peek().is_word("var") {
            // var int user_x; / var float user_y[4];
            self.next();
            self.type_ref()?;
            let n = self.ident()?;
            self.dims()?;
            self.expect_op(";")?;
            a.vars.push(n.to_ascii_lowercase());
            return Ok(());
        }
        if self.peek().is_word("const") || self.peek().is_word("enum") {
            if self.eat_word("const") {
                self.type_ref()?;
                let c = self.constant()?;
                a.consts.push(c);
            } else {
                self.next();
                self.enumeration()?;
            }
            return Ok(());
        }
        if self.peek().is_word("action") {
            // action native declarations
            while !self.at_end() && !self.eat_op(";") {
                self.next();
            }
            return Ok(());
        }
        // Property: name, then values up to the end of the line. Values
        // are parsed on that line alone, so `Height 16` followed by a
        // `+FLAG` line isn't read as `16 + FLAG`.
        let line = self.pos().line;
        let name = self.dotted()?;
        let end = (self.i..self.toks.len()).find(|&k| self.toks[k].1.line != line || matches!(self.toks[k].0, Tok::Eof)).unwrap_or(self.toks.len());
        let mut sub = Parser { toks: self.toks[self.i..end].to_vec(), i: 0, errors: Vec::new(), decorate: true };
        sub.toks.push((Tok::Eof, self.toks.get(end).map_or(Pos::default(), |t| t.1)));
        let values = sub.line_values()?;
        self.i += sub.i;
        a.defaults.push(DefaultItem::Property { name, values });
        Ok(())
    }

    /// Property values on one line (the parser holds just that line).
    fn line_values(&mut self) -> R<Vec<Expr>> {
        let mut values = Vec::new();
        while !self.peek().is_op("}") && !self.at_end() {
            if self.eat_op(";") {
                break;
            }
            if self.eat_op(",") {
                continue;
            }
            // A flag or `States` after a property on the same line.
            if (self.peek().is_op("+") || self.peek().is_op("-")) && matches!(self.peek_at(1), Tok::Ident(_)) {
                break;
            }
            // `States` (its block may open on the next line).
            if self.peek().is_word("states") && (self.peek_at(1).is_op("{") || self.peek_at(1).is_op("(") || matches!(self.peek_at(1), Tok::Eof)) {
                break;
            }
            values.push(self.expr()?);
        }
        Ok(values)
    }
}

/// Parse a single expression (DECORATE action arguments use ZScript's
/// expression grammar).
pub fn parse_expr(src: &str) -> Result<Expr, ParseError> {
    let toks = lex(src).map_err(|e| ParseError { pos: e.pos, message: e.message })?;
    let mut p = Parser { toks, i: 0, errors: Vec::new(), decorate: false };
    let e = p.expr()?;
    if !p.at_end() {
        return p.err("trailing input after expression");
    }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> File {
        let (f, errors) = parse(src);
        assert!(errors.is_empty(), "{errors:?}");
        f
    }

    #[test]
    fn classes_fields_functions_defaults_states() {
        let f = ok(r#"
            version "4.8"
            class Imp2 : DoomImp replaces DoomImp
            {
                int counter, list[4];
                Array<Actor> friends;
                class<Actor> spawnType;
                const SPEED = 12;
                enum EMode { M_A, M_B = 4 }
                property Rage: counter;
                Default
                {
                    Health 80;
                    Speed 10;
                    +NOGRAVITY -SOLID
                    Tag "$TAG_IMP";
                    Obituary "%o was clawed.";
                    Monster;
                }
                override void Tick()
                {
                    Super.Tick();
                    if (counter++ > 35 && target != null) { A_FaceTarget(); counter = 0; }
                    for (int i = 0; i < friends.Size(); i++) friends[i].angle += 5.;
                }
                action void A_Burst(int n = 3, double spread = 2.5)
                {
                    for (int i = 0; i < n; ++i)
                        A_SpawnProjectile("DoomImpBall", 32, frandom(-spread, spread), flags: CMF_AIMDIRECTION);
                }
                States
                {
                Spawn:
                    TROO AB 10 A_Look;
                    Loop;
                See:
                    TROO AABBCCDD random(2, 4) A_Chase();
                    #### # 0 { A_Burst(); return ResolveState("Missile"); }
                    Goto See+2;
                Death.Fire:
                    TROO I 8 Bright Light("ZOMBIEATK") Offset(0, 2) A_Scream;
                    TROO J -1;
                    Stop;
                }
            }
        "#);
        assert_eq!(f.version.as_deref(), Some("4.8"));
        let Decl::Class(c) = &f.decls[0] else { panic!() };
        assert_eq!((c.name.as_str(), c.parent.as_deref(), c.replaces.as_deref()), ("Imp2", Some("DoomImp"), Some("DoomImp")));
        let funcs: Vec<&str> = c.members.iter().filter_map(|m| if let Member::Function(f) = m { Some(f.name.as_str()) } else { None }).collect();
        assert_eq!(funcs, ["Tick", "A_Burst"]);
        let states = c.members.iter().find_map(|m| if let Member::States(s) = m { Some(s) } else { None }).unwrap();
        let labels: Vec<&str> = states.items.iter().filter_map(|i| if let StateItem::Label(l) = i { Some(l.as_str()) } else { None }).collect();
        assert_eq!(labels, ["Spawn", "See", "Death.Fire"]);
        let frames: Vec<&StateFrame> = states.items.iter().filter_map(|i| if let StateItem::Frame(f) = i { Some(f) } else { None }).collect();
        assert_eq!(frames[1].tics, Tics::Random(2, 4));
        assert!(matches!(frames[2].action, Some(StateAction::Block(_))));
        assert_eq!(frames[2].sprite, "####");
        assert!(frames[3].bright && frames[3].light == ["ZOMBIEATK"]);
        assert!(states.items.iter().any(|i| matches!(i, StateItem::Goto { target, offset: 2 } if target == "See")));
        let defaults = c.members.iter().find_map(|m| if let Member::Default(d) = m { Some(d) } else { None }).unwrap();
        assert_eq!(defaults.len(), 7);
    }

    #[test]
    fn expressions_and_statements() {
        let e = parse_expr("a + b * c ** 2 ** 3 .. \"x\" == d && !e is \"Actor\"").unwrap();
        let Expr::Binary("&&", lhs, _) = e else { panic!("{e:?}") };
        assert!(matches!(*lhs, Expr::Binary("==", _, _)));
        let f = ok(r#"
            class T : Actor {
                void F() {
                    let mo = Actor(Spawn("Imp", pos + (0, 0, 32)));
                    [ok, a] = A_SpawnItemEx("X");
                    Vector3 v = (1, 2, 3);
                    switch (x) { case 1: case 2: y = 3; break; default: return; }
                    do { x--; } until (x <= 0);
                    ThinkerIterator it = ThinkerIterator.Create("Inventory");
                    Inventory mo2;
                    while (mo2 = Inventory(it.Next())) { if (!(mo2.owner is "PlayerPawn")) continue; }
                    double s = sqrt((vel.x * vel.x) + (vel.y * vel.y));
                    x = cond ? a : b;
                    self.player.cheats |= CF_TOTALLYFROZEN;
                    name n = 'Foo';
                    let c = class<Actor>(GetClass());
                }
            }
        "#);
        let Decl::Class(c) = &f.decls[0] else { panic!() };
        let Member::Function(func) = &c.members[0] else { panic!() };
        assert_eq!(func.body.as_ref().unwrap().0.len(), 13);
    }

    #[test]
    fn decorate_actors() {
        let (f, errors) = parse_decorate(
            r#"
            #include "actors/more.txt"
            const int MAXRAGE = 4;
            ACTOR FastImp : DoomImp replaces DoomImp 30001
            {
              Health 100
              Speed 16
              +NOGRAVITY
              Obituary "%o met a fast imp"
              DropItem "Clip" 255 2
              var int user_rage;
              States
              {
              See:
                TROO AABB 2 A_Chase
                TROO C 0 A_Jump(128, "Rage")
                Loop
              Rage:
                TROO E 4 Bright A_SpawnItemEx("DoomImpBall", 0, 0, 32, 4, 0, 0, 0, SXF_NOCHECKPOSITION|SXF_TRANSFERPOINTERS)
                TROO F 4 { A_FaceTarget(); }
                TROO G 2
                Goto See
              }
            }
            damagetype Ice { Factor 0.5 }
            actor Puff2 : BulletPuff { +ALWAYSPUFF States { Spawn: PUFF A 4 Bright Stop } }
        "#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(f.includes, ["actors/more.txt"]);
        assert_eq!(f.actors.len(), 2);
        let a = &f.actors[0];
        assert_eq!((a.parent.as_deref(), a.replaces.as_deref(), a.doomednum), (Some("DoomImp"), Some("DoomImp"), Some(30001)));
        assert_eq!(a.vars, ["user_rage"]);
        let drop = a.defaults.iter().find_map(|d| if let DefaultItem::Property { name, values } = d { (name == "DropItem").then_some(values) } else { None });
        assert_eq!(drop.unwrap().len(), 3, "space-separated values");
        let frames: Vec<&StateFrame> = a.states[0].items.iter().filter_map(|i| if let StateItem::Frame(f) = i { Some(f) } else { None }).collect();
        assert_eq!(frames.len(), 5);
        assert!(frames[4].action.is_none(), "a newline ends the frame");
        let Some(StateAction::Call { name, args }) = &frames[1].action else { panic!() };
        assert_eq!((name.as_str(), args.len()), ("A_Jump", 2));
        let Some(StateAction::Call { args, .. }) = &frames[2].action else { panic!() };
        assert!(matches!(&args[8].value, Expr::Binary("|", _, _)));
        assert!(matches!(frames[3].action, Some(StateAction::Block(_))));
    }

    #[test]
    fn errors_are_local() {
        let (f, errors) = parse("class A : Actor { int x; @@@ } class B : Actor { int y; }");
        assert!(!errors.is_empty());
        assert_eq!(f.decls.len(), 2, "B still parses");
        // A bad statement loses only itself.
        let (f, errors) = parse("class C : Actor { void F() { a = ; b = 1; c = 2; } }");
        assert_eq!(errors.len(), 1);
        let Decl::Class(c) = &f.decls[0] else { panic!() };
        let Member::Function(func) = &c.members[0] else { panic!() };
        assert_eq!(func.body.as_ref().unwrap().0.len(), 2);
    }
}
