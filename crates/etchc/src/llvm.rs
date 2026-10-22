//! Lowering registers to LLVM IR (docs/IR.md, "Lowering to LLVM").
//!
//! For each register (a fragment kept in the snapshot) the compiler emits:
//! - its **input record**: a flat C struct with exactly the values its template uses (paths such
//!   as `m.creator.name`, and scalars such as `first(Account).restrict_room_creation`), then a
//!   (pointer, count) per list it reads; lists nest (a list per item of a list);
//! - `etch_load_<f>(ctx, db, key, rec, arena) -> i1`: fills the record from SQLite: one SELECT for
//!   the record, one per list (per item, for a nested list), items in arrays of the lists' bounds;
//! - `etch_bits_<f>(in, viewer) -> i64`: its variant key: each viewer-dependent condition's bit,
//!   shifted in, in template order (in loops, one per item). The compiler proves the key fits 64
//!   bits from the lists' bounds and refuses a fragment whose key wouldn't. A `per viewer` register
//!   is keyed by the viewer instead, and has no variant key;
//! - `etch_build_<f>(ctx, in, viewer, out, tmps, splits)`: its bytes: appends of constant ranges
//!   (escaped and merged at compile time), escaped holes, a branch per condition, a loop per list.
//!   In a `per viewer` register, a hole that depends on a parameter is a **splice**: the builder
//!   marks the place (`splits`), and the hole is its own small register, keyed by the parameter.
//!
//! Each builder is generated twice: the first pass only settles the record's layout.

use crate::ast::*;
use crate::check::{Model, Scope};
use crate::inline;
use std::collections::HashMap;
use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// Int, Time, Bool and references (a row's id): 64-bit integers; none is i64::MIN
    I64,
    /// text: (pointer, length); none is a null pointer
    Str,
}

fn shape_of(t: &Type) -> Shape {
    match t.strip_opt() {
        Type::Text | Type::Html | Type::Json | Type::Password => Shape::Str,
        _ => Shape::I64,
    }
}

#[derive(Debug, Clone)]
pub struct ListSlot {
    pub key: String,
    pub var: String,
    pub expr: Expr,
    pub items: Schema,
}

/// The record a register (or a list's items) is built from.
#[derive(Default, Debug, Clone)]
pub struct Schema {
    /// (path, shape): `m.creator.name`; a reference's id is the path to the reference
    pub fields: Vec<(String, Shape)>,
    /// fields that are values of other queries (`first(Account).f`): (field index, expression)
    pub scalars: Vec<(usize, Expr)>,
    pub lists: Vec<ListSlot>,
}

impl Schema {
    fn field(&mut self, path: &str, s: Shape) -> usize {
        if let Some(i) = self.fields.iter().position(|(p, _)| p == path) {
            return i;
        }
        self.fields.push((path.to_string(), s));
        self.fields.len() - 1
    }
    fn list(&mut self, key: &str, var: &str, expr: &Expr) -> usize {
        if let Some(i) = self.lists.iter().position(|l| l.key == key) {
            return i;
        }
        self.lists.push(ListSlot { key: key.to_string(), var: var.to_string(), expr: expr.clone(), items: Schema::default() });
        self.lists.len() - 1
    }
    fn at(&self, path: &[usize]) -> &Schema {
        let mut s = self;
        for &k in path {
            s = &s.lists[k].items;
        }
        s
    }
    fn at_mut(&mut self, path: &[usize]) -> &mut Schema {
        let mut s = self;
        for &k in path {
            s = &mut s.lists[k].items;
        }
        s
    }
}

fn type_name(frag: &str, path: &[usize]) -> String {
    format!("%In_{frag}{}", path.iter().map(|k| format!("_{k}")).collect::<String>())
}

/// A path from a root (a parameter, a loop variable, `viewer`, `it`) through fields.
fn path_of(e: &Expr) -> Option<(String, Vec<String>)> {
    match e {
        Expr::Name(n, _) => Some((n.clone(), vec![])),
        Expr::Dot(f, _) => Some(("it".into(), if f == "id" { vec![] } else { vec![f.clone()] })),
        Expr::Field(o, f, _) => {
            let (r, mut p) = path_of(o)?;
            if f != "id" {
                p.push(f.clone());
            }
            Some((r, p))
        }
        _ => None,
    }
}

/// An expression printed in full (a list's identity: the same text is the same list).
pub fn show(e: &Expr) -> String {
    match e {
        Expr::Name(n, _) => n.clone(),
        Expr::Dot(f, _) => format!(".{f}"),
        Expr::Field(o, f, _) => format!("{}.{f}", show(o)),
        Expr::Call(f, a, _) => format!("{f}({})", a.iter().map(show).collect::<Vec<_>>().join(", ")),
        Expr::Str(s, _) => format!("{s:?}"),
        Expr::Int(n, _) => n.to_string(),
        Expr::Bool(b, _) => b.to_string(),
        Expr::None(_) => "none".into(),
        Expr::Dur(n, _) => format!("{n}ms"),
        Expr::Not(x, _) => format!("not {}", show(x)),
        Expr::Bin(op, l, r, _) => format!("({} {op:?} {})", show(l), show(r)),
        Expr::If(c, a, b, _) => format!("if {} {{{}}} else {{{}}}", show(c), show(a), show(b)),
        Expr::List(i, _) => format!("[{}]", i.iter().map(show).collect::<Vec<_>>().join(", ")),
        Expr::Record(fs, _) => format!("{{{}}}", fs.iter().map(|(k, v)| format!("{k}: {}", show(v))).collect::<Vec<_>>().join(", ")),
        Expr::Comp(f, x, q, _) => format!("{f}({} for {})", show(x), show(&Expr::Query(q.clone()))),
        Expr::Query(q) => {
            let mut s = match &q.source {
                Source::Entity(e) => e.clone(),
                Source::Expr(e) => show(e),
            };
            if let Some(f) = &q.filter {
                s += &format!(" where {}", show(f));
            }
            for (k, d) in &q.order {
                s += &format!(" order {}{}", show(k), if *d { " desc" } else { "" });
            }
            if let Some(l) = &q.limit {
                s += &format!(" limit {}", show(l));
            }
            if let Some(w) = &q.window_last {
                s += &format!(" window {}", show(w));
            }
            s
        }
    }
}

/// A list expression as a list: a query with no clauses is its source.
fn as_list(q: &Query) -> Expr {
    match &q.source {
        Source::Expr(e) if q.filter.is_none() && q.order.is_empty() && q.limit.is_none() && q.window_last.is_none() => e.clone(),
        _ => Expr::Query(Box::new(q.clone())),
    }
}

/// The paths a list's query binds from outside it (its arguments): they must be in the record.
fn list_args(e: &Expr) -> Vec<Expr> {
    match e {
        Expr::Call(_, args, _) => args.clone(),
        Expr::Query(q) => match &q.source {
            Source::Expr(inner) => list_args(inner),
            Source::Entity(_) => vec![],
        },
        _ => vec![],
    }
}

#[derive(Clone)]
enum Root {
    /// the record at this pointer, at this place in the register's schema
    Rec { ptr: String, at: Vec<usize> },
    /// a constant record (a constant list's item, unrolled)
    Const(Expr),
}

fn esc_bytes(s: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(s.len());
    for &c in s {
        match c {
            b'&' => o.extend_from_slice(b"&amp;"),
            b'<' => o.extend_from_slice(b"&lt;"),
            b'>' => o.extend_from_slice(b"&gt;"),
            b'"' => o.extend_from_slice(b"&quot;"),
            b'\'' => o.extend_from_slice(b"&#39;"),
            c => o.push(c),
        }
    }
    o
}

fn ll_bytes(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len());
    for &c in b {
        if (0x20..0x7f).contains(&c) && c != b'"' && c != b'\\' {
            s.push(c as char);
        } else {
            let _ = write!(s, "\\{c:02X}");
        }
    }
    s
}

/// A register, as generated.
#[derive(Clone)]
pub struct RegInfo {
    pub name: String,
    pub schema: Schema,
    pub tmps: usize,
    pub bits: usize,
    pub per_viewer: bool,
    /// the parameter whose row the record is (None for a register keyed by the viewer)
    pub root: Option<(String, String)>,
    /// for a `per viewer` register: its splices' registers, in order
    pub splices: Vec<String>,
    /// every (entity, column) its loader reads ("*": the entity's rows)
    pub reads: Vec<(String, String)>,
}

pub struct Module {
    pub globals: String,
    strs: HashMap<Vec<u8>, String>,
    pub viewer: Schema,
    pub funcs: String,
    pub registers: Vec<RegInfo>,
    /// what the viewer's record reads (its variant bits' inputs)
    pub viewer_reads: Vec<(String, String)>,
    /// each register's viewer dependence, as read (the report)
    pub keys: Vec<(String, Vec<String>)>,
    /// every register's name, in kind order (known before any is generated)
    pub order: Vec<String>,
}

impl Module {
    pub fn viewer_field(&mut self, key: &str, s: Shape) -> usize {
        self.viewer.field(key, s)
    }
    pub fn str_const_pub(&mut self, b: &[u8]) -> String {
        self.str_const(b)
    }
    fn str_const(&mut self, b: &[u8]) -> String {
        if let Some(g) = self.strs.get(b) {
            return g.clone();
        }
        let g = format!("@s{}", self.strs.len());
        let _ = writeln!(self.globals, "{g} = private unnamed_addr constant [{} x i8] c\"{}\", align 1", b.len(), ll_bytes(b));
        self.strs.insert(b.to_vec(), g.clone());
        g
    }
}

struct Gen<'a, 'b> {
    m: &'a Model<'a>,
    md: &'b mut Module,
    frag: String,
    body: String,
    n: usize,
    pending: Vec<u8>,
    schema: Schema,
    scope: Scope,
    tmps: usize,
    depth: usize,
    bits: bool,
    max_bits: usize,
    mult: usize,
    per_viewer: bool,
    /// a `per viewer` register's parameters: holes on them are splices
    splice_params: Vec<String>,
    /// the splices' expressions, in order
    splices: Vec<Expr>,
    /// what about the viewer the variant key read (`viewer`: its identity; `viewer.role`)
    viewer_uses: Vec<String>,
    /// the variant key's per-item viewer conditions over the record's own lists, recorded once
    /// at its start (however often and wherever the template checks them): their keys
    hoisted: Vec<String>,
    prologue: String,
}

struct Loop {
    idx: String,
    i: String,
    head: String,
    done: String,
    saved: (Scope, usize),
}

impl<'a, 'b> Gen<'a, 'b> {
    fn v(&mut self) -> String {
        self.n += 1;
        format!("%v{}", self.n)
    }
    fn label(&mut self, what: &str) -> String {
        self.n += 1;
        format!("{what}{}", self.n)
    }
    fn line(&mut self, s: &str) {
        self.body.push_str("  ");
        self.body.push_str(s);
        self.body.push('\n');
    }
    fn block(&mut self, l: &str) {
        let _ = writeln!(self.body, "{l}:");
    }
    fn flush(&mut self, out: &str) {
        if self.pending.is_empty() || self.bits {
            self.pending.clear();
            return;
        }
        let b = std::mem::take(&mut self.pending);
        let g = self.md.str_const(&b);
        self.line(&format!("call void @etch_put(ptr {out}, ptr {g}, i64 {})", b.len()));
    }
    fn tmp(&mut self) -> String {
        let k = self.depth;
        self.depth += 1;
        if self.bits {
            // the variant key's function writes no text: no buffers
            return "%none".into();
        }
        self.tmps = self.tmps.max(self.depth);
        let (p, t) = (self.v(), self.v());
        self.line(&format!("{p} = getelementptr ptr, ptr %tmps, i64 {k}"));
        self.line(&format!("{t} = load ptr, ptr {p}"));
        self.line(&format!("call void @etch_clear(ptr {t})"));
        t
    }
    fn untmp(&mut self) {
        self.depth -= 1;
    }
    fn gep_load(&mut self, ptr: &str, ty: &str, i: usize, shape: Shape) -> String {
        let (p, v) = (self.v(), self.v());
        self.line(&format!("{p} = getelementptr inbounds {ty}, ptr {ptr}, i32 0, i32 {i}"));
        self.line(&format!("{v} = load {}, ptr {p}", if shape == Shape::Str { "%Str" } else { "i64" }));
        v
    }

    /// A path's value: i64, or a %Str.
    fn load(&mut self, roots: &HashMap<String, Root>, e: &Expr) -> Result<(String, Shape), String> {
        let (root, path) = path_of(e).ok_or_else(|| format!("can't lower `{}` in a fragment yet", show(e)))?;
        let ty = self.m.type_of(e, &self.scope).map_err(|x| format!("{}: {}", show(e), x.msg))?;
        let shape = shape_of(&ty);
        let key = std::iter::once(root.clone()).chain(path.iter().cloned()).collect::<Vec<_>>().join(".");
        if root == "viewer" {
            if !self.per_viewer {
                return Err(format!(
                    "fragment {}: reads the viewer (`{key}`), but a cached fragment is the same for everyone: mark it `per viewer` (rendered per request), or move what depends on the viewer out of it",
                    self.frag
                ));
            }
            let i = self.md.viewer.field(&key, shape);
            return Ok((self.gep_load("%viewer", "%Viewer", i, shape), shape));
        }
        match roots.get(&root) {
            Some(Root::Rec { ptr, at }) => {
                let (ptr, at) = (ptr.clone(), at.clone());
                let i = self.schema.at_mut(&at).field(&key, shape);
                let ty = type_name(&self.frag, &at);
                Ok((self.gep_load(&ptr, &ty, i, shape), shape))
            }
            _ => Err(format!("`{}` isn't a value here", show(e))),
        }
    }

    /// A compile-time text constant, if `e` is one.
    fn constant(&self, roots: &HashMap<String, Root>, e: &Expr) -> Option<String> {
        match e {
            Expr::Str(s, _) => Some(s.clone()),
            Expr::Int(n, _) => Some(n.to_string()),
            Expr::Bin(BinOp::Add, l, r, _) if !self.numeric(e) => Some(self.constant(roots, l)? + &self.constant(roots, r)?),
            Expr::Name(n, _) => match roots.get(n) {
                Some(Root::Const(c)) => self.constant(roots, c),
                _ => None,
            },
            // a file of the app, read at compile time
            Expr::Call(f, a, _) if f == "include" => match a.first() {
                Some(Expr::Str(p, _)) => {
                    let dir = std::path::Path::new(self.m.app.files.first()?).parent()?;
                    Some(std::fs::read_to_string(dir.join(p)).unwrap_or_else(|e| panic!("include {p}: {e}")))
                }
                _ => None,
            },
            Expr::Field(o, f, _) => match &**o {
                Expr::Name(n, _) => match roots.get(n) {
                    Some(Root::Const(Expr::Record(fs, _))) => fs.iter().find(|(k, _)| k == f).and_then(|(_, v)| self.constant(roots, v)),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        }
    }

    /// The list `list` (a view call or query) in the record of the root its arguments name (or
    /// the register's own record): (that record's pointer, its place, the list's index).
    fn list_slot(&mut self, roots: &HashMap<String, Root>, list: &Expr, var: &str) -> Result<(String, Vec<usize>, usize), String> {
        let mut parent = ("%in".to_string(), vec![]);
        for a in list_args(list) {
            if let Some((r, _)) = path_of(&a) {
                if let Some(Root::Rec { ptr, at }) = roots.get(&r) {
                    if at.len() >= parent.1.len() {
                        parent = (ptr.clone(), at.clone());
                    }
                }
                // the argument's value is a field of its record (the loader binds it)
                if r != "viewer" && roots.contains_key(&r) {
                    let saved = self.body.len();
                    self.load(roots, &a)?;
                    self.body.truncate(saved); // only its place in the record is needed here
                }
            }
        }
        let key = show(list);
        let li = self.schema.at_mut(&parent.1).list(&key, var, list);
        Ok((parent.0, parent.1, li))
    }

    /// Opens a loop over a list's items: the item record's pointer is bound to `var`.
    fn open_loop(&mut self, roots: &HashMap<String, Root>, list: &Expr, var: &str) -> Result<(HashMap<String, Root>, Loop), String> {
        let (parent, at, li) = self.list_slot(roots, list, var)?;
        let slot = self.schema.at(&at).fields.len() + li;
        let mut item_at = at.clone();
        item_at.push(li);
        // every item carries its row's id (a write's patch finds its item by it)
        let id_key = self.schema.at(&at).lists[li].var.clone();
        self.schema.at_mut(&item_at).field(&id_key, Shape::I64);
        let (lp, l, items, count) = (self.v(), self.v(), self.v(), self.v());
        self.line(&format!("{lp} = getelementptr inbounds {}, ptr {parent}, i32 0, i32 {slot}", type_name(&self.frag, &at)));
        self.line(&format!("{l} = load %List, ptr {lp}"));
        self.line(&format!("{items} = extractvalue %List {l}, 0"));
        self.line(&format!("{count} = extractvalue %List {l}, 1"));
        let idx = format!("%i{}", self.n + 1);
        self.n += 1;
        self.line(&format!("{idx} = alloca i64"));
        self.line(&format!("store i64 0, ptr {idx}"));
        let (head, item, done) = (self.label("loop"), self.label("item"), self.label("done"));
        self.line(&format!("br label %{head}"));
        self.block(&head);
        let (i, more) = (self.v(), self.v());
        self.line(&format!("{i} = load i64, ptr {idx}"));
        self.line(&format!("{more} = icmp slt i64 {i}, {count}"));
        self.line(&format!("br i1 {more}, label %{item}, label %{done}"));
        self.block(&item);
        let ip = self.v();
        self.line(&format!("{ip} = getelementptr inbounds {}, ptr {items}, i64 {i}", type_name(&self.frag, &item_at)));
        let mut inner = roots.clone();
        inner.insert(var.to_string(), Root::Rec { ptr: ip, at: item_at });
        let ty = match self.m.type_of(list, &self.scope).map_err(|x| x.msg)? {
            Type::List(t) => *t,
            t => return Err(format!("not a list: {}", t.show())),
        };
        let saved = (self.scope.clone(), self.mult);
        self.scope = self.scope.with(var, ty);
        self.mult *= self.m.bound_of(list, &saved.0).ok_or_else(|| format!("an unbounded list: {}", show(list)))? as usize;
        Ok((inner, Loop { idx, i, head, done, saved }))
    }

    fn close_loop(&mut self, l: Loop) {
        let nx = self.v();
        self.line(&format!("{nx} = add i64 {}, 1", l.i));
        self.line(&format!("store i64 {nx}, ptr {}", l.idx));
        self.line(&format!("br label %{}", l.head));
        self.block(&l.done);
        (self.scope, self.mult) = l.saved;
    }

    /// Whether `e` is arithmetic on numbers, times or durations (not text joined with `+`).
    fn numeric(&self, e: &Expr) -> bool {
        matches!(self.m.type_of(e, &self.scope).map(|t| t.strip_opt().clone()), Ok(Type::Int | Type::Time | Type::Duration))
    }

    /// `a op b` on i64s, as SQLite computes it on INTEGERs, so a page and an action's `require`
    /// agree: none (i64::MIN) in gives none; so do a divisor of 0 and an overflow (SQLite turns an
    /// overflow into a REAL, which a STRICT column refuses).
    fn arith(&mut self, op: &BinOp, a: &str, b: &str) -> String {
        const NONE: &str = "-9223372036854775808";
        let (na, nb, bad) = (self.v(), self.v(), self.v());
        self.line(&format!("{na} = icmp eq i64 {a}, {NONE}\n  {nb} = icmp eq i64 {b}, {NONE}\n  {bad} = or i1 {na}, {nb}"));
        let (x, fail) = match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                let f = match op {
                    BinOp::Add => "sadd",
                    BinOp::Sub => "ssub",
                    _ => "smul",
                };
                let (p, x, o, fail) = (self.v(), self.v(), self.v(), self.v());
                self.line(&format!("{p} = call {{i64, i1}} @llvm.{f}.with.overflow.i64(i64 {a}, i64 {b})\n  {x} = extractvalue {{i64, i1}} {p}, 0\n  {o} = extractvalue {{i64, i1}} {p}, 1\n  {fail} = or i1 {bad}, {o}"));
                (x, fail)
            }
            _ => {
                // the divisor made 1 where the answer is none anyway: sdiv by 0 is undefined
                let (z, fail, d, x) = (self.v(), self.v(), self.v(), self.v());
                let ins = if *op == BinOp::Div { "sdiv" } else { "srem" };
                self.line(&format!("{z} = icmp eq i64 {b}, 0\n  {fail} = or i1 {bad}, {z}\n  {d} = select i1 {fail}, i64 1, i64 {b}\n  {x} = {ins} i64 {a}, {d}"));
                (x, fail)
            }
        };
        let r = self.v();
        self.line(&format!("{r} = select i1 {fail}, i64 {NONE}, i64 {x}"));
        r
    }

    /// The smaller (`min`) or larger of two i64s; none when either is none, as SQLite's.
    fn pick(&mut self, max: bool, a: &str, b: &str) -> String {
        const NONE: &str = "-9223372036854775808";
        let (na, nb, bad, c, x, r) = (self.v(), self.v(), self.v(), self.v(), self.v(), self.v());
        self.line(&format!(
            "{na} = icmp eq i64 {a}, {NONE}\n  {nb} = icmp eq i64 {b}, {NONE}\n  {bad} = or i1 {na}, {nb}\n  {c} = icmp {} i64 {a}, {b}\n  {x} = select i1 {c}, i64 {a}, i64 {b}\n  {r} = select i1 {bad}, i64 {NONE}, i64 {x}",
            if max { "sgt" } else { "slt" }
        ));
        r
    }

    fn value(&mut self, roots: &HashMap<String, Root>, e: &Expr) -> Result<(String, Shape), String> {
        match e {
            Expr::Int(n, _) | Expr::Dur(n, _) => Ok((n.to_string(), Shape::I64)),
            Expr::Bin(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod), l, r, _) if self.numeric(e) => {
                let (a, _) = self.value(roots, l)?;
                let (b, _) = self.value(roots, r)?;
                Ok((self.arith(op, &a, &b), Shape::I64))
            }
            Expr::Call(f, a, _) if f == "min" || f == "max" => {
                let (x, _) = self.value(roots, &a[0])?;
                let (y, _) = self.value(roots, &a[1])?;
                Ok((self.pick(f == "max", &x, &y), Shape::I64))
            }
            Expr::Call(f, a, _) if f == "clamp" => {
                let (x, _) = self.value(roots, &a[0])?;
                let (lo, _) = self.value(roots, &a[1])?;
                let (hi, _) = self.value(roots, &a[2])?;
                let m = self.pick(false, &x, &hi);
                Ok((self.pick(true, &lo, &m), Shape::I64))
            }
            Expr::Call(f, a, _) if f == "abs" => {
                // none (i64::MIN) is its own negation, so it stays none
                let (x, _) = self.value(roots, &a[0])?;
                let (n, c, r) = (self.v(), self.v(), self.v());
                self.line(&format!("{n} = sub i64 0, {x}\n  {c} = icmp slt i64 {x}, 0\n  {r} = select i1 {c}, i64 {n}, i64 {x}"));
                Ok((r, Shape::I64))
            }
            Expr::Call(f, a, _) if f == "count" && matches!(&a[0], Expr::Query(q) if Self::split_viewer(q).1.is_some()) => {
                let Expr::Query(q) = &a[0] else { unreachable!() };
                let (rest, vc) = Self::split_viewer(q);
                let n = format!("%cnt{}", self.n + 1);
                self.n += 1;
                self.line(&format!("{n} = alloca i64\n  store i64 0, ptr {n}"));
                let hoisted = self.hoist(roots, &as_list(&rest), vc.as_ref().unwrap())?;
                let (inner, l) = self.open_loop(roots, &as_list(&rest), "it")?;
                let cv = self.cond(&inner, vc.as_ref().unwrap())?;
                if !hoisted {
                    self.bit(&cv);
                }
                let (c0, z, c1) = (self.v(), self.v(), self.v());
                self.line(&format!("{c0} = load i64, ptr {n}\n  {z} = zext i1 {cv} to i64\n  {c1} = add i64 {c0}, {z}\n  store i64 {c1}, ptr {n}"));
                self.close_loop(l);
                let r = self.v();
                self.line(&format!("{r} = load i64, ptr {n}"));
                Ok((r, Shape::I64))
            }
            Expr::Call(f, a, _) if f == "count" => {
                let (parent, at, li) = self.list_slot(roots, &a[0], "it")?;
                let slot = self.schema.at(&at).fields.len() + li;
                let (lp, l, n) = (self.v(), self.v(), self.v());
                self.line(&format!("{lp} = getelementptr inbounds {}, ptr {parent}, i32 0, i32 {slot}", type_name(&self.frag, &at)));
                self.line(&format!("{l} = load %List, ptr {lp}"));
                self.line(&format!("{n} = extractvalue %List {l}, 1"));
                Ok((n, Shape::I64))
            }
            Expr::Field(o, _, _) if matches!(&**o, Expr::Call(f, _, _) if f == "first") => {
                // a value of another query: a scalar field of the register's own record
                let key = show(e);
                let ty = self.m.type_of(e, &self.scope).map_err(|x| x.msg)?;
                let shape = shape_of(&ty);
                let i = self.schema.field(&key, shape);
                if !self.schema.scalars.iter().any(|(k, _)| *k == i) {
                    self.schema.scalars.push((i, e.clone()));
                }
                let ty = type_name(&self.frag, &[]);
                Ok((self.gep_load("%in", &ty, i, shape), shape))
            }
            _ => self.load(roots, e),
        }
    }

    /// Text written to `out`, escaped for HTML when `esc`.
    fn text(&mut self, roots: &HashMap<String, Root>, e: &Expr, out: &str, esc: bool) -> Result<(), String> {
        if let Some(c) = self.constant(roots, e) {
            if !self.bits {
                self.pending.extend(if esc { esc_bytes(c.as_bytes()) } else { c.into_bytes() });
            }
            return Ok(());
        }
        if let Expr::Bin(BinOp::Add, l, r, _) = e {
            if !self.numeric(e) {
                self.text(roots, l, out, esc)?;
                return self.text(roots, r, out, esc);
            }
        }
        self.flush(out);
        if let Expr::If(c, a, b, _) = e {
            let cv = self.cond(roots, c)?;
            if Self::on_viewer(c) {
                self.bit(&cv);
            }
            let (lt, lf, le) = (self.label("t"), self.label("f"), self.label("e"));
            self.line(&format!("br i1 {cv}, label %{lt}, label %{lf}"));
            self.block(&lt);
            self.text(roots, a, out, esc)?;
            self.flush(out);
            self.line(&format!("br label %{le}"));
            self.block(&lf);
            self.text(roots, b, out, esc)?;
            self.flush(out);
            self.line(&format!("br label %{le}"));
            self.block(&le);
            return Ok(());
        }
        let e1 = esc as i64;
        // a helper that takes its argument as text in a temporary buffer
        let via_tmp = |g: &mut Gen, arg: &Expr, call: &dyn Fn(&str) -> String| -> Result<(), String> {
            let t = g.tmp();
            g.text(roots, arg, &t, false)?;
            g.flush(&t);
            let c = call(&t);
            if !g.bits {
                g.line(&c);
            }
            g.untmp();
            Ok(())
        };
        match e {
            Expr::Call(f, args, _) => match f.as_str() {
                "text" | "grouped" => {
                    let dur = matches!(self.m.type_of(&args[0], &self.scope).map_err(|x| x.msg)?.strip_opt(), Type::Duration);
                    let (v, _) = self.value(roots, &args[0])?;
                    if !self.bits {
                        if dur {
                            self.line(&format!("call void @etch_time(ptr {out}, i64 3, i64 {v}, i64 {e1})"));
                        } else if f == "grouped" {
                            self.line(&format!("call void @etch_time(ptr {out}, i64 4, i64 {v}, i64 {e1})"));
                        } else {
                            self.line(&format!("call void @etch_int(ptr {out}, i64 {v})"));
                        }
                    }
                }
                "json" | "quote" => via_tmp(self, &args[0], &|t| format!("call void @etch_json(ptr {out}, ptr {t}, i64 {e1})"))?,
                "sign" => via_tmp(self, &args[0], &|t| format!("call void @etch_sign(ptr %ctx, ptr {out}, ptr {t}, i64 {e1})"))?,
                "autolink" => via_tmp(self, &args[0], &|t| format!("call void @etch_autolink(ptr {out}, ptr {t})"))?,
                "asset" => via_tmp(self, &args[0], &|t| format!("call void @etch_asset(ptr %ctx, ptr {out}, ptr {t}, i64 {e1})"))?,
                "choose" => {
                    let items: Vec<String> = match &args[0] {
                        Expr::Name(n, _) => match self.m.consts.get(n) {
                            Some(Decl::Const { value: Expr::List(xs, _), .. }) => xs.iter().map(|x| self.constant(roots, x).ok_or("choose's list holds texts")).collect::<Result<_, _>>()?,
                            _ => return Err("choose takes a constant list".into()),
                        },
                        _ => return Err("choose takes a constant list".into()),
                    };
                    let t = self.tmp();
                    self.text(roots, &args[1], &t, false)?;
                    self.flush(&t);
                    if !self.bits {
                        let i = self.v();
                        self.line(&format!("{i} = call i64 @etch_crc_mod(ptr {t}, i64 {})", items.len()));
                        let done = self.label("chosen");
                        let labels: Vec<String> = (0..items.len()).map(|_| self.label("choice")).collect();
                        let cases: String = labels.iter().enumerate().map(|(k, l)| format!("i64 {k}, label %{l} ")).collect();
                        self.line(&format!("switch i64 {i}, label %{done} [ {cases}]"));
                        for (k, l) in labels.iter().enumerate() {
                            self.block(l);
                            let b = if esc { esc_bytes(items[k].as_bytes()) } else { items[k].clone().into_bytes() };
                            let g = self.md.str_const(&b);
                            self.line(&format!("call void @etch_put(ptr {out}, ptr {g}, i64 {})", b.len()));
                            self.line(&format!("br label %{done}"));
                        }
                        self.block(&done);
                    }
                    self.untmp();
                }
                "first_word" | "initials" | "lower" | "upper" => {
                    let f = f.clone();
                    via_tmp(self, &args[0], &|t| format!("call void @etch_{f}(ptr {out}, ptr {t}, i64 {e1})"))?
                }
                "replace" => {
                    let (from, to) = match (self.constant(roots, &args[1]), self.constant(roots, &args[2])) {
                        (Some(a), Some(b)) => (a, b),
                        _ => return Err("replace takes constant text to find and to put".into()),
                    };
                    let (gf, gt) = (self.md.str_const(from.as_bytes()), self.md.str_const(to.as_bytes()));
                    let (fl, tl) = (from.len(), to.len());
                    via_tmp(self, &args[0], &|t| format!("call void @etch_replace(ptr {out}, ptr {t}, ptr {gf}, i64 {fl}, ptr {gt}, i64 {tl}, i64 {e1})"))?
                }
                "iso" | "date" | "clock" => {
                    let k = ["iso", "date", "clock"].iter().position(|x| x == f).unwrap();
                    let (v, _) = self.value(roots, &args[0])?;
                    if !self.bits {
                        self.line(&format!("call void @etch_time(ptr {out}, i64 {k}, i64 {v}, i64 {e1})"));
                    }
                }
                "sentence" => {
                    // the list's items, each written then ended with a NUL (text has none), then joined
                    let (x, qy) = match &args[0] {
                        Expr::Comp(c, x, qy, _) if c == "list" => (x, qy),
                        _ => return Err("sentence takes list(… for …)".into()),
                    };
                    let t = self.tmp();
                    let (rest, vc) = Self::split_viewer(qy);
                    let list = as_list(&rest);
                    let hoisted = match &vc {
                        Some(c) => self.hoist(roots, &list, c)?,
                        None => false,
                    };
                    let (inner, l) = self.open_loop(roots, &list, "it")?;
                    let skip = match &vc {
                        Some(c) => {
                            let cv = self.cond(&inner, c)?;
                            if !hoisted {
                                self.bit(&cv);
                            }
                            let (yes, no) = (self.label("in"), self.label("out"));
                            self.line(&format!("br i1 {cv}, label %{yes}, label %{no}"));
                            self.block(&yes);
                            Some(no)
                        }
                        None => None,
                    };
                    self.text(&inner, x, &t, false)?;
                    self.pending.push(0);
                    self.flush(&t);
                    if let Some(no) = skip {
                        self.line(&format!("br label %{no}"));
                        self.block(&no);
                    }
                    self.close_loop(l);
                    if !self.bits {
                        self.line(&format!("call void @etch_sentence(ptr {out}, ptr {t}, i64 {e1})"));
                    }
                    self.untmp();
                }
                // a number (`min`, `max`, `clamp`, `abs`): its digits
                _ if self.numeric(e) && matches!(f.as_str(), "min" | "max" | "clamp" | "abs") => {
                    let (v, _) = self.value(roots, e)?;
                    if !self.bits {
                        self.line(&format!("call void @etch_int(ptr {out}, i64 {v})"));
                    }
                }
                _ => return Err(format!("`{f}` can't be lowered in a fragment yet")),
            },
            Expr::Field(o, f, _) if matches!(&**o, Expr::Name(n, _) if n == "config") => {
                let k = self.m.app.decls.iter().filter_map(|d| if let Decl::Config { name, .. } = d { Some(name) } else { None }).position(|n| n == f).unwrap();
                if !self.bits {
                    self.line(&format!("call void @etch_config(ptr %ctx, ptr {out}, i64 {k}, i64 {e1})"));
                }
            }
            _ => {
                let (v, shape) = self.value(roots, e)?;
                if self.bits {
                    return Ok(());
                }
                match shape {
                    Shape::I64 => self.line(&format!("call void @etch_int(ptr {out}, i64 {v})")),
                    Shape::Str => {
                        let (p, l) = (self.v(), self.v());
                        self.line(&format!("{p} = extractvalue %Str {v}, 0"));
                        self.line(&format!("{l} = extractvalue %Str {v}, 1"));
                        self.line(&format!("call void @etch_{}(ptr {out}, ptr {p}, i64 {l})", if esc { "esc" } else { "put" }));
                    }
                }
            }
        }
        Ok(())
    }

    /// An i1 condition.
    fn cond(&mut self, roots: &HashMap<String, Root>, e: &Expr) -> Result<String, String> {
        match e {
            Expr::Bool(b, _) => Ok(b.to_string()),
            Expr::Not(x, _) => {
                let c = self.cond(roots, x)?;
                let v = self.v();
                self.line(&format!("{v} = xor i1 {c}, true"));
                Ok(v)
            }
            Expr::Bin(op @ (BinOp::And | BinOp::Or), l, r, _) => {
                let (a, b) = (self.cond(roots, l)?, self.cond(roots, r)?);
                let v = self.v();
                self.line(&format!("{v} = {} i1 {a}, {b}", if *op == BinOp::And { "and" } else { "or" }));
                Ok(v)
            }
            Expr::Bin(op @ (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge), l, r, _) => {
                let pred = match op {
                    BinOp::Lt => "slt",
                    BinOp::Le => "sle",
                    BinOp::Gt => "sgt",
                    _ => "sge",
                };
                let (a, _) = self.value(roots, l)?;
                let (b, _) = self.value(roots, r)?;
                let v = self.v();
                self.line(&format!("{v} = icmp {pred} i64 {a}, {b}"));
                Ok(v)
            }
            Expr::Bin(op @ (BinOp::Eq | BinOp::Ne), l, r, _) => {
                let pred = if *op == BinOp::Eq { "eq" } else { "ne" };
                let v = self.v();
                let (none_l, none_r) = (matches!(**l, Expr::None(_)), matches!(**r, Expr::None(_)));
                if none_l || none_r {
                    let (xv, shape) = self.value(roots, if none_l { r } else { l })?;
                    if shape == Shape::Str {
                        let p = self.v();
                        self.line(&format!("{p} = extractvalue %Str {xv}, 0"));
                        self.line(&format!("{v} = icmp {pred} ptr {p}, null"));
                    } else {
                        self.line(&format!("{v} = icmp {pred} i64 {xv}, -9223372036854775808"));
                    }
                    return Ok(v);
                }
                if let Some(c) = self.constant(roots, r) {
                    let (lv, shape) = self.value(roots, l)?;
                    if shape == Shape::Str {
                        let g = self.md.str_const(c.as_bytes());
                        let (p, n, eq) = (self.v(), self.v(), self.v());
                        self.line(&format!("{p} = extractvalue %Str {lv}, 0"));
                        self.line(&format!("{n} = extractvalue %Str {lv}, 1"));
                        self.line(&format!("{eq} = call i1 @etch_str_eq(ptr {p}, i64 {n}, ptr {g}, i64 {})", c.len()));
                        self.line(&format!("{v} = xor i1 {eq}, {}", if *op == BinOp::Eq { "false" } else { "true" }));
                    } else {
                        self.line(&format!("{v} = icmp {pred} i64 {lv}, {c}"));
                    }
                    return Ok(v);
                }
                let (a, sa) = self.value(roots, l)?;
                let (b, _) = self.value(roots, r)?;
                if sa == Shape::Str {
                    return Err("comparing two texts isn't lowered yet".into());
                }
                self.line(&format!("{v} = icmp {pred} i64 {a}, {b}"));
                Ok(v)
            }
            // a Bool value: true when not 0
            _ => {
                let (x, _) = self.value(roots, e)?;
                let v = self.v();
                self.line(&format!("{v} = icmp ne i64 {x}, 0"));
                Ok(v)
            }
        }
    }

    fn on_viewer(e: &Expr) -> bool {
        match e {
            Expr::Name(n, _) => n == "viewer",
            Expr::Field(o, _, _) | Expr::Not(o, _) => Self::on_viewer(o),
            Expr::Bin(_, l, r, _) => Self::on_viewer(l) || Self::on_viewer(r),
            Expr::Call(_, a, _) => a.iter().any(Self::on_viewer),
            Expr::If(c, a, b, _) => Self::on_viewer(c) || Self::on_viewer(a) || Self::on_viewer(b),
            Expr::Comp(_, x, q, _) => Self::on_viewer(x) || Self::on_viewer(&Expr::Query(q.clone())),
            Expr::Query(q) => {
                q.filter.as_ref().map(Self::on_viewer).unwrap_or(false)
                    || matches!(&q.source, Source::Expr(x) if Self::on_viewer(x))
                    || q.order.iter().any(|(k, _)| Self::on_viewer(k))
            }
            _ => false,
        }
    }

    /// A shared register's text may not depend on the viewer (only its conditions may: they are
    /// its variant bits). The value parts of a hole: not conditions, nor filters split out below.
    fn viewer_value(e: &Expr) -> bool {
        match e {
            Expr::Name(n, _) => n == "viewer",
            Expr::Field(o, _, _) => Self::viewer_value(o),
            Expr::If(_, a, b, _) => Self::viewer_value(a) || Self::viewer_value(b),
            Expr::Bin(BinOp::Add, l, r, _) => Self::viewer_value(l) || Self::viewer_value(r),
            Expr::Call(f, a, _) if f == "count" => a.iter().any(|x| match x {
                Expr::Query(q) => Self::viewer_source(q),
                x => Self::on_viewer(x),
            }),
            Expr::Call(_, a, _) => a.iter().any(Self::viewer_value),
            Expr::Comp(_, x, q, _) => Self::viewer_value(x) || Self::viewer_source(q),
            Expr::Query(q) => Self::viewer_source(q),
            _ => false,
        }
    }

    /// A query that depends on the viewer other than through filter conjuncts checked per item.
    fn viewer_source(q: &Query) -> bool {
        matches!(&q.source, Source::Expr(x) if Self::on_viewer(x)) || q.order.iter().any(|(k, _)| Self::on_viewer(k)) || q.limit.as_ref().map(Self::on_viewer).unwrap_or(false)
    }

    /// A query's filter split: the conjuncts on the viewer (checked per item, at build time: each
    /// a variant bit), and the query without them (loaded with the record).
    fn split_viewer(q: &Query) -> (Query, Option<Expr>) {
        let mut conj = vec![];
        fn split(e: &Expr, out: &mut Vec<Expr>) {
            if let Expr::Bin(BinOp::And, l, r, _) = e {
                split(l, out);
                split(r, out);
            } else {
                out.push(e.clone());
            }
        }
        if let Some(f) = &q.filter {
            split(f, &mut conj);
        }
        let (on, off): (Vec<Expr>, Vec<Expr>) = conj.into_iter().partition(Self::on_viewer);
        let join = |v: Vec<Expr>| v.into_iter().reduce(|a, b| Expr::Bin(BinOp::And, Box::new(a), Box::new(b), q.span));
        (Query { filter: join(off), ..q.clone() }, join(on))
    }

    /// A per-item viewer condition over a list: its bits, once. Over a list of the register's own
    /// record (not inside another loop) whose condition reads only the item and the viewer, they
    /// are recorded at the key's start, the first time; true then (the caller records none).
    fn hoist(&mut self, roots: &HashMap<String, Root>, list: &Expr, vc: &Expr) -> Result<bool, String> {
        if !self.bits || self.per_viewer || self.mult != 1 {
            return Ok(false);
        }
        fn roots_of(e: &Expr, out: &mut Vec<String>) {
            match e {
                Expr::Dot(..) => out.push("it".into()),
                Expr::Name(n, _) => out.push(n.clone()),
                Expr::Field(o, _, _) | Expr::Not(o, _) => roots_of(o, out),
                Expr::Bin(_, l, r, _) => {
                    roots_of(l, out);
                    roots_of(r, out);
                }
                Expr::Str(..) | Expr::Int(..) | Expr::Bool(..) | Expr::None(_) | Expr::Dur(..) => {}
                _ => out.push("?".into()),
            }
        }
        let mut rs = vec![];
        roots_of(vc, &mut rs);
        if !rs.iter().all(|r| r == "it" || r == "viewer") {
            return Ok(false);
        }
        let saved = self.body.len();
        let (parent, _, _) = self.list_slot(roots, list, "it")?;
        self.body.truncate(saved);
        if parent != "%in" {
            return Ok(false);
        }
        let key = format!("{} | {}", show(list), show(vc));
        if !self.hoisted.contains(&key) {
            self.hoisted.push(key);
            let body = std::mem::take(&mut self.body);
            let (inner, l) = self.open_loop(roots, list, "it")?;
            let cv = self.cond(&inner, vc)?;
            self.bit(&cv);
            self.close_loop(l);
            let code = std::mem::replace(&mut self.body, body);
            self.prologue.push_str(&code);
        }
        Ok(true)
    }

    /// A viewer-dependent condition's value, shifted into the variant key (a shared register).
    fn bit(&mut self, cv: &str) {
        if !self.bits || self.per_viewer {
            return;
        }
        self.max_bits += self.mult;
        let (k, z, s, o) = (self.v(), self.v(), self.v(), self.v());
        self.line(&format!("{k} = load i64, ptr %key"));
        self.line(&format!("{z} = zext i1 {cv} to i64"));
        self.line(&format!("{s} = shl i64 {k}, 1"));
        self.line(&format!("{o} = or i64 {s}, {z}"));
        self.line(&format!("store i64 {o}, ptr %key"));
    }

    fn mentions(e: &Expr, names: &[String]) -> bool {
        match e {
            Expr::Name(n, _) => names.contains(n),
            Expr::Field(o, _, _) | Expr::Not(o, _) => Self::mentions(o, names),
            Expr::Bin(_, l, r, _) => Self::mentions(l, names) || Self::mentions(r, names),
            Expr::Call(_, a, _) => a.iter().any(|x| Self::mentions(x, names)),
            Expr::If(c, a, b, _) => Self::mentions(c, names) || Self::mentions(a, names) || Self::mentions(b, names),
            _ => false,
        }
    }

    fn nodes(&mut self, roots: &HashMap<String, Root>, ns: &[TNode], out: &str) -> Result<(), String> {
        for n in ns {
            match n {
                TNode::Text(t) => {
                    if !self.bits {
                        self.pending.extend_from_slice(t.as_bytes());
                    }
                }
                TNode::Hole(e) => {
                    if let Expr::Call(f, args, _) = e {
                        if let (true, Some(kind)) = (self.per_viewer, self.md.order.iter().position(|r| r == f)) {
                            if !matches!(self.m.fragments.get(f), Some(Decl::Fragment { per_viewer: true, .. })) {
                                // a cached piece, merged where it goes: its kind and key
                                self.flush(out);
                                let key = match args.first() {
                                    Some(a) => self.value(roots, a)?.0,
                                    None => "0".into(),
                                };
                                self.line(&format!("call void @etch_piece(ptr %splits, ptr {out}, i64 {kind}, i64 {key})"));
                                continue;
                            }
                        }
                        if let Some(Decl::Fragment { params, body, .. }) = self.m.fragments.get(f) {
                            // a fragment called with constants: its template, inlined
                            let env: HashMap<String, Expr> = params.iter().map(|p| p.name.clone()).zip(args.iter().cloned()).collect();
                            let inner = inline::nodes(self.m, &body.nodes, &env);
                            self.nodes(roots, &inner, out)?;
                            continue;
                        }
                    }
                    if self.per_viewer && Self::mentions(e, &self.splice_params) {
                        // a splice: its place is marked; it's its own register, keyed by the parameter
                        self.flush(out);
                        if !self.bits {
                            // the splice's site: which splice register goes here
                            let key = show(e);
                            let site = match self.splices.iter().position(|x| show(x) == key) {
                                Some(i) => i,
                                None => {
                                    self.splices.push(e.clone());
                                    self.splices.len() - 1
                                }
                            };
                            self.line(&format!("call void @etch_split(ptr %splits, ptr {out}, i64 {site})"));
                        }
                        continue;
                    }
                    let t = self.m.type_of(e, &self.scope).map_err(|x| format!("{}: {}", show(e), x.msg))?;
                    self.text(roots, e, out, t.strip_opt() != &Type::Html)?;
                }
                TNode::If(c, a, b) => {
                    self.flush(out);
                    let cv = self.cond(roots, c)?;
                    if Self::on_viewer(c) {
                        self.bit(&cv);
                    }
                    let (lt, lf, le) = (self.label("then"), self.label("else"), self.label("end"));
                    self.line(&format!("br i1 {cv}, label %{lt}, label %{lf}"));
                    self.block(&lt);
                    self.nodes(roots, a, out)?;
                    self.flush(out);
                    self.line(&format!("br label %{le}"));
                    self.block(&lf);
                    self.nodes(roots, b, out)?;
                    self.flush(out);
                    self.line(&format!("br label %{le}"));
                    self.block(&le);
                }
                TNode::For(v, list, _, body) => {
                    if let Expr::Name(n, _) = list {
                        if let Some(Decl::Const { value: Expr::List(items, _), .. }) = self.m.consts.get(n) {
                            // a constant list: unrolled at compile time
                            let t = self.m.type_of(&items[0], &Scope::default()).map_err(|x| x.msg)?;
                            let saved = self.scope.clone();
                            self.scope = self.scope.with(v, t);
                            for it in items {
                                let mut inner = roots.clone();
                                inner.insert(v.clone(), Root::Const(it.clone()));
                                self.nodes(&inner, body, out)?;
                            }
                            self.scope = saved;
                            continue;
                        }
                    }
                    self.flush(out);
                    let (inner, l) = self.open_loop(roots, list, v)?;
                    self.nodes(&inner, body, out)?;
                    self.flush(out);
                    self.close_loop(l);
                }
            }
        }
        Ok(())
    }
}

/// The struct types of a register's records, fields then lists.
fn types(frag: &str, s: &Schema, at: &mut Vec<usize>, out: &mut String) {
    let mut parts: Vec<&str> = s.fields.iter().map(|(_, sh)| if *sh == Shape::Str { "%Str" } else { "i64" }).collect();
    parts.extend(s.lists.iter().map(|_| "%List"));
    let _ = writeln!(out, "{} = type {{ {} }}", type_name(frag, at), parts.join(", "));
    for (k, l) in s.lists.iter().enumerate() {
        at.push(k);
        types(frag, &l.items, at, out);
        at.pop();
    }
}

/// One register's functions: its variant key and its builder (its loader comes after, from the
/// settled record). Its splices are registers too.
fn register(m: &Model, md: &mut Module, name: &str, params: &[Param], nodes: &[TNode], per_viewer: bool) -> Result<(), String> {
    // a cached register reads its row (its parameter) or nothing; one rendered per request reads
    // its row and the viewer
    let roots: HashMap<String, Root> = params.iter().map(|p| (p.name.clone(), Root::Rec { ptr: "%in".into(), at: vec![] })).collect();
    let mut schema = Schema::default();
    let (mut body, mut tmps) = (String::new(), 0usize);
    // the first pass settles the record's layout
    for pass in 0..2 {
        let mut g = Gen {
            m,
            md: &mut *md,
            frag: name.to_string(),
            body: String::new(),
            n: 0,
            pending: vec![],
            schema: schema.clone(),
            scope: Scope::of(params),
            tmps: 0,
            depth: 0,
            bits: false,
            max_bits: 0,
            mult: 1,
            per_viewer,
            splice_params: vec![],
            splices: vec![],
            hoisted: vec![],
            prologue: String::new(),
            viewer_uses: vec![],
        };
        g.nodes(&roots, nodes, "%out")?;
        g.flush("%out");
        if pass == 0 {
            schema = g.schema;
        } else {
            body = g.body;
            tmps = g.tmps;
        }
    }
    let mut f = String::new();
    types(name, &schema, &mut vec![], &mut f);
    let _ = writeln!(f, "; {name}: its bytes ({tmps} temporary buffers){}", if per_viewer { ", per request" } else { ", cached" });
    let _ = writeln!(f, "define void @etch_build_{name}(ptr noalias %ctx, ptr noalias %in, ptr noalias %viewer, ptr noalias %out, ptr noalias %tmps, ptr noalias %splits) {{\nentry:\n{body}  ret void\n}}\n");
    md.funcs.push_str(&f);
    let root = match params.first().map(|p| (p.name.clone(), p.ty.strip_opt().clone())) {
        Some((r, Type::Ref(e))) if params.len() == 1 => Some((r, e)),
        // no parameters: one instance for the app (key 0)
        None => None,
        _ => return Err(format!("fragment {name}: a fragment the page keeps or renders takes one row parameter, or none")),
    };
    md.registers.push(RegInfo { name: name.to_string(), schema, tmps, bits: 0, per_viewer, root, splices: vec![], reads: vec![] });
    Ok(())
}

/// The loader's code for a record's lists: each list's query, bound to the key or to fields of
/// the record it hangs off; items fill an array of the list's bound; nested lists per item.
fn emit_lists(m: &Model, md: &mut Module, b: &mut String, n: &mut usize, qid: &mut usize, reg: &RegInfo, rec: &str, at: &[usize]) -> Result<(), String> {
    let schema = reg.schema.at(at).clone();
    let nf = schema.fields.len();
    for (k, slot) in schema.lists.iter().enumerate() {
        let mut v = || {
            *n += 1;
            format!("%l{}", *n)
        };
        let paths: Vec<String> = slot.items.fields.iter().map(|(p, _)| p.clone()).collect();
        let scope = Scope::default();
        let sel = crate::sql::list_select(m, &slot.expr, &paths, &scope)?;
        let bound = m.bound_of(&slot.expr, &scope).ok_or_else(|| format!("an unbounded list: {}", slot.key))?;
        let mut item_at = at.to_vec();
        item_at.push(k);
        let ity = type_name(&reg.name, &item_at);
        let pty = type_name(&reg.name, at);
        let g = md.str_const(sel.sql.as_bytes());
        *qid += 1;
        let (st, size, sz, arr) = (v(), v(), v(), v());
        let _ = writeln!(b, "  ; {}", sel.sql);
        let _ = writeln!(b, "  {st} = call ptr @etch_stmt(ptr %db, i64 {qid}, ptr {g}, i64 {})", sel.sql.len());
        for (bi, bind) in sel.binds.iter().enumerate() {
            let crate::sql::Bind::Path(e) = bind;
            let (root, path) = path_of(e).ok_or("a query's parameter isn't a path")?;
            if path.is_empty() && reg.root.as_ref().map(|(r, _)| *r == root).unwrap_or(false) {
                let _ = writeln!(b, "  call i32 @sqlite3_bind_int64(ptr {st}, i32 {}, i64 %key)", bi + 1);
                continue;
            }
            // the viewer (a per-request register's lists: the sidebar's rooms)
            if root == "viewer" && path.is_empty() {
                let _ = writeln!(b, "  call i32 @sqlite3_bind_int64(ptr {st}, i32 {}, i64 %viewer)", bi + 1);
                continue;
            }
            // a field of the record the list hangs off
            let key = std::iter::once(root).chain(path).collect::<Vec<_>>().join(".");
            let fi = schema.fields.iter().position(|(p, _)| *p == key).ok_or_else(|| format!("{}: the query's parameter {key} isn't in the record", reg.name))?;
            let (p, x) = (v(), v());
            let _ = writeln!(b, "  {p} = getelementptr inbounds {pty}, ptr {rec}, i32 0, i32 {fi}");
            match schema.fields[fi].1 {
                Shape::I64 => {
                    let _ = writeln!(b, "  {x} = load i64, ptr {p}\n  call i32 @sqlite3_bind_int64(ptr {st}, i32 {}, i64 {x})", bi + 1);
                }
                Shape::Str => {
                    let _ = writeln!(b, "  call void @etch_bind_str(ptr {st}, i32 {}, ptr {p})", bi + 1);
                }
            }
        }
        // the array, sized by the list's bound: allocated once
        let _ = writeln!(b, "  {size} = getelementptr {ity}, ptr null, i64 {bound}\n  {sz} = ptrtoint ptr {size} to i64\n  {arr} = call ptr @etch_alloc(ptr %arena, i64 {sz})");
        let idx = v();
        let (head, step, item, done) = (format!("lh{idx}").replace('%', ""), format!("ls{idx}").replace('%', ""), format!("li{idx}").replace('%', ""), format!("ld{idx}").replace('%', ""));
        let (rc, ok, i, full, it, nx, cnt, l1, l2, lp) = (v(), v(), v(), v(), v(), v(), v(), v(), v(), v());
        let _ = writeln!(b, "  {idx} = alloca i64\n  store i64 0, ptr {idx}\n  br label %{head}");
        let _ = writeln!(b, "{head}:\n  {i} = load i64, ptr {idx}\n  {full} = icmp sge i64 {i}, {bound}\n  br i1 {full}, label %{done}, label %{step}");
        let _ = writeln!(b, "{step}:\n  {rc} = call i32 @sqlite3_step(ptr {st})\n  {ok} = icmp eq i32 {rc}, 100\n  br i1 {ok}, label %{item}, label %{done}");
        let _ = writeln!(b, "{item}:\n  {it} = getelementptr inbounds {ity}, ptr {arr}, i64 {i}");
        for (fi, (_, sh)) in slot.items.fields.iter().enumerate() {
            let (p, x) = (v(), v());
            let _ = writeln!(b, "  {p} = getelementptr inbounds {ity}, ptr {it}, i32 0, i32 {fi}");
            match sh {
                Shape::I64 => {
                    let _ = writeln!(b, "  {x} = call i64 @etch_col_i64(ptr {st}, i32 {fi})\n  store i64 {x}, ptr {p}");
                }
                Shape::Str => {
                    let _ = writeln!(b, "  call void @etch_col_str(ptr {st}, i32 {fi}, ptr %arena, ptr {p})");
                }
            }
        }
        let _ = writeln!(b, "  {nx} = add i64 {i}, 1\n  store i64 {nx}, ptr {idx}\n  br label %{head}");
        let _ = writeln!(b, "{done}:\n  call i32 @sqlite3_reset(ptr {st})\n  {cnt} = load i64, ptr {idx}");
        let _ = writeln!(b, "  {l1} = insertvalue %List undef, ptr {arr}, 0\n  {l2} = insertvalue %List {l1}, i64 {cnt}, 1");
        let _ = writeln!(b, "  {lp} = getelementptr inbounds {pty}, ptr {rec}, i32 0, i32 {}\n  store %List {l2}, ptr {lp}", nf + k);
        // nested lists: per item, after the statement is reset (each has its own statement)
        if !slot.items.lists.is_empty() {
            let (j, jj, more, ip) = (v(), v(), v(), v());
            let (nh, nb, nd) = (format!("nh{}", *n), format!("nb{}", *n), format!("nd{}", *n));
            let _ = writeln!(b, "  {j} = alloca i64\n  store i64 0, ptr {j}\n  br label %{nh}");
            let _ = writeln!(b, "{nh}:\n  {jj} = load i64, ptr {j}\n  {more} = icmp slt i64 {jj}, {cnt}\n  br i1 {more}, label %{nb}, label %{nd}");
            let _ = writeln!(b, "{nb}:\n  {ip} = getelementptr inbounds {ity}, ptr {arr}, i64 {jj}");
            emit_lists(m, md, b, n, qid, reg, &ip, &item_at)?;
            let nx2 = format!("%l{}", {
                *n += 1;
                *n
            });
            let _ = writeln!(b, "  {nx2} = add i64 {jj}, 1\n  store i64 {nx2}, ptr {j}\n  br label %{nh}");
            let _ = writeln!(b, "{nd}:");
        }
    }
    Ok(())
}

/// `etch_load_<f>(ctx, db, key, rec, arena) -> i1`: fills the register's record from SQLite. False
/// when its row is gone.
fn loader(m: &Model, md: &mut Module, reg: &RegInfo, qid: &mut usize) -> Result<String, String> {
    let mut b = String::new();
    let mut n = 0usize;
    let ty = type_name(&reg.name, &[]);
    let _ = writeln!(b, "define i1 @etch_load_{}(ptr noalias %ctx, ptr noalias %db, i64 %key, i64 %viewer, ptr noalias %rec, ptr noalias %arena) {{\nentry:", reg.name);
    let schema = &reg.schema;
    let root_name = reg.root.as_ref().map(|(r, _)| r.clone()).unwrap_or_default();
    let only_key = schema.scalars.is_empty() && schema.fields.iter().all(|(p, _)| *p == root_name);
    if only_key {
        for (i, _) in schema.fields.iter().enumerate() {
            let _ = writeln!(b, "  %k{i} = getelementptr inbounds {ty}, ptr %rec, i32 0, i32 {i}\n  store i64 %key, ptr %k{i}");
        }
    } else {
        let paths: Vec<String> = schema.fields.iter().map(|(p, _)| p.clone()).collect();
        let ent = reg.root.as_ref().map(|(_, e)| e.as_str());
        let sel = crate::sql::row_select(m, ent, &root_name, &paths, &schema.scalars)?;
        let g = md.str_const(sel.sql.as_bytes());
        *qid += 1;
        let _ = writeln!(b, "  ; {}", sel.sql);
        let _ = writeln!(b, "  %st = call ptr @etch_stmt(ptr %db, i64 {qid}, ptr {g}, i64 {})", sel.sql.len());
        // the scalars' parameters first (the row itself, or the viewer), then the row's id
        for (bi, bind) in sel.binds.iter().enumerate() {
            let crate::sql::Bind::Path(e) = bind;
            let (root, path) = path_of(e).ok_or("a scalar's parameter isn't a path")?;
            let x = match (root.as_str(), path.is_empty()) {
                (r, true) if r == root_name => "%key",
                ("viewer", true) => "%viewer",
                _ => return Err(format!("{}: a scalar's parameter {} isn't the row or the viewer", reg.name, show(e))),
            };
            let _ = writeln!(b, "  call i32 @sqlite3_bind_int64(ptr %st, i32 {}, i64 {x})", bi + 1);
        }
        if ent.is_some() {
            let _ = writeln!(b, "  call i32 @sqlite3_bind_int64(ptr %st, i32 {}, i64 %key)", sel.binds.len() + 1);
        }
        let _ = writeln!(b, "  %rc = call i32 @sqlite3_step(ptr %st)\n  %ok = icmp eq i32 %rc, 100\n  br i1 %ok, label %row, label %gone");
        let _ = writeln!(b, "gone:\n  call i32 @sqlite3_reset(ptr %st)\n  ret i1 false\nrow:");
        for (i, (_, sh)) in schema.fields.iter().enumerate() {
            let _ = writeln!(b, "  %f{i} = getelementptr inbounds {ty}, ptr %rec, i32 0, i32 {i}");
            match sh {
                Shape::I64 => {
                    let _ = writeln!(b, "  %x{i} = call i64 @etch_col_i64(ptr %st, i32 {i})\n  store i64 %x{i}, ptr %f{i}");
                }
                Shape::Str => {
                    let _ = writeln!(b, "  call void @etch_col_str(ptr %st, i32 {i}, ptr %arena, ptr %f{i})");
                }
            }
        }
        let _ = writeln!(b, "  call i32 @sqlite3_reset(ptr %st)");
    }
    emit_lists(m, md, &mut b, &mut n, qid, reg, "%rec", &[])?;
    let _ = writeln!(b, "  ret i1 true\n}}\n");
    Ok(b)
}

/// The viewer's record: `etch_load_viewer(ctx, db, key, viewer, rec, arena) -> i1` (the viewer is the key).
fn viewer_loader(m: &Model, md: &mut Module, qid: &mut usize) -> Result<String, String> {
    let ent = m.viewer_entity().ok_or("no viewer declaration")?.to_string();
    let reg = RegInfo { name: "viewer".into(), schema: md.viewer.clone(), tmps: 0, bits: 0, per_viewer: false, root: Some(("viewer".into(), ent)), splices: vec![], reads: vec![] };
    let s = loader(m, md, &reg, qid)?;
    Ok(s.replace("%In_viewer", "%Viewer"))
}

const DECLS: &str = r#"%Str = type { ptr, i64 }
%List = type { ptr, i64 }
declare {i64, i1} @llvm.sadd.with.overflow.i64(i64, i64)
declare {i64, i1} @llvm.ssub.with.overflow.i64(i64, i64)
declare {i64, i1} @llvm.smul.with.overflow.i64(i64, i64)
declare void @etch_put(ptr, ptr, i64)
declare void @etch_esc(ptr, ptr, i64)
declare void @etch_clear(ptr)
declare void @etch_int(ptr, i64)
declare void @etch_json(ptr, ptr, i64)
declare void @etch_sign(ptr, ptr, ptr, i64)
declare void @etch_autolink(ptr, ptr)
declare void @etch_asset(ptr, ptr, ptr, i64)
declare void @etch_first_word(ptr, ptr, i64)
declare void @etch_initials(ptr, ptr, i64)
declare void @etch_lower(ptr, ptr, i64)
declare void @etch_sentence(ptr, ptr, i64)
declare void @etch_piece(ptr, ptr, i64, i64)
declare void @etch_replace(ptr, ptr, ptr, i64, ptr, i64, i64)
declare void @etch_time(ptr, i64, i64, i64)
declare void @etch_config(ptr, ptr, i64, i64)
declare i1 @etch_str_eq(ptr, i64, ptr, i64)
declare ptr @etch_stmt(ptr, i64, ptr, i64)
declare i64 @etch_col_i64(ptr, i32)
declare i64 @etch_col_int(ptr, i32)
declare void @etch_col_str(ptr, i32, ptr, ptr)
declare void @etch_bind_str(ptr, i32, ptr)
declare void @etch_bind_i64(ptr, i32, i64)
declare ptr @etch_alloc(ptr, i64)
declare i32 @sqlite3_bind_int64(ptr, i32, i64)
declare i32 @sqlite3_step(ptr)
declare i32 @sqlite3_reset(ptr)
declare void @etch_exec(ptr)
declare void @etch_done(ptr, i32)
declare void @etch_delta(ptr, i32, i64, i64, i64)
declare i64 @etch_w_now(ptr)
declare i64 @etch_w_seq(ptr)
declare i64 @etch_w_seq_now(ptr)
declare i64 @etch_w_id(ptr, ptr, ptr, i64)
declare void @etch_w_token(ptr, i64, ptr)
declare ptr @etch_w_alloc(ptr, i64)
declare void @etch_w_copy(ptr, ptr, ptr)
declare void @etch_delta_patch(ptr, i64, i64, i64, i64, i64, i64)
declare void @etch_delta_find(ptr, i64, i64, i64, i64)
declare i64 @etch_str_chars(ptr)
declare void @etch_w_sign_in(ptr, ptr)
declare void @etch_w_sign_out(ptr)
declare i64 @etch_crc_mod(ptr, i64)
declare void @etch_upper(ptr, ptr, i64)
declare %List @etch_w_ids(ptr, ptr)
declare void @etch_w_go(ptr, ptr, i64, i64, ptr, i64)
"#;

/// The module for these registers, and a Rust header with the same records.
pub fn module(m: &Model, registers: &[&str], datalayout: &str, triple: &str) -> Result<(String, String), String> {
    let mut md = Module { globals: String::new(), strs: HashMap::new(), viewer: Schema::default(), funcs: String::new(), registers: vec![], viewer_reads: vec![], keys: vec![], order: vec![] };
    md.viewer.field("viewer", Shape::I64);
    // the registers: those named, or (by default) what the screens show
    let auto = crate::glue::registers_of(m);
    let registers: Vec<&str> = if registers.is_empty() { auto.iter().map(String::as_str).collect() } else { registers.to_vec() };
    md.order = registers.iter().map(|r| r.to_string()).collect();
    for r in &registers {
        let (params, body, per_viewer) = match m.fragments.get(&r.to_string()) {
            Some(Decl::Fragment { params, body, per_viewer, .. }) => (params, body, (*per_viewer, ())),
            _ => return Err(format!("no fragment {r}")),
        };
        let nodes = inline::nodes(m, &body.nodes, &HashMap::new());
        register(m, &mut md, r, params, &nodes, per_viewer.0)?;
    }
    let mut qid = 0usize;
    let regs = md.registers.clone();
    let mut reads_of = vec![];
    for reg in &regs {
        let (f, reads) = crate::sql::reading(|| loader(m, &mut md, reg, &mut qid));
        let f = f?;
        reads_of.push(reads);
        md.funcs.push_str(&f);
    }
    for (r, reads) in md.registers.iter_mut().zip(reads_of) {
        r.reads = reads;
        if std::env::var("ETCH_SHOW_READS").is_ok() {
            eprintln!("{} reads {}", r.name, r.reads.iter().map(|(e, c)| format!("{e}.{c}")).collect::<Vec<_>>().join(" "));
        }
    }
    // what the viewer's record reads: every column of its entity (forwarding adds fields to it
    // while the actions are lowered), and what it reads through references (from a trial build)
    let (_, mut viewer_reads) = crate::sql::reading(|| viewer_loader(m, &mut md, &mut qid.clone()));
    if let Some(e) = m.viewer_entity() {
        viewer_reads.retain(|(x, _)| x != e);
        for f in &m.entities[e].fields {
            viewer_reads.push((e.to_string(), f.name.clone()));
        }
    }
    md.viewer_reads = viewer_reads;
    let screens = crate::actions::screen_lists(m)?;
    let (acts, patches, finds) = crate::actions::lower(m, &mut md, &mut qid, &screens)?;
    // the viewer's loader last: forwarding may have added fields to the viewer's record
    let vl = viewer_loader(m, &mut md, &mut qid)?;
    md.funcs.push_str(&vl);
    // each piece: cached (keyed by its row, or one for the app) or rendered per request
    eprintln!("pieces:");
    for r in &md.registers {
        let row = match &r.root {
            Some((p, e)) => format!("{p} ({e})"),
            None => "nothing (one for the app)".to_string(),
        };
        eprintln!("  {}: {}", r.name, if r.per_viewer { format!("rendered per request (for the viewer, at {row})") } else { format!("cached, keyed by {row}") });
    }
    for a in &acts {
        eprintln!("{}: delta edges", a.name);
        for e in &a.edges {
            eprintln!("  {e}");
        }
    }
    let mut ll = String::new();
    let _ = writeln!(ll, "; generated by etchc from the app's spec: do not edit\n{datalayout}\n{triple}\n");
    ll.push_str(DECLS);
    let mut vt = String::new();
    types("", &md.viewer, &mut vec![], &mut vt);
    ll.push_str(&vt.replace("%In_", "%Viewer"));
    ll.push('\n');
    ll.push_str(&md.globals);
    ll.push('\n');
    ll.push_str(&md.funcs);
    let mut rs = rust_header(&md);
    // the actions: their arguments, outcomes; the registers' and screen lists' indices (delta edges name them)
    let _ = writeln!(rs, "pub const REGISTERS: &[&str] = &[{}];", md.registers.iter().map(|r| format!("{:?}", r.name)).collect::<Vec<_>>().join(", "));
    let _ = writeln!(rs, "pub const LISTS: &[&str] = &[{}];", screens.iter().map(|l| format!("{:?}", l.name)).collect::<Vec<_>>().join(", "));
    for a in &acts {
        let _ = writeln!(rs, "#[repr(C)]\n#[derive(Clone, Copy)]\npub struct Args_{} {{", a.name);
        for (n, s) in &a.args {
            let _ = writeln!(rs, "    pub {n}: {},", if *s == Shape::Str { "Str" } else { "i64" });
        }
        let _ = writeln!(rs, "}}");
        let _ = writeln!(rs, "pub const OUTCOMES_{}: &[&str] = &[{}];", a.name.to_uppercase(), a.outcomes.iter().map(|o| format!("{o:?}")).collect::<Vec<_>>().join(", "));
        let _ = writeln!(rs, "extern \"C\" {{\n    pub fn etch_act_{}(ctx: *const etch_rt::abi::Ctx, db: *mut etch_rt::abi::Db, w: *mut etch_rt::abi::Writer, viewer: i64, vrec: *const Viewer, args: *const Args_{}, d: *mut etch_rt::abi::Deltas) -> i32;\n}}\n", a.name, a.name);
    }
    // patches of kept records: (register, list, field) to the generated function
    let _ = writeln!(rs, "extern \"C\" {{");
    for (r, l, f) in &patches {
        let _ = writeln!(rs, "    fn etch_patch_{}_{l}_{f}(rec: *mut u8, item: i64, value: i64) -> bool;", md.registers[*r].name);
    }
    for (r, l, f) in &finds {
        let _ = writeln!(rs, "    fn etch_find_{}_{l}_{f}(rec: *mut u8, item: i64, value: i64) -> bool;", md.registers[*r].name);
    }
    let _ = writeln!(rs, "}}\n/// Patches the item with this id in a kept record: false if it isn't there.\npub unsafe fn patch(reg: i64, list: i64, field: i64, rec: *mut u8, item: i64, value: i64) -> bool {{\n    match (reg, list, field) {{");
    for (r, l, f) in &patches {
        let _ = writeln!(rs, "        ({r}, {l}, {f}) => etch_patch_{}_{l}_{f}(rec, item, value),", md.registers[*r].name);
    }
    let _ = writeln!(rs, "        _ => false,\n    }}\n}}");
    let _ = writeln!(rs, "/// Whether a kept record's list holds an item referencing this row.\npub unsafe fn find(reg: i64, list: i64, field: i64, rec: *mut u8, row: i64) -> bool {{\n    match (reg, list, field) {{");
    for (r, l, f) in &finds {
        let _ = writeln!(rs, "        ({r}, {l}, {f}) => etch_find_{}_{l}_{f}(rec, row, 0),", md.registers[*r].name);
    }
    let _ = writeln!(rs, "        _ => true,\n    }}\n}}");
    rs.push_str(&crate::glue::glue(m, &md, &screens, &acts)?);
    Ok((ll, rs))
}

fn rust_name(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect::<String>().trim_matches('_').replace("__", "_")
}

/// `#[repr(C)]` structs matching the records, and the generated functions' declarations.
fn rust_header(md: &Module) -> String {
    let mut s = String::from("// generated by etchc from the app's spec: do not edit\nuse etch_rt::abi::{Str, List};\n\n");
    fn rec(s: &mut String, name: &str, sch: &Schema, at: &mut Vec<usize>) {
        let ty = format!("In_{name}{}", at.iter().map(|k| format!("_{k}")).collect::<String>());
        let _ = writeln!(s, "#[repr(C)]\n#[derive(Clone, Copy)]\npub struct {} {{", if name.is_empty() { "Viewer".to_string() } else { ty.clone() });
        for (i, (p, sh)) in sch.fields.iter().enumerate() {
            let _ = writeln!(s, "    /// {p}\n    pub f{i}_{}: {},", rust_name(p), if *sh == Shape::Str { "Str" } else { "i64" });
        }
        for (k, l) in sch.lists.iter().enumerate() {
            let _ = writeln!(s, "    /// {}\n    pub list{k}: List,", l.key);
        }
        let _ = writeln!(s, "}}\n");
        for (k, l) in sch.lists.iter().enumerate() {
            at.push(k);
            rec(s, name, &l.items, at);
            at.pop();
        }
    }
    rec(&mut s, "", &md.viewer, &mut vec![]);
    let _ = writeln!(s, "extern \"C\" {{");
    let _ = writeln!(s, "    pub fn etch_load_viewer(ctx: *const etch_rt::abi::Ctx, db: *mut etch_rt::abi::Db, key: i64, viewer: i64, rec: *mut Viewer, arena: *mut etch_rt::abi::Arena) -> bool;");
    for r in &md.registers {
        let name = &r.name;
        let _ = writeln!(s, "    pub fn etch_load_{name}(ctx: *const etch_rt::abi::Ctx, db: *mut etch_rt::abi::Db, key: i64, viewer: i64, rec: *mut In_{name}, arena: *mut etch_rt::abi::Arena) -> bool;");
        let _ = writeln!(s, "    pub fn etch_build_{name}(ctx: *const etch_rt::abi::Ctx, input: *const In_{name}, viewer: *const Viewer, out: *mut Vec<u8>, tmps: *const *mut Vec<u8>, splits: *mut Vec<usize>);");
    }
    let _ = writeln!(s, "}}\n");
    for r in &md.registers {
        let _ = writeln!(s, "/// {}: {} temporary buffers, {}", r.name, r.tmps, if r.per_viewer { "rendered per request" } else { "cached" });
        let _ = writeln!(s, "pub const TMPS_{}: usize = {};", r.name.to_uppercase(), r.tmps);
        rec(&mut s, &r.name, &r.schema, &mut vec![]);
    }
    s
}
