// SPDX-License-Identifier: CC0-1.0

//! Shared support for the suites that pin this service's enum-membership rule.
//!
//! The rule is one property of the SERVICE — a request member whose target shape is a
//! Smithy enumeration accepts only the values that enumeration declares — so the family of
//! values a row is probed with is stated ONCE here and used by every suite that asserts
//! it (`enum_membership.rs` for the two start operations,
//! `enum_membership_other_operations.rs` for the rest), rather than once per suite where
//! the two copies could drift.

/// Values the enumeration `declared` does not declare, derived from the catalog itself so
/// one generator serves every member of every operation.
///
/// The rows differ along what the rule generalises over: an unrelated word, a value that
/// looks like a condition of another dialect, the empty string, how near the value comes
/// to a declared one (case either way, trailing whitespace, one character short, one
/// character longer), a value far longer than any declared one, and — the hardest kind —
/// a value a SIBLING enumeration of this service declares, which is a legal value of the
/// service but not of this member. `siblings` is the list of catalogs those last rows are
/// drawn from.
///
/// Anything derived that is in fact declared (a single-value enumeration, a catalog whose
/// first entry is already upper case) is dropped, so no row can assert a refusal of
/// something the model admits.
pub(crate) fn undeclared_values(
    declared: &[&str],
    siblings: &[&[&str]],
) -> Vec<(String, String)> {
    let near = declared[0];
    let mut rows = vec![
        // The two values the issue recorded.
        ("an unrelated word".to_string(), "Bogus".to_string()),
        ("an SQL-looking condition".to_string(), "LIKE".to_string()),
        ("the empty string".to_string(), String::new()),
        (
            "a declared value in the wrong case".to_string(),
            near.to_lowercase(),
        ),
        (
            "a declared value in upper case".to_string(),
            near.to_uppercase(),
        ),
        (
            "a declared value with trailing whitespace".to_string(),
            format!("{near} "),
        ),
        (
            // Truncated by CHARACTER, not by byte, so the generator holds for any catalog
            // it is handed rather than only for the ASCII ones this service declares.
            "a declared value one character short".to_string(),
            {
                let mut shorter = near.to_string();
                let _ = shorter.pop();
                shorter
            },
        ),
        (
            "a declared value with a suffix".to_string(),
            format!("{near}X"),
        ),
        (
            "a value longer than any declared one".to_string(),
            "z".repeat(120),
        ),
    ];
    for catalog in siblings {
        if let Some(foreign) = catalog.iter().find(|value| !declared.contains(value)) {
            rows.push((
                "a value a sibling enumeration of this service declares".to_string(),
                (*foreign).to_string(),
            ));
        }
    }
    rows.retain(|(_, value)| !declared.contains(&value.as_str()));
    rows
}
