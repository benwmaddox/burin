# One-off: a find delta of its own, and the header's find dispatch.
def edit(p, old, new):
    s = open(p, encoding='utf-8').read()
    assert s.count(old) == 1, (p, old[:80], s.count(old))
    s = s.replace(old, new)
    open(p, 'w', encoding='utf-8', newline='\n').write(s)

A = 'D:/code/etch/crates/etchc/src/actions.rs'
L = 'D:/code/etch/crates/etchc/src/llvm.rs'
edit(A, '''self.line(&format!("call void @etch_delta_patch(ptr %d, i64 {}, i64 -9, i64 {id}, i64 {slot}, i64 {pi}, i64 0)", l.reg));''',
     '''self.line(&format!("call void @etch_delta_find(ptr %d, i64 {}, i64 {id}, i64 {slot}, i64 {pi})", l.reg));''')
edit(L, '''    let (acts, patches) = crate::actions::lower(m, &mut md, &mut qid, &screens)?;''',
     '''    let (acts, patches, finds) = crate::actions::lower(m, &mut md, &mut qid, &screens)?;''')
edit(L, '''        let _ = writeln!(rs, "    fn etch_patch_{}_{l}_{f}(rec: *mut u8, item: i64, value: i64) -> bool;", md.registers[*r].name);
    }''', '''        let _ = writeln!(rs, "    fn etch_patch_{}_{l}_{f}(rec: *mut u8, item: i64, value: i64) -> bool;", md.registers[*r].name);
    }
    for (r, l, f) in &finds {
        let _ = writeln!(rs, "    fn etch_find_{}_{l}_{f}(rec: *mut u8, item: i64, value: i64) -> bool;", md.registers[*r].name);
    }''')
edit(L, '''    let _ = writeln!(rs, "        _ => false,\\n    }}\\n}}");''', '''    let _ = writeln!(rs, "        _ => false,\\n    }}\\n}}");
    let _ = writeln!(rs, "/// Whether a kept record's list holds an item referencing this row.\\npub unsafe fn find(reg: i64, list: i64, field: i64, rec: *mut u8, row: i64) -> bool {{\\n    match (reg, list, field) {{");
    for (r, l, f) in &finds {
        let _ = writeln!(rs, "        ({r}, {l}, {f}) => etch_find_{}_{l}_{f}(rec, row, 0),", md.registers[*r].name);
    }
    let _ = writeln!(rs, "        _ => true,\\n    }}\\n}}");''')
edit(L, '''declare void @etch_delta_patch(ptr, i64, i64, i64, i64, i64, i64)''', '''declare void @etch_delta_patch(ptr, i64, i64, i64, i64, i64, i64)
declare void @etch_delta_find(ptr, i64, i64, i64, i64)''')
print('ok')
