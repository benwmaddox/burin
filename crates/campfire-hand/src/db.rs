//! The app's queries: each selects only the columns its uses read, and decodes each row
//! straight into a plain struct (no boxed values). This is what the compiler is to generate from
//! the views and the template's uses (DESIGN.md principle 5).

use rusqlite::{params, Connection, OptionalExtension};

pub type Id = i64;

#[derive(Clone, Debug)]
pub struct User {
    pub id: Id,
    pub name: String,
    pub admin: bool,
    pub active: bool,
    pub created: i64,
    pub seq: i64,
}

#[derive(Clone, Debug)]
pub struct Room {
    pub id: Id,
    pub direct: bool,
    pub name: Option<String>,
    pub created: i64,
    pub updated: i64,
}

#[derive(Clone, Debug)]
pub struct Message {
    pub id: Id,
    pub room: Id,
    pub creator: Id,
    pub created: i64,
    pub seq: i64,
    pub body: String,
    pub client_id: String,
    pub has_attachment: bool,
}

#[derive(Clone, Debug)]
pub struct Boost {
    pub id: Id,
    pub message: Id,
    pub booster: Id,
    pub content: String,
}

/// A membership as the sidebar shows it.
#[derive(Clone, Debug)]
pub struct SideRoom {
    pub room: Room,
    pub unread: bool,
}

pub struct Account {
    pub has_logo: bool,
    pub restrict_room_creation: bool,
}

pub fn open(path: &str, write: bool) -> rusqlite::Result<Connection> {
    let c = if write {
        Connection::open(path)?
    } else {
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?
    };
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    if write {
        // as crudspec's writer: WAL, synchronous NORMAL, checkpoints in the background
        c.pragma_update(None, "journal_mode", "WAL")?;
        c.pragma_update(None, "synchronous", "NORMAL")?;
        c.pragma_update(None, "wal_autocheckpoint", 0)?;
    }
    Ok(c)
}

pub fn users(c: &Connection) -> rusqlite::Result<Vec<User>> {
    let mut s = c.prepare_cached("SELECT id, name, role, status, created, seq FROM \"User\" ORDER BY created, seq")?;
    let rows = s.query_map([], |r| {
        Ok(User {
            id: r.get(0)?,
            name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            admin: r.get::<_, Option<String>>(2)?.as_deref() == Some("administrator"),
            active: r.get::<_, Option<String>>(3)?.as_deref().unwrap_or("active") == "active",
            created: r.get(4)?,
            seq: r.get(5)?,
        })
    })?;
    rows.collect()
}

pub fn sessions(c: &Connection) -> rusqlite::Result<Vec<(String, Id)>> {
    let mut s = c.prepare_cached("SELECT token, \"user\" FROM Session WHERE token IS NOT NULL")?;
    let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

pub fn session_user(c: &Connection, token: &str) -> rusqlite::Result<Option<Id>> {
    c.prepare_cached("SELECT \"user\" FROM Session WHERE token = ?")?.query_row([token], |r| r.get(0)).optional()
}

fn room_of(r: &rusqlite::Row) -> rusqlite::Result<Room> {
    Ok(Room { id: r.get(0)?, direct: r.get::<_, Option<String>>(1)?.as_deref() == Some("direct"), name: r.get(2)?, created: r.get(3)?, updated: r.get(4)? })
}

pub fn room(c: &Connection, id: Id) -> rusqlite::Result<Option<Room>> {
    c.prepare_cached("SELECT id, type, name, created, updated FROM Room WHERE id = ?")?.query_row([id], room_of).optional()
}

/// The room created first (`original_room()`).
pub fn original_room(c: &Connection) -> rusqlite::Result<Option<Id>> {
    c.prepare_cached("SELECT id FROM Room ORDER BY created, seq LIMIT 1")?.query_row([], |r| r.get(0)).optional()
}

pub fn message_count(c: &Connection, room: Id) -> rusqlite::Result<i64> {
    c.prepare_cached("SELECT count(*) FROM Message WHERE room = ?")?.query_row([room], |r| r.get(0))
}

/// The room's members' user ids, in order.
pub fn members(c: &Connection, room: Id) -> rusqlite::Result<Vec<Id>> {
    let mut s = c.prepare_cached("SELECT \"user\" FROM Membership WHERE room = ? ORDER BY \"user\"")?;
    let rows = s.query_map([room], |r| r.get(0))?;
    rows.collect()
}

/// The room's last 40 messages, oldest first (`room_messages(room)`: window 40 last).
pub fn last_messages(c: &Connection, room: Id) -> rusqlite::Result<Vec<Message>> {
    let mut s = c.prepare_cached(
        "SELECT id, room, creator, created, seq, body, client_id, attachment IS NOT NULL FROM Message WHERE room = ? ORDER BY created DESC, seq DESC LIMIT 40",
    )?;
    let mut v: Vec<Message> = s
        .query_map([room], |r| {
            Ok(Message {
                id: r.get(0)?,
                room: r.get(1)?,
                creator: r.get(2)?,
                created: r.get(3)?,
                seq: r.get(4)?,
                body: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                client_id: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                has_attachment: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    v.reverse();
    Ok(v)
}

/// The boosts of these messages, in the order each message shows them.
pub fn boosts(c: &Connection, messages: &[Id]) -> rusqlite::Result<Vec<Boost>> {
    if messages.is_empty() {
        return Ok(vec![]);
    }
    let ids = messages.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let mut s = c.prepare_cached("SELECT id, message, booster, content FROM Boost WHERE message IN (SELECT value FROM json_each(?)) ORDER BY created, seq")?;
    let rows = s.query_map(["[".to_string() + &ids + "]"], |r| {
        Ok(Boost { id: r.get(0)?, message: r.get(1)?, booster: r.get(2)?, content: r.get::<_, Option<String>>(3)?.unwrap_or_default() })
    })?;
    rows.collect()
}

/// The user's visible memberships, with their rooms, ordered by the room's lower-cased name
/// (`sidebar_rooms()`).
pub fn sidebar_rooms(c: &Connection, user: Id) -> rusqlite::Result<Vec<SideRoom>> {
    let mut s = c.prepare_cached(
        "SELECT r.id, r.type, r.name, r.created, r.updated, m.unread_since IS NOT NULL FROM Membership m JOIN Room r ON r.id = m.room \
         WHERE m.\"user\" = ? AND m.involvement IS NOT 'invisible' ORDER BY lower(r.name), m.created, m.seq",
    )?;
    let rows = s.query_map([user], |r| Ok(SideRoom { room: room_of(r)?, unread: r.get(5)? }))?;
    rows.collect()
}

/// Up to `limit` other members of a room, in membership order.
pub fn other_members(c: &Connection, room: Id, user: Id, limit: i64) -> rusqlite::Result<Vec<Id>> {
    let mut s = c.prepare_cached("SELECT \"user\" FROM Membership WHERE room = ? AND \"user\" IS NOT ? ORDER BY created, seq LIMIT ?")?;
    let rows = s.query_map(params![room, user, limit], |r| r.get(0))?;
    rows.collect()
}

/// The other members' names, in the order the users were created (display_name's sentence).
pub fn other_member_names_by_user_created(c: &Connection, room: Id, user: Id) -> rusqlite::Result<Vec<String>> {
    let mut s = c.prepare_cached(
        "SELECT u.name FROM Membership m JOIN \"User\" u ON u.id = m.\"user\" WHERE m.room = ? AND m.\"user\" IS NOT ? ORDER BY u.created, u.seq",
    )?;
    let rows = s.query_map(params![room, user], |r| Ok(r.get::<_, Option<String>>(0)?.unwrap_or_default()))?;
    rows.collect()
}

/// Up to 20 active people the user isn't in a direct room with (`direct_placeholders()`).
pub fn direct_placeholders(c: &Connection, user: Id) -> rusqlite::Result<Vec<User>> {
    let in_directs: i64 = c
        .prepare_cached(
            "SELECT count(DISTINCT m.\"user\") FROM Membership m JOIN Room r ON r.id = m.room WHERE r.type = 'direct' \
             AND EXISTS (SELECT 1 FROM Membership x WHERE x.room = m.room AND x.\"user\" = ?)",
        )?
        .query_row([user], |r| r.get(0))?;
    let limit = (20 - in_directs).max(0);
    let mut s = c.prepare_cached(
        "SELECT id, name, role, status, created, seq FROM \"User\" u WHERE u.status = 'active' AND u.id IS NOT ? \
         AND NOT EXISTS (SELECT 1 FROM Membership m JOIN Room r ON r.id = m.room WHERE m.\"user\" = u.id AND r.type = 'direct' \
                         AND EXISTS (SELECT 1 FROM Membership x WHERE x.room = m.room AND x.\"user\" = ?)) \
         ORDER BY created, seq LIMIT ?",
    )?;
    let rows = s.query_map(params![user, user, limit], |r| {
        Ok(User {
            id: r.get(0)?,
            name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            admin: r.get::<_, Option<String>>(2)?.as_deref() == Some("administrator"),
            active: true,
            created: r.get(4)?,
            seq: r.get(5)?,
        })
    })?;
    rows.collect()
}

pub fn account(c: &Connection) -> rusqlite::Result<Account> {
    c.prepare_cached("SELECT logo IS NOT NULL, coalesce(restrict_room_creation, 0) FROM Account LIMIT 1")?
        .query_row([], |r| Ok(Account { has_logo: r.get(0)?, restrict_room_creation: r.get::<_, i64>(1)? != 0 }))
        .optional()
        .map(|a| a.unwrap_or(Account { has_logo: false, restrict_room_creation: false }))
}
