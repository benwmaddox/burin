// The standard library's SQL forms (lang/std.etch): what a function is inside a query, `$1`
// … its arguments. Included by the compiler (sql.rs) and by the runtime's test that checks each
// form against its Rust (crates/etch-rt/src/stdlib.rs) on many inputs. A function without one
// can't run inside a query.
pub const STD_SQL: &[(&str, &str)] = &[
    ("length", "length($1)"),
    ("contains", "(instr($1, $2) > 0)"),
    ("starts_with", "(substr($1, 1, length($2)) = $2)"),
    ("ends_with", "(length($2) = 0 OR substr($1, -length($2)) = $2)"),
    ("index_of", "(instr($1, $2) - 1)"),
    ("trim", "trim($1, ' ' || char(9) || char(10) || char(13))"),
    ("truncate", "(CASE WHEN length($1) > max($2, 0) THEN substr($1, 1, max($2, 0)) || '…' ELSE $1 END)"),
    ("slice", "substr($1, max($2, 0) + 1, max($3, 0))"),
    ("replace_all", "replace($1, $2, $3)"),
    ("plural", "(CASE WHEN $1 = 1 THEN $2 ELSE $3 END)"),
];
