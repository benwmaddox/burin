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

fn verdict(extra: &str) -> Result<(), String> {
    let src = format!("{BASE}\n{extra}");
    let decls = parse::parse_file(&src, 0).map_err(|e| format!("parse: {}", e.msg))?;
    let app = App { files: vec!["t.etch".into()], decls };
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
    let decls = parse::parse_file(&src, 0).map_err(|e| format!("parse: {}", e.msg))?;
    let app = App { files: vec!["t.etch".into()], decls };
    let m = check::Model::new(&app).map_err(|e| e.msg)?;
    check::check(&m).map_err(|e| e.msg)?;
    crate::llvm::module(&m, &[frag], "", "").map(|_| ())
}

#[test]
fn viewer_text_in_a_shared_fragment_is_refused() {
    let e = lowered(r#"fragment p(x: Post) <<<<b>{{viewer.name}}</b>{{x.body}}>>>"#, "p").unwrap_err();
    assert!(e.contains("depends on the viewer"), "{e}");
}

#[test]
fn viewer_conditions_in_a_shared_fragment_pass() {
    lowered(r#"fragment p(x: Post) varies by viewer <<<{{if x.author == viewer}}<b>mine</b>{{end}}{{x.body}}>>>"#, "p").unwrap();
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

#[test]
fn a_variant_its_key_doesnt_declare_is_refused() {
    let e = lowered(r#"fragment p(x: Post) <<<{{if x.author == viewer}}<b>mine</b>{{end}}{{x.body}}>>>"#, "p").unwrap_err();
    assert!(e.contains("which its key doesn't declare: add `varies by viewer`"), "{e}");
    let e = lowered(r#"fragment p(x: Post) varies by viewer <<<{{if x.author.name == "Ada" or viewer.name == "Bo"}}<b>!</b>{{end}}>>>"#, "p").unwrap_err();
    assert!(e.contains("`viewer.name`"), "{e}");
}

#[test]
fn a_key_declaring_more_than_it_uses_is_refused() {
    let e = lowered(r#"fragment p(x: Post) varies by viewer, viewer.name <<<{{if x.author == viewer}}<b>mine</b>{{end}}>>>"#, "p").unwrap_err();
    assert!(e.contains("declares `varies by viewer.name`, but nothing it shows depends on"), "{e}");
}
