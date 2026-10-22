root = '/mnt/d/code/etch/crates/etchc/src/'

def edit(path, pairs):
    p = root + path
    s = open(p, encoding='utf-8').read()
    for old, new in pairs:
        assert old in s, (path, old[:70])
        s = s.replace(old, new, 1)
    open(p, 'w', encoding='utf-8').write(s)

edit('ast.rs', [
    ('''    Fragment { name: String, params: Vec<Param>, per_viewer: bool, body: Template, span: Span },''',
     '''    /// `varies by viewer, viewer.role`: what about the viewer a shared fragment's variants may
    /// depend on (its cache key beyond its row); checked against what its conditions read
    Fragment { name: String, params: Vec<Param>, per_viewer: bool, varies: Vec<String>, body: Template, span: Span },'''),
])
edit('parse.rs', [
    ('''                let per_viewer = if self.eat_kw("per") {
                    self.expect_kw("viewer")?;
                    true
                } else {
                    false
                };
                Ok(Decl::Fragment { name, params, per_viewer, body: self.template()?, span })''', '''                let per_viewer = if self.eat_kw("per") {
                    self.expect_kw("viewer")?;
                    true
                } else {
                    false
                };
                // `varies by viewer, viewer.role`
                let mut varies = vec![];
                if self.eat_kw("varies") {
                    self.expect_kw("by")?;
                    loop {
                        let mut p = self.ident()?;
                        if p != "viewer" {
                            return self.err("varies by names the viewer or its fields (viewer, viewer.role)");
                        }
                        while self.eat(".") {
                            p = format!("{p}.{}", self.ident()?);
                        }
                        varies.push(p);
                        if !self.eat(",") {
                            break;
                        }
                    }
                }
                Ok(Decl::Fragment { name, params, per_viewer, varies, body: self.template()?, span })'''),
])
edit('llvm.rs', [
    ('''    /// the variant key's per-item viewer conditions over the record's own lists, recorded once''', '''    /// what about the viewer the variant key read (`viewer`: its identity; `viewer.role`)
    viewer_uses: Vec<String>,
    /// the variant key's per-item viewer conditions over the record's own lists, recorded once'''),
    ('''                hoisted: vec![],
                prologue: String::new(),
            };''', '''                hoisted: vec![],
                prologue: String::new(),
                viewer_uses: vec![],
            };'''),
    ('''        if root == "viewer" {
            let i = self.md.viewer.field(&key, shape);''', '''        if root == "viewer" {
            if self.bits && !self.viewer_uses.contains(&key) {
                self.viewer_uses.push(key.clone());
            }
            let i = self.md.viewer.field(&key, shape);'''),
    ('''            let (b, sch, tmps, maxb, sp) = (if bits { g.prologue + &g.body } else { g.body }, g.schema, g.tmps, g.max_bits, g.splices);''', '''            if bits && pass == 1 {
                uses = g.viewer_uses.clone();
            }
            let (b, sch, tmps, maxb, sp) = (if bits { g.prologue + &g.body } else { g.body }, g.schema, g.tmps, g.max_bits, g.splices);'''),
    ('''    let mut splices = vec![];
    for pass in 0..2 {''', '''    let mut splices = vec![];
    let mut uses: Vec<String> = vec![];
    for pass in 0..2 {'''),
    ('''    if out.3 > 63 {''', '''    // its declared cache key: what about the viewer its variants depend on, exactly
    if !per_viewer && !name.contains("__s") {
        uses.sort();
        let mut want = varies.to_vec();
        want.sort();
        want.dedup();
        if let Some(u) = uses.iter().find(|u| !want.contains(u)) {
            return Err(format!(
                "fragment {name}: its variants depend on {} (through its conditions), which its key doesn't declare: add `varies by {}`, or mark it `per viewer`",
                if u == "viewer" { "who the viewer is".to_string() } else { format!("`{u}`") },
                uses.join(", ")
            ));
        }
        if let Some(w) = want.iter().find(|w| !uses.contains(w)) {
            return Err(format!("fragment {name}: declares `varies by {w}`, but nothing it shows depends on {w} (drop it)"));
        }
    }
    md.keys.push((name.to_string(), uses.clone()));
    if out.3 > 63 {'''),
    ('''fn register(m: &Model, md: &mut Module, name: &str, params: &[Param], nodes: &[TNode], per_viewer: bool) -> Result<(), String> {''',
     '''fn register(m: &Model, md: &mut Module, name: &str, params: &[Param], nodes: &[TNode], per_viewer: bool, varies: &[String]) -> Result<(), String> {'''),
    ('''        register(m, md, &sname, &used, &[TNode::Hole(e.clone())], false)?;''', '''        register(m, md, &sname, &used, &[TNode::Hole(e.clone())], false, &[])?;'''),
    ('''            Some(Decl::Fragment { params, body, per_viewer, .. }) => (params, body, *per_viewer),
            _ => return Err(format!("no fragment {r}")),
        };
        let nodes = inline::nodes(m, &body.nodes, &HashMap::new());
        register(m, &mut md, r, params, &nodes, per_viewer)?;''', '''            Some(Decl::Fragment { params, body, per_viewer, varies, .. }) => (params, body, *per_viewer, varies),
            _ => return Err(format!("no fragment {r}")),
        };
        let nodes = inline::nodes(m, &body.nodes, &HashMap::new());
        register(m, &mut md, r, params, &nodes, per_viewer.0, per_viewer.1)?;'''),
    ('''    /// what the viewer's record reads (its variant bits' inputs)
    pub viewer_reads: Vec<(String, String)>,
}''', '''    /// what the viewer's record reads (its variant bits' inputs)
    pub viewer_reads: Vec<(String, String)>,
    /// each register's viewer dependence, as read (the report)
    pub keys: Vec<(String, Vec<String>)>,
}'''),
    ('''funcs: String::new(), registers: vec![], viewer_reads: vec![] };''', '''funcs: String::new(), registers: vec![], viewer_reads: vec![], keys: vec![] };'''),
])
# the tuple destructure above needs (params, body, (per_viewer, varies))
p = root + 'llvm.rs'
s = open(p, encoding='utf-8').read()
s = s.replace('''        let (params, body, per_viewer) = match m.fragments.get(&r.to_string()) {
            Some(Decl::Fragment { params, body, per_viewer, varies, .. }) => (params, body, *per_viewer, varies),''', '''        let (params, body, per_viewer) = match m.fragments.get(&r.to_string()) {
            Some(Decl::Fragment { params, body, per_viewer, varies, .. }) => (params, body, (*per_viewer, varies)),''', 1)
open(p, 'w', encoding='utf-8').write(s)
print('ok')
