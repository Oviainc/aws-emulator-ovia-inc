// SPDX-License-Identifier: CC0-1.0

//! The UNIT a `TagMap` entry's modelled length is measured in: CHARACTERS, not bytes.
//!
//! The RULE under test: a `tags` entry's modelled length bounds the number of CHARACTERS
//! its key and its value carry, so whether an entry is accepted depends only on those two
//! counts and never on how many bytes the characters encode to. An entry at the bounds is
//! accepted and stored verbatim whatever alphabet it is written in, and one character past
//! either bound is refused with HTTP 400 `ValidationException` with nothing written — the
//! same counts for every alphabet, so the acceptance point of a Japanese key is the
//! acceptance point of an ASCII one.
//!
//! The rows differ along what the rule generalises over: the UTF-8 WIDTH of the characters
//! the entry is written in (1, 2, 3 and 4 bytes per character), which half of the entry
//! carries them (key, value, both), which end of which bound the entry sits at (the
//! minimum, the maximum, one character past the maximum), and which operation carries the
//! map (`CreateDomain`'s `tags` and `TagResource`'s `tags`, the two this service models).
//! A row whose characters are wider than one byte encodes to several times the character
//! bound in bytes — asserted per row — so a bound measured in bytes could not accept it,
//! which is what makes the two units distinguishable here.
//!
//! Sources for every bound and catalog written by hand here:
//!
//! * `smithy_query(query="shape",shape="TagMap,TagKey,TagValue",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `TagMap`'s key targets `TagKey` (`@length` min 1, max 128) and its value targets
//!   `TagValue` (`@length` min 0, max 256); the crate states those numbers as
//!   `state::MIN_TAG_KEY_LEN` / `MAX_TAG_KEY_LEN` / `MAX_TAG_VALUE_LEN`, which is what this
//!   suite imports rather than restating.
//! * `smithy_query(query="digest")` — the operations whose input carries a `TagMap` are
//!   `CreateDomain` (`POST /domain`, 201) and `TagResource` (`POST /tags/{resourceArn}`,
//!   204); the readers the round trip goes back through are `GetDomain`'s `tags` member and
//!   `ListTagsForResource`.
//!
//! The per-length sweep of both bounds, and the multi-entry maps, live in the sibling suite
//! `tests/all/tag_map_bounds.rs`; this one varies the ENCODING of the characters at those
//! bounds and reuses that suite's map builder and refusal assertion so the two cannot
//! disagree about either.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::{
    MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN, MIN_TAG_KEY_LEN,
};

use crate::domain_subscription_lifecycle::{string_at, Emulator};
use crate::list_tags_for_resource::list_tags;
use crate::resource_tagging::{domain_tags, entries, tags_path};
use crate::tag_map_bounds::{assert_refused, of_length, one_tag};

/// The alphabets every bound is exercised in, each with the number of bytes one of its
/// characters encodes to in UTF-8 — one per width UTF-8 has, so the widest row is four
/// times the byte count of the narrowest for the same character count. The width of each
/// character is asserted from `char::len_utf8` rather than trusted, so the table cannot
/// drift from the encoding it claims.
const ALPHABETS: [(&str, char, usize); 4] = [
    ("ASCII", 'k', 1),
    ("two-byte Latin", 'é', 2),
    ("three-byte Japanese", '日', 3),
    ("four-byte supplementary-plane", '🩺', 4),
];

/// One row: a label, the `tags` map one call carries, and whether the rule accepts it.
type Row = (String, Value, bool);

/// A fresh domain to tag, as (`domainId`, `arn`), so no row reads a tag set another row
/// wrote.
async fn fresh_domain(emulator: &Emulator, name: &str) -> (String, String) {
    let created = emulator.create_domain(&json!({ "name": name })).await;
    (
        string_at(&created, "domainId"),
        string_at(&created, "arn"),
    )
}

/// The entries of `tags` as the readers report them.
fn expected_set(tags: &Value) -> BTreeMap<String, String> {
    entries(tags).into_iter().collect()
}

/// The rows for one alphabet: each published bound of each half of an entry, at its
/// minimum, at its maximum and one character past it, written in `character`.
fn rows(form: &str, character: char) -> Vec<Row> {
    let shortest_key = of_length(character, MIN_TAG_KEY_LEN);
    let longest_key = of_length(character, MAX_TAG_KEY_LEN);
    let past_longest_key = of_length(character, MAX_TAG_KEY_LEN + 1);
    let longest_value = of_length(character, MAX_TAG_VALUE_LEN);
    let past_longest_value = of_length(character, MAX_TAG_VALUE_LEN + 1);
    vec![
        (
            format!("a {form} key at the modelled minimum of {MIN_TAG_KEY_LEN} character"),
            one_tag(&shortest_key, "v"),
            true,
        ),
        (
            format!("a {form} key at the modelled maximum of {MAX_TAG_KEY_LEN} characters"),
            one_tag(&longest_key, "v"),
            true,
        ),
        (
            format!("a {form} key one character past the modelled maximum"),
            one_tag(&past_longest_key, "v"),
            false,
        ),
        (
            format!("a {form} value at the modelled maximum of {MAX_TAG_VALUE_LEN} characters"),
            one_tag("k", &longest_value),
            true,
        ),
        (
            format!("a {form} value one character past the modelled maximum"),
            one_tag("k", &past_longest_value),
            false,
        ),
        (
            format!("a {form} key AND value both at their modelled maximum"),
            one_tag(&longest_key, &longest_value),
            true,
        ),
    ]
}

#[tokio::test]
async fn a_tag_entrys_modelled_length_is_counted_in_characters_whatever_alphabet_it_uses() {
    for (form, character, utf8_width) in ALPHABETS {
        assert_eq!(
            character.len_utf8(),
            utf8_width,
            "case {form:?}: the table claims {utf8_width} bytes per character, got {}",
            character.len_utf8()
        );
        // What makes this row distinguish a character count from a byte count: an entry at
        // the character bound occupies that many bytes PER CHARACTER, so every row below
        // the ASCII one is far outside the same number of bytes.
        let longest_key = of_length(character, MAX_TAG_KEY_LEN);
        assert_eq!(
            longest_key.len(),
            MAX_TAG_KEY_LEN * utf8_width,
            "case {form:?}: a {MAX_TAG_KEY_LEN}-character key must encode to \
             {MAX_TAG_KEY_LEN}x{utf8_width} bytes, got {} bytes",
            longest_key.len()
        );

        for (case, tags, accepted) in rows(form, character) {
            // A fresh emulator per row: one row's domains are then never what a later row
            // reads, and no row approaches the account-wide domain cap.
            let emulator = Emulator::new("us-east-1");

            // --- CreateDomain carries the map on the create itself.
            let (status, created) = emulator
                .send(
                    "POST",
                    "/domain",
                    Some(&json!({ "name": "clinic", "tags": tags })),
                )
                .await;
            if accepted {
                assert_eq!(
                    status, 201,
                    "case {case:?}: CreateDomain must accept the entry, got {status} with {created}"
                );
                let domain_id = string_at(&created, "domainId");
                let arn = string_at(&created, "arn");
                let expected = expected_set(&tags);
                let reported = domain_tags(&emulator, &domain_id).await;
                assert_eq!(
                    reported, expected,
                    "case {case:?}: GetDomain must report the entry verbatim, got {reported:?}"
                );
                let listed = list_tags(&emulator, &case, &arn).await;
                assert_eq!(
                    listed, expected,
                    "case {case:?}: ListTagsForResource must report the same set, got {listed:?}"
                );
            } else {
                assert_refused(&format!("{case} (CreateDomain)"), status, &created);
            }

            // --- TagResource carries the same map against a domain that already exists.
            let (domain_id, arn) = fresh_domain(&emulator, "clinic-tagged").await;
            let (status, body) = emulator
                .send("POST", &tags_path(&arn), Some(&json!({ "tags": tags })))
                .await;
            if accepted {
                assert_eq!(
                    status, 204,
                    "case {case:?}: TagResource must accept the entry, got {status} with {body}"
                );
                let expected = expected_set(&tags);
                let reported = domain_tags(&emulator, &domain_id).await;
                assert_eq!(
                    reported, expected,
                    "case {case:?}: the merged set must be the entry verbatim, got {reported:?}"
                );
                let listed = list_tags(&emulator, &case, &arn).await;
                assert_eq!(
                    listed, expected,
                    "case {case:?}: ListTagsForResource must report the same set, got {listed:?}"
                );
            } else {
                assert_refused(&format!("{case} (TagResource)"), status, &body);
                let reported = domain_tags(&emulator, &domain_id).await;
                assert!(
                    reported.is_empty(),
                    "case {case:?}: a refused TagResource must write nothing, got {reported:?}"
                );
                let listed = list_tags(&emulator, &case, &arn).await;
                assert!(
                    listed.is_empty(),
                    "case {case:?}: no reader may report a refused entry, got {listed:?}"
                );
            }
        }
    }
}
