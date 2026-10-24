// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the modelled `TagKey` constraint, wherever this service
//! accepts a tag key.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a tag key this service accepts must satisfy the modelled
//! `TagKey` pattern `^(?!aws:).{1,128}$`
//! (`smithy_query(query="shape",shape="TagKey")`) — 1 to 128 characters, none of them
//! the line feed `.` excludes, not beginning with the reserved `aws:` prefix — and a
//! request naming a key outside it is the modelled 400 `ValidationException` that
//! persists and removes nothing. It holds at every member whose target is `TagKey`:
//! `CreateEmailContact$tags` and `TagResource$tags` (through `TagMap`'s key) and
//! `UntagResource$tagKeys` (through `TagKeys`' member).

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::email_contacts::{call, create, emulator, error_code, list, Emulator};
use crate::resource_tag_writes::{tag, untag};
use crate::resource_tags::{create_tagged, pair, tag_map, tags_ok, MAX_TAG_KEY_LEN};

/// The fewest characters the modelled pattern's `.{1,128}` admits — `TagKey` carries no
/// separate `@length`, so the bound lives only inside the pattern
/// (`smithy_query(query="shape",shape="TagKey")`).
const MIN_TAG_KEY_LEN: usize = 1;

/// The prefix the pattern's leading `(?!aws:)` look-ahead excludes.
const RESERVED_PREFIX: &str = "aws:";

/// One tag key and whether the modelled `TagKey` pattern admits it.
struct Case {
    /// What this row varies, named in every assertion.
    what: &'static str,
    key: String,
    /// `true` when `^(?!aws:).{1,128}$` matches `key`.
    admitted: bool,
}

fn case(what: &'static str, key: impl Into<String>, admitted: bool) -> Case {
    Case {
        what,
        key: key.into(),
        admitted,
    }
}

/// Every row is one tag key, the rows differing along what the pattern generalises
/// over: length (at each bound of `.{1,128}` and one past the maximum), the characters
/// a key carries (`.` is every scalar value but the line feed, so punctuation, spaces
/// and non-ASCII text are all admitted), how wide those characters are in UTF-8 (the
/// bound counts characters, not bytes), and where the reserved `aws:` prefix sits (the
/// look-ahead is anchored at the start and is case-sensitive).
///
/// The reserved-prefix rows deliberately include the boundary cases the issue did not
/// cite: `aws` and `awsome` are NOT the reserved prefix, and `AWS:` is admitted because
/// the modelled pattern is case-sensitive.
fn tag_key_table() -> Vec<Case> {
    vec![
        case("an ordinary key", "team", true),
        case(
            "the shortest key the pattern admits",
            "k".repeat(MIN_TAG_KEY_LEN),
            true,
        ),
        case(
            "punctuation the dot admits",
            "cost:center/eu-west=1+2.3_4@5",
            true,
        ),
        case("a key carrying a space", "spaced key", true),
        case("non-ascii characters", "équipe", true),
        case("the reserved word without its colon", "aws", true),
        case("a word that merely starts with aws", "awsome", true),
        case(
            "the reserved prefix away from the start",
            "team:aws:owner",
            true,
        ),
        case(
            "the reserved prefix in upper case, which the case-sensitive pattern admits",
            "AWS:owner",
            true,
        ),
        case(
            "a key at the maximum length",
            "k".repeat(MAX_TAG_KEY_LEN),
            true,
        ),
        case(
            "a key at the maximum length in multi-byte characters",
            "é".repeat(MAX_TAG_KEY_LEN),
            true,
        ),
        case("the reserved prefix with a name", "aws:owner", false),
        case("the reserved prefix alone", RESERVED_PREFIX, false),
        case(
            "the reserved prefix on an otherwise long key",
            format!("{RESERVED_PREFIX}{}", "k".repeat(100)),
            false,
        ),
        case("the empty key, one below the minimum", "", false),
        case(
            "one character past the maximum",
            "k".repeat(MAX_TAG_KEY_LEN + 1),
            false,
        ),
        case(
            "one multi-byte character past the maximum",
            "é".repeat(MAX_TAG_KEY_LEN + 1),
            false,
        ),
        case("far past the maximum", "k".repeat(1_000), false),
        case(
            "a key carrying the line feed the dot excludes",
            "line\nfeed",
            false,
        ),
    ]
}

/// A key short enough to name in an assertion message, so a 1000-character row does not
/// print a wall of text.
fn shown(key: &str) -> String {
    let head: String = key.chars().take(24).collect();
    if head.chars().count() == key.chars().count() {
        format!("{head:?}")
    } else {
        format!("{head:?}… [{} characters]", key.chars().count())
    }
}

/// `POST /2022-09-19/emailcontacts` with one tag pair — `CreateEmailContact`.
async fn create_with_tag(em: &Emulator, name: &str, key: &str, value: &str) -> (u16, Value) {
    let mut tags = Map::new();
    tags.insert(key.to_string(), json!(value));
    create(
        em,
        &json!({
            "name": name,
            "emailAddress": format!("{name}@example.com"),
            "tags": Value::Object(tags),
        }),
    )
    .await
}

/// How many email contacts the store currently holds, read through `ListEmailContacts`.
async fn contact_count(em: &Emulator) -> usize {
    let (status, body) = list(em, "").await;
    assert_eq!(
        status, 200,
        "ListEmailContacts must answer 200, got {status} / {body:?}"
    );
    body["emailContacts"]
        .as_array()
        .unwrap_or_else(|| panic!("emailContacts is @required, got {body:?}"))
        .len()
}

/// The ARN of a contact carrying exactly `key` — used by the removal suite, which can
/// only remove a key a create was allowed to store.
async fn arn_tagged_with(em: &Emulator, name: &str, key: &str) -> String {
    create_tagged(
        em,
        name,
        Some(&tag_map([pair(key, "stored"), pair("keep", "me")])),
    )
    .await
}

fn assert_refused(case: &Case, operation: &str, status: u16, body: &Value) {
    assert_eq!(
        status,
        400,
        "case {:?} ({})/{operation}: a key outside the modelled TagKey pattern must be \
         refused, got {status} / {body:?}",
        case.what,
        shown(&case.key)
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {:?} ({})/{operation}: expected ValidationException, got {body:?}",
        case.what,
        shown(&case.key)
    );
}

#[tokio::test]
async fn create_email_contact_applies_the_modelled_tag_key_pattern() {
    let em = emulator();
    let table = tag_key_table();
    assert!(
        table.len() >= 5,
        "the tag-key table must span the class, got {} rows",
        table.len()
    );

    for (index, case) in table.iter().enumerate() {
        let before = contact_count(&em).await;
        let name = format!("contact-{index}");
        let (status, body) = create_with_tag(&em, &name, &case.key, "v").await;

        if case.admitted {
            assert_eq!(
                status,
                201,
                "case {:?} ({}): a key the pattern admits must be created, got {status} / {body:?}",
                case.what,
                shown(&case.key)
            );
            let arn = body["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateEmailContact must return an arn, got {body:?}"))
                .to_string();
            let reported = tags_ok(&em, &arn).await;
            assert_eq!(
                reported,
                tag_map([pair(&case.key, "v")]),
                "case {:?} ({}): the created contact must carry exactly the tag it named, got \
                 {} pairs",
                case.what,
                shown(&case.key),
                reported.len()
            );
            assert_eq!(
                contact_count(&em).await,
                before + 1,
                "case {:?} ({}): an accepted create must store one contact",
                case.what,
                shown(&case.key)
            );
        } else {
            assert_refused(case, "CreateEmailContact", status, &body);
            assert_eq!(
                contact_count(&em).await,
                before,
                "case {:?} ({}): a refused create must persist nothing",
                case.what,
                shown(&case.key)
            );
        }
    }
}

#[tokio::test]
async fn tag_resource_applies_the_modelled_tag_key_pattern() {
    let em = emulator();
    let start = tag_map([pair("keep", "me")]);

    for (index, case) in tag_key_table().iter().enumerate() {
        let arn = create_tagged(&em, &format!("contact-{index}"), Some(&start)).await;
        let request: BTreeMap<String, String> = tag_map([pair(&case.key, "v")]);
        let (status, body) = tag(&em, &arn, &request).await;

        let expected = if case.admitted {
            assert_eq!(
                status,
                200,
                "case {:?} ({}): a key the pattern admits must be stored, got {status} / {body:?}",
                case.what,
                shown(&case.key)
            );
            let mut merged = start.clone();
            merged.extend(request);
            merged
        } else {
            assert_refused(case, "TagResource", status, &body);
            start.clone()
        };

        let reported = tags_ok(&em, &arn).await;
        assert_eq!(
            reported,
            expected,
            "case {:?} ({}): the persisted set must hold exactly the accepted keys, got {} pairs",
            case.what,
            shown(&case.key),
            reported.len()
        );
    }
}

#[tokio::test]
async fn untag_resource_applies_the_modelled_tag_key_pattern() {
    let em = emulator();

    for (index, case) in tag_key_table().iter().enumerate() {
        let name = format!("contact-{index}");
        // A key the pattern admits is stored first, so the removal has something to
        // remove; a key it refuses can never have been stored, so that contact starts
        // with the one key the removal must leave alone.
        let (arn, start) = if case.admitted {
            (
                arn_tagged_with(&em, &name, &case.key).await,
                tag_map([pair(&case.key, "stored"), pair("keep", "me")]),
            )
        } else {
            let start = tag_map([pair("keep", "me")]);
            (
                create_tagged(&em, &name, Some(&start)).await,
                start.clone(),
            )
        };

        let (status, body) = untag(&em, &arn, std::slice::from_ref(&case.key)).await;
        let expected = if case.admitted {
            assert_eq!(
                status,
                200,
                "case {:?} ({}): a key the pattern admits must be removable, got {status} / \
                 {body:?}",
                case.what,
                shown(&case.key)
            );
            tag_map([pair("keep", "me")])
        } else {
            assert_refused(case, "UntagResource", status, &body);
            start.clone()
        };

        let reported = tags_ok(&em, &arn).await;
        assert_eq!(
            reported,
            expected,
            "case {:?} ({}): only a key the request legally named may be removed, got {} pairs",
            case.what,
            shown(&case.key),
            reported.len()
        );
    }
}

#[tokio::test]
async fn a_refused_tag_key_is_judged_before_the_resource_is_resolved() {
    let em = emulator();
    // The key fails the input shape, so the refusal is the 400 the shape earns rather
    // than the 404 the ARN would earn: an input AWS refuses is refused whatever
    // resource the request names.
    let unknown = crate::email_contacts::unknown_arn('q');
    let reserved = format!("{RESERVED_PREFIX}owner");

    let (status, body) = tag(&em, &unknown, &tag_map([pair(&reserved, "v")])).await;
    assert_eq!(
        status, 400,
        "case \"TagResource on an unknown arn with a reserved key\": expected 400, got \
         {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"TagResource on an unknown arn with a reserved key\": expected \
         ValidationException, got {body:?}"
    );

    let (status, body) = untag(&em, &unknown, std::slice::from_ref(&reserved)).await;
    assert_eq!(
        status, 400,
        "case \"UntagResource on an unknown arn with a reserved key\": expected 400, got \
         {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"UntagResource on an unknown arn with a reserved key\": expected \
         ValidationException, got {body:?}"
    );

    // A well-formed key on that same unknown ARN is still the modelled 404, so the
    // refusal above came from the key and not from the ARN.
    let (status, body) = tag(&em, &unknown, &tag_map([pair("team", "v")])).await;
    assert_eq!(
        status, 404,
        "case \"TagResource on an unknown arn with a legal key\": expected 404, got \
         {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"TagResource on an unknown arn with a legal key\": expected \
         ResourceNotFoundException, got {body:?}"
    );
}

#[tokio::test]
async fn a_refused_key_beside_legal_ones_persists_none_of_them() {
    let em = emulator();
    let start = tag_map([pair("keep", "me")]);
    let arn = create_tagged(&em, "atomic", Some(&start)).await;

    // Each row is one request mixing keys the pattern admits with exactly one it does
    // not: the whole request is refused, so not even the legal pairs land.
    for (what, request) in [
        (
            "a reserved key beside a legal one",
            tag_map([pair("team", "infra"), pair("aws:owner", "x")]),
        ),
        (
            "an over-long key beside two legal ones",
            tag_map([
                pair("team", "infra"),
                pair("env", "prod"),
                pair(&"k".repeat(MAX_TAG_KEY_LEN + 1), "x"),
            ]),
        ),
        (
            "an empty key beside a legal one",
            tag_map([pair("team", "infra"), pair("", "x")]),
        ),
    ] {
        let (status, body) = tag(&em, &arn, &request).await;
        assert_eq!(
            status, 400,
            "case {what:?}: a request carrying one illegal key must be refused, got \
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
            "case {what:?}: a refused request must persist none of its pairs, got {reported:?}"
        );
    }

    // And the same request with its one illegal key dropped is accepted, so the refusal
    // was the key and not the request's shape.
    let legal = tag_map([pair("team", "infra"), pair("env", "prod")]);
    let (status, body) = tag(&em, &arn, &legal).await;
    assert_eq!(
        status, 200,
        "case \"the same pairs without the illegal key\": expected 200, got {status} / {body:?}"
    );
    let mut expected = start.clone();
    expected.extend(legal);
    let reported = tags_ok(&em, &arn).await;
    assert_eq!(
        reported, expected,
        "case \"the same pairs without the illegal key\": the legal pairs must land, got \
         {reported:?}"
    );
}

#[tokio::test]
async fn a_key_the_pattern_admits_round_trips_through_every_tag_operation() {
    let em = emulator();
    // One pass over the admitted rows through create -> read -> tag -> read -> untag ->
    // read, so a key is not merely accepted but stored and reported verbatim.
    for (index, case) in tag_key_table().iter().filter(|c| c.admitted).enumerate() {
        let arn = create_tagged(
            &em,
            &format!("round-{index}"),
            Some(&tag_map([pair(&case.key, "created")])),
        )
        .await;
        let reported = tags_ok(&em, &arn).await;
        assert_eq!(
            reported.get(&case.key).map(String::as_str),
            Some("created"),
            "case {:?} ({}): the created key must be reported verbatim, got {reported:?}",
            case.what,
            shown(&case.key)
        );

        assert_eq!(
            tag(&em, &arn, &tag_map([pair(&case.key, "retagged")])).await.0,
            200,
            "case {:?} ({}): retagging the same key must succeed",
            case.what,
            shown(&case.key)
        );
        let reported = tags_ok(&em, &arn).await;
        assert_eq!(
            reported.get(&case.key).map(String::as_str),
            Some("retagged"),
            "case {:?} ({}): the retagged value must replace the created one, got {reported:?}",
            case.what,
            shown(&case.key)
        );

        assert_eq!(
            untag(&em, &arn, std::slice::from_ref(&case.key)).await.0,
            200,
            "case {:?} ({}): removing the same key must succeed",
            case.what,
            shown(&case.key)
        );
        let reported = tags_ok(&em, &arn).await;
        assert!(
            reported.is_empty(),
            "case {:?} ({}): the key must be gone, got {reported:?}",
            case.what,
            shown(&case.key)
        );
    }
}

/// `GET /tags/{arn}` carries no tag key, so the `TagKey` rule has nothing to say about
/// it: a contact holding keys at the pattern's bounds still reports them unchanged.
#[tokio::test]
async fn list_tags_for_resource_reports_keys_at_the_patterns_bounds() {
    let em = emulator();
    let bounds = tag_map([
        pair(&"k".repeat(MIN_TAG_KEY_LEN), "shortest"),
        pair(&"k".repeat(MAX_TAG_KEY_LEN), "longest"),
        pair("AWS:owner", "case-sensitive"),
    ]);
    let arn = create_tagged(&em, "bounds", Some(&bounds)).await;
    let reported = tags_ok(&em, &arn).await;
    assert_eq!(
        reported, bounds,
        "keys at the pattern's bounds must be reported verbatim, got {reported:?}"
    );

    // The read is a report: it leaves the set exactly as it was.
    let (status, body) = call(
        &em,
        "GET",
        &format!("/tags/{}", crate::email_contacts::label(&arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListTagsForResource must answer 200, got {status} / {body:?}"
    );
    let again = tags_ok(&em, &arn).await;
    assert_eq!(
        again, bounds,
        "a second read must report the same set, got {again:?}"
    );
}
