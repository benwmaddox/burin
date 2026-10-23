//! Helpers: `fn f(…): T { … }`, functions with locals that change, loops and arrays, compiled
//! straight to LLVM IR (DESIGN.md: helpers). Each is a function of the external
//! functions' C ABI (`burin_fn_f`): the parser declares it as an `extern fn`, so it's called,
//! placed and checked as one. A helper is pure (it sees only its arguments), so a call's reads are
//! its arguments' and the rest of burin's read tracking is unchanged.
//!
//! One generator both checks a helper (types, names, the rules) and writes its IR. Locals are
//! stack slots (LLVM makes them registers); arrays come zeroed: small ones of a size written out
//! on the stack, the rest, of any size, from a bump arena per
//! thread (time over memory: released to the helper's mark on its way out, the memory kept for the
//! next call). The loops' step budget is the thread's (the writer's is tight). Whatever goes wrong
//! (an overflow, a divisor of 0, an index outside its array, a shift outside 0 to 63, a byte
//! outside its text, the step budget spent) jumps to the one exit that answers none: i64::MIN
//! for an Int, false for a Bool, nothing written for a Text.

use crate::ast::*;
use crate::check::Model;
use std::collections::HashMap;
use std::fmt::Write;
/// The most bytes one array may ask for: the runtime's HELPER_ARENA_MAX (all of a thread's
/// helpers' arrays at once), so a size past it is none before the arena is asked.
const ARRAY_MAX: i64 = 256 << 20;
/// An array of a size written out, at most this many bytes, is on the stack instead.
const SMALL_ARRAY: usize = 4096;
const NONE: &str = "-9223372036854775808";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Elem {
    I64,
    I32,
    U8,
}

impl Elem {
    fn ll(self) -> &'static str {
        match self {
            Elem::I64 => "i64",
            Elem::I32 => "i32",
            Elem::U8 => "i8",
        }
    }
    fn bytes(self) -> usize {
        match self {
            Elem::I64 => 8,
            Elem::I32 => 4,
            Elem::U8 => 1,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Ty {
    Int,
    Bool,
    Text,
    Arr(Elem),
}

impl Ty {
    fn show(self) -> String {
        match self {
            Ty::Int => "Int".into(),
            Ty::Bool => "Bool".into(),
            Ty::Text => "Text".into(),
            Ty::Arr(e) => format!("{}(…)", match e {
                Elem::I64 => "ints",
                Elem::I32 => "int32s",
                Elem::U8 => "bytes",
            }),
        }
    }
}

/// A name in a helper: where its value is.
#[derive(Clone, Debug)]
enum Local {
    /// a parameter's value (an Int's or Bool's i64 made i1 for a Bool)
    Value(String, Ty),
    /// a Text parameter: its pointer and length
    Text(String, String),
    /// a stack slot (an Int or Bool); `mutable`: a `var`
    Slot(String, Ty, bool),
    /// an array in the arena: its pointer, its length (each an SSA value or a number)
    Arr(String, String, Elem),
}

type E = (Span, String);

fn e<T>(span: Span, msg: impl Into<String>) -> Result<T, E> {
    Err((span, msg.into()))
}

/// The type a helper's parameter or result has here (Time and Duration are Ints).
fn ty_of(t: &Type) -> Option<Ty> {
    match t {
        Type::Int | Type::Time | Type::Duration => Some(Ty::Int),
        Type::Bool => Some(Ty::Bool),
        Type::Text => Some(Ty::Text),
        _ => None,
    }
}

struct Gen<'a> {
    m: &'a Model<'a>,
    name: String,
    ret: Ty,
    /// the stack slots, made in the entry block
    allocas: String,
    body: String,
    n: usize,
    /// the block being written (a phi names where it came from)
    cur: String,
    /// the block ended (a branch or return): what follows starts a new, unreachable one
    ended: bool,
    scopes: Vec<HashMap<String, Local>>,
    /// (continue's block, break's block)
    loops: Vec<(String, String)>,
    /// it makes arrays: the arena is marked on entry and released on every way out
    arena: bool,
    /// the helpers it calls
    calls: Vec<String>,
    steps: bool,
}

impl<'a> Gen<'a> {
    fn v(&mut self) -> String {
        self.n += 1;
        format!("%h{}", self.n)
    }
    fn label(&mut self, what: &str) -> String {
        self.n += 1;
        format!("{what}{}", self.n)
    }
    fn line(&mut self, s: &str) {
        if self.ended {
            let l = self.label("dead");
            self.start(&l);
        }
        let _ = writeln!(self.body, "  {s}");
    }
    fn start(&mut self, l: &str) {
        let _ = writeln!(self.body, "{l}:");
        self.cur = l.to_string();
        self.ended = false;
    }
    fn br(&mut self, to: &str) {
        self.line(&format!("br label %{to}"));
        self.ended = true;
    }
    fn cond_br(&mut self, c: &str, yes: &str, no: &str) {
        self.line(&format!("br i1 {c}, label %{yes}, label %{no}"));
        self.ended = true;
    }
    /// Fails (none) when `bad` holds.
    fn fail_if(&mut self, bad: &str) {
        let ok = self.label("ok");
        self.cond_br(bad, "fail", &ok);
        self.start(&ok);
    }

    fn lookup(&self, n: &str) -> Option<&Local> {
        self.scopes.iter().rev().find_map(|s| s.get(n))
    }
    fn declare(&mut self, span: Span, n: &str, l: Local) -> Result<(), E> {
        if self.lookup(n).is_some() {
            return e(span, format!("`{n}` is already a name here: a helper's names don't shadow"));
        }
        self.scopes.last_mut().unwrap().insert(n.to_string(), l);
        Ok(())
    }

    /// An Int's or Bool's value (i64, or i1).
    fn int(&mut self, x: &Expr) -> Result<String, E> {
        match self.ex(x)? {
            (v, Ty::Int) => Ok(v),
            (_, t) => e(x.span(), format!("wants an Int, not {}", t.show())),
        }
    }
    fn bool(&mut self, x: &Expr) -> Result<String, E> {
        match self.ex(x)? {
            (v, Ty::Bool) => Ok(v),
            (_, t) => e(x.span(), format!("wants a Bool, not {}", t.show())),
        }
    }
    /// A Text argument: a Text parameter's name.
    fn text(&mut self, x: &Expr) -> Result<(String, String), E> {
        match x {
            Expr::Name(n, s) => match self.lookup(n) {
                Some(Local::Text(p, l)) => Ok((p.clone(), l.clone())),
                _ => e(*s, format!("`{n}` isn't a Text parameter (a helper's text is its parameters')")),
            },
            other => e(other.span(), "a helper's text is its Text parameters (no text is made inside one yet)"),
        }
    }

    fn ex(&mut self, x: &Expr) -> Result<(String, Ty), E> {
        match x {
            Expr::Int(n, _) | Expr::Dur(n, _) => Ok((n.to_string(), Ty::Int)),
            Expr::Bool(b, _) => Ok(((if *b { "true" } else { "false" }).into(), Ty::Bool)),
            Expr::Name(n, s) => match self.lookup(n).cloned() {
                Some(Local::Value(v, t)) => Ok((v, t)),
                Some(Local::Slot(p, t @ (Ty::Int | Ty::Bool), _)) => {
                    let v = self.v();
                    self.line(&format!("{v} = load {}, ptr {p}", if t == Ty::Bool { "i1" } else { "i64" }));
                    Ok((v, t))
                }
                Some(Local::Slot(_, t, _)) => e(*s, format!("`{n}` is {}", t.show())),
                Some(Local::Arr(..)) => e(*s, format!("`{n}` is an array: index it (`{n}[i]`)")),
                Some(Local::Text(..)) => e(*s, format!("`{n}` is Text: read it with byte_at({n}, i) and byte_length({n})")),
                None if n == "viewer" || n == "now" => e(*s, format!("a helper sees only its arguments: pass `{n}`'s value in")),
                // a constant: its value, when it's a number or a flag
                None if self.m.consts.contains_key(n) => match self.m.consts.get(n) {
                    Some(Decl::Const { value: v @ (Expr::Int(..) | Expr::Dur(..) | Expr::Bool(..)), .. }) => self.ex(&v.clone()),
                    _ => e(*s, format!("`{n}` isn't a number or a flag: a helper uses only those constants")),
                },
                None => e(*s, format!("no `{n}` here (a helper sees only its parameters and its own names)")),
            },
            Expr::Not(a, _) => {
                let c = self.bool(a)?;
                let v = self.v();
                self.line(&format!("{v} = xor i1 {c}, true"));
                Ok((v, Ty::Bool))
            }
            Expr::Bin(op @ (BinOp::And | BinOp::Or), a, b, _) => {
                // the right side only when it decides (`i < n and a[i] == 0`)
                let l = self.bool(a)?;
                let from = self.cur.clone();
                let (rhs, done) = (self.label("rhs"), self.label("sc"));
                if *op == BinOp::And {
                    self.cond_br(&l, &rhs, &done);
                } else {
                    self.cond_br(&l, &done, &rhs);
                }
                self.start(&rhs);
                let r = self.bool(b)?;
                let rend = self.cur.clone();
                self.br(&done);
                self.start(&done);
                let v = self.v();
                let short = if *op == BinOp::And { "false" } else { "true" };
                self.line(&format!("{v} = phi i1 [ {short}, %{from} ], [ {r}, %{rend} ]"));
                Ok((v, Ty::Bool))
            }
            Expr::Bin(op @ (BinOp::Eq | BinOp::Ne), a, b, s) => {
                let (l, ta) = self.ex(a)?;
                let (r, tb) = self.ex(b)?;
                if ta != tb || !matches!(ta, Ty::Int | Ty::Bool) {
                    return e(*s, format!("can't compare {} with {}", ta.show(), tb.show()));
                }
                let v = self.v();
                self.line(&format!("{v} = icmp {} {} {l}, {r}", if *op == BinOp::Eq { "eq" } else { "ne" }, if ta == Ty::Bool { "i1" } else { "i64" }));
                Ok((v, Ty::Bool))
            }
            Expr::Bin(op @ (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge), a, b, _) => {
                let (l, r) = (self.int(a)?, self.int(b)?);
                let p = match op {
                    BinOp::Lt => "slt",
                    BinOp::Le => "sle",
                    BinOp::Gt => "sgt",
                    _ => "sge",
                };
                let v = self.v();
                self.line(&format!("{v} = icmp {p} i64 {l}, {r}"));
                Ok((v, Ty::Bool))
            }
            Expr::Bin(BinOp::In, _, _, s) => e(*s, "`in` takes a list of rows: not in a helper"),
            Expr::Bin(op, a, b, s) => {
                let (l, r) = (self.int(a)?, self.int(b)?);
                let v = match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul => {
                        let f = match op {
                            BinOp::Add => "sadd",
                            BinOp::Sub => "ssub",
                            _ => "smul",
                        };
                        let (p, x, o) = (self.v(), self.v(), self.v());
                        self.line(&format!("{p} = call {{i64, i1}} @llvm.{f}.with.overflow.i64(i64 {l}, i64 {r})\n  {x} = extractvalue {{i64, i1}} {p}, 0\n  {o} = extractvalue {{i64, i1}} {p}, 1"));
                        self.fail_if(&o);
                        x
                    }
                    BinOp::Div | BinOp::Mod => {
                        // by 0, and i64::MIN by -1 (it overflows): none
                        let (z, a1, a2, a3, bad, x) = (self.v(), self.v(), self.v(), self.v(), self.v(), self.v());
                        self.line(&format!("{z} = icmp eq i64 {r}, 0\n  {a1} = icmp eq i64 {r}, -1\n  {a2} = icmp eq i64 {l}, {NONE}\n  {a3} = and i1 {a1}, {a2}\n  {bad} = or i1 {z}, {a3}"));
                        self.fail_if(&bad);
                        self.line(&format!("{x} = {} i64 {l}, {r}", if *op == BinOp::Div { "sdiv" } else { "srem" }));
                        x
                    }
                    BinOp::Shl | BinOp::Shr => {
                        let (bad, x) = (self.v(), self.v());
                        self.line(&format!("{bad} = icmp ugt i64 {r}, 63"));
                        self.fail_if(&bad);
                        self.line(&format!("{x} = {} i64 {l}, {r}", if *op == BinOp::Shl { "shl" } else { "ashr" }));
                        x
                    }
                    BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::AddWrap | BinOp::SubWrap | BinOp::MulWrap => {
                        let ins = match op {
                            BinOp::BitAnd => "and",
                            BinOp::BitOr => "or",
                            BinOp::BitXor => "xor",
                            BinOp::AddWrap => "add",
                            BinOp::SubWrap => "sub",
                            _ => "mul",
                        };
                        let x = self.v();
                        self.line(&format!("{x} = {ins} i64 {l}, {r}"));
                        x
                    }
                    _ => return e(*s, format!("`{}` isn't arithmetic", op.sym())),
                };
                Ok((v, Ty::Int))
            }
            Expr::If(c, a, b, _) => {
                let c = self.bool(c)?;
                let (yes, no, done) = (self.label("then"), self.label("else"), self.label("fi"));
                self.cond_br(&c, &yes, &no);
                self.start(&yes);
                let (x, tx) = self.ex(a)?;
                let xend = self.cur.clone();
                self.br(&done);
                self.start(&no);
                let (y, ty) = self.ex(b)?;
                let yend = self.cur.clone();
                self.br(&done);
                if tx != ty || !matches!(tx, Ty::Int | Ty::Bool) {
                    return e(x_span(a), format!("`if`'s two sides are {} and {}", tx.show(), ty.show()));
                }
                self.start(&done);
                let v = self.v();
                self.line(&format!("{v} = phi {} [ {x}, %{xend} ], [ {y}, %{yend} ]", if tx == Ty::Bool { "i1" } else { "i64" }));
                Ok((v, tx))
            }
            Expr::Call(f, a, s) => self.call(f, a, *s),
            other => e(other.span(), "not in a helper: a helper computes on its arguments (Ints, Bools, Text bytes) and its own names"),
        }
    }

    fn args(&self, f: &str, a: &[Expr], n: usize, s: Span) -> Result<(), E> {
        if a.len() != n {
            return e(s, format!("{f} takes {n} argument{}", if n == 1 { "" } else { "s" }));
        }
        Ok(())
    }

    fn call(&mut self, f: &str, a: &[Expr], s: Span) -> Result<(String, Ty), E> {
        match f {
            "__at" => {
                let Expr::Name(n, ns) = &a[0] else { return e(s, "only an array is indexed") };
                let (p, len, el) = match self.lookup(n) {
                    Some(Local::Arr(p, len, el)) => (p.clone(), len.clone(), *el),
                    _ => return e(*ns, format!("`{n}` isn't an array")),
                };
                let at = self.index(&a[1], &len)?;
                let (q, x) = (self.v(), self.v());
                self.line(&format!("{q} = getelementptr inbounds {t}, ptr {p}, i64 {at}\n  {x} = load {t}, ptr {q}", t = el.ll()));
                Ok((self.widen(&x, el), Ty::Int))
            }
            "byte_at" => {
                self.args(f, a, 2, s)?;
                let (p, l) = self.text(&a[0])?;
                let i = self.int(&a[1])?;
                let (bad, q, x, w) = (self.v(), self.v(), self.v(), self.v());
                self.line(&format!("{bad} = icmp uge i64 {i}, {l}"));
                self.fail_if(&bad);
                self.line(&format!("{q} = getelementptr inbounds i8, ptr {p}, i64 {i}\n  {x} = load i8, ptr {q}\n  {w} = zext i8 {x} to i64"));
                Ok((w, Ty::Int))
            }
            "byte_length" => {
                self.args(f, a, 1, s)?;
                let (_, l) = self.text(&a[0])?;
                Ok((l, Ty::Int))
            }
            "min" | "max" => {
                self.args(f, a, 2, s)?;
                let (x, y) = (self.int(&a[0])?, self.int(&a[1])?);
                let (c, v) = (self.v(), self.v());
                self.line(&format!("{c} = icmp {} i64 {x}, {y}\n  {v} = select i1 {c}, i64 {x}, i64 {y}", if f == "max" { "sgt" } else { "slt" }));
                Ok((v, Ty::Int))
            }
            "abs" => {
                self.args(f, a, 1, s)?;
                let x = self.int(&a[0])?;
                let (bad, n, c, v) = (self.v(), self.v(), self.v(), self.v());
                self.line(&format!("{bad} = icmp eq i64 {x}, {NONE}"));
                self.fail_if(&bad);
                self.line(&format!("{n} = sub i64 0, {x}\n  {c} = icmp slt i64 {x}, 0\n  {v} = select i1 {c}, i64 {n}, i64 {x}"));
                Ok((v, Ty::Int))
            }
            _ if self.m.helpers.contains_key(f) => {
                let Some(Decl::Helper { params, ret, .. }) = self.m.helpers.get(f) else { unreachable!() };
                let rt = ty_of(ret).unwrap_or(Ty::Int);
                if rt == Ty::Text {
                    return e(s, format!("`{f}` makes text: a helper calls only helpers that answer an Int or a Bool (yet)"));
                }
                self.args(f, a, params.len(), s)?;
                let mut list = vec![];
                for (p, x) in params.iter().zip(a) {
                    match ty_of(&p.ty) {
                        Some(Ty::Text) => {
                            let (ptr, len) = self.text(x)?;
                            list.push(format!("ptr {ptr}, i64 {len}"));
                        }
                        Some(Ty::Bool) => {
                            let c = self.bool(x)?;
                            let z = self.v();
                            self.line(&format!("{z} = zext i1 {c} to i64"));
                            list.push(format!("i64 {z}"));
                        }
                        _ => list.push(format!("i64 {}", self.int(x)?)),
                    }
                }
                if !self.calls.contains(&f.to_string()) {
                    self.calls.push(f.to_string());
                }
                let (r, bad) = (self.v(), self.v());
                self.line(&format!("{r} = call i64 @burin_fn_{f}({})", list.join(", ")));
                if rt == Ty::Bool {
                    let b = self.v();
                    self.line(&format!("{b} = icmp ne i64 {r}, 0"));
                    return Ok((b, Ty::Bool));
                }
                // its none is this one's
                self.line(&format!("{bad} = icmp eq i64 {r}, {NONE}"));
                self.fail_if(&bad);
                Ok((r, Ty::Int))
            }
            _ if self.m.externs.contains_key(f) => e(s, format!("`{f}` is an external function: a helper calls helpers and its builtins (byte_at, byte_length, min, max, abs)")),
            "ints" | "int32s" | "bytes" => e(s, format!("`{f}(n)` makes an array: `var a = {f}(n)`")),
            _ => e(s, format!("no function `{f}` in a helper (its builtins: byte_at, byte_length, min, max, abs; and helpers)")),
        }
    }

    /// An index into an array of `len`: in it, or none.
    fn index(&mut self, i: &Expr, len: &str) -> Result<String, E> {
        let i = self.int(i)?;
        let bad = self.v();
        self.line(&format!("{bad} = icmp uge i64 {i}, {len}"));
        self.fail_if(&bad);
        Ok(i)
    }

    fn widen(&mut self, x: &str, el: Elem) -> String {
        match el {
            Elem::I64 => x.to_string(),
            Elem::I32 => {
                let w = self.v();
                self.line(&format!("{w} = sext i32 {x} to i64"));
                w
            }
            Elem::U8 => {
                let w = self.v();
                self.line(&format!("{w} = zext i8 {x} to i64"));
                w
            }
        }
    }

    fn block(&mut self, stmts: &[HStmt]) -> Result<(), E> {
        self.scopes.push(HashMap::new());
        for s in stmts {
            self.stmt(s)?;
        }
        self.scopes.pop();
        Ok(())
    }

    /// A loop's turn: one step of the budget.
    fn step(&mut self) {
        if !self.steps {
            return;
        }
        let (k, k1, bad) = (self.v(), self.v(), self.v());
        self.line(&format!("{k} = load i64, ptr %steps\n  {k1} = sub i64 {k}, 1\n  store i64 {k1}, ptr %steps\n  {bad} = icmp slt i64 {k1}, 0"));
        self.fail_if(&bad);
    }

    fn stmt(&mut self, st: &HStmt) -> Result<(), E> {
        match st {
            HStmt::Var(n, init, s) => {
                // an array: `ints(n)`, zeroed, in the arena (n any Int: a negative one or one
                // past the arena's room is none)
                if let Expr::Call(f, a, cs) = init {
                    let el = match f.as_str() {
                        "ints" => Some(Elem::I64),
                        "int32s" => Some(Elem::I32),
                        "bytes" => Some(Elem::U8),
                        _ => None,
                    };
                    if let Some(el) = el {
                        if a.len() != 1 {
                            return e(*cs, format!("{f}(n): an array of n"));
                        }
                        // a small one of a size written out: on the stack (LLVM keeps such in
                        // registers; a few KB is safe however deep helpers call)
                        if let Expr::Int(k, _) = &a[0] {
                            if *k > 0 && *k as usize * el.bytes() <= SMALL_ARRAY {
                                let p = format!("%a_{n}_{}", self.n);
                                self.n += 1;
                                let bytes = *k as usize * el.bytes();
                                let _ = writeln!(self.allocas, "  {p} = alloca [{k} x {}], align 16", el.ll());
                                self.line(&format!("call void @llvm.memset.p0.i64(ptr align 16 {p}, i8 0, i64 {bytes}, i1 false)"));
                                return self.declare(*s, n, Local::Arr(p, k.to_string(), el));
                            }
                        }
                        let len = self.int(&a[0])?;
                        let (bad, bytes, p, null) = (self.v(), self.v(), self.v(), self.v());
                        self.line(&format!("{bad} = icmp ugt i64 {len}, {}", ARRAY_MAX / el.bytes() as i64));
                        self.fail_if(&bad);
                        self.line(&format!("{bytes} = mul nuw nsw i64 {len}, {}\n  {p} = call noalias ptr @burin_ha_alloc(i64 {bytes})\n  {null} = icmp eq ptr {p}, null", el.bytes()));
                        self.fail_if(&null);
                        self.line(&format!("call void @llvm.memset.p0.i64(ptr align 8 {p}, i8 0, i64 {bytes}, i1 false)"));
                        return self.declare(*s, n, Local::Arr(p, len, el));
                    }
                }
                let (v, t) = self.ex(init)?;
                if !matches!(t, Ty::Int | Ty::Bool) {
                    return e(*s, format!("a var holds an Int or a Bool, not {}", t.show()));
                }
                let p = format!("%v_{n}_{}", self.n);
                self.n += 1;
                let ll = if t == Ty::Bool { "i1" } else { "i64" };
                let _ = writeln!(self.allocas, "  {p} = alloca {ll}");
                self.line(&format!("store {ll} {v}, ptr {p}"));
                self.declare(*s, n, Local::Slot(p, t, true))
            }
            HStmt::Set(n, x, s) => {
                let (p, t) = match self.lookup(n) {
                    Some(Local::Slot(p, t @ (Ty::Int | Ty::Bool), true)) => (p.clone(), *t),
                    Some(Local::Arr(..)) => return e(*s, format!("`{n}` is an array: set an element (`{n}[i] = …`)")),
                    Some(_) => return e(*s, format!("`{n}` doesn't change (a parameter, or a loop's counter): copy it into a `var`")),
                    None => return e(*s, format!("no `{n}`: declare it with `var {n} = …`")),
                };
                let (v, tv) = self.ex(x)?;
                if tv != t {
                    return e(*s, format!("`{n}` is {}, not {}", t.show(), tv.show()));
                }
                self.line(&format!("store {} {v}, ptr {p}", if t == Ty::Bool { "i1" } else { "i64" }));
                Ok(())
            }
            HStmt::SetAt(n, i, x, s) => {
                let (p, len, el) = match self.lookup(n) {
                    Some(Local::Arr(p, len, el)) => (p.clone(), len.clone(), *el),
                    _ => return e(*s, format!("`{n}` isn't an array")),
                };
                let at = self.index(i, &len)?;
                let v = self.int(x)?;
                // what the element holds: a value too big for it is none
                let stored = match el {
                    Elem::I64 => v,
                    Elem::I32 | Elem::U8 => {
                        let (lo, hi) = if el == Elem::I32 { (i32::MIN as i64, i32::MAX as i64) } else { (0, 255) };
                        let (b1, b2, bad, t) = (self.v(), self.v(), self.v(), self.v());
                        self.line(&format!("{b1} = icmp slt i64 {v}, {lo}\n  {b2} = icmp sgt i64 {v}, {hi}\n  {bad} = or i1 {b1}, {b2}"));
                        self.fail_if(&bad);
                        self.line(&format!("{t} = trunc i64 {v} to {}", el.ll()));
                        t
                    }
                };
                let q = self.v();
                self.line(&format!("{q} = getelementptr inbounds {t}, ptr {p}, i64 {at}\n  store {t} {stored}, ptr {q}", t = el.ll()));
                Ok(())
            }
            HStmt::If(c, a, b, _) => {
                let c = self.bool(c)?;
                let (yes, no, done) = (self.label("then"), self.label("else"), self.label("fi"));
                self.cond_br(&c, &yes, &no);
                self.start(&yes);
                self.block(a)?;
                self.br(&done);
                self.start(&no);
                self.block(b)?;
                self.br(&done);
                self.start(&done);
                Ok(())
            }
            HStmt::While(c, body, _) => {
                let (head, inner, done) = (self.label("while"), self.label("body"), self.label("wend"));
                self.br(&head);
                self.start(&head);
                // a counted loop ends by itself (`counted`): no step
                if !self.counted(c, body) {
                    self.step();
                }
                let c = self.bool(c)?;
                self.cond_br(&c, &inner, &done);
                self.start(&inner);
                self.loops.push((head.clone(), done.clone()));
                self.block(body)?;
                self.loops.pop();
                self.br(&head);
                self.start(&done);
                Ok(())
            }
            HStmt::For(n, from, to, body, s) => {
                let a = self.int(from)?;
                let b = self.int(to)?;
                let p = format!("%i_{n}_{}", self.n);
                self.n += 1;
                let _ = writeln!(self.allocas, "  {p} = alloca i64");
                self.line(&format!("store i64 {a}, ptr {p}"));
                let (head, inner, next, done) = (self.label("for"), self.label("body"), self.label("next"), self.label("fend"));
                self.br(&head);
                self.start(&head);
                // (no step: it ends by itself, its end read once; a loop inside it pays its own)
                let (i, more) = (self.v(), self.v());
                self.line(&format!("{i} = load i64, ptr {p}\n  {more} = icmp slt i64 {i}, {b}"));
                self.cond_br(&more, &inner, &done);
                self.start(&inner);
                self.scopes.push(HashMap::new());
                self.declare(*s, n, Local::Slot(p.clone(), Ty::Int, false))?;
                self.loops.push((next.clone(), done.clone()));
                self.block(body)?;
                self.loops.pop();
                self.scopes.pop();
                self.br(&next);
                self.start(&next);
                let (i2, i3) = (self.v(), self.v());
                // (i < b, so i + 1 doesn't overflow)
                self.line(&format!("{i2} = load i64, ptr {p}\n  {i3} = add nsw i64 {i2}, 1\n  store i64 {i3}, ptr {p}"));
                self.br(&head);
                self.start(&done);
                Ok(())
            }
            HStmt::Break(s) | HStmt::Continue(s) => {
                let Some((cont, brk)) = self.loops.last().cloned() else { return e(*s, "outside a loop") };
                self.br(if matches!(st, HStmt::Break(_)) { &brk } else { &cont });
                Ok(())
            }
            HStmt::Return(x, s) => {
                match (self.ret, x) {
                    (Ty::Text, None) => {
                        self.leave();
                        self.line("ret void");
                    }
                    (Ty::Text, Some(_)) => return e(*s, "a Text helper's result is what it wrote (out_byte): `return` takes no value"),
                    (_, None) => return e(*s, format!("{} answers {}: `return` a value", self.name, self.ret.show())),
                    (Ty::Int, Some(x)) => {
                        let v = self.int(x)?;
                        self.leave();
                        self.line(&format!("ret i64 {v}"));
                    }
                    (_, Some(x)) => {
                        let c = self.bool(x)?;
                        let z = self.v();
                        self.line(&format!("{z} = zext i1 {c} to i64"));
                        self.leave();
                        self.line(&format!("ret i64 {z}"));
                    }
                }
                self.ended = true;
                Ok(())
            }
            HStmt::Out(x, s) => {
                if self.ret != Ty::Text {
                    return e(*s, "out_byte is a Text helper's: what it writes is its result");
                }
                let v = self.int(x)?;
                let (b1, b2, bad, ok, full) = (self.v(), self.v(), self.v(), self.v(), self.v());
                self.line(&format!("{b1} = icmp slt i64 {v}, 0\n  {b2} = icmp sgt i64 {v}, 255\n  {bad} = or i1 {b1}, {b2}"));
                self.fail_if(&bad);
                self.line(&format!("{ok} = call i64 @burin_out_byte(ptr %out, i64 {v})\n  {full} = icmp eq i64 {ok}, 0"));
                self.fail_if(&full);
                Ok(())
            }
        }
    }
}

impl Gen<'_> {
    /// Whether a `while` ends by itself, as a `for` does: its condition is `x < y` or `x <= y`
    /// (or that `and` more); `x` is a `var` the body changes in one place only, a statement of its
    /// own (not inside an `if` or a loop) `x = x + c` with c a number above 0; `y` reads nothing
    /// the loop changes; and nothing `continue`s it (every turn that goes on reaches the step). An
    /// overflow of x ends the call (none), so x reaches y.
    fn counted(&self, c: &Expr, body: &[HStmt]) -> bool {
        let mut first = c;
        while let Expr::Bin(BinOp::And, l, _, _) = first {
            first = l;
        }
        let Expr::Bin(BinOp::Lt | BinOp::Le, x, y, _) = first else { return false };
        let Expr::Name(x, _) = &**x else { return false };
        if !matches!(self.lookup(x), Some(Local::Slot(_, Ty::Int, true))) {
            return false;
        }
        let changed = assigned(body);
        let steps_up = |s: &HStmt| match s {
            HStmt::Set(n, Expr::Bin(BinOp::Add, a, b, _), _) if n == x => matches!((&**a, &**b), (Expr::Name(m, _), Expr::Int(k, _)) | (Expr::Int(k, _), Expr::Name(m, _)) if m == x && *k > 0),
            _ => false,
        };
        changed.iter().filter(|n| *n == x).count() == 1 && body.iter().any(steps_up) && !reads(y, &changed) && !continues(body)
    }

    /// Before a way out: the arena back to where the helper found it.
    fn leave(&mut self) {
        if self.arena {
            self.line("call void @burin_ha_release(i64 %mark)");
        }
    }
}

fn x_span(x: &Expr) -> Span {
    x.span()
}

/// The names statements assign (each time it's written: `x = …` twice is x twice), and the
/// arrays whose elements they set, anywhere in them.
fn assigned(body: &[HStmt]) -> Vec<String> {
    let mut out = vec![];
    for s in body {
        match s {
            HStmt::Set(n, _, _) | HStmt::SetAt(n, _, _, _) => out.push(n.clone()),
            HStmt::If(_, a, b, _) => {
                out.extend(assigned(a));
                out.extend(assigned(b));
            }
            HStmt::While(_, a, _) | HStmt::For(_, _, _, a, _) => out.extend(assigned(a)),
            _ => {}
        }
    }
    out
}

/// Whether an expression reads any of `names`, or an array's element (which a loop may set).
fn reads(e: &Expr, names: &[String]) -> bool {
    match e {
        Expr::Name(n, _) => names.contains(n),
        Expr::Call(f, a, _) => f == "__at" || a.iter().any(|x| reads(x, names)),
        Expr::Bin(_, l, r, _) => reads(l, names) || reads(r, names),
        Expr::Not(x, _) => reads(x, names),
        Expr::If(c, a, b, _) => reads(c, names) || reads(a, names) || reads(b, names),
        Expr::Int(..) | Expr::Dur(..) | Expr::Bool(..) => false,
        // (anything else isn't in a helper: the generator refuses it)
        _ => true,
    }
}

/// Whether a loop's body `continue`s it (a `continue` in a loop inside is that loop's).
fn continues(body: &[HStmt]) -> bool {
    body.iter().any(|s| match s {
        HStmt::Continue(_) => true,
        HStmt::If(_, a, b, _) => continues(a) || continues(b),
        _ => false,
    })
}

/// Whether statements make an array (`var a = ints(n)`, anywhere in them).
fn makes_arrays(body: &[HStmt]) -> bool {
    body.iter().any(|s| match s {
        HStmt::Var(_, Expr::Call(f, _, _), _) => matches!(f.as_str(), "ints" | "int32s" | "bytes"),
        HStmt::If(_, a, b, _) => makes_arrays(a) || makes_arrays(b),
        HStmt::While(_, a, _) | HStmt::For(_, _, _, a, _) => makes_arrays(a),
        _ => false,
    })
}

/// One helper: its IR (`define …`) and the helpers it calls.
fn helper(m: &Model, d: &Decl, steps: bool) -> Result<(String, Vec<String>), E> {
    let Decl::Helper { name, params, ret, body, span } = d else { unreachable!() };
    let Some(rt) = ty_of(ret) else { return e(*span, format!("a helper answers an Int, a Bool or Text, not {}", ret.show())) };
    let arena = makes_arrays(body);
    let mut g = Gen { m, name: name.clone(), ret: rt, allocas: String::new(), body: String::new(), n: 0, cur: "body".into(), ended: false, scopes: vec![HashMap::new()], loops: vec![], arena, calls: vec![], steps };
    let mut sig = vec![];
    let mut checks = String::new();
    for p in params {
        match ty_of(&p.ty) {
            Some(Ty::Text) => {
                let (pp, pl) = (format!("%p_{}_ptr", p.name), format!("%p_{}_len", p.name));
                sig.push(format!("ptr {pp}"));
                sig.push(format!("i64 {pl}"));
                g.declare(*span, &p.name, Local::Text(pp, pl))?;
            }
            Some(Ty::Bool) => {
                let (pv, b) = (format!("%p_{}", p.name), format!("%b_{}", p.name));
                sig.push(format!("i64 {pv}"));
                let _ = writeln!(checks, "  {b} = icmp ne i64 {pv}, 0");
                g.declare(*span, &p.name, Local::Value(b, Ty::Bool))?;
            }
            Some(_) => {
                // a none argument: a none result, as burin's arithmetic has it
                let pv = format!("%p_{}", p.name);
                sig.push(format!("i64 {pv}"));
                let _ = writeln!(checks, "  %none_{n} = icmp eq i64 {pv}, {NONE}\n  br i1 %none_{n}, label %fail, label %arg_{n}\narg_{n}:", n = p.name);
                g.declare(*span, &p.name, Local::Value(pv, Ty::Int))?;
            }
            None => return e(*span, format!("a helper's parameters are Ints, Bools, Times, Durations or Text (values, not rows: pass the fields it needs), not {}", p.ty.show())),
        }
    }
    if rt == Ty::Text {
        sig.push("ptr %out".into());
    }
    g.start("body");
    g.block(body)?;
    // the end without a `return`: an Int or Bool helper has no answer (none)
    if !g.ended {
        if rt == Ty::Text {
            g.leave();
            g.line("ret void");
        } else {
            g.br("fail");
        }
    }
    let mut f = String::new();
    let rty = if rt == Ty::Text { "void" } else { "i64" };
    let _ = writeln!(f, "define {rty} @burin_fn_{name}({}) {{\nentry:", sig.join(", "));
    if rt == Ty::Text {
        f.push_str("  %out0 = call i64 @burin_vec_len(ptr %out)\n");
    }
    // the loops' budget: the thread's, for the work it's doing (the writer's is tight)
    if steps {
        f.push_str("  %steps = alloca i64\n  %budget = call i64 @burin_ha_budget()\n  store i64 %budget, ptr %steps\n");
    }
    if arena {
        f.push_str("  %mark = call i64 @burin_ha_mark()\n");
    }
    f.push_str(&g.allocas);
    f.push_str(&checks);
    f.push_str("  br label %body\n");
    f.push_str(&g.body);
    f.push_str("fail:\n");
    if arena {
        f.push_str("  call void @burin_ha_release(i64 %mark)\n");
    }
    match rt {
        Ty::Text => f.push_str("  call void @burin_vec_truncate(ptr %out, i64 %out0)\n  ret void\n"),
        Ty::Bool => f.push_str("  ret i64 0\n"),
        _ => {
            let _ = writeln!(f, "  ret i64 {NONE}");
        }
    }
    f.push_str("}\n\n");
    Ok((f, g.calls))
}

/// Every helper's IR, checked, and no recursion. `steps`: the loops' budget (off only to
/// measure what it costs).
pub fn emit(m: &Model, steps: bool) -> Result<String, E> {
    let mut names: Vec<&String> = m.helpers.keys().collect();
    names.sort();
    let mut out = String::new();
    let mut graph: HashMap<String, (Vec<String>, Span)> = HashMap::new();
    for n in names {
        let d = m.helpers[n];
        let Decl::Helper { body, span, .. } = d else { continue };
        // the `fn`s it calls, put in place (they're expressions: they may call helpers too)
        let body = inline_fns(m, body);
        let mut d2 = d.clone();
        if let Decl::Helper { body: b, .. } = &mut d2 {
            *b = body;
        }
        let (f, calls) = helper(m, &d2, steps)?;
        out.push_str(&f);
        graph.insert(n.clone(), (calls, *span));
    }
    fn walk(g: &HashMap<String, (Vec<String>, Span)>, n: &str, path: &mut Vec<String>) -> Result<(), E> {
        if let Some(i) = path.iter().position(|p| p == n) {
            let mut cycle = path[i..].to_vec();
            cycle.push(n.to_string());
            return e(g[n].1, format!("helpers can't recurse: {}", cycle.join(" calls ")));
        }
        path.push(n.to_string());
        for c in &g[n].0 {
            walk(g, c, path)?;
        }
        path.pop();
        Ok(())
    }
    for n in graph.keys() {
        walk(&graph, n, &mut vec![])?;
    }
    if !out.is_empty() {
        out.insert_str(0, "declare void @llvm.memset.p0.i64(ptr, i8, i64, i1)\ndeclare void @burin_vec_truncate(ptr, i64)\ndeclare i64 @burin_out_byte(ptr, i64)\ndeclare i64 @burin_ha_budget()\ndeclare i64 @burin_ha_mark()\ndeclare void @burin_ha_release(i64)\ndeclare noalias ptr @burin_ha_alloc(i64)\n\n");
    }
    Ok(out)
}

/// A helper's statements with every `fn` call put in place.
fn inline_fns(m: &Model, body: &[HStmt]) -> Vec<HStmt> {
    let x = |e: &Expr| crate::inline::expr(m, e, &crate::inline::Env::new(), 0, 0);
    let b = |s: &[HStmt]| inline_fns(m, s);
    body.iter()
        .map(|s| match s {
            HStmt::Var(n, v, sp) => HStmt::Var(n.clone(), x(v), *sp),
            HStmt::Set(n, v, sp) => HStmt::Set(n.clone(), x(v), *sp),
            HStmt::SetAt(n, i, v, sp) => HStmt::SetAt(n.clone(), x(i), x(v), *sp),
            HStmt::If(c, a, o, sp) => HStmt::If(x(c), b(a), b(o), *sp),
            HStmt::While(c, a, sp) => HStmt::While(x(c), b(a), *sp),
            HStmt::For(n, f, t, a, sp) => HStmt::For(n.clone(), x(f), x(t), b(a), *sp),
            HStmt::Return(v, sp) => HStmt::Return(v.as_ref().map(x), *sp),
            HStmt::Out(v, sp) => HStmt::Out(x(v), *sp),
            other => other.clone(),
        })
        .collect()
}
