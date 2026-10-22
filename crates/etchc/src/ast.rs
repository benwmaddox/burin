//! The syntax tree of an etch app (every .etch file in its directory, with no imports).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Span {
    pub file: u32,
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    Text,
    Int,
    Bool,
    Time,
    Duration,
    /// a reference to a row
    Ref(String),
    Opt(Box<Type>),
    List(Box<Type>),
    Record(Vec<(String, Type)>),
    /// HTML that is already safe to write as it is
    Html,
    /// a JSON value, the only thing a hole in an on…= attribute takes
    Json,
    None,
}

impl Type {
    pub fn strip_opt(&self) -> &Type {
        match self {
            Type::Opt(t) => t,
            t => t,
        }
    }
    pub fn show(&self) -> String {
        match self {
            Type::Text => "Text".into(),
            Type::Int => "Int".into(),
            Type::Bool => "Bool".into(),
            Type::Time => "Time".into(),
            Type::Duration => "Duration".into(),
            Type::Ref(e) => e.clone(),
            Type::Opt(t) => format!("{}?", t.show()),
            Type::List(t) => format!("list of {}", t.show()),
            Type::Record(fs) => format!("{{{}}}", fs.iter().map(|(n, t)| format!("{n}: {}", t.show())).collect::<Vec<_>>().join(", ")),
            Type::Html => "Html".into(),
            Type::Json => "Json".into(),
            Type::None => "none".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub ty: Type,
    /// `max N` for text
    pub max: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub name: String,
    pub fields: Vec<Field>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Str(String, Span),
    Int(i64, Span),
    Bool(bool, Span),
    /// a duration in milliseconds (`60s`)
    Dur(i64, Span),
    None(Span),
    /// a name: a parameter, a local, a const, `viewer`, `it`, `now`
    Name(String, Span),
    /// `.field`: a field of `it`
    Dot(String, Span),
    Field(Box<Expr>, String, Span),
    Call(String, Vec<Expr>, Span),
    Bin(BinOp, Box<Expr>, Box<Expr>, Span),
    Not(Box<Expr>, Span),
    If(Box<Expr>, Box<Expr>, Box<Expr>, Span),
    List(Vec<Expr>, Span),
    Record(Vec<(String, Expr)>, Span),
    Query(Box<Query>),
    /// `list(e for q)`, `count_distinct(e for q)`
    Comp(String, Box<Expr>, Box<Query>, Span),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Str(_, s) | Expr::Int(_, s) | Expr::Bool(_, s) | Expr::Dur(_, s) | Expr::None(s) | Expr::Name(_, s) | Expr::Dot(_, s) => *s,
            Expr::Field(_, _, s) | Expr::Call(_, _, s) | Expr::Bin(_, _, _, s) | Expr::Not(_, s) | Expr::If(_, _, _, s) => *s,
            Expr::List(_, s) | Expr::Record(_, s) | Expr::Comp(_, _, _, s) => *s,
            Expr::Query(q) => q.span,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Source {
    /// all rows of an entity
    Entity(String),
    /// a view's or a query-valued expression's rows (`others(r)`)
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct Query {
    pub source: Source,
    pub filter: Option<Expr>,
    pub order: Vec<(Expr, bool)>, // (key, descending)
    pub limit: Option<Expr>,
    /// `window N last`: the last N in the order, oldest first
    pub window_last: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TNode {
    Text(String),
    Hole(Expr),
    If(Expr, Vec<TNode>, Vec<TNode>),
    For(String, Expr, Option<Expr>, Vec<TNode>),
}

#[derive(Clone, Debug)]
pub struct Template {
    pub nodes: Vec<TNode>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Else {
    Outcome(String),
    Go(String),
    Unsupported,
}

#[derive(Clone, Debug)]
pub struct Require {
    pub cond: Expr,
    pub otherwise: Else,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Require(Require),
    Create(String, Vec<(String, Expr)>, Span),
    Touch(Expr, Span),
    Set(Expr, String, Expr, Span),
    ForEach(String, Query, Vec<Stmt>, Span),
}

#[derive(Clone, Debug)]
pub enum Decl {
    Config { name: String, ty: Type, span: Span },
    Entity(Entity),
    Viewer { entity: String, cookie: String, via_entity: String, via_field: String, to_field: String, span: Span },
    Fn { name: String, params: Vec<Param>, ret: Type, body: Expr, span: Span },
    View { name: String, params: Vec<Param>, query: Query, span: Span },
    Const { name: String, value: Expr, span: Span },
    /// `per viewer`: keyed by the viewer (its memory per viewer is reported)
    Fragment { name: String, params: Vec<Param>, per_viewer: bool, body: Template, span: Span },
    /// `cold`: rendered per request, outside the fast path
    Screen { name: String, params: Vec<Param>, route: String, cold: bool, requires: Vec<Require>, cookies: Vec<(String, Expr)>, body: Template, span: Span },
    Action { name: String, params: Vec<Param>, body: Vec<Stmt>, span: Span },
}

pub struct App {
    pub files: Vec<String>,
    pub decls: Vec<Decl>,
}
