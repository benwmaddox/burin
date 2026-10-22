# One-off: the module decides its registers from the screens and appends the glue.
def edit(p, old, new):
    s = open(p, encoding='utf-8').read()
    assert s.count(old) == 1, (p, old[:80], s.count(old))
    s = s.replace(old, new)
    open(p, 'w', encoding='utf-8', newline='\n').write(s)

L = 'D:/code/etch/crates/etchc/src/llvm.rs'
M = 'D:/code/etch/crates/etchc/src/main.rs'
edit(L, '''    md.viewer.field("viewer", Shape::I64);
    for r in registers {''', '''    md.viewer.field("viewer", Shape::I64);
    // the registers: those named, or (by default) what the screens show
    let auto = crate::glue::registers_of(m);
    let registers: Vec<&str> = if registers.is_empty() { auto.iter().map(String::as_str).collect() } else { registers.to_vec() };
    for r in &registers {''')
edit(L, '''        let (params, body, per_viewer) = match m.fragments.get(*r) {''', '''        let (params, body, per_viewer) = match m.fragments.get(&r.to_string()) {''')
edit(L, '''    Ok((ll, rs))''', '''    rs.push_str(&crate::glue::glue(m, &md, &screens, &acts)?);
    Ok((ll, rs))''')
edit(M, '''            let (out, regs) = match (args.get(3), args.get(4)) {
                (Some(o), Some(r)) => (Path::new(o), r.split(',').collect::<Vec<_>>()),
                _ => {
                    eprintln!("usage: etchc emit <app dir> <out dir> <register,…>");
                    return ExitCode::from(64);
                }
            };''', '''            // (the registers are what the screens show, unless named)
            let (out, regs) = match (args.get(3), args.get(4)) {
                (Some(o), Some(r)) => (Path::new(o), r.split(',').collect::<Vec<_>>()),
                (Some(o), None) => (Path::new(o), vec![]),
                _ => {
                    eprintln!("usage: etchc emit <app dir> <out dir> [register,…]");
                    return ExitCode::from(64);
                }
            };''')
edit(M, 'mod check;\n', 'mod check;\nmod glue;\n')
print('ok')
