//! etchc: the etch compiler.
//!
//!   etchc check <app dir>     names, types and the efficiency rules
//!   etchc report <app dir>    the dataflow netlist: each node's stage, the registers, splices

mod ast;
mod check;
mod netlist;
mod parse;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (cmd, dir) = match (args.get(1), args.get(2)) {
        (Some(c), Some(d)) => (c.as_str(), Path::new(d)),
        _ => {
            eprintln!("usage: etchc check|report <app dir>");
            return ExitCode::from(64);
        }
    };
    let app = match parse::parse_app(dir) {
        Ok(a) => a,
        Err((files, e)) => {
            eprintln!("{}:{}: {}", files.get(e.span.file as usize).map(String::as_str).unwrap_or("?"), e.span.line, e.msg);
            return ExitCode::from(1);
        }
    };
    let where_ = |s: ast::Span| format!("{}:{}", app.files.get(s.file as usize).map(String::as_str).unwrap_or("?"), s.line);
    let model = match check::Model::new(&app).and_then(|m| check::check(&m).map(|_| m)) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{}: {}", where_(e.span), e.msg);
            return ExitCode::from(1);
        }
    };
    match cmd {
        "check" => {
            println!(
                "ok: {} entities, {} views, {} fragments, {} screens, {} actions",
                model.entities.len(),
                model.views.len(),
                model.fragments.len(),
                model.screens.len(),
                model.actions.len()
            );
            ExitCode::SUCCESS
        }
        "report" => {
            print!("{}", netlist::report(&netlist::build(&model)));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("unknown command {cmd}");
            ExitCode::from(64)
        }
    }
}
