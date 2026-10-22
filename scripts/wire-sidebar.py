# One-off: campfire-etch's sidebar, splice and messages come from the generated registers.
D = 'D:/code/etch/crates/campfire-etch/src/'

def edit(f, old, new):
    p = D + f
    s = open(p, encoding='utf-8').read()
    assert s.count(old) == 1, (f, old[:80], s.count(old))
    s = s.replace(old, new)
    open(p, 'w', encoding='utf-8', newline='\n').write(s)

edit('state.rs', '''pub fn build_msg(c: &Connection, ctx: &Ctx, id: Id, members: &[Id], users: &HashMap<Id, User>) -> Option<MsgFrag> {
    let viewers: Vec<gen::Viewer> = members
        .iter()
        .map(|uid| match users.get(uid) {
            Some(u) => gen::viewer(u),
            None => gen::Viewer { viewer: *uid, viewer_role: etch_rt::abi::Str::NONE },
        })
        .collect();
    let (variants, by_member) = gen::message_variants(c, &ctx.abi, id, &viewers)?;
    Some(MsgFrag { id, variants: variants.into_iter().map(Seg::new).collect(), by_member })
}''', '''/// Messages' fragments for a room's members: loaded, keyed and built by the generated code.
pub fn build_msgs(c: &Connection, ctx: &Ctx, ids: &[Id], members: &[Id]) -> Vec<MsgFrag> {
    ids.iter()
        .zip(gen::messages(c, &ctx.abi, ids, members))
        .filter_map(|(id, r)| r.map(|(variants, by_member)| MsgFrag { id: *id, variants: variants.into_iter().map(Seg::new).collect(), by_member }))
        .collect()
}''')
edit('state.rs', '''    let frags = msgs.iter().filter_map(|m| build_msg(c, ctx, m.id, &members, users).map(Arc::new)).collect();''',
     '''    let _ = users;
    let ids: Vec<Id> = msgs.iter().map(|m| m.id).collect();
    let frags = build_msgs(c, ctx, &ids, &members).into_iter().map(Arc::new).collect();''')
edit('state.rs', '''    let tok_seg = Arc::new(Seg::new(tok.plain.as_bytes().to_vec()));''',
     '''    let tok_seg = Arc::new(Seg::new(gen::sidebar_splice0(c, &ctx.abi, room_id).expect("the room")));''')
edit('state.rs', '''    Ok(Sidebar { pieces: render::sidebar(ctx, &side_data(c, users, uid)?).into_iter().map(Seg::new).collect() })''',
     '''    let _ = users;
    let pieces = gen::sidebar(c, &ctx.abi, uid).unwrap_or_default();
    Ok(Sidebar { pieces: pieces.into_iter().map(Seg::new).collect() })''')
edit('writer.rs', '''                let frag = match state::build_msg(&c, &ctx, m.id, &view.members, &next.users) {
                    Some(f) => Arc::new(f),
                    None => continue,
                };''', '''                let frag = match state::build_msgs(&c, &ctx, &[m.id], &view.members).pop() {
                    Some(f) => Arc::new(f),
                    None => continue,
                };''')
print('ok')
