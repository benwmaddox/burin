//! etchc: the etch compiler.
//!
//!   etchc check <app dir>     names, types and the efficiency rules
//!   etchc report <app dir>    the dataflow netlist: each node's stage, the registers, splices

mod ast;
mod actions;
mod check;
mod glue;
mod inline;
mod llvm;
mod netlist;
mod parse;
mod sql;
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
        "emit" => {
            // etchc emit <app> <out dir> <register,…>: the registers' LLVM module and Rust header
            // (the registers are what the screens show, unless named)
            let (out, regs) = match (args.get(3), args.get(4)) {
                (Some(o), Some(r)) => (Path::new(o), r.split(',').collect::<Vec<_>>()),
                (Some(o), None) => (Path::new(o), vec![]),
                _ => {
                    eprintln!("usage: etchc emit <app dir> <out dir> [register,…]");
                    return ExitCode::from(64);
                }
            };
            let dl = std::env::var("ETCH_DATALAYOUT").unwrap_or_else(|_| "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128".into());
            let tt = std::env::var("ETCH_TRIPLE").unwrap_or_else(|_| "x86_64-unknown-linux-gnu".into());
            match llvm::module(&model, &regs, &format!("target datalayout = \"{dl}\""), &format!("target triple = \"{tt}\"")) {
                Ok((ll, rs)) => {
                    let _ = std::fs::create_dir_all(out);
                    std::fs::write(out.join("app.ll"), ll).unwrap();
                    // the header only when it changed: a template's edit doesn't recompile the glue
                    let hp = out.join("app_gen.rs");
                    if std::fs::read_to_string(&hp).ok().as_deref() != Some(rs.as_str()) {
                        std::fs::write(&hp, rs).unwrap();
                    }
                    println!("wrote {}", out.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("emit: {e}");
                    ExitCode::from(1)
                }
            }
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
