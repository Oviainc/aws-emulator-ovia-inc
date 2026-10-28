// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHITESPACE as part of a `<lexeme>`'s replacement, the half of
//! the per-lexeme measurement that `tests/all/pls_replacement_measurement.rs` leaves
//! unpinned.
//!
//! RULE: the per-lexeme quota is measured against the whole character content of a
//! `<lexeme>`'s `<alias>`/`<phoneme>` element, and a whitespace character is a
//! character of that content wherever it sits — at the element's leading or trailing
//! edge, next to an entity or character reference, around a nested element, or inside a
//! CDATA section — so `PutLexicon` refuses the document with HTTP 400
//! `MaxLexemeLengthExceededException` and stores nothing exactly when the total
//! (whitespace included) passes the quota.
//!
//! The tests check that over a TABLE whose rows differ along WHERE the whitespace sits
//! (leading, trailing, both edges, interior, adjacent to a reference, around a nested
//! replacement element, inside CDATA), WHICH whitespace characters it is (space, tab,
//! newline, carriage return), how the characters beside it are spelled (plain text, an
//! entity reference, a numeric character reference, CDATA) and whether the total lands
//! under, on or past the quota — plus a sweep of every padding length across the
//! boundary, which is where the measurement either counts the padding or does not. The
//! one document an issue happens to quote is a single row.

use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::{
    MAX_LEXEME_CHARACTERS, PLS_NAMESPACE, PLS_VERSION,
};
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, json, Polly};

/// A PLS lexicon whose single `<lexeme>` carries `body` as its replacement element.
fn pls(body: &str) -> String {
    format!(
        "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" alphabet=\"ipa\" \
         xml:lang=\"en-US\"><lexeme><grapheme>G</grapheme>{body}</lexeme></lexicon>"
    )
}

/// One row: the markup between the replacement element's tags, and how many characters
/// of content it holds — counted by hand, so the expectation does not come from the
/// parser under test.
struct Row {
    case: String,
    inner: String,
    characters: usize,
}

/// Put `content` under `name` and assert it is answered as the quota rule requires for
/// a replacement holding `characters` characters.
async fn check(polly: &Polly, case: &str, name: &str, content: &str, characters: usize) {
    let response = polly.put_lexicon(name, content).await;
    if characters > MAX_LEXEME_CHARACTERS {
        let body = json(&response);
        assert_eq!(
            (
                response.status().as_u16(),
                error_code(&response),
                body.get("message").and_then(|m| m.as_str())
            ),
            (
                400,
                "MaxLexemeLengthExceededException".to_string(),
                Some(errors::MAX_LEXEME_LENGTH_EXCEEDED)
            ),
            "case {case:?} ({characters} characters of replacement, content \
             {content:?}): expected the lexeme-quota refusal, got {body:?}"
        );
        let read_back = polly.get_lexicon(name).await;
        assert_eq!(
            (read_back.status().as_u16(), error_code(&read_back)),
            (404, "LexiconNotFoundException".to_string()),
            "case {case:?}: a refused put must store nothing, got {:?}",
            json(&read_back)
        );
    } else {
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?} ({characters} characters of replacement, content \
             {content:?}): expected the document to be stored, got {:?}",
            json(&response)
        );
        let got = json(&polly.get_lexicon(name).await);
        assert_eq!(
            (
                got["Lexicon"]["Content"].as_str(),
                got["LexiconAttributes"]["LexemesCount"].as_u64()
            ),
            (Some(content), Some(1)),
            "case {case:?}: the stored document must read back as it was put, got {got:?}"
        );
    }
}

/// The rows, built for `outer` as the replacement element being measured and `nested` as
/// a replacement element nested inside it.
fn rows(outer: &str, nested: &str) -> Vec<Row> {
    let quota = MAX_LEXEME_CHARACTERS;
    let mut rows: Vec<Row> = Vec::new();
    let mut row = |case: &str, inner: String, characters: usize| {
        rows.push(Row {
            case: case.to_string(),
            inner,
            characters,
        });
    };

    // The control pair: no whitespace at all, so the boundary itself is anchored.
    row("no whitespace, at the quota", "x".repeat(quota), quota);
    row(
        "no whitespace, one past the quota",
        "x".repeat(quota + 1),
        quota + 1,
    );

    // Whitespace at the element's EDGES — the fragments a per-event trim eats first.
    row(
        "trailing spaces take the total past the quota",
        format!("{}{}", "x".repeat(quota), " ".repeat(10)),
        quota + 10,
    );
    row(
        "trailing spaces leave the total at the quota",
        format!("{}{}", "x".repeat(quota - 10), " ".repeat(10)),
        quota,
    );
    row(
        "leading spaces take the total past the quota",
        format!("{}{}", " ".repeat(10), "x".repeat(quota)),
        quota + 10,
    );
    row(
        "leading spaces leave the total at the quota",
        format!("{}{}", " ".repeat(10), "x".repeat(quota - 10)),
        quota,
    );
    row(
        "spaces on both edges take the total past the quota",
        format!("{}{}{}", " ".repeat(5), "x".repeat(quota - 4), " ".repeat(5)),
        quota + 6,
    );
    row(
        "spaces on both edges leave the total at the quota",
        format!(
            "{}{}{}",
            " ".repeat(5),
            "x".repeat(quota - 10),
            " ".repeat(5)
        ),
        quota,
    );
    // The other whitespace characters XML recognises in character data: tab, newline
    // and carriage return (XML 1.0 S = #x20 | #x9 | #xD | #xA).
    row(
        "tabs and newlines at the edges take the total past the quota",
        format!("\n\t{}\t\n", "x".repeat(quota)),
        quota + 4,
    );
    row(
        "a carriage return at each edge leaves the total at the quota",
        format!("\r{}\r", "x".repeat(quota - 2)),
        quota,
    );
    row(
        "a whitespace-only replacement under the quota",
        " ".repeat(10),
        10,
    );
    row(
        "a whitespace-only replacement one past the quota",
        " ".repeat(quota + 1),
        quota + 1,
    );
    row(
        "interior whitespace takes the total past the quota",
        format!("{} {}", "x".repeat(quota / 2), "y".repeat(quota - quota / 2)),
        quota + 1,
    );

    // Whitespace next to a REFERENCE — the reader breaks character data there, so the
    // spaces arrive as fragments of their own.
    row(
        "a space on each side of an entity reference, one past the quota",
        format!("{} &amp; ", "x".repeat(quota - 2)),
        quota + 1,
    );
    row(
        "a space on each side of an entity reference, at the quota",
        format!("{} &amp; ", "x".repeat(quota - 3)),
        quota,
    );
    row(
        "a space on each side of a numeric character reference, one past the quota",
        format!("{} &#120; ", "x".repeat(quota - 2)),
        quota + 1,
    );
    row(
        "whitespace between two entity references, one past the quota",
        format!("&amp;{}&amp;", " ".repeat(quota - 1)),
        quota + 1,
    );
    row(
        "whitespace between two entity references, at the quota",
        format!("&amp;{}&amp;", " ".repeat(quota - 2)),
        quota,
    );

    // Whitespace carried VERBATIM by a CDATA section.
    row(
        "a CDATA section of whitespace, one past the quota",
        format!("<![CDATA[{}]]>", " ".repeat(quota + 1)),
        quota + 1,
    );
    row(
        "a CDATA section of whitespace, at the quota",
        format!("<![CDATA[{}]]>", " ".repeat(quota)),
        quota,
    );
    row(
        "text padded with spaces inside a CDATA section, one past the quota",
        format!("<![CDATA[{}x{}]]>", " ".repeat(50), " ".repeat(50)),
        101,
    );

    // Whitespace around and inside a NESTED replacement element.
    row(
        &format!("whitespace filling a nested <{nested}>, one past the quota"),
        format!(
            "{}<{nested}>{}</{nested}>",
            "x".repeat(quota - 10),
            " ".repeat(11)
        ),
        quota + 1,
    );
    row(
        &format!("whitespace around a nested <{nested}>, one past the quota"),
        format!(
            "{} <{nested}>z</{nested}>{}",
            "x".repeat(quota - 10),
            " ".repeat(9)
        ),
        quota + 1,
    );
    row(
        &format!("whitespace around a nested <{nested}>, at the quota"),
        format!(
            "{} <{nested}>z</{nested}>{}",
            "x".repeat(quota - 11),
            " ".repeat(9)
        ),
        quota,
    );

    for row in &mut rows {
        row.inner = format!("<{outer}>{}</{outer}>", row.inner);
    }
    rows
}

/// RULE: whitespace is character content of a replacement wherever it sits, so the
/// per-lexeme quota is measured over it too and a document whose replacement passes the
/// quota only once its whitespace is counted is refused.
#[tokio::test]
async fn whitespace_in_a_replacement_is_measured_like_any_other_character() {
    // AWS's published quotas for Amazon Polly cap a lexeme at 100 characters.
    assert_eq!(
        MAX_LEXEME_CHARACTERS, 100,
        "the published per-lexeme quota is 100 characters"
    );
    let polly = Polly::new();
    for (outer, nested) in [("alias", "phoneme"), ("phoneme", "alias")] {
        for (index, row) in rows(outer, nested).into_iter().enumerate() {
            let case = format!("<{outer}> with <{nested}> inside: {}", row.case);
            let name = format!("Ws{}{index:02}", outer[..1].to_uppercase());
            let content = pls(&row.inner);
            check(&polly, &case, &name, &content, row.characters).await;
        }
    }
}

/// RULE (the same measurement, as the boundary invariant it implies): padding a
/// replacement with `pad` spaces makes its content `text + pad` characters long, so the
/// document is stored exactly while that sum is at or under the quota — the answer flips
/// once and only at the quota, whichever side the padding sits on.
#[tokio::test]
async fn padding_a_replacement_moves_the_boundary_by_exactly_the_padding() {
    let polly = Polly::new();
    let quota = MAX_LEXEME_CHARACTERS;
    // Text short enough that a few spaces of padding straddle the quota, swept over
    // every padding length from none to past the boundary and over each placement.
    let text = quota - 3;
    for (placement, before, after) in [
        ("leading", true, false),
        ("trailing", false, true),
        ("split across both edges", true, true),
    ] {
        for pad in 0..=8usize {
            let (head, tail) = if before && after {
                (pad / 2, pad - pad / 2)
            } else if before {
                (pad, 0)
            } else {
                (0, pad)
            };
            let inner = format!(
                "{}{}{}",
                " ".repeat(head),
                "x".repeat(text),
                " ".repeat(tail)
            );
            let characters = text + pad;
            let case = format!("{pad} {placement} spaces around {text} characters");
            let name = format!("Pad{}{pad:02}", placement[..1].to_uppercase());
            let content = pls(&format!("<alias>{inner}</alias>"));
            check(&polly, &case, &name, &content, characters).await;
        }
    }
}
