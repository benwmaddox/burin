# One-off: points crates/campfire-etch at the generated fragments (message, room_head, room_tail).
import re
D = 'D:/code/etch/crates/campfire-etch/'

def edit(f, old, new, count=1):
    p = D + f
    s = open(p, encoding='utf-8').read()
    assert s.count(old) == count, (f, old[:70], s.count(old))
    s = s.replace(old, new)
    open(p, 'w', encoding='utf-8', newline='\n').write(s)

# users carry their role as text (the viewer record has it)
edit('src/db.rs', '    pub admin: bool,\n    pub active: bool,', '    pub admin: bool,\n    pub role: String,\n    pub active: bool,')
edit('src/db.rs', '''            admin: r.get::<_, Option<String>>(2)?.as_deref() == Some("administrator"),
            active: r.get::<_, Option<String>>(3)?.as_deref().unwrap_or("active") == "active",''',
     '''            admin: r.get::<_, Option<String>>(2)?.as_deref() == Some("administrator"),
            role: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            active: r.get::<_, Option<String>>(3)?.as_deref().unwrap_or("active") == "active",''')
edit('src/db.rs', '''            admin: r.get::<_, Option<String>>(2)?.as_deref() == Some("administrator"),
            active: true,''', '''            admin: r.get::<_, Option<String>>(2)?.as_deref() == Some("administrator"),
            role: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            active: true,''')

# the start-time context the generated code reads
edit('src/render.rs', '''pub struct Ctx {
    pub signer: Signer,''', '''pub struct Ctx {
    /// what generated fragments read: the secret, the assets, the configs (host, vapid_public)
    pub abi: etch_rt::abi::Ctx,
    pub signer: Signer,''')

edit('src/lib.rs', 'pub mod db;\n', 'pub mod db;\npub mod gen;\n')

# state: generated builders and the generated variant key
edit('src/state.rs', '''pub fn build_msg(ctx: &Ctx, room: Id, host: &str, tok: &RoomTokens, data: MsgData, members: &[Id], users: &HashMap<Id, User>) -> MsgFrag {
    let mut kinds: Vec<MsgViewer> = vec![];
    let mut variants = vec![];
    let mut by_member = Vec::with_capacity(members.len());
    for uid in members {
        let v = match users.get(uid) {
            Some(u) => render::viewer_of(&data, u),
            None => MsgViewer { me: false, admin: false, booster: None },
        };
        let i = match kinds.iter().position(|k| *k == v) {
            Some(i) => i,
            None => {
                kinds.push(v);
                variants.push(render::message(ctx, room, host, tok, &data, v));
                kinds.len() - 1
            }
        };''', '''pub fn build_msg(ctx: &Ctx, _room: Id, _host: &str, _tok: &RoomTokens, data: MsgData, members: &[Id], users: &HashMap<Id, User>) -> MsgFrag {
    // the variant key and the bytes come from the generated code (etch_bits_message,
    // etch_build_message): one variant per distinct key among the audience
    let mut kinds: Vec<i64> = vec![];
    let mut variants = vec![];
    let mut by_member = Vec::with_capacity(members.len());
    for uid in members {
        let v = match users.get(uid) {
            Some(u) => gen::viewer(u),
            None => gen::Viewer { viewer: *uid, viewer_role: etch_rt::abi::Str::NONE },
        };
        let k = gen::message_key(&data, &v);
        let i = match kinds.iter().position(|x| *x == k) {
            Some(i) => i,
            None => {
                kinds.push(k);
                variants.push(gen::message(&ctx.abi, &data, &v));
                kinds.len() - 1
            }
        };''')
edit('src/state.rs', 'use crate::render::{self, Ctx, MsgData, MsgViewer, RoomTokens, SideData, SideDirect};',
     'use crate::gen;\nuse crate::render::{self, Ctx, MsgData, RoomTokens, SideData, SideDirect};')
edit('src/state.rs', '''    let title = room.name.clone().unwrap_or_default();
    let head = render::room_head(ctx, room_id, &title, room.direct, account.has_logo);
    let tail = render::room_tail(ctx, room_id, &tok);''', '''    assert!(!account.has_logo, "phase 1: an account logo isn't supported");
    let head = gen::room_head(&ctx.abi, room_id, room.name.as_deref());
    let tail = gen::room_tail(&ctx.abi, room_id);''')

# main: the context's generated half
edit('src/main.rs', '''    let ctx = Arc::new(render::Ctx { signer: Signer::new(&secret), assets: asset_urls(Path::new(&assets)), vapid_public });''',
     '''    let assets = asset_urls(Path::new(&assets));
    let abi = etch_rt::abi::Ctx { signer: Signer::new(&secret), assets: assets.clone(), configs: vec![host.clone(), vapid_public.clone()] };
    let ctx = Arc::new(render::Ctx { abi, signer: Signer::new(&secret), assets, vapid_public });''')
print('ok')
