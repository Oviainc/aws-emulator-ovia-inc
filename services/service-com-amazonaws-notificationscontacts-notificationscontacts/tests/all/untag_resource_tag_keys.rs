// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the modelled `TagKey` constraint at the LIST member
//! `UntagResourceRequest$tagKeys`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization (each element travelling as its own repeated `tagKeys=`
//! query parameter), the modelled-constraint walk and the handler — so what is
//! asserted is what a signed AWS SDK request observes, and every tag set is read back
//! through `ListTagsForResource` rather than out of the store.
//!
//! The rule under test: EVERY element of a list whose member targets `TagKey` must
//! satisfy that shape's pattern `^(?!aws:).{1,128}$`
//! (`smithy_query(query="shape",shape="TagKeys")` reports member `TagKey` with exactly
//! that pattern), so a request carrying one element outside it is refused in whole with
//! the modelled 400 `ValidationException` and removes nothing, whatever the list's
//! length, wherever the offending element sits in it, and however many times it is
//! repeated — while a list every element of which the pattern admits removes exactly
//! the keys it names.
//!
//! The sibling `tag_key_constraints` suite spans the VALUES the pattern admits and
//! refuses, one per request; this suite spans the LIST that carries them.

use std::collections::BTreeMap;

use crate::email_contacts::{emulator, error_code, get_ok, unknown_arn};
use crate::resource_tag_writes::untag;
use crate::resource_tags::{create_tagged, pair, tag_map, tags_ok, MAX_TAG_KEY_LEN};

/// The prefix the pattern's leading `(?!aws:)` look-ahead excludes.
const RESERVED_PREFIX: &str = "aws:";

/// The tag set every contact below starts from.
///
/// It deliberately holds keys at the pattern's edges as well as ordinary ones: a key at
/// the maximum length, a key carrying the reserved word in upper case (the pattern is
/// case-sensitive, so `AWS:` is admitted), and a key carrying the reserved prefix away
/// from the start (the look-ahead is anchored at the start).
fn starting_tags() -> BTreeMap<String, String> {
    tag_map([
        pair("team", "infra"),
        pair("env", "prod"),
        pair("tier", "1"),
        pair("AWS:owner", "case-sensitive"),
        pair("team:aws:owner", "not-anchored"),
        (
            "k".repeat(MAX_TAG_KEY_LEN),
            "at-the-bound".to_string(),
        ),
    ])
}

/// One `UntagResource` request: the key list it carries and whether the modelled
/// pattern admits every element of it.
struct Case {
    /// What this row varies, named in every assertion.
    what: String,
    keys: Vec<String>,
    /// `true` when `^(?!aws:).{1,128}$` matches EVERY element of `keys`.
    admitted: bool,
}

fn case(what: impl Into<String>, keys: &[String], admitted: bool) -> Case {
    Case {
        what: what.into(),
        keys: keys.to_vec(),
        admitted,
    }
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// A key list short enough to name in an assertion message, so a 1000-character element
/// does not print a wall of text.
fn shown(list: &[String]) -> String {
    let elements: Vec<String> = list
        .iter()
        .map(|key| {
            let head: String = key.chars().take(16).collect();
            if head.chars().count() == key.chars().count() {
                format!("{head:?}")
            } else {
                format!("{head:?}… [{} characters]", key.chars().count())
            }
        })
        .collect();
    format!("[{}]", elements.join(", "))
}

/// Every row is one `tagKeys` list, the rows differing along what the rule generalises
/// over: how many elements the list carries, whether each is admitted, WHERE an element
/// the pattern refuses sits (first, middle, last, alone), how many refused elements
/// there are, whether an element repeats, and which part of the pattern the refused
/// element breaks (the reserved prefix, each side of the 1..128 bound, a line terminator
/// the `.` excludes).
fn tag_keys_table() -> Vec<Case> {
    let long = "k".repeat(MAX_TAG_KEY_LEN);
    let too_long = "k".repeat(MAX_TAG_KEY_LEN + 1);
    let far_too_long = "k".repeat(1_000);
    let wide_at_bound = "é".repeat(MAX_TAG_KEY_LEN);
    let wide_past_bound = "é".repeat(MAX_TAG_KEY_LEN + 1);
    let reserved = format!("{RESERVED_PREFIX}owner");
    vec![
        case("one element naming a stored key", &keys(&["team"]), true),
        case(
            "several elements naming stored keys",
            &keys(&["team", "env", "tier"]),
            true,
        ),
        case(
            "an element at the pattern's maximum length",
            std::slice::from_ref(&long),
            true,
        ),
        case(
            "an element at the maximum length in multi-byte characters, naming no stored key",
            std::slice::from_ref(&wide_at_bound),
            true,
        ),
        case(
            "elements naming keys the contact does not carry",
            &keys(&["absent-one", "absent-two"]),
            true,
        ),
        case("the same element repeated", &keys(&["team", "team"]), true),
        case(
            "an element carrying the reserved word in upper case, which the pattern admits",
            &keys(&["AWS:owner"]),
            true,
        ),
        case(
            "an element carrying the reserved prefix away from the start",
            &keys(&["team:aws:owner"]),
            true,
        ),
        case(
            "an element that is the reserved word without its colon",
            &keys(&["aws"]),
            true,
        ),
        case(
            "every element the contact carries at once",
            &starting_tags().into_keys().collect::<Vec<String>>(),
            true,
        ),
        case(
            "a reserved element alone",
            &keys(&[RESERVED_PREFIX]),
            false,
        ),
        case(
            "a reserved element first in a list of stored keys",
            &[vec![reserved.clone()], keys(&["team", "env"])].concat(),
            false,
        ),
        case(
            "a reserved element in the middle of a list of stored keys",
            &[keys(&["team"]), vec![reserved.clone()], keys(&["env"])].concat(),
            false,
        ),
        case(
            "a reserved element last in a list of stored keys",
            &[keys(&["team", "env"]), vec![reserved.clone()]].concat(),
            false,
        ),
        case(
            "the same reserved element repeated",
            &[reserved.clone(), reserved.clone()],
            false,
        ),
        case(
            "one element past the maximum length",
            std::slice::from_ref(&too_long),
            false,
        ),
        case(
            "an over-long element beside stored keys",
            &[keys(&["team", "env"]), vec![too_long.clone()]].concat(),
            false,
        ),
        case(
            "an element far past the maximum length",
            std::slice::from_ref(&far_too_long),
            false,
        ),
        case(
            "one multi-byte element past the maximum length",
            std::slice::from_ref(&wide_past_bound),
            false,
        ),
        case(
            "an element at the maximum length beside one past it",
            &[long.clone(), too_long.clone()],
            false,
        ),
        case("the empty element", &[String::new()], false),
        case(
            "an empty element beside a stored key",
            &[keys(&["team"]), vec![String::new()]].concat(),
            false,
        ),
        case(
            "two elements refused for different reasons",
            &[reserved, too_long],
            false,
        ),
        case(
            "an element carrying a line feed",
            &keys(&["line\nbreak"]),
            false,
        ),
        case(
            "an element carrying a carriage return beside a stored key",
            &keys(&["team", "line\rbreak"]),
            false,
        ),
        case(
            "an element carrying the line separator U+2028",
            &keys(&["line\u{2028}break"]),
            false,
        ),
        case(
            "an element carrying the paragraph separator U+2029",
            &keys(&["line\u{2029}break"]),
            false,
        ),
        case(
            "an element that is nothing but a tab, which the dot admits",
            &keys(&["\t"]),
            true,
        ),
    ]
}

/// The tag set a request carrying `removed` must leave behind: a list the pattern admits
/// removes exactly the keys it names, and a list it refuses removes nothing at all.
fn surviving(
    start: &BTreeMap<String, String>,
    removed: &[String],
    admitted: bool,
) -> BTreeMap<String, String> {
    let mut expected = start.clone();
    if admitted {
        for key in removed {
            expected.remove(key);
        }
    }
    expected
}

#[tokio::test]
async fn every_tag_keys_element_is_judged_against_the_modelled_pattern() {
    let em = emulator();
    let table = tag_keys_table();
    assert!(
        table.len() >= 5,
        "the tagKeys table must span the class, got {} rows",
        table.len()
    );

    for (index, case) in table.iter().enumerate() {
        let start = starting_tags();
        let arn = create_tagged(&em, &format!("contact-{index}"), Some(&start)).await;
        let before = get_ok(&em, &arn).await;

        let (status, body) = untag(&em, &arn, &case.keys).await;
        if case.admitted {
            assert_eq!(
                status,
                200,
                "case {:?} ({}): a list every element of which the pattern admits must be \
                 accepted, got {status} / {body:?}",
                case.what,
                shown(&case.keys)
            );
        } else {
            assert_eq!(
                status,
                400,
                "case {:?} ({}): a list carrying an element outside the modelled TagKey \
                 pattern must be refused, got {status} / {body:?}",
                case.what,
                shown(&case.keys)
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {:?} ({}): expected ValidationException, got {body:?}",
                case.what,
                shown(&case.keys)
            );
        }

        let expected = surviving(&start, &case.keys, case.admitted);
        let reported = tags_ok(&em, &arn).await;
        assert_eq!(
            reported,
            expected,
            "case {:?} ({}): only the keys a legal request named may be removed, got {reported:?}",
            case.what,
            shown(&case.keys)
        );

        // The removal is a write to the tag set alone: the members `EmailContact`
        // reports are the ones the create stored, refused request or not.
        let after = get_ok(&em, &arn).await;
        assert_eq!(
            after,
            before,
            "case {:?} ({}): the contact's own members must be untouched, got {after:?}",
            case.what,
            shown(&case.keys)
        );
    }
}

/// One refused element refuses the WHOLE list: not even the elements the pattern admits
/// take effect, and the same list with that one element dropped removes every key it
/// names.
#[tokio::test]
async fn one_refused_element_refuses_the_whole_list() {
    let em = emulator();
    let start = starting_tags();
    let legal: Vec<String> = start.keys().cloned().collect();

    for (index, (what, illegal)) in [
        ("a reserved element", format!("{RESERVED_PREFIX}owner")),
        ("the reserved prefix alone", RESERVED_PREFIX.to_string()),
        ("an over-long element", "k".repeat(MAX_TAG_KEY_LEN + 1)),
        ("the empty element", String::new()),
        ("an element carrying a line terminator", "line\u{2028}break".to_string()),
    ]
    .into_iter()
    .enumerate()
    {
        let arn = create_tagged(&em, &format!("atomic-{index}"), Some(&start)).await;

        // Every key the contact carries, plus the one element the pattern refuses.
        let mut request = legal.clone();
        request.push(illegal);
        let (status, body) = untag(&em, &arn, &request).await;
        assert_eq!(
            status, 400,
            "case {what:?}: a list carrying one refused element must be refused, got \
             {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {what:?}: expected ValidationException, got {body:?}"
        );
        let reported = tags_ok(&em, &arn).await;
        assert_eq!(
            reported, start,
            "case {what:?}: a refused removal must leave every stored pair, got {reported:?}"
        );

        // The same list with its one refused element dropped empties the set, so the
        // refusal above came from that element and not from the list's length or shape.
        let (status, body) = untag(&em, &arn, &legal).await;
        assert_eq!(
            status, 200,
            "case {what:?}: the same list without the refused element must be accepted, got \
             {status} / {body:?}"
        );
        let reported = tags_ok(&em, &arn).await;
        assert!(
            reported.is_empty(),
            "case {what:?}: every key the legal list named must be gone, got {reported:?}"
        );
    }
}

/// An element outside the pattern fails the INPUT shape, so it is refused whatever
/// resource the ARN names — while the same list with every element admitted earns the
/// modelled 404 for an ARN naming no contact.
#[tokio::test]
async fn a_refused_element_is_judged_before_the_contact_is_resolved() {
    let em = emulator();
    let survivor = create_tagged(&em, "survivor", Some(&starting_tags())).await;
    let unknown = unknown_arn('q');

    for (what, list) in [
        (
            "a reserved element beside a legal one",
            vec![format!("{RESERVED_PREFIX}owner"), "team".to_string()],
        ),
        (
            "an over-long element alone",
            vec!["k".repeat(MAX_TAG_KEY_LEN + 1)],
        ),
        ("the empty element alone", vec![String::new()]),
    ] {
        let (status, body) = untag(&em, &unknown, &list).await;
        assert_eq!(
            status, 400,
            "case {what:?}: an unknown arn must not excuse a refused element, got \
             {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {what:?}: expected ValidationException, got {body:?}"
        );
    }

    let (status, body) = untag(&em, &unknown, &keys(&["team", "env"])).await;
    assert_eq!(
        status, 404,
        "case \"a list of legal elements on an unknown arn\": expected 404, got \
         {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"a list of legal elements on an unknown arn\": expected \
         ResourceNotFoundException, got {body:?}"
    );

    let reported = tags_ok(&em, &survivor).await;
    assert_eq!(
        reported,
        starting_tags(),
        "no refusal may disturb the contact that does exist, got {reported:?}"
    );
}

/// `@idempotent`: repeating a list the pattern admits leaves the same set, and repeating
/// one it refuses answers the same refusal — so neither answer depends on how often the
/// caller asked.
#[tokio::test]
async fn repeating_a_request_answers_the_same_way() {
    let em = emulator();
    let start = starting_tags();

    for (index, case) in tag_keys_table().iter().enumerate() {
        let arn = create_tagged(&em, &format!("repeat-{index}"), Some(&start)).await;
        let expected = surviving(&start, &case.keys, case.admitted);
        let wanted_status = if case.admitted { 200 } else { 400 };

        for attempt in 1..=2 {
            let (status, body) = untag(&em, &arn, &case.keys).await;
            assert_eq!(
                status,
                wanted_status,
                "case {:?} ({}), attempt {attempt}: expected {wanted_status}, got \
                 {status} / {body:?}",
                case.what,
                shown(&case.keys)
            );
            let reported = tags_ok(&em, &arn).await;
            assert_eq!(
                reported,
                expected,
                "case {:?} ({}), attempt {attempt}: the stored set must not depend on how \
                 often the request was made, got {reported:?}",
                case.what,
                shown(&case.keys)
            );
        }
    }
}
