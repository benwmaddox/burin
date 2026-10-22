//! Lowering registers to LLVM IR (docs/IR.md, "Lowering to LLVM").
//!
//! For each register (a fragment kept in the snapshot) the compiler emits:
//! - its **input record**: a flat C struct with exactly the values its template uses (paths such
//!   as `m.creator.name`), then a (pointer, count) per list it loops over; the runtime fills it;
//! - `etch_bits_<f>(in, viewer) -> i64`: its variant key: each viewer-dependent condition's bit,
//!   shifted in, in template order (in loops, one per item). The compiler proves the key fits 64
//!   bits from the lists' bounds, and refuses a fragment whose key wouldn't;
//! - `etch_build_<f>(ctx, in, viewer, out, tmps)`: its bytes for that viewer: appends of constant
//!   ranges (escaped and merged at compile time), escaped holes, a branch per condition, a loop per
//!   list of rows; constant lists are unrolled. Temporary buffers are a fixed set, counted here.
//!
//! Each function is generated twice: the first pass only settles the record's layout.
//! The runtime's helpers (etch-rt's `abi`) are linked under cross-language LTO and inline.

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
        Type::Text | Type::Html | Type::Json => Shape::Str,
        _ => Shape::I64,
    }
}

/// The record a register (or a list's items) is built from.
#[derive(Default, Debug, Clone)]
pub struct Schema {
    /// (path, shape): `m.creator.name`; a reference's id is the path to the reference
    pub fields: Vec<(String, Shape)>,
    /// (the list, its loop variable, its items' record)
    pub lists: Vec<(String, String, Schema)>,
}

impl Schema {
    fn field(&mut self, path: &str, s: Shape) -> usize {
        if let Some(i) = self.fields.iter().position(|(p, _)| p == path) {
            return i;
        }
        self.fields.push((path.to_string(), s));
        self.fields.len() - 1
    }
    fn list(&mut self, key: &str, var: &str) -> usize {
        if let Some(i) = self.lists.iter().position(|(k, _, _)| k == key) {
            return i;
        }
        self.lists.push((key.to_string(), var.to_string(), Schema::default()));
        self.lists.len() - 1
    }
    fn at(&self, path: &[usize]) -> &Schema {
        let mut s = self;
        for &k in path {
            s = &s.lists[k].2;
        }
        s
    }
    fn at_mut(&mut self, path: &[usize]) -> &mut Schema {
        let mut s = self;
        for &k in path {
            s = &mut s.lists[k].2;
        }
        s
    }
}

fn type_name(frag: &str, path: &[usize]) -> String {
    format!("%In_{frag}{}", path.iter().map(|k| format!("_{k}")).collect::<String>())
}

/// A path from a root (a parameter, a loop variable, `viewer`) through fields.
fn path_of(e: &Expr) -> Option<(String, Vec<String>)> {
    match e {
        Expr::Name(n, _) => Some((n.clone(), vec![])),
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

fn show(e: &Expr) -> String {
    match e {
        Expr::Name(n, _) => n.clone(),
        Expr::Field(o, f, _) => format!("{}.{f}", show(o)),
        Expr::Call(f, a, _) => format!("{f}({})", a.iter().map(show).collect::<Vec<_>>().join(", ")),
        Expr::Str(s, _) => format!("{s:?}"),
        Expr::Int(n, _) => n.to_string(),
        _ => "…".into(),
    }
}

/// Where a root's values live while building.
#[derive(Clone)]
enum Root {
    /// the record at this pointer (an SSA value), at this place in the register's schema
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

/// Module-wide state: constant strings, and the viewer's record (shared by every register).
pub struct Module {
    pub globals: String,
    strs: HashMap<Vec<u8>, String>,
    pub viewer: Schema,
    pub funcs: String,
    /// per register: (name, schema, temporaries, most variant bits)
    pub registers: Vec<(String, Schema, usize, usize)>,
}

impl Module {
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
    /// the types of the roots in scope (for type_of)
    scope: Scope,
    tmps: usize,
    depth: usize,
    /// generating the bits function: conditions only, no bytes
    bits: bool,
    /// most bits the key can have (from the lists' bounds)
    max_bits: usize,
    /// the product of the bounds of the loops we are in
    mult: usize,
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
        let ty = self.m.type_of(e, &self.scope).map_err(|x| x.msg)?;
        let shape = shape_of(&ty);
        let key = std::iter::once(root.clone()).chain(path.iter().cloned()).collect::<Vec<_>>().join(".");
        if root == "viewer" {
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
            Expr::Bin(BinOp::Add, l, r, _) => Some(self.constant(roots, l)? + &self.constant(roots, r)?),
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

    fn value(&mut self, roots: &HashMap<String, Root>, e: &Expr) -> Result<(String, Shape), String> {
        if let Expr::Int(n, _) = e {
            return Ok((n.to_string(), Shape::I64));
        }
        self.load(roots, e)
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
            self.text(roots, l, out, esc)?;
            return self.text(roots, r, out, esc);
        }
        self.flush(out);
        if self.bits {
            // bits come only from conditions; text is skipped, but its paths still settle the
            // record's layout in the first pass
            return Ok(());
        }
        let e1 = esc as i64;
        // a helper that takes its argument as text in a temporary buffer
        let mut via_tmp = |g: &mut Gen, arg: &Expr, call: &dyn Fn(&str) -> String| -> Result<(), String> {
            let t = g.tmp();
            g.text(roots, arg, &t, false)?;
            g.flush(&t);
            let c = call(&t);
            g.line(&c);
            g.untmp();
            Ok(())
        };
        match e {
            Expr::Call(f, args, _) => match f.as_str() {
                "text" => {
                    let (v, _) = self.value(roots, &args[0])?;
                    self.line(&format!("call void @etch_int(ptr {out}, i64 {v})"));
                }
                "json" | "quote" => via_tmp(self, &args[0], &|t| format!("call void @etch_json(ptr {out}, ptr {t}, i64 {e1})"))?,
                "sign" => via_tmp(self, &args[0], &|t| format!("call void @etch_sign(ptr %ctx, ptr {out}, ptr {t}, i64 {e1})"))?,
                "autolink" => via_tmp(self, &args[0], &|t| format!("call void @etch_autolink(ptr {out}, ptr {t})"))?,
                "asset" => via_tmp(self, &args[0], &|t| format!("call void @etch_asset(ptr %ctx, ptr {out}, ptr {t}, i64 {e1})"))?,
                "first_word" | "initials" | "lower" => {
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
                    self.line(&format!("call void @etch_time(ptr {out}, i64 {k}, i64 {v}, i64 {e1})"));
                }
                _ => return Err(format!("`{f}` can't be lowered in a fragment yet")),
            },
            Expr::Field(o, f, _) if matches!(&**o, Expr::Name(n, _) if n == "config") => {
                let k = self.m.app.decls.iter().filter_map(|d| if let Decl::Config { name, .. } = d { Some(name) } else { None }).position(|n| n == f).unwrap();
                self.line(&format!("call void @etch_config(ptr %ctx, ptr {out}, i64 {k}, i64 {e1})"));
            }
            _ => {
                let (v, shape) = self.value(roots, e)?;
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
            _ => Err(format!("this condition can't be lowered yet: {}", show(e))),
        }
    }

    fn on_viewer(e: &Expr) -> bool {
        match e {
            Expr::Name(n, _) => n == "viewer",
            Expr::Field(o, _, _) | Expr::Not(o, _) => Self::on_viewer(o),
            Expr::Bin(_, l, r, _) => Self::on_viewer(l) || Self::on_viewer(r),
            Expr::Call(_, a, _) => a.iter().any(Self::on_viewer),
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
                        if let Some(Decl::Fragment { params, body, .. }) = self.m.fragments.get(f) {
                            // a fragment called with constants: its template, inlined
                            let env: HashMap<String, Expr> = params.iter().map(|p| p.name.clone()).zip(args.iter().cloned()).collect();
                            let inner = inline::nodes(self.m, &body.nodes, &env);
                            self.nodes(roots, &inner, out)?;
                            continue;
                        }
                    }
                    let t = self.m.type_of(e, &self.scope).map_err(|x| x.msg)?;
                    self.text(roots, e, out, t.strip_opt() != &Type::Html)?;
                }
                TNode::If(c, a, b) => {
                    self.flush(out);
                    let cv = self.cond(roots, c)?;
                    if self.bits && Self::on_viewer(c) {
                        self.max_bits += self.mult;
                        let (k, z, s, o) = (self.v(), self.v(), self.v(), self.v());
                        self.line(&format!("{k} = load i64, ptr %key"));
                        self.line(&format!("{z} = zext i1 {cv} to i64"));
                        self.line(&format!("{s} = shl i64 {k}, 1"));
                        self.line(&format!("{o} = or i64 {s}, {z}"));
                        self.line(&format!("store i64 {o}, ptr %key"));
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
                    // a list of rows: its array in the record of the root its arguments name
                    self.flush(out);
                    let (parent, at) = self.list_parent(roots, list);
                    let key = show(list);
                    let li = self.schema.at_mut(&at).list(&key, v);
                    let slot = self.schema.at(&at).fields.len() + li;
                    let mut item_at = at.clone();
                    item_at.push(li);
                    let (lp, l, items, count) = (self.v(), self.v(), self.v(), self.v());
                    self.line(&format!("{lp} = getelementptr inbounds {}, ptr {parent}, i32 0, i32 {slot}", type_name(&self.frag, &at)));
                    self.line(&format!("{l} = load %List, ptr {lp}"));
                    self.line(&format!("{items} = extractvalue %List {l}, 0"));
                    self.line(&format!("{count} = extractvalue %List {l}, 1"));
                    // the index lives in a stack slot (mem2reg makes it a register)
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
                    inner.insert(v.clone(), Root::Rec { ptr: ip, at: item_at });
                    let ty = match self.m.type_of(list, &self.scope).map_err(|x| x.msg)? {
                        Type::List(t) => *t,
                        t => return Err(format!("not a list: {}", t.show())),
                    };
                    let saved = (self.scope.clone(), self.mult);
                    self.scope = self.scope.with(v, ty);
                    self.mult *= self.m.bound_of(list, &saved.0).ok_or("an unbounded list")? as usize;
                    self.nodes(&inner, body, out)?;
                    self.flush(out);
                    (self.scope, self.mult) = saved;
                    let nx = self.v();
                    self.line(&format!("{nx} = add i64 {i}, 1"));
                    self.line(&format!("store i64 {nx}, ptr {idx}"));
                    self.line(&format!("br label %{head}"));
                    self.block(&done);
                }
            }
        }
        Ok(())
    }

    /// The record a list hangs off: the record of the root its arguments name.
    fn list_parent(&self, roots: &HashMap<String, Root>, list: &Expr) -> (String, Vec<usize>) {
        if let Expr::Call(_, args, _) = list {
            for a in args {
                if let Some((r, _)) = path_of(a) {
                    if let Some(Root::Rec { ptr, at }) = roots.get(&r) {
                        return (ptr.clone(), at.clone());
                    }
                }
            }
        }
        ("%in".to_string(), vec![])
    }
}

/// The struct types of a register's records, fields then lists.
fn types(frag: &str, s: &Schema, at: &mut Vec<usize>, out: &mut String) {
    let mut parts: Vec<&str> = s.fields.iter().map(|(_, sh)| if *sh == Shape::Str { "%Str" } else { "i64" }).collect();
    parts.extend(s.lists.iter().map(|_| "%List"));
    let _ = writeln!(out, "{} = type {{ {} }}", type_name(frag, at), parts.join(", "));
    for (k, (_, _, inner)) in s.lists.iter().enumerate() {
        at.push(k);
        types(frag, inner, at, out);
        at.pop();
    }
}

/// One register's two functions.
fn register(m: &Model, md: &mut Module, name: &str) -> Result<(), String> {
    let (params, body) = match m.fragments.get(name) {
        Some(Decl::Fragment { params, body, .. }) => (params, body),
        _ => return Err(format!("no fragment {name}")),
    };
    let nodes = inline::nodes(m, &body.nodes, &HashMap::new());
    let roots: HashMap<String, Root> = params.iter().map(|p| (p.name.clone(), Root::Rec { ptr: "%in".into(), at: vec![] })).collect();
    let mut schema = Schema::default();
    let mut out = (String::new(), String::new(), 0usize, 0usize);
    // pass 1 settles the record's layout (fields and lists in first-use order); pass 2 emits
    for pass in 0..2 {
        for bits in [true, false] {
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
                bits,
                max_bits: 0,
                mult: 1,
            };
            if bits {
                g.line("%key = alloca i64");
                g.line("store i64 0, ptr %key");
            }
            g.nodes(&roots, &nodes, "%out")?;
            g.flush("%out");
            let (b, sch, tmps, maxb) = (g.body, g.schema, g.tmps, g.max_bits);
            if pass == 0 {
                schema = sch;
                continue;
            }
            if bits {
                out.0 = b;
                out.3 = maxb;
            } else {
                out.1 = b;
                out.2 = tmps;
            }
        }
    }
    if out.3 > 64 {
        return Err(format!("fragment {name}: its variant key needs up to {} bits; at most 64 (bound its lists, or mark it `per viewer`)", out.3));
    }
    let mut f = String::new();
    types(name, &schema, &mut vec![], &mut f);
    let _ = writeln!(f, "\n; {name}: its variant key ({} bits at most)", out.3);
    let _ = writeln!(f, "define i64 @etch_bits_{name}(ptr noalias %in, ptr noalias %viewer) {{\nentry:\n{}  %r = load i64, ptr %key\n  ret i64 %r\n}}\n", out.0);
    let _ = writeln!(f, "; {name}: its bytes for a viewer ({} temporary buffers)", out.2);
    let _ = writeln!(f, "define void @etch_build_{name}(ptr noalias %ctx, ptr noalias %in, ptr noalias %viewer, ptr noalias %out, ptr noalias %tmps) {{\nentry:\n{}  ret void\n}}\n", out.1);
    md.funcs.push_str(&f);
    md.registers.push((name.to_string(), schema, out.2, out.3));
    Ok(())
}

const DECLS: &str = r#"%Str = type { ptr, i64 }
%List = type { ptr, i64 }
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
declare void @etch_replace(ptr, ptr, ptr, i64, ptr, i64, i64)
declare void @etch_time(ptr, i64, i64, i64)
declare void @etch_config(ptr, ptr, i64, i64)
declare i1 @etch_str_eq(ptr, i64, ptr, i64)
"#;

/// The module for these registers, and a Rust header with the same records.
pub fn module(m: &Model, registers: &[&str], datalayout: &str, triple: &str) -> Result<(String, String), String> {
    let mut md = Module { globals: String::new(), strs: HashMap::new(), viewer: Schema::default(), funcs: String::new(), registers: vec![] };
    md.viewer.field("viewer", Shape::I64);
    for r in registers {
        register(m, &mut md, r)?;
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
    Ok((ll, rust_header(&md)))
}

fn rust_name(path: &str) -> String {
    path.replace('.', "_")
}

/// `#[repr(C)]` structs matching the records, and the generated functions' declarations.
fn rust_header(md: &Module) -> String {
    let mut s = String::from("// generated by etchc from the app's spec: do not edit\nuse etch_rt::abi::{Str, List};\n\n");
    fn rec(s: &mut String, name: &str, sch: &Schema, at: &mut Vec<usize>) {
        let ty = format!("In_{name}{}", at.iter().map(|k| format!("_{k}")).collect::<String>());
        let _ = writeln!(s, "#[repr(C)]\n#[derive(Clone, Copy)]\npub struct {} {{", if name.is_empty() { "Viewer".to_string() } else { ty.clone() });
        for (p, sh) in &sch.fields {
            let _ = writeln!(s, "    pub {}: {},", rust_name(p), if *sh == Shape::Str { "Str" } else { "i64" });
        }
        for (k, (l, _, _)) in sch.lists.iter().enumerate() {
            let _ = writeln!(s, "    /// {l}\n    pub list{k}: List,");
        }
        let _ = writeln!(s, "}}\n");
        for (k, (_, _, inner)) in sch.lists.iter().enumerate() {
            at.push(k);
            rec(s, name, inner, at);
            at.pop();
        }
    }
    rec(&mut s, "", &md.viewer, &mut vec![]);
    let _ = writeln!(s, "extern \"C\" {{");
    for (name, _, _, _) in &md.registers {
        let _ = writeln!(s, "    pub fn etch_bits_{name}(input: *const In_{name}, viewer: *const Viewer) -> i64;");
        let _ = writeln!(s, "    pub fn etch_build_{name}(ctx: *const etch_rt::abi::Ctx, input: *const In_{name}, viewer: *const Viewer, out: *mut Vec<u8>, tmps: *const *mut Vec<u8>);");
    }
    let _ = writeln!(s, "}}\n");
    for (name, sch, tmps, bits) in &md.registers {
        let _ = writeln!(s, "/// {name}: {tmps} temporary buffers, a variant key of at most {bits} bits");
        let _ = writeln!(s, "pub const TMPS_{}: usize = {tmps};", name.to_uppercase());
        rec(&mut s, name, sch, &mut vec![]);
    }
    s
}
