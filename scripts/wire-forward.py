# One-off: forwarding in llvm.rs (module order, header, declarations).
p = 'D:/code/etch/crates/etchc/src/llvm.rs'
s = open(p, encoding='utf-8').read()

def rep(old, new):
    global s
    assert s.count(old) == 1, (old[:80], s.count(old))
    s = s.replace(old, new)

rep('''    pub fn str_const_pub(&mut self, b: &[u8]) -> String {''', '''    pub fn viewer_field(&mut self, key: &str, s: Shape) -> usize {
        self.viewer.field(key, s)
    }
    pub fn str_const_pub(&mut self, b: &[u8]) -> String {''')
rep('''    let vl = viewer_loader(m, &mut md, &mut qid)?;
    md.funcs.push_str(&vl);
    let screens = crate::actions::screen_lists(m)?;
    let acts = crate::actions::lower(m, &mut md, &mut qid, &screens)?;''', '''    let screens = crate::actions::screen_lists(m)?;
    let (acts, patches) = crate::actions::lower(m, &mut md, &mut qid, &screens)?;
    // the viewer's loader last: forwarding may have added fields to the viewer's record
    let vl = viewer_loader(m, &mut md, &mut qid)?;
    md.funcs.push_str(&vl);
    for a in &acts {
        eprintln!("{}: delta edges", a.name);
        for e in &a.edges {
            eprintln!("  {e}");
        }
    }''')
old_ext = '''        let _ = writeln!(rs, "extern \\"C\\" {{\\n    pub fn etch_act_{}(ctx: *const etch_rt::abi::Ctx, db: *mut etch_rt::abi::Db, w: *mut etch_rt::abi::Writer, viewer: i64, args: *const Args_{}, d: *mut etch_rt::abi::Deltas) -> i32;\\n}}\\n", a.name, a.name);
    }'''
new_ext = '''        let _ = writeln!(rs, "extern \\"C\\" {{\\n    pub fn etch_act_{}(ctx: *const etch_rt::abi::Ctx, db: *mut etch_rt::abi::Db, w: *mut etch_rt::abi::Writer, viewer: i64, vrec: *const Viewer, args: *const Args_{}, d: *mut etch_rt::abi::Deltas) -> i32;\\n}}\\n", a.name, a.name);
    }
    // patches of kept records: (register, list, field) to the generated function
    let _ = writeln!(rs, "extern \\"C\\" {{");
    for (r, l, f) in &patches {
        let _ = writeln!(rs, "    fn etch_patch_{}_{l}_{f}(rec: *mut u8, item: i64, value: i64) -> bool;", md.registers[*r].name);
    }
    let _ = writeln!(rs, "}}\\n/// Patches the item with this id in a kept record: false if it isn't there.\\npub unsafe fn patch(reg: i64, list: i64, field: i64, rec: *mut u8, item: i64, value: i64) -> bool {{\\n    match (reg, list, field) {{");
    for (r, l, f) in &patches {
        let _ = writeln!(rs, "        ({r}, {l}, {f}) => etch_patch_{}_{l}_{f}(rec, item, value),", md.registers[*r].name);
    }
    let _ = writeln!(rs, "        _ => false,\\n    }}\\n}}");'''
rep(old_ext, new_ext)
rep('''declare void @etch_w_token(ptr, i64, ptr)
"#;''', '''declare void @etch_w_token(ptr, i64, ptr)
declare ptr @etch_w_alloc(ptr, i64)
declare void @etch_delta_patch(ptr, i64, i64, i64, i64, i64, i64)
"#;''')
open(p, 'w', encoding='utf-8', newline='\n').write(s)
print('ok')
