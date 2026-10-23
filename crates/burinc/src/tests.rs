//! The checker's refusals: each spec breaks one rule and must be refused, naming why.

use crate::ast::App;
use crate::{check, parse};

const BASE: &str = r#"
entity User { name: Text }
entity Post { author: User  body: Text }
entity Session { user: User  token: Text }
viewer User from cookie session_token = Session.token -> .user
view recent() = Post order by .created window 20 last
view all_posts() = Post order by .created
"#;

/// A spec's app, with the standard library every app has.
fn app_of(src: &str) -> Result<App, String> {
    let mut decls = parse::parse_file(src, 0).map_err(|e| format!("parse: {}", e.msg))?;
    decls.extend(parse::prelude(1).map_err(|e| format!("the prelude: {}", e.msg))?);
    Ok(App { files: vec!["t.burin".into(), parse::PRELUDE_NAME.into()], decls })
}

fn verdict(extra: &str) -> Result<(), String> {
    let src = format!("{BASE}\n{extra}");
    let app = app_of(&src)?;
    let m = check::Model::new(&app).map_err(|e| e.msg)?;
    check::check(&m).map_err(|e| e.msg)
}

#[test]
fn a_good_page_passes() {
    verdict(r#"fragment p() <<<{{for p in recent()}}<p title="{{p.author.name}}" onclick="go({{json(p.body)}})">{{p.body}}</p>{{end}}>>>"#).unwrap();
}

#[test]
fn text_in_an_event_handler_is_refused() {
    let e = verdict(r#"fragment p() <<<{{for p in recent()}}<p onclick="go('{{p.body}}')">x</p>{{end}}>>>"#).unwrap_err();
    assert!(e.contains("event handler takes only json"), "{e}");
}

#[test]
fn an_unbounded_list_is_refused() {
    let e = verdict(r#"fragment p() <<<{{for p in all_posts()}}<p>{{p.body}}</p>{{end}}>>>"#).unwrap_err();
    assert!(e.contains("bounded"), "{e}");
}

#[test]
fn html_in_an_attribute_is_refused() {
    let e = verdict(r#"fragment p() <<<{{for p in recent()}}<p title="{{autolink(p.body)}}">x</p>{{end}}>>>"#).unwrap_err();
    assert!(e.contains("Html goes only in text"), "{e}");
}

#[test]
fn a_hole_in_a_tag_is_refused() {
    let e = verdict(r#"fragment p() <<<{{for p in recent()}}<p {{p.body}}>x</p>{{end}}>>>"#).unwrap_err();
    assert!(e.contains("quoted attribute"), "{e}");
}

#[test]
fn an_if_that_ends_in_different_places_is_refused() {
    let e = verdict(r#"fragment p() <<<{{for p in recent()}}{{if p.author == viewer}}<b class="{{end}}x{{end}}>>>"#).unwrap_err();
    assert!(e.contains("different places"), "{e}");
}

#[test]
fn a_template_ending_in_gt_keeps_it() {
    let decls = parse::parse_file("fragment f() <<<<b>x</b>>>>", 0).ok().unwrap();
    match &decls[0] {
        crate::ast::Decl::Fragment { body, .. } => match &body.nodes[0] {
            crate::ast::TNode::Text(t) => assert_eq!(t, "<b>x</b>"),
            _ => panic!(),
        },
        _ => panic!(),
    }
}

#[test]
fn a_delete_leaving_references_is_refused() {
    let e = verdict("action drop(u: User) {\n  delete u\n}").unwrap_err();
    assert!(e.contains("would leave Post.author referring to it"), "{e}");
}

#[test]
fn a_delete_after_its_references_go_passes() {
    verdict("action drop(u: User) {\n  for each p in Post where .author == u { delete p }\n  for each s in Session where .user == u { delete s }\n  delete u\n}").unwrap();
}

/// The registers' lowering of a spec (the BASE, a fragment): its error, if any.
fn lowered(extra: &str, frag: &str) -> Result<(), String> {
    let src = format!("{BASE}\n{extra}");
    let app = app_of(&src)?;
    let m = check::Model::new(&app).map_err(|e| e.msg)?;
    check::check(&m).map_err(|e| e.msg)?;
    crate::llvm::module(&m, &[frag], "", "").map(|_| ())
}

#[test]
fn a_cached_fragment_reading_the_viewer_is_refused() {
    // text and conditions alike: a cached fragment is the same for everyone
    let e = lowered(r#"fragment p(x: Post) <<<<b>{{viewer.name}}</b>{{x.body}}>>>"#, "p").unwrap_err();
    assert!(e.contains("a cached fragment is the same for everyone"), "{e}");
    let e = lowered(r#"fragment p(x: Post) <<<{{if x.author == viewer}}<b>mine</b>{{end}}{{x.body}}>>>"#, "p").unwrap_err();
    assert!(e.contains("mark it `per viewer`"), "{e}");
}

#[test]
fn a_per_request_fragment_merges_the_cached_ones_it_calls() {
    // the post is a cached piece, merged by reference where the per-request list calls it
    let src = "fragment post(x: Post) <<<<p>{{x.body}}</p>>>>
fragment mine() per viewer <<<{{for x in recent()}}{{if x.author == viewer}}{{post(x)}}{{end}}{{end}}>>>";
    let decls = parse::parse_file(&format!("{BASE}
{src}"), 0).map_err(|e| e.msg).unwrap();
    let app = App { files: vec!["t.burin".into()], decls };
    let m = check::Model::new(&app).map_err(|e| e.msg).unwrap();
    check::check(&m).map_err(|e| e.msg).unwrap();
    let (ll, _) = crate::llvm::module(&m, &["post", "mine"], "", "").unwrap();
    assert!(ll.contains("call void @burin_piece(ptr %splits"), "no merged piece");
}

#[test]
fn varies_by_is_gone() {
    let e = verdict(r#"fragment p(x: Post) varies by viewer <<<{{x.body}}>>>"#).unwrap_err();
    assert!(e.contains("a cached fragment is the same for everyone"), "{e}");
}

#[test]
fn a_public_action_reading_the_viewer_is_refused() {
    let e = verdict("action p(b: Text) public {\n  create Post { author: viewer, body: b }\n}").unwrap_err();
    assert!(e.contains("public action"), "{e}");
}

#[test]
fn a_password_in_a_page_is_refused() {
    let src = "entity Login { user: User  password: Password }\nfragment p(l: Login) <<<<b>{{l.password}}</b>>>>";
    let e = verdict(src).unwrap_err();
    assert!(e.contains("Password can't go in text"), "{e}");
}

#[test]
fn signing_in_and_out_pass() {
    verdict("entity Login { user: User  password: Password? }\naction sign_in(u: User, p: Text) public {\n  let l = first(Login where .user == u)\n  require l != none and authenticates(l.password, p) else unauthenticated\n  sign in u\n}\naction sign_out() {\n  sign out\n}").unwrap();
}

// Numbers and time: an Int, a Time (an instant) and a Duration (a length of time), all in i64
// (times and durations in milliseconds).

const BUILD: &str = "entity Build { level: Int  started: Time  done: Time  took: Duration  bonus: Int? }";

/// The type a function's body has, by the checker: its error, if any.
fn typed(params: &str, ret: &str, body: &str) -> Result<(), String> {
    verdict(&format!("{BUILD}\nfn f({params}): {ret} = {body}"))
}

#[test]
fn time_and_duration_arithmetic_has_types() {
    typed("b: Build", "Duration", "b.done - b.started").unwrap();
    typed("b: Build", "Time", "b.started + b.took").unwrap();
    typed("b: Build", "Time", "b.took + b.started").unwrap();
    typed("b: Build", "Time", "b.done - 30s").unwrap();
    typed("b: Build", "Duration", "b.took * 3 + 2d").unwrap();
    typed("b: Build", "Duration", "b.took / 2").unwrap();
    typed("b: Build", "Int", "b.took / 1h").unwrap();
    typed("b: Build", "Duration", "b.took % 1min").unwrap();
    typed("b: Build", "Bool", "b.took < 5min and b.done - b.started >= b.took").unwrap();
    typed("b: Build", "Duration", "max(b.took, 1s)").unwrap();
    typed("b: Build", "Time", "min(b.started, b.done)").unwrap();
    typed("b: Build", "Duration", "abs(b.took - 1h)").unwrap();
    typed("b: Build", "Text", "text(b.took)").unwrap();
}

#[test]
fn meaningless_time_arithmetic_is_refused() {
    let e = typed("b: Build", "Time", "b.started + b.done").unwrap_err();
    assert!(e.contains("can't add Time and Time"), "{e}");
    let e = typed("b: Build", "Time", "b.started * 2").unwrap_err();
    assert!(e.contains("can't multiply Time by Int"), "{e}");
    let e = typed("b: Build", "Int", "b.level / b.took").unwrap_err();
    assert!(e.contains("can't divide Int by Duration"), "{e}");
    let e = typed("b: Build", "Duration", "b.took * b.took").unwrap_err();
    assert!(e.contains("can't multiply Duration by Duration"), "{e}");
    let e = typed("b: Build", "Int", "b.level + b.took").unwrap_err();
    assert!(e.contains("can't add Int and Duration"), "{e}");
    let e = typed("b: Build", "Time", "max(b.started, b.took)").unwrap_err();
    assert!(e.contains("all of one kind"), "{e}");
}

#[test]
fn int_arithmetic_and_none() {
    typed("b: Build", "Int", "b.level * 2 + 1 - b.level % 3").unwrap();
    typed("b: Build", "Int", "clamp(b.level, 0, 40)").unwrap();
    typed("b: Build", "Text", "grouped(b.level * 1000)").unwrap();
    typed("b: Build", "Int", "-5 * abs(b.level)").unwrap();
    // a divisor that may be 0, and a value that may be none, give a value that may be none
    let m = |body: &str| {
        let src = format!("{BASE}\n{BUILD}");
        let decls = parse::parse_file(&src, 0).map_err(|e| e.msg).unwrap();
        let app = App { files: vec!["t.burin".into()], decls };
        let m = check::Model::new(&app).map_err(|e| e.msg).unwrap();
        let e = parse::parse_expr(body).map_err(|e| e.msg).unwrap();
        let sc = check::Scope::default().with("b", crate::ast::Type::Ref("Build".into()));
        m.type_of(&e, &sc).map_err(|e| e.msg).unwrap().show()
    };
    assert_eq!(m("b.level / 4"), "Int");
    assert_eq!(m("b.level / b.level"), "Int?");
    assert_eq!(m("b.level % 0"), "Int?");
    assert_eq!(m("b.bonus + 1"), "Int?");
    assert_eq!(m("b.took / 1h"), "Int");
    assert_eq!(m("max(b.bonus, 0)"), "Int?");
}

#[test]
fn multiplication_binds_tighter_than_addition() {
    let e = parse::parse_expr("1 + 2 * 3 - 4 / 2").map_err(|e| e.msg).unwrap();
    assert_eq!(crate::llvm::show(&e), "((1 Add (2 Mul 3)) Sub (4 Div 2))");
    let e = parse::parse_expr("-2d + -30s").map_err(|e| e.msg).unwrap();
    assert_eq!(crate::llvm::show(&e), "(-172800000ms Add -30000ms)");
}

#[test]
fn a_page_adds_numbers_instead_of_joining_them() {
    // `+` on Ints in a hole is arithmetic (it used to be lowered as text joined)
    let src = format!("{BUILD}\nview builds() = Build order by .created window 5 last");
    let frag = r#"fragment p() <<<{{for b in builds()}}<p>{{b.level + 1}} {{text(b.done - b.started)}} {{grouped(b.level * 1000)}}</p>{{end}}>>>"#;
    let decls = parse::parse_file(&format!("{BASE}\n{src}\n{frag}"), 0).map_err(|e| e.msg).unwrap();
    let app = App { files: vec!["t.burin".into()], decls };
    let m = check::Model::new(&app).map_err(|e| e.msg).unwrap();
    check::check(&m).map_err(|e| e.msg).unwrap();
    let (ll, _) = crate::llvm::module(&m, &["p"], "", "").unwrap();
    assert!(ll.contains("@llvm.sadd.with.overflow.i64(i64"), "no addition");
    assert!(ll.contains("@llvm.ssub.with.overflow.i64(i64"), "no subtraction");
    assert!(ll.contains("call void @burin_time(ptr") && ll.contains("i64 3, i64"), "no duration text");
    assert!(ll.contains("i64 4, i64"), "no grouped number");
}

const EXTERNS: &str = r#"
extern fn slug(t: Text, max: Int): Text
extern fn long(t: Text): Bool
extern effect fn notify(t: Text): Int
"#;

#[test]
fn a_pure_extern_in_a_page_passes() {
    verdict(&format!(r#"{EXTERNS}
fragment p() <<<{{{{for p in recent()}}}}<a href="/p/{{{{slug(p.body, 20)}}}}">{{{{if long(p.body)}}}}long{{{{end}}}}</a>{{{{end}}}}>>>"#)).unwrap();
}

#[test]
fn an_extern_with_effects_in_a_page_is_refused() {
    let e = verdict(&format!(r#"{EXTERNS}
fragment p() <<<{{{{for p in recent()}}}}{{{{notify(p.body)}}}}{{{{end}}}}>>>"#)).unwrap_err();
    assert!(e.contains("only actions call it"), "{e}");
}

#[test]
fn an_extern_with_effects_in_an_action_passes() {
    verdict(&format!(r#"{EXTERNS}
action post(body: Text) {{
  create Post {{ author: viewer, body: slug(body, 20) + text(notify(body)) }}
}}"#)).unwrap();
}

#[test]
fn an_extern_of_a_type_the_c_abi_has_not_is_refused() {
    let e = verdict("extern fn who(u: User): Text").unwrap_err();
    assert!(e.contains("an external function takes Int, Bool, Time, Duration or Text"), "{e}");
    let e = verdict("extern fn author_of(t: Text): User").unwrap_err();
    assert!(e.contains("an external function returns"), "{e}");
}

#[test]
fn an_extern_with_the_wrong_arguments_is_refused() {
    let e = verdict(&format!(r#"{EXTERNS}
fragment p() <<<{{{{for p in recent()}}}}{{{{slug(p.body)}}}}{{{{end}}}}>>>"#)).unwrap_err();
    assert!(e.contains("arguments") || e.contains("takes"), "{e}");
}

#[test]
fn a_template_let_names_a_value_for_the_rest_of_its_block() {
    let decls = parse::parse_file(r#"fragment f(p: Post) <<<{{let n = p.body}}{{if present(n)}}{{let m = upper(n)}}<b>{{m}}</b>{{end}}{{n}}{{for n in recent()}}{{n.body}}{{end}}>>>"#, 0).ok().unwrap();
    let crate::ast::Decl::Fragment { body, .. } = &decls[0] else { panic!() };
    let shown = format!("{:?}", body.nodes);
    // the names are gone: each use is the value it named, and the for's own `n` is its row
    assert!(!shown.contains(r#"Name("m""#), "{shown}");
    assert!(shown.contains(r#"Call("upper", [Field(Name("p""#), "{shown}");
    assert!(shown.contains(r#"Field(Name("n", Span { file: 0, line: 1 }), "body""#), "{shown}");
    // and the template checks
    verdict(r#"fragment f(p: Post) <<<{{let n = p.body}}{{if present(n)}}{{let m = upper(n)}}<b>{{m}}</b>{{end}}{{n}}>>>"#).unwrap();
}

#[test]
fn the_standard_library_is_in_every_app() {
    verdict(r#"fragment p() <<<{{for p in recent()}}{{if contains(p.body, "x") and length(p.body) > 3}}{{truncate(p.body, 10)}} {{plural(length(p.body), "char", "chars")}}{{end}}{{end}}>>>"#).unwrap();
    let e = verdict("extern fn length(t: Text): Int").unwrap_err();
    assert!(e.contains("declared twice"), "{e}");
}

#[test]
fn a_standard_function_with_a_sql_form_runs_in_a_query() {
    lowered(r#"view long_ones() = Post where length(.body) > 3 and contains(.body, "x") order by .created window 20 last
fragment f() <<<{{for p in long_ones()}}<p>{{p.body}}</p>{{end}}>>>"#, "f").unwrap();
    // one without a SQL form, on the query's rows, can't
    let e = lowered(r#"view padded() = Post where pad_start(.body, 3, "0") == "007" order by .created window 20 last
fragment f() <<<{{for p in padded()}}<p>{{p.body}}</p>{{end}}>>>"#, "f").unwrap_err();
    assert!(e.contains("can't run inside a query"), "{e}");
}

#[test]
fn a_signal_goes_to_a_screen_of_its_type_and_targets_a_row_or_number() {
    let page = "screen post_page(post: Post) at \"/posts/{post}\"\n<<<x>>>\n";
    verdict(&format!("{page}action post(b: Text) {{\n  create Post {{ author: viewer, body: b }} as p\n  signal posted(p) to post_page(p)\n  signal seen() to viewer\n  signal any(1) to everyone\n}}")).unwrap();
    let e = verdict(&format!("{page}action post(b: Text) {{\n  signal posted(b) to everyone\n}}")).unwrap_err();
    assert!(e.contains("a row or a number"), "{e}");
    let e = verdict(&format!("{page}action post(u: User) {{\n  signal posted(u) to post_page(u)\n}}")).unwrap_err();
    assert!(e.contains("post_page takes"), "{e}");
    let e = verdict(&format!("{page}action post(u: User) {{\n  signal posted(u) to nowhere(u)\n}}")).unwrap_err();
    assert!(e.contains("a screen with one parameter"), "{e}");
}
