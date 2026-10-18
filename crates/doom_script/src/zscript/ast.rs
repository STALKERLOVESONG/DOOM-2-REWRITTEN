//! ZScript syntax tree.
//!
//! The parser keeps everything a ZScript lump says, without resolving
//! names: classes, structs, enums, constants, fields, functions with their
//! bodies, `default` blocks and `states`. Later passes ([`super::classes`])
//! build the class table and lower bodies for the VM.

pub use super::lexer::Pos;

#[derive(Clone, Debug, Default)]
pub struct File {
    pub version: Option<String>,
    pub includes: Vec<String>,
    pub decls: Vec<Decl>,
}

/// A DECORATE lump.
#[derive(Clone, Debug, Default)]
pub struct DecorateFile {
    pub includes: Vec<String>,
    pub actors: Vec<ActorDecl>,
    pub consts: Vec<ConstDecl>,
    pub enums: Vec<EnumDecl>,
}

/// A DECORATE `actor` block.
#[derive(Clone, Debug, Default)]
pub struct ActorDecl {
    pub name: String,
    pub parent: Option<String>,
    pub replaces: Option<String>,
    pub doomednum: Option<i64>,
    /// Properties and flags in order.
    pub defaults: Vec<DefaultItem>,
    /// `var` declarations (lower case).
    pub vars: Vec<String>,
    pub consts: Vec<ConstDecl>,
    pub states: Vec<StatesDecl>,
    pub pos: Pos,
}

#[derive(Clone, Debug)]
pub enum Decl {
    Class(ClassDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Const(ConstDecl),
}

#[derive(Clone, Debug, Default)]
pub struct ClassDecl {
    pub name: String,
    pub parent: Option<String>,
    pub replaces: Option<String>,
    /// `abstract`, `native`, `ui`, `play`, `version(...)`, ...
    pub flags: Vec<String>,
    /// `extend class X`: adds members to an existing class.
    pub extend: bool,
    /// `mixin class X`: members pasted into classes that say `mixin X;`.
    pub mixin: bool,
    pub members: Vec<Member>,
    pub pos: Pos,
}

#[derive(Clone, Debug, Default)]
pub struct StructDecl {
    pub name: String,
    pub flags: Vec<String>,
    pub extend: bool,
    pub members: Vec<Member>,
    pub pos: Pos,
}

#[derive(Clone, Debug, Default)]
pub struct EnumDecl {
    pub name: String,
    pub values: Vec<(String, Option<Expr>)>,
}

#[derive(Clone, Debug)]
pub struct ConstDecl {
    pub name: String,
    pub value: Expr,
}

#[derive(Clone, Debug)]
pub enum Member {
    Field(FieldDecl),
    Function(FuncDecl),
    Default(Vec<DefaultItem>),
    States(StatesDecl),
    Enum(EnumDecl),
    Const(ConstDecl),
    Struct(StructDecl),
    /// `property Name: field[, field];`
    Property { name: String, fields: Vec<String> },
    /// `flagdef Name: field, bit;`
    FlagDef { name: String, field: String, bit: i64 },
    /// `mixin Name;`
    Mixin(String),
    /// `static const Type name[] = { ... };`
    StaticArray { ty: TypeRef, name: String, values: Vec<Expr> },
}

#[derive(Clone, Debug)]
pub struct FieldDecl {
    pub flags: Vec<String>,
    pub ty: TypeRef,
    /// Names with array dimensions (`int a[4]`).
    pub names: Vec<(String, Vec<Expr>)>,
    pub pos: Pos,
}

#[derive(Clone, Debug)]
pub struct FuncDecl {
    /// `virtual`, `override`, `static`, `action`, `native`, `final`,
    /// `private`, `protected`, `clearscope`, ...
    pub flags: Vec<String>,
    pub returns: Vec<TypeRef>,
    pub name: String,
    pub params: Vec<Param>,
    /// Trailing `const`.
    pub is_const: bool,
    /// `None` for native or abstract declarations.
    pub body: Option<Block>,
    pub pos: Pos,
}

impl FuncDecl {
    pub fn has_flag(&self, f: &str) -> bool {
        self.flags.iter().any(|x| x.eq_ignore_ascii_case(f))
    }
}

#[derive(Clone, Debug)]
pub struct Param {
    /// `out`, `in`.
    pub flags: Vec<String>,
    pub ty: TypeRef,
    pub name: String,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeRef {
    /// `int`, `double`, `Actor`, `Thinker.Something`, ...
    Named(String),
    /// `class<Actor>` (`None`: plain `class`).
    Class(Option<String>),
    /// `Array<T>`
    DynArray(Box<TypeRef>),
    /// `Map<K, V>` / `MapIterator<K, V>`
    Map(String, Box<TypeRef>, Box<TypeRef>),
    /// `readonly<T>`
    ReadOnly(Box<TypeRef>),
    /// `let` (inferred).
    Let,
    Void,
}

impl TypeRef {
    pub fn named(&self) -> Option<&str> {
        match self {
            TypeRef::Named(n) => Some(n),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum DefaultItem {
    Flag { on: bool, name: String },
    Property { name: String, values: Vec<Expr> },
}

#[derive(Clone, Debug, Default)]
pub struct StatesDecl {
    /// `states(Actor, Overlay)` scope list.
    pub scope: Vec<String>,
    pub items: Vec<StateItem>,
}

#[derive(Clone, Debug)]
pub enum StateItem {
    /// `Spawn:` / `Death.Fire:`
    Label(String),
    Frame(StateFrame),
    Stop,
    Loop,
    Wait,
    Fail,
    /// `goto [Class::]Label[+n]`
    Goto { target: String, offset: i32 },
}

#[derive(Clone, Debug)]
pub struct StateFrame {
    /// Four characters (`####`, `----` keep the previous).
    pub sprite: String,
    /// One state per frame letter.
    pub frames: String,
    pub tics: Tics,
    pub bright: bool,
    pub fast: bool,
    pub slow: bool,
    pub nodelay: bool,
    pub canraise: bool,
    pub light: Vec<String>,
    pub offset: Option<(Expr, Expr)>,
    pub action: Option<StateAction>,
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tics {
    Fixed(i32),
    Random(i32, i32),
}

#[derive(Clone, Debug)]
pub enum StateAction {
    Call { name: String, args: Vec<Arg> },
    Block(Block),
}

#[derive(Clone, Debug, Default)]
pub struct Block(pub Vec<Stmt>);

#[derive(Clone, Debug)]
pub enum Stmt {
    Expr(Expr),
    /// `Type a = 1, b[3];` / `let x = e;`
    Local { ty: TypeRef, vars: Vec<LocalVar> },
    If { cond: Expr, then: Box<Stmt>, otherwise: Option<Box<Stmt>> },
    While { cond: Expr, body: Box<Stmt> },
    /// `do ... while (c)` / `do ... until (c)`.
    DoWhile { body: Box<Stmt>, cond: Expr, until: bool },
    /// `until (c) body`.
    Until { cond: Expr, body: Box<Stmt> },
    For { init: Vec<Stmt>, cond: Option<Expr>, step: Vec<Expr>, body: Box<Stmt> },
    /// `foreach (x : list) body`
    ForEach { var: String, list: Expr, body: Box<Stmt> },
    Switch { value: Expr, body: Vec<SwitchItem> },
    Break,
    Continue,
    Return(Vec<Expr>),
    Block(Block),
    /// `[a, b] = f();`
    MultiAssign { targets: Vec<Expr>, value: Expr },
    Empty,
}

#[derive(Clone, Debug)]
pub struct LocalVar {
    pub name: String,
    pub dims: Vec<Expr>,
    pub init: Option<Expr>,
    /// `int a[] = { 1, 2 };`
    pub init_list: Option<Vec<Expr>>,
}

#[derive(Clone, Debug)]
pub enum SwitchItem {
    Case(Expr),
    Default,
    Stmt(Stmt),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    /// Named argument (`A_SpawnItemEx(type, flags: SXF_X)`).
    pub name: Option<String>,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Name(String),
    Bool(bool),
    Null,
    Ident(String),
    Super,
    SelfRef,
    Member(Box<Expr>, String),
    /// `Class::member`
    Scoped(String, String),
    Index(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Arg>),
    Unary(&'static str, Box<Expr>),
    /// `x++` / `x--`
    Postfix(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Assign(&'static str, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `(x, y)` / `(x, y, z)`
    Vector(Vec<Expr>),
    /// `new("Class")` / `new('Class')`
    New(Box<Expr>),
    /// `class<Actor>(expr)`
    ClassCast(String, Box<Expr>),
    /// `sizeof(x)` / `alignof(x)`
    SizeOf(Box<Expr>),
}
