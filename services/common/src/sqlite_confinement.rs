// SPDX-License-Identifier: CC0-1.0

//! The ONE list of SQL functions a caller's statements may call, for every data plane in
//! this emulator that runs them on the in-process SQLite engine.
//!
//! A data plane that runs caller SQL confines the engine with an AUTHORIZER, and the
//! function arm of that authorizer has to be an ALLOW-LIST: a function a future SQLite or
//! `rusqlite` build defines is then refused until it has been reviewed and named here,
//! rather than allowed because nobody added it to a list of refusals. The list is the long
//! part of that policy and says nothing about any one service, so it lives here and the
//! planes read it; each plane keeps its own authorizer, because the authorizer names the
//! engine's own types and no shared crate takes a dependency on the engine.
//!
//! Services still carrying their own copy of this list: this module postdates the RDS Data
//! API's (`service-com-amazonaws-rdsdata-rdsdataservice/src/state.rs`
//! `PERMITTED_FUNCTIONS`), which is to be MIGRATED onto it; it is left in place only
//! because a change does not own that service's issues. Nothing new inlines the list
//! again — call this module.

/// Every SQL function caller SQL may call.
///
/// The names are SQLite's own documented library functions: the core scalar functions
/// (<https://sqlite.org/lang_corefunc.html>), the date and time functions
/// (<https://sqlite.org/lang_datefunc.html>) and the three date and time KEYWORDS the
/// engine resolves as functions of the same names (<https://sqlite.org/lang_expr.html>),
/// the aggregate functions (<https://sqlite.org/lang_aggfunc.html>), the built-in window
/// functions (<https://sqlite.org/windowfunctions.html>), the math functions
/// (<https://sqlite.org/lang_mathfunc.html>) and the JSON functions
/// (<https://sqlite.org/json1.html>) — every one of which computes over the values in the
/// database it is asked in, or over the clock, and nothing else.
///
/// DELIBERATELY ABSENT, so a default-deny authorizer refuses them: `load_extension` (the
/// engine's own code loader), the `sqlite3` shell's file functions (`readfile`,
/// `writefile`, `edit`, `fsdir`), `sqlite_dbpage` and `fts3_tokenizer`, and the
/// build-reporting functions (`sqlite_version`, `sqlite_source_id`,
/// `sqlite_compileoption_get`, `sqlite_compileoption_used`, `sqlite_offset`) — none of
/// which a client of an emulated database needs.
pub const PERMITTED_SQL_FUNCTIONS: &[&str] = &[
    // Core scalar functions.
    "abs",
    "changes",
    "char",
    "coalesce",
    "concat",
    "concat_ws",
    "format",
    "glob",
    "hex",
    "iif",
    "ifnull",
    "instr",
    "last_insert_rowid",
    "length",
    "like",
    "likelihood",
    "likely",
    "lower",
    "ltrim",
    "max",
    "min",
    "nullif",
    "octet_length",
    "printf",
    "quote",
    "random",
    "randomblob",
    "replace",
    "round",
    "rtrim",
    "sign",
    "soundex",
    "substr",
    "substring",
    "total_changes",
    "trim",
    "typeof",
    "unhex",
    "unicode",
    "unlikely",
    "upper",
    "zeroblob",
    // Date and time functions.
    "current_date",
    "current_time",
    "current_timestamp",
    "date",
    "datetime",
    "julianday",
    "strftime",
    "time",
    "timediff",
    "unixepoch",
    // Aggregate functions (`max`/`min`/`count` double as scalars above).
    "avg",
    "count",
    "group_concat",
    "string_agg",
    "sum",
    "total",
    // Built-in window functions.
    "cume_dist",
    "dense_rank",
    "first_value",
    "lag",
    "last_value",
    "lead",
    "nth_value",
    "ntile",
    "percent_rank",
    "rank",
    "row_number",
    // Math functions.
    "acos",
    "acosh",
    "asin",
    "asinh",
    "atan",
    "atan2",
    "atanh",
    "ceil",
    "ceiling",
    "cos",
    "cosh",
    "degrees",
    "exp",
    "floor",
    "ln",
    "log",
    "log10",
    "log2",
    "mod",
    "pi",
    "pow",
    "power",
    "radians",
    "sin",
    "sinh",
    "sqrt",
    "tan",
    "tanh",
    "trunc",
    // JSON functions, including the `->` and `->>` operators SQLite implements as
    // functions of those names.
    "->",
    "->>",
    "json",
    "json_array",
    "json_array_length",
    "json_error_position",
    "json_extract",
    "json_group_array",
    "json_group_object",
    "json_insert",
    "json_object",
    "json_patch",
    "json_pretty",
    "json_quote",
    "json_remove",
    "json_replace",
    "json_set",
    "json_type",
    "json_valid",
];

/// Whether `function_name` is one of [`PERMITTED_SQL_FUNCTIONS`].
///
/// Compared case-INSENSITIVELY, because SQL function names are: a statement may spell one
/// in any case and the engine resolves the same function, so the policy has to read it the
/// same way.
#[must_use]
pub fn sql_function_is_permitted(function_name: &str) -> bool {
    PERMITTED_SQL_FUNCTIONS
        .iter()
        .any(|permitted| function_name.eq_ignore_ascii_case(permitted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_listed_functions_are_permitted() {
        // The rows span what the reading generalises over: a function of each family the
        // list names, the letter cases a statement may spell one in, the engine's own
        // code loader and the shell's file functions (the host escapes the list exists to
        // refuse), and a name no build defines.
        let cases: &[(&str, bool)] = &[
            ("length", true),
            ("LENGTH", true),
            ("Json_Extract", true),
            ("->>", true),
            ("row_number", true),
            ("sqrt", true),
            ("current_timestamp", true),
            ("load_extension", false),
            ("readfile", false),
            ("writefile", false),
            ("fsdir", false),
            ("edit", false),
            ("sqlite_dbpage", false),
            ("sqlite_version", false),
            ("not_a_function", false),
            ("", false),
        ];
        for (name, permitted) in cases {
            let answer = sql_function_is_permitted(name);
            assert_eq!(
                answer, *permitted,
                "case {name:?}: expected permitted {permitted}, got {answer}"
            );
        }
    }
}
