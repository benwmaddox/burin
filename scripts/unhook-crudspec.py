# One-off: etch stands alone (no crudspec modes, headers, form conventions, paths).
import os, re, subprocess
B = 'D:/code/etch/'

def edit(p, old, new, count=1):
    s = open(B + p, encoding='utf-8').read()
    assert s.count(old) == count, (p, old[:80], s.count(old))
    s = s.replace(old, new)
    open(B + p, 'w', encoding='utf-8', newline='\n').write(s)

def sub_all(p, old, new):
    s = open(B + p, encoding='utf-8').read()
    s = s.replace(old, new)
    open(B + p, 'w', encoding='utf-8', newline='\n').write(s)

# ---- the compiler: no crudspec storage mode
edit('crates/etchc/src/actions.rs', '''                                let is_time = matches!(self.m.field_type(&ent, f).as_ref().map(|t| t.strip_opt().clone()), Some(Type::Time));
                                if self.crudspec && is_time {
                                    // crudspec's storage: a time's order column beside it
                                    sets.push((format!("{f}__seq"), Expr::Name("@seq".into(), Span::default())));
                                }
''', '')
edit('crates/etchc/src/actions.rs', '''    let crudspec = matches!(m.consts.get("storage"), Some(Decl::Const { value: Expr::Str(s, _), .. }) if s == "crudspec");
''', '')
sub_all('crates/etchc/src/actions.rs', ', crudspec, creates,', ', creates,')
edit('crates/etchc/src/actions.rs', '''    crudspec: bool,
''', '')
edit('crates/etchc/src/glue.rs', '''    // a database another program owns (crudspec's Campfire): its schema is its own
    if matches!(m.consts.get("storage"), Some(Decl::Const { value: Expr::Str(v, _), .. }) if v == "crudspec") {
        return s;
    }
''', '')

# ---- the runtime: etch's own outcome header; arguments by their names
edit('crates/etch-rt/src/app.rs', '''            // the outcome, under crudspec's header name (its benchmark reads it)
            b.extend_from_slice(b"X-Crudspec-Outcome: ");''', '''            b.extend_from_slice(b"X-Etch-Outcome: ");''')
edit('crates/etch-rt/src/app.rs', '''/// A form field's value (`name`, or crudspec's `__arg_name` with its `#`).
pub fn field<'a>(form: &'a [(String, String)], name: &str) -> Option<&'a str> {
    form.iter()
        .find(|(k, _)| k == name || k.strip_prefix("__arg_") == Some(name))
        .map(|(_, v)| v.trim_start_matches('#'))
}''', '''/// A form field's value: an action's argument by its name.
pub fn field<'a>(form: &'a [(String, String)], name: &str) -> Option<&'a str> {
    form.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}''')

# ---- Campfire's spec: its own app, its own files
os.rename(B + 'apps/campfire/crudspec-compat.etch', B + 'apps/campfire/tokens.etch')
s = open(B + 'apps/campfire/tokens.etch', encoding='utf-8').read()
s = s.split('\n\n// crudspec stores')[0] + '\n'
s = s.replace('''// Compatibility with crudspec's pages, so the two can be compared byte for byte. These are
// crudspec's implementation details (its handler element ids 88, 137 and 85, and its token
// JSON's shapes), not etch's design; nothing else in the app depends on them.''', '''// The signed tokens the room page's forms carry (their shapes follow the reference page this
// Campfire was built to match).''')
open(B + 'apps/campfire/tokens.etch', 'w', encoding='utf-8', newline='\n').write(s)
edit('apps/campfire/room.etch', '''// The room page (crudspec's room_screen, byte for byte without crudspec's live-update markers).''',
     '''// The room page.''')
edit('apps/campfire/room.etch', '''<input type="hidden" name="__arg_room" value="#{{room.id}}">''', '''<input type="hidden" name="room" value="{{room.id}}">''')
edit('apps/campfire/schema.etch', '''// Campfire's data, as the tables crudspec's Campfire keeps (etch reads the same SQLite file).''',
     '''// Campfire's data (data/campfire is a copy of a Campfire database; etch keeps it as its own).''')
edit('apps/campfire/rules.etch', '''  // unread for the members who aren't connected (crudspec's trigger on create Message)''',
     '''  // unread for the members who aren't connected''')

# ---- etch's expected pages: the reference pages as etch's own outputs (live-update markers
# and script gone, the composer's field named as the action's argument)
for g in ['room.html', 'room-jason.html']:
    p = B + 'data/golden-pages/' + g
    t = open(p, encoding='utf-8').read()
    t = re.sub(r'<!--/?f:[^>]*-->', '', t)
    t = re.sub(r'\n<script id="crudspec-page".*?</script><script src="/_live\.[0-9a-f]+\.js" defer></script>', '\n', t, flags=re.S)
    t = re.sub(r'<input type="hidden" name="__arg_room" value="#(\d+)">', r'<input type="hidden" name="room" value="\1">', t)
    open(p, 'w', encoding='utf-8', newline='').write(t)

# ---- scripts: etch's own folders
for f in ['scripts/golden.sh', 'scripts/writes-verify.sh', 'scripts/outcomes.sh', 'scripts/train.sh', 'scripts/cpu-split.sh', 'bench/lifetime.sh', 'bench/writes.sh']:
    sub_all(f, '/mnt/d/code/crudspec-assets/campfire', '/mnt/d/code/etch/apps/campfire/assets')
    sub_all(f, '$HOME/cm258/loadgen-linux/release/loadgen', '/mnt/d/code/etch/tools/loadgen')
    sub_all(f, '$UH/cm258/loadgen-linux/release/loadgen', '/mnt/d/code/etch/tools/loadgen')
sub_all('scripts/cpu-split.sh', 'cd /mnt/d/code/crudspec\n', '')
# golden: an exact comparison with etch's expected pages
s = open(B + 'scripts/golden.sh', encoding='utf-8').read()
s = s.replace('''# Serves crudspec's golden data copy with etch, fetches the room page as both golden viewers, and
# compares them with crudspec's pages (without crudspec's live-update markers and scripts).''', '''# Serves a fresh copy of the golden data, fetches the room page as both golden viewers (cold, fast,
# gzip), and compares them byte for byte with etch's expected pages (data/golden-pages).''')
s = s.replace('''def norm(t):
    t = re.sub(r'<!--/?f:[^>]*-->', '', t)
    t = re.sub(r'\\n<script id="crudspec-page".*?</script><script src="/_live\\.[0-9a-f]+\\.js" defer></script>', '\\n', t, flags=re.S)
    return t''', '''def norm(t):
    return t''')
open(B + 'scripts/golden.sh', 'w', encoding='utf-8', newline='\n').write(s)
sub_all('scripts/outcomes.sh', 'grep -i outcome', 'grep -i x-etch-outcome')
print('ok')
