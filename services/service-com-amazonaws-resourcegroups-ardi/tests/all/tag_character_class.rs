// SPDX-License-Identifier: CC0-1.0

//! The character class a tag key and a tag value must stay inside, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suites' `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The rule under test, asserted over a TABLE of inputs spanning the Unicode
//! general categories the class is written in rather than one row: a tag key and a
//! tag value are stored exactly when every character of them is one the modelled
//! pattern `TagKey` and `TagValue` share,
//! `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` — the general categories L, Z and N plus the
//! eight punctuation characters it lists
//! (`smithy_query(query="shape",shape="TagKey,TagValue")`) — and a text carrying
//! any other character is `BadRequestException` (400) that stores nothing, on
//! `Tag`, on `CreateGroup`'s `Tags` and on `Untag`'s `Keys` alike.
//!
//! The rows that matter most are the combining marks (general categories Mn and
//! Mc): Unicode's `Alphabetic` PROPERTY holds them, the general category L does
//! not, so a service deciding the class from Rust's `char` predicates admits a
//! text the modelled pattern refuses.

use std::collections::BTreeMap;

use serde_json::json;

use super::group_tags::{group_arn, stored_tags, tag, tag_json};
use super::group_untag::untag;
use super::resource_group_lifecycle::{error_code, Emulator};

/// One row of the character table: a tag text, and whether the modelled pattern
/// admits every character of it.
struct TagTextCase {
    /// What the row is about, named in every assertion of it.
    case: &'static str,
    /// The text, used once as a tag KEY and once as a tag VALUE.
    text: &'static str,
    /// Whether the modelled pattern admits it.
    admitted: bool,
}

/// The texts every case below drives, differing along the Unicode general
/// category of the character that decides the row — the categories the modelled
/// pattern names (L, Z, N) and the ones it does not (M, C, P, S) — and along which
/// of the eight punctuation characters a text carries.
///
/// Source for each category membership: the Unicode character database entry of
/// the character named in the row, against the class the modelled pattern lists
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`).
fn tag_text_rows() -> Vec<TagTextCase> {
    vec![
        TagTextCase {
            case: "ASCII letters, a digit and a space (Lu, Ll, Nd, Zs)",
            text: "Stage 2",
            admitted: true,
        },
        TagTextCase {
            case: "precomposed non-ASCII letters (Lu, Ll)",
            text: "Équipe",
            admitted: true,
        },
        TagTextCase {
            case: "letters outside the Latin script (Lo)",
            text: "日本語ก",
            admitted: true,
        },
        TagTextCase {
            case: "a modifier letter (Lm)",
            text: "aᵃ",
            admitted: true,
        },
        TagTextCase {
            case: "a letter-number, ROMAN NUMERAL EIGHT (Nl)",
            text: "Ⅷ",
            admitted: true,
        },
        TagTextCase {
            case: "an other-number, VULGAR FRACTION ONE HALF (No)",
            text: "½",
            admitted: true,
        },
        TagTextCase {
            case: "a non-ASCII decimal digit, ARABIC-INDIC DIGIT THREE (Nd)",
            text: "٣",
            admitted: true,
        },
        TagTextCase {
            case: "a separator that is not the space, NO-BREAK SPACE (Zs)",
            text: "a\u{00a0}b",
            admitted: true,
        },
        TagTextCase {
            case: "a line separator, LINE SEPARATOR (Zl)",
            text: "a\u{2028}b",
            admitted: true,
        },
        TagTextCase {
            case: "every punctuation character the class lists",
            text: "ns:key_part.one/two=three+four-five@six",
            admitted: true,
        },
        TagTextCase {
            case: "a non-spacing mark the Alphabetic property holds, THAI CHARACTER SARA I (Mn)",
            text: "ก\u{0e34}",
            admitted: false,
        },
        TagTextCase {
            case: "a spacing mark the Alphabetic property holds, DEVANAGARI VOWEL SIGN AA (Mc)",
            text: "क\u{093e}",
            admitted: false,
        },
        TagTextCase {
            case: "a Hebrew point the Alphabetic property holds, HEBREW POINT HIRIQ (Mn)",
            text: "ב\u{05b4}",
            admitted: false,
        },
        TagTextCase {
            case: "a non-spacing mark outside Alphabetic, COMBINING GRAVE ACCENT (Mn)",
            text: "e\u{0300}",
            admitted: false,
        },
        TagTextCase {
            case: "punctuation outside the eight the class lists, ASTERISK (Po)",
            text: "star*key",
            admitted: false,
        },
        TagTextCase {
            case: "punctuation outside the eight the class lists, LEFT PARENTHESIS (Ps)",
            text: "paren(text)",
            admitted: false,
        },
        TagTextCase {
            case: "a currency symbol, EURO SIGN (Sc)",
            text: "price€",
            admitted: false,
        },
        TagTextCase {
            case: "a pictographic symbol, GRINNING FACE (So)",
            text: "face😀",
            admitted: false,
        },
        TagTextCase {
            case: "a control character, which is no separator, CHARACTER TABULATION (Cc)",
            text: "tab\tkey",
            admitted: false,
        },
        TagTextCase {
            case: "a format character, ZERO WIDTH SPACE (Cf)",
            text: "a\u{200b}b",
            admitted: false,
        },
    ]
}

/// The key a row's text is written UNDER when the row exercises a tag value, so
/// the value is the only half of the pair under test.
const CARRIER_KEY: &str = "Carrier";

/// `Tag` stores a pair exactly when the modelled pattern admits every character of
/// its key and of its value, and a pair it refuses is 400 `BadRequestException`
/// that stores nothing — read back through `GetTags` either way.
#[tokio::test]
async fn tagging_admits_exactly_the_characters_the_modelled_pattern_admits() {
    let emulator = Emulator::new();
    for (index, row) in tag_text_rows().into_iter().enumerate() {
        let TagTextCase {
            case,
            text,
            admitted,
        } = row;
        let name = format!("chars-{index}");
        emulator.ok("/groups", &json!({ "Name": name })).await;
        let arn = group_arn("us-east-1", &name);
        let mut expected: BTreeMap<String, String> = BTreeMap::new();

        // The text as a tag KEY, which no boundary walk judges: the map key's
        // modelled constraints are this service's own to decide.
        let (status, body) = tag(
            &emulator,
            "us-east-1",
            &arn,
            &tag_json(&[(text.to_string(), "stored".to_string())]),
        )
        .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?} as a tag key: expected Tag to store {text:?}, got {status} {body}",
            );
            expected.insert(text.to_string(), "stored".to_string());
        } else {
            assert_eq!(
                (status, error_code(&body)),
                (400, "BadRequestException"),
                "case {case:?} as a tag key: expected Tag to refuse {text:?} with \
                 400 BadRequestException, got {status} {body}",
            );
        }
        let stored = stored_tags(&emulator, "us-east-1", case, &arn).await;
        assert_eq!(
            stored, expected,
            "case {case:?} as a tag key: expected the stored map {expected:?}, got {stored:?}",
        );

        // The same text as a tag VALUE, under a key every row admits.
        let (status, body) = tag(
            &emulator,
            "us-east-1",
            &arn,
            &tag_json(&[(CARRIER_KEY.to_string(), text.to_string())]),
        )
        .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?} as a tag value: expected Tag to store {text:?}, got {status} {body}",
            );
            expected.insert(CARRIER_KEY.to_string(), text.to_string());
        } else {
            assert_eq!(
                (status, error_code(&body)),
                (400, "BadRequestException"),
                "case {case:?} as a tag value: expected Tag to refuse {text:?} with \
                 400 BadRequestException, got {status} {body}",
            );
        }
        let stored = stored_tags(&emulator, "us-east-1", case, &arn).await;
        assert_eq!(
            stored, expected,
            "case {case:?} as a tag value: expected the stored map {expected:?}, got {stored:?}",
        );
    }
}

/// `CreateGroup`'s own `Tags` member judges a tag key by the same class, and a
/// refused create creates nothing.
#[tokio::test]
async fn create_group_judges_a_tag_key_by_the_same_class() {
    let emulator = Emulator::new();
    for (index, row) in tag_text_rows().into_iter().enumerate() {
        let TagTextCase {
            case,
            text,
            admitted,
        } = row;
        let name = format!("created-chars-{index}");
        let (status, body) = emulator
            .call(
                "/groups",
                &json!({ "Name": name, "Tags": tag_json(&[(text.to_string(), "created".to_string())]) }),
            )
            .await;
        if !admitted {
            assert_eq!(
                (status, error_code(&body)),
                (400, "BadRequestException"),
                "case {case:?}: expected CreateGroup to refuse the tag key {text:?} with \
                 400 BadRequestException, got {status} {body}",
            );
            let (read_status, read_body) = emulator
                .call("/get-group", &json!({ "Group": name }))
                .await;
            assert_eq!(
                (read_status, error_code(&read_body)),
                (404, "NotFoundException"),
                "case {case:?}: a refused create must create nothing, \
                 got {read_status} {read_body}",
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateGroup to accept the tag key {text:?}, \
             got {status} {body}",
        );
        let arn = group_arn("us-east-1", &name);
        let expected: BTreeMap<String, String> =
            [(text.to_string(), "created".to_string())].into_iter().collect();
        let stored = stored_tags(&emulator, "us-east-1", case, &arn).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the created group to carry {expected:?}, got {stored:?}",
        );
    }
}

/// `Untag` judges a key by the same class: a key the class admits is removed (and
/// removing one the group never carried is a no-op success), while a key carrying a
/// character outside it is 400 `BadRequestException` and removes nothing.
#[tokio::test]
async fn untagging_judges_a_key_by_the_same_class() {
    let emulator = Emulator::new();
    emulator
        .ok("/groups", &json!({ "Name": "untag-chars" }))
        .await;
    let arn = group_arn("us-east-1", "untag-chars");

    // Every text the class admits, written through `Tag` so the store — not this
    // test — decides what is stored.
    let admitted: Vec<&str> = tag_text_rows()
        .iter()
        .filter(|row| row.admitted)
        .map(|row| row.text)
        .collect();
    let pairs: Vec<(String, String)> = admitted
        .iter()
        .map(|text| ((*text).to_string(), "stored".to_string()))
        .collect();
    let (status, body) = tag(&emulator, "us-east-1", &arn, &tag_json(&pairs)).await;
    assert_eq!(
        status, 200,
        "writing the admitted keys expected 200, got {status} {body}",
    );
    let mut expected: BTreeMap<String, String> = pairs.into_iter().collect();
    let stored = stored_tags(&emulator, "us-east-1", "the admitted keys", &arn).await;
    assert_eq!(
        stored, expected,
        "expected the starting map {expected:?}, got {stored:?}",
    );

    for row in tag_text_rows() {
        let TagTextCase {
            case,
            text,
            admitted,
        } = row;
        let (status, body) = untag(
            &emulator,
            "us-east-1",
            &arn,
            &[text.to_string()],
        )
        .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected Untag to accept the key {text:?}, got {status} {body}",
            );
            expected.remove(text);
        } else {
            assert_eq!(
                (status, error_code(&body)),
                (400, "BadRequestException"),
                "case {case:?}: expected Untag to refuse the key {text:?} with \
                 400 BadRequestException, got {status} {body}",
            );
        }
        let stored = stored_tags(&emulator, "us-east-1", case, &arn).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the stored map {expected:?}, got {stored:?}",
        );
    }

    assert!(
        expected.is_empty(),
        "every admitted key must have been removed, {expected:?} were left",
    );
}
