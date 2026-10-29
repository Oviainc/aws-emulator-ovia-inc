// SPDX-License-Identifier: CC0-1.0

//! The injection detectors a `SqliMatchStatement`, an `XssMatchStatement` and the
//! `SQLi`/`CrossSiteScripting` rules of the AWS Managed Rules groups inspect with.
//!
//! `SqliMatchStatement` is "a rule statement that inspects for malicious SQL code.
//! Attackers insert malicious SQL code into web requests to do things like modify your
//! database or extract data from it", and `XssMatchStatement` is the same for
//! "cross-site scripting (XSS) attacks". Neither is a parse of the request: WAF
//! decides by looking for the SIGNATURES an injected fragment carries — a string
//! literal broken out of, a comment terminator, a statement keyword in a place a
//! value belongs, a tag or an event handler in text that should carry none — which is
//! what this module computes.
//!
//! WHY the detection is written here rather than taken from a crate: the maintained
//! implementation of this detection is libinjection, whose Rust packaging
//! (`libinjection` on crates.io, every release) is a `bindgen` binding to the C
//! library — it generates `unsafe` FFI declarations and needs libclang and a C
//! toolchain at build time, and this workspace forbids `unsafe_code`. No
//! pure-Rust crate implements the detection, so the signatures below are written out,
//! each citing the shape of injection it recognises.
//!
//! Both detectors are O(n) in the length of the value they inspect, over a fixed
//! signature table: a request cannot make either of them expensive.

/// Whether `value` carries SQL code injected into what should be a value.
///
/// `extended` is the `HIGH` sensitivity level: `SqliMatchStatement$SensitivityLevel`
/// documents `LOW` as "generally a better choice for resources that already have
/// other protections against SQL injection attacks or that have a low tolerance for
/// false positives" and `HIGH` as the default, which "detects more attacks" — so the
/// low level looks for the unambiguous statement fragments alone and the high level
/// adds the broader signatures that a benign value can also carry.
pub fn is_sql_injection(value: &[u8], extended: bool) -> bool {
    let normalised = normalised(value);
    if SQL_SIGNATURES
        .iter()
        .any(|signature| matches_signature(&normalised, signature))
    {
        return true;
    }
    extended
        && SQL_EXTENDED_SIGNATURES
            .iter()
            .any(|signature| matches_signature(&normalised, signature))
}

/// Whether `value` carries script injected into what should be text.
pub fn is_cross_site_scripting(value: &[u8]) -> bool {
    let normalised = normalised(value);
    XSS_SIGNATURES
        .iter()
        .any(|signature| matches_signature(&normalised, signature))
}

/// One signature: the fragments that must appear, in this order, for the value to
/// carry the injection the signature recognises.
type Signature = &'static [&'static str];

/// The signatures an injected SQL fragment carries unambiguously — each one is a
/// statement fragment that cannot occur in a value a form or a path legitimately
/// carries.
const SQL_SIGNATURES: &[Signature] = &[
    // A second statement spliced onto the first: the UNION-based extraction, and the
    // stacked query.
    &["union", "select"],
    &["union", "all", "select"],
    &[";", "select"],
    &[";", "insert"],
    &[";", "update"],
    &[";", "delete"],
    &[";", "drop"],
    &[";", "alter"],
    &[";", "exec"],
    &[";", "shutdown"],
    // The catalogue an extraction enumerates the schema through.
    &["information_schema"],
    &["sysobjects"],
    &["pg_sleep"],
    // The time-based probe.
    &["sleep("],
    &["benchmark("],
    &["waitfor", "delay"],
    // The out-of-band read and the stacked procedure call SQL Server exposes.
    &["xp_cmdshell"],
    &["load_file("],
    &["into", "outfile"],
    // A string literal closed and a comment opened to swallow the rest of the query.
    &["'", "--"],
    &["\"", "--"],
    &["'", "/*"],
    &["'", "#"],
    // A literal closed and the predicate replaced.
    &["'", "or", "'"],
    &["'", "or", "="],
    &["'", "and", "="],
    &["'", "union"],
    &["'", ";"],
    &["'", ")", "or"],
];

/// The broader signatures the `HIGH` sensitivity level adds: fragments an injected
/// predicate carries, which a value can also carry innocently.
const SQL_EXTENDED_SIGNATURES: &[Signature] = &[
    // The tautology, with and without quoting around the operands.
    &["or", "1=1"],
    &["or", "1 = 1"],
    &["and", "1=1"],
    &["or", "true"],
    &["'='"],
    &["' = '"],
    // A comment that swallows the rest of a query, which is what makes it an injected
    // fragment rather than a hyphen: a terminator BESIDE a quote, a comparison or a
    // statement separator. A value that merely carries a double hyphen (a
    // hyphen-separated identifier, a date range) carries no SQL and is left alone — the
    // quoted forms in the unambiguous table above cover the injected use of it.
    &["=", "--"],
    &[")", "--"],
    &[";", "--"],
    &["/*", "*/"],
    // A predicate keyword beside a comparison, without the literal that would make it
    // unambiguous.
    &["or", "="],
    &["select", "from"],
    &["having", "="],
    &["union"],
    &["is", "null"],
    &["cast(", "as"],
    &["concat(", ","],
];

/// The signatures injected script carries: a tag that executes, a URI scheme that
/// executes, and an event-handler attribute.
const XSS_SIGNATURES: &[Signature] = &[
    &["<script"],
    &["</script"],
    &["<iframe"],
    &["<object"],
    &["<embed"],
    &["<svg", "on"],
    &["<img", "on"],
    &["<body", "on"],
    &["<a", "javascript:"],
    &["javascript:"],
    &["vbscript:"],
    &["data:text/html"],
    &["onerror="],
    &["onload="],
    &["onclick="],
    &["onmouseover="],
    &["onfocus="],
    &["document.cookie"],
    &["document.write"],
    &["window.location"],
    &["eval("],
    &["expression("],
    &["<style", "expression"],
];

/// Whether each fragment of `signature` occurs in `value`, in order and without
/// overlapping, and — for a fragment that begins or ends in a WORD — only where that
/// word stands on its own.
///
/// The boundary condition is what keeps a signature from matching the inside of an
/// ordinary word: `or` is a SQL operator in `' or 1=1` and three letters of a field
/// name in `color=red`, and only a check on the characters either side of the fragment
/// tells the two apart. A fragment that neither begins nor ends in a word character
/// (`--`, `/*`, `'='`) is matched wherever it occurs, because such a sequence is not
/// part of a longer word.
fn matches_signature(value: &str, signature: Signature) -> bool {
    let mut searched = 0_usize;
    for fragment in signature {
        match word_bounded_find(value, fragment, searched) {
            Some(at) => searched = at + fragment.len(),
            None => return false,
        }
    }
    true
}

/// The first offset at or after `from` where `fragment` occurs in `value` on its own
/// word boundaries, or `None` when it does not; see [`matches_signature`].
fn word_bounded_find(value: &str, fragment: &str, from: usize) -> Option<usize> {
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let starts_in_word = fragment.bytes().next().is_some_and(is_word);
    let ends_in_word = fragment.bytes().next_back().is_some_and(is_word);
    let bytes = value.as_bytes();
    let mut searched = from;
    while let Some(offset) = value.get(searched..)?.find(fragment) {
        let at = searched + offset;
        let end = at + fragment.len();
        let before_ok = !starts_in_word || at == 0 || !is_word(bytes[at - 1]);
        let after_ok = !ends_in_word || end == bytes.len() || !is_word(bytes[end]);
        if before_ok && after_ok {
            return Some(at);
        }
        searched = at + 1;
    }
    None
}

/// `value` as the signatures are matched against it: lower-cased, with inline
/// comments replaced by a space and every run of whitespace collapsed to one.
///
/// This is the one normalisation the detection needs, and it is the one an injected
/// fragment hides behind: `UNION/**/SELECT` and `union     select` are the same
/// fragment as `union select`, so collapsing them is what keeps a signature from
/// being evaded by whitespace or by a comment. Nothing else is rewritten — the
/// statement's own `TextTransformations` are applied before the value reaches here.
fn normalised(value: &[u8]) -> String {
    let lowered = String::from_utf8_lossy(value).to_ascii_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut rest = lowered.as_str();
    // An inline comment becomes a space: `/*` with no `*/` after it swallows the rest
    // of the value, which is what the database would do with it too.
    while let Some(at) = rest.find("/*") {
        out.push_str(&rest[..at]);
        out.push_str("/*");
        match rest[at + 2..].find("*/") {
            Some(end) => {
                out.push_str("*/ ");
                rest = &rest[at + 2 + end + 2..];
            }
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    // Collapse whitespace, so a signature's fragments sit next to each other however
    // the value spaced them.
    let mut collapsed = String::with_capacity(out.len());
    let mut in_space = false;
    for character in out.chars() {
        if character.is_whitespace() {
            if !in_space {
                collapsed.push(' ');
            }
            in_space = true;
        } else {
            collapsed.push(character);
            in_space = false;
        }
    }
    collapsed
}

#[cfg(test)]
mod tests {
    //! The detectors over a table of injected and benign values: the rule is that an
    //! injected fragment is recognised however it spaces or comments itself, and that
    //! a value a form legitimately carries is not.

    use super::*;

    #[test]
    fn sql_injection_is_recognised_however_it_is_spaced() {
        let injected: &[&str] = &[
            "' OR '1'='1",
            "1' UNION SELECT password FROM users--",
            "1 UNION/**/SELECT 1",
            "admin'--",
            "1; DROP TABLE users",
            "' or 1=1 --",
            "1 AND SLEEP(5)",
            "x' AND (SELECT 1 FROM information_schema.tables)='1",
        ];
        for value in injected {
            assert!(
                is_sql_injection(value.as_bytes(), true),
                "case {value:?}: expected the HIGH-sensitivity detector to recognise \
                 injected SQL, got no match"
            );
        }
        let benign: &[&str] = &["alice", "order-1234", "a b c", "2026-01-01", "O'Brien"];
        for value in benign {
            assert!(
                !is_sql_injection(value.as_bytes(), false),
                "case {value:?}: expected the LOW-sensitivity detector to pass a value a \
                 form legitimately carries, got a match"
            );
        }
    }

    #[test]
    fn the_low_sensitivity_level_leaves_the_broader_signatures_to_high() {
        let broad = "id=1 or x=1";
        assert!(
            !is_sql_injection(broad.as_bytes(), false),
            "case {broad:?}: expected LOW to leave a bare predicate fragment alone"
        );
        assert!(
            is_sql_injection(broad.as_bytes(), true),
            "case {broad:?}: expected HIGH to recognise a bare predicate fragment"
        );
    }

    #[test]
    fn a_value_that_merely_looks_like_sql_is_not_sql() {
        // The false-positive boundary, pinned: each row carries a signature's letters
        // inside an ordinary word, or a character sequence an identifier, a date range
        // or a sentence legitimately carries. None of them is SQL, and HIGH is the
        // DEFAULT sensitivity, so each must pass there.
        let benign: &[&str] = &[
            // A keyword's letters inside a longer word.
            "color=red",
            "sponsor=acme&id=7",
            "border=1",
            "selection=from-list",
            "recharacterise(x)",
            "author=Orwell",
            // A double hyphen that terminates no query.
            "foo--bar",
            "2026-01-01--2026-02-01",
            "well--known",
            // Words a sentence carries that a predicate also uses.
            "sort order by date",
            "group by team",
            "convert(this) to that",
            "chart(2026)",
        ];
        for value in benign {
            assert!(
                !is_sql_injection(value.as_bytes(), true),
                "case {value:?}: expected HIGH to pass a value a form legitimately \
                 carries, got a match"
            );
        }
        // And the injected forms of the same characters are still recognised.
        let injected: &[&str] = &[
            "id=1 or 2=2",
            "1' or '1'='1",
            "1) --",
            "1; --",
            "1 union select 1",
        ];
        for value in injected {
            assert!(
                is_sql_injection(value.as_bytes(), true),
                "case {value:?}: expected HIGH to recognise the injected form, got no \
                 match"
            );
        }
    }

    #[test]
    fn cross_site_scripting_is_recognised_in_its_published_forms() {
        let injected: &[&str] = &[
            "<script>alert(1)</script>",
            "<IMG SRC=x onerror=alert(1)>",
            "<a href=\"javascript:alert(1)\">x</a>",
            "<svg/onload=alert(1)>",
        ];
        for value in injected {
            assert!(
                is_cross_site_scripting(value.as_bytes()),
                "case {value:?}: expected injected script to be recognised, got no match"
            );
        }
        let benign: &[&str] = &["a < b", "5 > 3", "plain text"];
        for value in benign {
            assert!(
                !is_cross_site_scripting(value.as_bytes()),
                "case {value:?}: expected benign text to pass, got a match"
            );
        }
    }
}
