//! The room page's fragments, built from rows. Each builder runs when data changes (the writer)
//! or on the cold path, never per request on the fast path. Holes are marked by their class
//! (DESIGN.md principle 1): [row] from the row, [viewer] a finite per-viewer input (a variant),
//! [room] constant per room (its signed tokens), [const] constant for the app.
//!
//! The HTML is crudspec's room page, byte for byte, without crudspec's live-update markers.

use crate::consts::{FORMAT_SCRIPT, JUMP_SCRIPT};
use crate::db::{Boost, Id, Message, SideRoom, User};
use etch_rt::html::{autolink, esc, esc_string, json_string};
use etch_rt::sign::Signer;
use etch_rt::time;
use std::collections::HashMap;

/// What is fixed when the server starts: the secret, the assets' addresses, the push key.
pub struct Ctx {
    /// what generated fragments read: the secret, the assets, the configs (host, vapid_public)
    pub abi: etch_rt::abi::Ctx,
    pub signer: Signer,
    pub assets: HashMap<String, String>,
    pub vapid_public: String,
}

impl Ctx {
    /// An asset's address, with its content's hash (crudspec's AssetURL).
    pub fn asset(&self, name: &str) -> String {
        self.assets.get(name).cloned().unwrap_or_else(|| "/assets/".to_string() + name)
    }

    fn icon(&self, b: &mut Vec<u8>, name: &str) {
        put(b, "<img src=\"");
        put(b, &self.asset(name));
        put(b, "\" width=\"20\" height=\"20\" aria-hidden=\"true\" class=\"colorize--black\">");
    }
}

pub const REACTIONS: [(&str, &str); 8] = [
    ("👍", "Thumbs up"),
    ("👏", "Clapping"),
    ("👋", "Waving hand"),
    ("💪", "Muscle"),
    ("❤️", "Red heart"),
    ("😂", "Face with tears of joy"),
    ("🎉", "Party popper"),
    ("🔥", "Fire"),
];

fn put(b: &mut Vec<u8>, s: &str) {
    b.extend_from_slice(s.as_bytes());
}

fn int(b: &mut Vec<u8>, n: i64) {
    put(b, &n.to_string());
}

/// The room's signed tokens: constant per room (the page's path is in them).
#[derive(Clone)]
pub struct RoomTokens {
    /// handlers with no element ids (boost, delete, unboost, create_direct_room)
    pub plain: String,
    /// the composer's post_message form
    pub composer: String,
    /// the composer's mention picker
    pub picker: String,
}

pub fn room_tokens(ctx: &Ctx, room: Id) -> RoomTokens {
    let path = format!("/rooms/{room}");
    RoomTokens {
        plain: ctx.signer.token(&format!(r#"{{"e":[],"p":{},"v":[]}}"#, json_string(&path))),
        composer: ctx.signer.token(&format!(r#"{{"e":[88],"p":{},"v":[["room",{{"ref":"Room","id":{room}}}]]}}"#, json_string(&path))),
        picker: ctx.signer.token(&format!(r#"{{"e":85,"v":[["room",{{"id":{room},"ref":"Room"}}]]}}"#)),
    }
}

/// The edit form's token: per message.
pub fn edit_token(ctx: &Ctx, room: Id, message: Id) -> String {
    ctx.signer.token(&format!(r#"{{"e":[137],"p":"/rooms/{room}","v":[["m",{{"ref":"Message","id":{message}}}]]}}"#))
}

/// What a viewer changes in a message: the finite inputs that pick a variant.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MsgViewer {
    /// [viewer] the message is the viewer's
    pub me: bool,
    /// [viewer] can_administer(viewer, creator): may edit and delete it
    pub admin: bool,
    /// [viewer] the viewer, if they boosted it (their boosts get a remove button)
    pub booster: Option<Id>,
}

/// The data a message's fragment is built from, kept so its variants can be built later.
pub struct MsgData {
    pub m: Message,
    pub creator_name: String,
    pub boosts: Vec<(Boost, String)>,
    pub edit_token: String,
}

/// plain_body(m)
fn plain_body(m: &Message) -> &str {
    &m.body
}

pub fn message(ctx: &Ctx, room: Id, host: &str, tok: &RoomTokens, d: &MsgData, v: MsgViewer) -> Vec<u8> {
    let m = &d.m;
    let mut b = Vec::with_capacity(9000);
    let iso = time::iso_utc(m.created);
    put(&mut b, "<div id=\"message_");
    esc(&mut b, &m.client_id); // [row]
    put(&mut b, "\" class=\"message message--formatted");
    if v.me {
        put(&mut b, " message--me"); // [viewer]
    }
    put(&mut b, "\" data-mentioned=\"false\" data-message-id=\""); // mentions: none in phase 1a
    int(&mut b, m.id);
    put(&mut b, "\" data-user-id=\"");
    int(&mut b, m.creator);
    put(&mut b, "\"><div class=\"message__day-separator\"><time class=\"message__timestamp\" datetime=\"");
    put(&mut b, &iso);
    put(&mut b, "\">");
    esc(&mut b, &time::date(m.created));
    put(&mut b, "</time></div><div class=\"avatar message__avatar\"><a href=\"/users/");
    int(&mut b, m.creator);
    put(&mut b, "\" class=\"btn avatar\" title=\"");
    esc(&mut b, &d.creator_name);
    put(&mut b, "\"><img aria-hidden=\"true\" src=\"/avatars/User/");
    int(&mut b, m.creator);
    put(&mut b, "\" width=\"48\" height=\"48\"></a></div><div class=\"message__body\"><div class=\"message__body-content\"><div class=\"message__meta\"><div class=\"message__heading\"><div class=\"message__author\"><strong class=\"\" title=\"\">");
    esc(&mut b, &d.creator_name);
    put(&mut b, "</strong></div><a href=\"/rooms/");
    int(&mut b, room);
    put(&mut b, "?at=");
    int(&mut b, m.id);
    put(&mut b, "\" class=\"message__permalink\"><time class=\"message__timestamp\" datetime=\"");
    put(&mut b, &iso);
    put(&mut b, "\">");
    esc(&mut b, &time::clock(m.created));
    put(&mut b, "</time></a></div><div class=\"message__actions\"><details class=\"position-relative\"><summary class=\"btn message__action-btn message__options-btn\">");
    ctx.icon(&mut b, "menu-dots-horizontal.svg");
    put(&mut b, "<span class=\"for-screen-reader\">Message options</span></summary><div class=\"message__actions-menu border shadow\"><div class=\"quick-boosts\">");
    for (ch, title) in REACTIONS {
        put(&mut b, "<form method=\"post\" action=\"/_do/boost\" class=\"flex-inline\" ><input type=\"hidden\" name=\"__arg_message\" value=\"#");
        int(&mut b, m.id);
        put(&mut b, "\"><input type=\"hidden\" name=\"__arg_content\" value=\"");
        put(&mut b, ch);
        put(&mut b, "\"><input type=\"hidden\" name=\"__h\" value=\"");
        put(&mut b, &tok.plain); // [room]
        put(&mut b, "\"><button type=\"submit\" class=\"btn\">");
        put(&mut b, ch);
        put(&mut b, "<span class=\"for-screen-reader\">");
        put(&mut b, title);
        put(&mut b, "</span></button></form>");
    }
    put(&mut b, "<details class=\"position-relative\"><summary class=\"message__action-btn\">");
    ctx.icon(&mut b, "boost.svg");
    put(&mut b, "<span class=\"for-screen-reader\">New boost</span></summary><div class=\"message__actions-menu border shadow\"><form method=\"post\" action=\"/_do/boost\" enctype=\"multipart/form-data\" class=\"boost__form flex align-center gap\" id=\"\"><input type=\"hidden\" name=\"__arg_message\" value=\"#");
    int(&mut b, m.id);
    put(&mut b, "\"><input type=\"hidden\" name=\"__h\" value=\"");
    put(&mut b, &tok.plain);
    put(&mut b, "\"><fieldset  class=\"flex flex-column gap borderless unpad\"><label class=\"flex align-center gap input input--actor\"><input type=\"text\" name=\"content\" class=\"input\" required maxlength=\"16\" value=\"\"></label><button type=\"submit\" class=\"btn btn--reversed center\">");
    ctx.icon(&mut b, "check.svg");
    put(&mut b, "<span class=\"for-screen-reader\">Submit</span></button></fieldset></form></div></details></div><div class=\"flex align-center gap\">");
    // reply (no attachments in phase 1a)
    // crudspec's reply widget: every line quoted
    let quote = json_string(&format!("> {}\n\n", plain_body(m).replace('\n', "\n> ")));
    // the constant code is written as it is; only the hole is escaped
    put(&mut b, "<button type=\"button\" class=\"btn\" onclick=\"const t=document.querySelector('#composer textarea');if(t){t.value=");
    esc(&mut b, &quote);
    put(&mut b, "+t.value;t.focus()}\">");
    ctx.icon(&mut b, "reply.svg");
    put(&mut b, "<span class=\"for-screen-reader\">Reply</span></button>");
    // copy link: the request's host is a hole, fixed per snapshot (the configured host)
    let link = json_string(&format!("http://{host}/rooms/{room}?at={}", m.id));
    put(&mut b, "<button type=\"button\" class=\"btn\" onclick=\"navigator.clipboard.writeText(");
    esc(&mut b, &link);
    put(&mut b, ")\">");
    ctx.icon(&mut b, "link.svg");
    put(&mut b, "<span class=\"for-screen-reader\">Copy link</span></button>");
    if v.admin {
        put(&mut b, "<details class=\"position-relative\"><summary class=\"message__action-btn\">");
        ctx.icon(&mut b, "pencil.svg");
        put(&mut b, "<span class=\"for-screen-reader\">Edit</span></summary><div class=\"message__actions-menu border shadow\"><form method=\"post\" action=\"/_do/edit_message\" enctype=\"multipart/form-data\" class=\"flex flex-column gap\" id=\"\"><input type=\"hidden\" name=\"__arg_message\" value=\"#");
        int(&mut b, m.id);
        put(&mut b, "\"><input type=\"hidden\" name=\"__h\" value=\"");
        put(&mut b, &d.edit_token); // [row]
        put(&mut b, "\"><fieldset  class=\"flex flex-column gap borderless unpad\"><label class=\"flex align-center gap input input--actor\"><textarea name=\"body\" class=\"input\" rows=\"1\" data-richtext   onkeydown=\"if(event.key==='Enter'&&!event.shiftKey&&!this.dataset.picking){event.preventDefault();this.form.requestSubmit()}\">");
        esc(&mut b, &m.body);
        put(&mut b, "</textarea></label><button type=\"submit\" class=\"btn btn--reversed center\">");
        ctx.icon(&mut b, "check.svg");
        put(&mut b, "<span class=\"for-screen-reader\">Save</span></button></fieldset></form><form method=\"post\" action=\"/_do/delete_message\" class=\"flex-inline\" onsubmit=\"return confirm(&quot;Are you sure?&quot;)\"><input type=\"hidden\" name=\"__arg_message\" value=\"#");
        int(&mut b, m.id);
        put(&mut b, "\"><input type=\"hidden\" name=\"__h\" value=\"");
        put(&mut b, &tok.plain);
        put(&mut b, "\"><button type=\"submit\" class=\"btn\">");
        ctx.icon(&mut b, "trash.svg");
        put(&mut b, "<span class=\"for-screen-reader\">Delete</span></button></form></div></details>");
    }
    put(&mut b, "</div></div></details></div></div><div id=\"presentation_message_");
    int(&mut b, m.id);
    put(&mut b, "\" class=\"message__presentation\" dir=\"auto\">");
    put(&mut b, &autolink(&esc_string(plain_body(m))));
    put(&mut b, "</div><div class=\"boosts flex flex-wrap align-center gap full-width\"><div class=\"\" data-live-list>");
    for (bo, name) in &d.boosts {
        put(&mut b, "<div class=\"boost\"><figure class=\"avatar \"><img src=\"/avatars/User/");
        int(&mut b, bo.booster);
        put(&mut b, "\" alt=\"");
        esc(&mut b, &format!("{name} boosted {}", bo.content));
        put(&mut b, "\"></figure><span class=\"\">");
        esc(&mut b, &bo.content);
        put(&mut b, "</span>");
        if v.booster == Some(bo.booster) {
            put(&mut b, "<form method=\"post\" action=\"/_do/unboost\" class=\"flex-inline\" ><input type=\"hidden\" name=\"__arg_boost\" value=\"#");
            int(&mut b, bo.id);
            put(&mut b, "\"><input type=\"hidden\" name=\"__h\" value=\"");
            put(&mut b, &tok.plain);
            put(&mut b, "\"><button type=\"submit\" class=\"btn\">");
            ctx.icon(&mut b, "minus.svg");
            put(&mut b, "<span class=\"for-screen-reader\">Delete this boost</span></button></form>");
        }
        put(&mut b, "</div>");
    }
    put(&mut b, "</div></div></div></div></div>");
    b
}

/// The variant inputs of a message for a viewer.
pub fn viewer_of(d: &MsgData, viewer: &User) -> MsgViewer {
    MsgViewer {
        me: d.m.creator == viewer.id,
        admin: viewer.admin || d.m.creator == viewer.id,
        booster: d.boosts.iter().any(|(b, _)| b.booster == viewer.id).then_some(viewer.id),
    }
}

/// The page up to the first message: the head, the nav, and the message area's start.
/// [room] for open and closed rooms; a direct room's title depends on the viewer.
pub fn room_head(ctx: &Ctx, room: Id, title: &str, direct: bool, has_logo: bool) -> Vec<u8> {
    let mut b = Vec::with_capacity(4096);
    put(&mut b, "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"color-scheme\" content=\"light dark\">\n<title>");
    esc(&mut b, title);
    put(&mut b, "</title>");
    for css in STYLESHEETS {
        put(&mut b, "<link rel=\"stylesheet\" href=\"");
        esc(&mut b, &ctx.asset(css));
        put(&mut b, "\">");
    }
    put(&mut b, "<link rel=\"alternate\" type=\"text/markdown\" href=\"/llms.txt\" title=\"For AI agents\"></head>\n<body class=\"sidebar\">\n<a href=\"#main-content\" class=\"skip-navigation btn\">Skip to main content</a>\n<nav id=\"nav\">");
    assert!(!has_logo, "phase 1a: an account logo isn't supported");
    put(&mut b, "<div class=\"btn btn--reversed btn--faux room--current\"><h1 class=\"room__contents txt-medium overflow-ellipsis\">");
    esc(&mut b, title);
    put(&mut b, "</h1></div><a href=\"/rooms/");
    int(&mut b, room);
    put(&mut b, "/edit\" class=\"btn\">");
    ctx.icon(&mut b, "menu-dots-horizontal.svg");
    let kind = if direct { "Ping" } else { "room" };
    put(&mut b, "<span class=\"for-screen-reader\">Settings for this ");
    put(&mut b, kind);
    put(&mut b, "</span></a><button type=\"button\" class=\"btn\" hidden data-push-subscribe=\"/_do/subscribe_push\" data-push-params=\"endpoint,p256dh_key,auth_key\" data-push-key=\"");
    esc(&mut b, &ctx.vapid_public);
    put(&mut b, "\">");
    ctx.icon(&mut b, "notification-bell-alert.svg");
    put(&mut b, "<span data-push-label class=\"for-screen-reader\">Notification settings for this ");
    put(&mut b, kind);
    put(&mut b, "</span></button></nav>\n\n<main id=\"main-content\"><section class=\"message-area\" id=\"message-area\"><div class=\"messages\" data-live-list>");
    b
}

const STYLESHEETS: [&str; 26] = [
    "_reset.css", "actiontext.css", "animation.css", "autocomplete.css", "avatars.css", "base.css", "boosts.css", "buttons.css",
    "code.css", "colorize.css", "colors.css", "composer.css", "embeds.css", "filters.css", "flash.css", "inputs.css", "layout.css",
    "lightbox.css", "messages.css", "nav.css", "panels.css", "separators.css", "sidebar.css", "signup.css", "spinner.css", "utilities.css",
];

/// From the end of the messages to the end of `<main>`: the jump button, the scripts, and the
/// composer. [room]
pub fn room_tail(ctx: &Ctx, room: Id, tok: &RoomTokens) -> Vec<u8> {
    let mut b = Vec::with_capacity(4096);
    put(&mut b, "</div><button type=\"button\" class=\"message-area__return-to-latest btn\" hidden onclick=\"document.querySelector('.messages')?.lastElementChild?.scrollIntoView({behavior:'smooth'})\"><img src=\"");
    put(&mut b, &ctx.asset("arrow-down.svg"));
    put(&mut b, "\" width=\"20\" height=\"20\" aria-hidden=\"true\"><span class=\"for-screen-reader\">Jump to newest message</span></button>");
    put(&mut b, JUMP_SCRIPT);
    put(&mut b, FORMAT_SCRIPT);
    put(&mut b, "</section><footer id=\"footer\"><div class=\"composer flex align-end gap position-relative\"><a href=\"/searches\" class=\"btn flex-item-no-shrink composer__context-btn\">");
    ctx.icon(&mut b, "search.svg");
    put(&mut b, "<span class=\"for-screen-reader\">Search</span></a><form method=\"post\" action=\"/_do/post_message\" enctype=\"multipart/form-data\" class=\"margin-block flex-item-grow contain\" id=\"composer\"><input type=\"hidden\" name=\"__arg_room\" value=\"#");
    int(&mut b, room);
    put(&mut b, "\"><input type=\"hidden\" name=\"__h\" value=\"");
    put(&mut b, &tok.composer);
    put(&mut b, "\"><fieldset  style=\"display: contents\"><div class=\"flex composer__input input input--actor fill-white min-width\">");
    ctx.icon(&mut b, "messages-outlined.svg");
    put(&mut b, "<label class=\"flex-item-grow min-width\"><textarea name=\"body\" class=\"input\" rows=\"1\" data-richtext data-mentions-src=\"/_pick?ctx=");
    put(&mut b, &tok.picker);
    put(&mut b, "\" data-unfurl-src=\"/_view/unfurl_link?link=\" onkeydown=\"if(event.key==='Enter'&&!event.shiftKey&&!this.dataset.picking){event.preventDefault();this.form.requestSubmit()}\"></textarea><input type=\"hidden\" name=\"body__embeds\" value=\"[]\"><div class=\"embed-previews\"></div><input type=\"hidden\" name=\"body__mentions\" value=\"\"><input type=\"hidden\" name=\"body__mentions_entity\" value=\"User\"><div class=\"mention-picker\" hidden style=\"position:absolute;bottom:100%;background:var(--color-bg,#fff);border:1px solid #ccc;border-radius:.5em;padding:.25em;z-index:10\"></div><span class=\"for-screen-reader\">Write a message</span></label><label class=\"btn btn--borderless txt-small flex-item-no-shrink input--file\"><img src=\"");
    put(&mut b, &ctx.asset("attachment.svg"));
    put(&mut b, "\" width=\"24\" height=\"24\" aria-hidden=\"true\" class=\"colorize--black\"><input type=\"file\" name=\"attachment\"  ><span class=\"for-screen-reader\">Attach a file</span></label><button type=\"submit\" class=\"btn btn--reversed center\">");
    ctx.icon(&mut b, "arrow-up.svg");
    put(&mut b, "<span class=\"for-screen-reader\">Send Message</span></button></div></fieldset></form><div class=\"typing-indicator gap txt-small align-center flex-inline\" data-signal=\"typing:");
    int(&mut b, room);
    put(&mut b, "\" data-indicator data-active-class=\"typing-indicator--active\" aria-live=\"polite\"><div class=\"typing-indicator__author spinner\"></div></div></div></footer></main>\n");
    b
}

/// One direct room in the sidebar.
pub struct SideDirect {
    pub room: SideRoom,
    pub avatars: Vec<Id>,
    pub label: String,
}

/// The sidebar's data for one user.
pub struct SideData {
    pub user: Id,
    pub directs: Vec<SideDirect>,
    pub rooms: Vec<SideRoom>,
    pub placeholders: Vec<User>,
    pub may_create: bool,
}

/// The sidebar, in pieces: between each two pieces goes the page's plain token ([room]: the
/// path is in it). [viewer] per user.
pub fn sidebar(ctx: &Ctx, d: &SideData) -> Vec<Vec<u8>> {
    let mut pieces = vec![];
    let mut b = Vec::with_capacity(4096);
    put(&mut b, "<aside id=\"sidebar\"><div class=\"sidebar__container overflow-y overflow-hide-scrollbar\"><div class=\"directs gap overflow-x overflow-hide-scrollbar\"><a href=\"/rooms/directs/new\" class=\"direct direct__new\"><div class=\"avatar avatar--icon\">");
    ctx.icon(&mut b, "messages-add.svg");
    put(&mut b, "</div><div class=\"direct__author flex max-width min-width border-radius pad-inline-half\"><span class=\"for-screen-reader\">New</span><span class=\"txt-small overflow-clip\">Ping</span></div></a><div class=\"\" data-live-list>");
    for dr in &d.directs {
        put(&mut b, "<a href=\"/rooms/");
        int(&mut b, dr.room.room.id);
        put(&mut b, "\" class=\"direct");
        if dr.room.unread {
            put(&mut b, " unread");
        }
        put(&mut b, "\"><div class=\"avatar__group\">");
        for u in &dr.avatars {
            put(&mut b, "<span class=\"avatar\"><img src=\"/avatars/User/");
            int(&mut b, *u);
            put(&mut b, "\" alt=\"\"></span>");
        }
        put(&mut b, "</div><div class=\"direct__author flex align-center gap max-width min-width border-radius txt-small\"><span class=\"txt-nowrap overflow-ellipsis\">");
        esc(&mut b, &dr.label);
        put(&mut b, "</span></div></a>");
    }
    put(&mut b, "</div><div class=\"\" data-live-list>");
    for u in &d.placeholders {
        put(&mut b, "<form method=\"post\" action=\"/_do/create_direct_room\" class=\"flex-inline\" ><input type=\"hidden\" name=\"__arg_users\" value=\"@");
        int(&mut b, u.id);
        put(&mut b, "\"><input type=\"hidden\" name=\"__h\" value=\"");
        pieces.push(std::mem::take(&mut b)); // the page's plain token goes here
        put(&mut b, "\"><button type=\"submit\" class=\"direct borderless fill-transparent unpad\"><div class=\"avatar\"><img aria-hidden=\"true\" src=\"/avatars/User/");
        int(&mut b, u.id);
        put(&mut b, "\" width=\"48\" height=\"48\"></div><div class=\"direct__author flex align-center gap max-width min-width border-radius txt-small\"><div class=\"txt-nowrap overflow-ellipsis\"><span class=\"for-screen-reader\">Start a ping with</span><span class=\"\">");
        esc(&mut b, first_word(&u.name));
        put(&mut b, "</span></div></div></button></form>");
    }
    put(&mut b, "</div></div><div class=\"rooms position-relative flex flex-column gap\"><div class=\"\" data-live-list>");
    for r in &d.rooms {
        put(&mut b, "<a href=\"/rooms/");
        int(&mut b, r.room.id);
        put(&mut b, "\" class=\"align-center gap room btn txt-nowrap");
        if r.unread {
            put(&mut b, " unread");
        }
        put(&mut b, "\"><span class=\"overflow-ellipsis\">");
        esc(&mut b, r.room.name.as_deref().unwrap_or(""));
        put(&mut b, "</span></a>");
    }
    put(&mut b, "</div>");
    if d.may_create {
        put(&mut b, "<a href=\"/rooms/opens/new\" class=\"rooms__new-btn btn room align-center gap txt-reversed\" title=\"New Chat Room\">");
        ctx.icon(&mut b, "add.svg");
        put(&mut b, "</a>");
    }
    put(&mut b, "</div></div><div class=\"flex align-end sidebar__tools gap justify-end\"><a href=\"/users/me/profile\" class=\"btn avatar flex-item-no-shrink sidebar__tool\"><img aria-hidden=\"true\" src=\"/avatars/User/");
    int(&mut b, d.user);
    put(&mut b, "\" width=\"48\" height=\"48\"><span class=\"for-screen-reader\">My Settings</span></a><a href=\"/account/edit\" class=\"btn align-center gap txt-reversed sidebar__tool\">");
    ctx.icon(&mut b, "settings.svg");
    put(&mut b, "<span class=\"for-screen-reader\">Account Settings</span></a></div></aside>\n");
    pieces.push(b);
    pieces
}

/// The end of every page. [const]
pub fn page_end(ctx: &Ctx) -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, "<a href=\"https://once.com\" id=\"app-logo\" target=\"_blank\" aria-label=\"Once software from 37signals home page\"><img src=\"");
    put(&mut b, &ctx.asset("campfire-icon.png"));
    put(&mut b, "\" alt=\"Campfire logo\"></a>\n</body></html>");
    b
}

pub fn first_word(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

fn initials(s: &str) -> String {
    s.split_whitespace().filter_map(|w| w.chars().next()).collect()
}

/// "a", "a and b", "a, b, and c"
pub fn sentence(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        2 => format!("{} and {}", items[0], items[1]),
        n => format!("{}, and {}", items[..n - 1].join(", "), items[n - 1]),
    }
}

/// direct_label(r): initials when there are several others, else the first word of the room's
/// display name.
pub fn direct_label(other_names_by_membership: &[String], other_names_by_created: &[String], viewer_name: &str) -> String {
    if other_names_by_membership.len() > 1 {
        let inits: Vec<String> = other_names_by_membership.iter().map(|n| initials(n)).collect();
        sentence(&inits)
    } else {
        let display = if other_names_by_created.is_empty() { viewer_name.to_string() } else { sentence(other_names_by_created) };
        first_word(&display).to_string()
    }
}
