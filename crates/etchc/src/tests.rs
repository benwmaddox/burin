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
