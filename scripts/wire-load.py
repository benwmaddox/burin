# One-off: campfire-etch's registers load through the generated loaders (no hand queries for them).
D = 'D:/code/etch/crates/campfire-etch/src/'

def edit(f, old, new, count=1):
    p = D + f
    s = open(p, encoding='utf-8').read()
    assert s.count(old) == count, (f, old[:80], s.count(old))
    s = s.replace(old, new)
    open(p, 'w', encoding='utf-8', newline='\n').write(s)

edit('state.rs', '''pub struct MsgFrag {
    pub data: MsgData,''', '''pub struct MsgFrag {
    pub id: Id,''')
a = open(D + 'state.rs', encoding='utf-8').read()
i = a.index('pub fn build_msg(')
j = a.index('/// The room\'s view, from SQLite')
new_fns = '''/// A message's fragment for a room's members: loaded, keyed and built by the generated code.
pub fn build_msg(c: &Connection, ctx: &Ctx, id: Id, members: &[Id], users: &HashMap<Id, User>) -> Option<MsgFrag> {
    let viewers: Vec<gen::Viewer> = members
        .iter()
        .map(|uid| match users.get(uid) {
            Some(u) => gen::viewer(u),
            None => gen::Viewer { viewer: *uid, viewer_role: etch_rt::abi::Str::NONE },
        })
        .collect();
    let (variants, by_member) = gen::message_variants(c, &ctx.abi, id, &viewers)?;
    Some(MsgFrag { id, variants: variants.into_iter().map(Seg::new).collect(), by_member })
}

'''
a = a[:i] + new_fns + a[j:]
open(D + 'state.rs', 'w', encoding='utf-8', newline='\n').write(a)
edit('state.rs', '''    let head = gen::room_head(&ctx.abi, room_id, room.name.as_deref());
    let tail = gen::room_tail(&ctx.abi, room_id);
    let msgs = db::last_messages(c, room_id)?;
    let ids: Vec<Id> = msgs.iter().map(|m| m.id).collect();
    let mut boosts: HashMap<Id, Vec<db::Boost>> = HashMap::new();
    for b in db::boosts(c, &ids)? {
        boosts.entry(b.message).or_default().push(b);
    }
    let frags = msgs
        .into_iter()
        .map(|m| {
            let bs = boosts.remove(&m.id).unwrap_or_default();
            Arc::new(build_msg(ctx, room_id, host, &tok, msg_data(ctx, room_id, m, bs, users), &members, users))
        })
        .collect();''', '''    let _ = host;
    let head = gen::room_head(c, &ctx.abi, room_id).expect("the room");
    let tail = gen::room_tail(c, &ctx.abi, room_id).expect("the room");
    let msgs = db::last_messages(c, room_id)?;
    let frags = msgs.iter().filter_map(|m| build_msg(c, ctx, m.id, &members, users).map(Arc::new)).collect();''')
edit('state.rs', 'use crate::render::{self, Ctx, MsgData, RoomTokens, SideData, SideDirect};', 'use crate::render::{self, Ctx, SideData, SideDirect};')

edit('writer.rs', '''                let data = state::msg_data(&ctx, rid, m, vec![], &next.users);
                let frag = Arc::new(state::build_msg(&ctx, rid, &next.host, &view.tok, data, &view.members, &next.users));''',
     '''                let frag = match state::build_msg(&c, &ctx, m.id, &view.members, &next.users) {
                    Some(f) => Arc::new(f),
                    None => continue,
                };''')
print('ok')
