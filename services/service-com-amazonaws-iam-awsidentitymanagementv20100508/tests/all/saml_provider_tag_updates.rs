// SPDX-License-Identifier: CC0-1.0

//! `TagSAMLProvider` and `UntagSAMLProvider`, end to end over the awsQuery wire, read
//! back through `GetSAMLProvider` and `ListSAMLProviderTags`.
//!
//! RULE under test: a tag call MERGES the supplied tags into the tag set the named
//! provider persists — a supplied key the provider already holds has its value
//! overwritten and a key it does not hold is added — bounded by the per-resource tag
//! quota, with a request carrying any invalid tag storing nothing; an untag call drops
//! exactly the held tags whose keys it names, comparing keys exactly; and the set either
//! leaves behind is what every later read reports, sorted by tag key.
//! `TagSAMLProvider`'s own documentation: "if a tag with the same key name already
//! exists, then that tag is overwritten with the new value", and "if any one of the tags
//! is invalid or if you exceed the allowed maximum number of tags, then the entire
//! request fails"; `UntagSAMLProvider$TagKeys`: "the tags with matching keys are removed
//! from the specified SAML identity provider"; `GetSAMLProvider$Tags`: "the returned list
//! of tags is sorted by tag key".
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour of a provider that was PERSISTED rather than a handler called directly.
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns, the parameter and
//! tag readers are `tests/all/role_lifecycle.rs`'s, the tag-list builders are
//! `tests/all/role_tags.rs`'s and the metadata document a create needs comes from
//! `tests/all/saml_providers.rs`, so a request is built and read exactly one way across
//! this crate.
//!
//! The suites are tables whose rows differ along what the rule generalises over: the held
//! tag count, the number of calls, whether a key is new or held, the case of a key, keys
//! repeated inside one request, how many keys one untag names, the alphabet and the
//! published length bounds of `tagKeyType` (1..128) and `tagValueType` (0..256), and the
//! published per-resource tag quota.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_TAGS_PER_ENTITY;
use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::role_tags::{expected_map, tags, with_tag_params};
use crate::saml_providers::{MetadataForm, expected_arn, metadata_document};
use crate::user_lifecycle::{call, element, elements, error_code, router};

/// The `TagKeys.member.N` parameters of an awsQuery tag-key list, after the parameters
/// `leading` carries.
fn with_key_params(leading: Vec<(String, String)>, keys: &[String]) -> Vec<(String, String)> {
    let mut params = leading;
    for (index, key) in keys.iter().enumerate() {
        params.push(param(&format!("TagKeys.member.{}", index + 1), key));
    }
    params
}

/// `list` as the owned key list a table row names.
fn keys(list: &[&str]) -> Vec<String> {
    list.iter().map(|key| (*key).to_string()).collect()
}

/// `count` distinct tags, the fixture the quota rows are built from.
fn fixture(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect()
}

/// Create the provider `name` carrying `initial`, and answer the ARN it is held under.
async fn create_provider(
    router: &MultiServiceRouter,
    name: &str,
    initial: &[(String, String)],
) -> String {
    let document = metadata_document(MetadataForm::Entity, "https://idp.example.com/saml", None);
    let params = with_tag_params(
        vec![
            param("Name", name),
            param("SAMLMetadataDocument", &document),
        ],
        initial,
    );
    let (status, body) = call(router, "CreateSAMLProvider", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture CreateSAMLProvider must succeed, got {status}: {body}"
    );
    let arn = element(&body, "SAMLProviderArn");
    assert_eq!(
        arn,
        expected_arn(name),
        "case {name:?}: the create must hold the provider under the ARN derived from its \
         name, got {arn:?}"
    );
    arn
}

/// One `TagSAMLProvider` call: the HTTP status and the XML body.
async fn tag_provider(
    router: &MultiServiceRouter,
    arn: &str,
    list: &[(String, String)],
) -> (u16, String) {
    let params = with_tag_params(vec![param("SAMLProviderArn", arn)], list);
    call(router, "TagSAMLProvider", &borrowed(&params)).await
}

/// One `UntagSAMLProvider` call: the HTTP status and the XML body.
async fn untag_provider(
    router: &MultiServiceRouter,
    arn: &str,
    removed: &[String],
) -> (u16, String) {
    let params = with_key_params(vec![param("SAMLProviderArn", arn)], removed);
    call(router, "UntagSAMLProvider", &borrowed(&params)).await
}

/// The tag set `GetSAMLProvider` reports for `arn`, as a map, plus the tag keys in the
/// order the response carried them — so a duplicated key cannot hide inside a map and the
/// reported ORDER is assertable.
async fn stored_tags(
    router: &MultiServiceRouter,
    arn: &str,
) -> (BTreeMap<String, String>, Vec<String>) {
    let (status, body) = call(router, "GetSAMLProvider", &[("SAMLProviderArn", arn)]).await;
    assert_eq!(
        status, 200,
        "case {arn:?}: GetSAMLProvider must report the tagged provider, got {status}: {body}"
    );
    (reported_tags(&body), elements(&body, "Key"))
}

/// The tag set `ListSAMLProviderTags` reports for `arn` — the other reader of the same
/// stored set, so both halves of the round trip are asserted on every row.
async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> (BTreeMap<String, String>, Vec<String>) {
    let (status, body) = call(router, "ListSAMLProviderTags", &[("SAMLProviderArn", arn)]).await;
    assert_eq!(
        status, 200,
        "case {arn:?}: ListSAMLProviderTags must report the tagged provider, got {status}: {body}"
    );
    (reported_tags(&body), elements(&body, "Key"))
}

/// Assert both readers report `expected`, sorted by tag key.
async fn assert_reads(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    expected: &BTreeMap<String, String>,
) {
    let sorted: Vec<String> = expected.keys().cloned().collect();

    let (reported, reported_keys) = stored_tags(router, arn).await;
    assert_eq!(
        &reported, expected,
        "case {case:?}: expected GetSAMLProvider to report the tag set {expected:?}, got \
         {reported:?}"
    );
    assert_eq!(
        reported_keys, sorted,
        "case {case:?}: expected the reported keys sorted by key {sorted:?}, got \
         {reported_keys:?}"
    );

    let (listed, listed_keys) = listed_tags(router, arn).await;
    assert_eq!(
        &listed, expected,
        "case {case:?}: expected ListSAMLProviderTags to report the tag set {expected:?}, \
         got {listed:?}"
    );
    assert_eq!(
        listed_keys, sorted,
        "case {case:?}: expected the listed keys sorted by key {sorted:?}, got \
         {listed_keys:?}"
    );
}

/// One row: a provider created with `initial`, then `calls` successive
/// `TagSAMLProvider` requests, then the tag set the provider must hold.
struct MergeCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    calls: Vec<Vec<(String, String)>>,
    expected: Vec<(String, String)>,
}

/// RULE: `TagSAMLProvider` merges its tags into the provider's stored set — a held key
/// is overwritten, a new key is added — and every read reports exactly that merged set,
/// sorted by tag key. One row per thing the rule generalises over: the held tag count,
/// the number of calls, new versus held keys, the case of a key, a key repeated inside
/// one request, and the published length bounds of `tagKeyType` and `tagValueType`.
#[tokio::test]
async fn tagging_merges_its_tags_into_the_persisted_set() {
    // `tagValueType` is length 0..256 and `tagKeyType` length 1..128
    // (`smithy_query(query="shape", shape="Tag")`), so these are the published bounds a
    // stored tag must survive.
    let longest_value = "v".repeat(256);
    let longest_key = "k".repeat(128);

    let cases = vec![
        MergeCase {
            name: "a provider holding no tags gains one",
            initial: Vec::new(),
            calls: vec![tags(&[("team", "federation")])],
            expected: tags(&[("team", "federation")]),
        },
        MergeCase {
            name: "a new key is added beside the held ones",
            initial: tags(&[("team", "federation"), ("env", "prod")]),
            calls: vec![tags(&[("owner", "identity")])],
            expected: tags(&[
                ("team", "federation"),
                ("env", "prod"),
                ("owner", "identity"),
            ]),
        },
        MergeCase {
            name: "a held key is overwritten rather than duplicated",
            initial: tags(&[("team", "federation"), ("env", "prod")]),
            calls: vec![tags(&[("env", "staging")])],
            expected: tags(&[("team", "federation"), ("env", "staging")]),
        },
        MergeCase {
            name: "one call both overwrites and adds",
            initial: tags(&[("a", "1"), ("b", "2")]),
            calls: vec![tags(&[("b", "two"), ("c", "3")])],
            expected: tags(&[("a", "1"), ("b", "two"), ("c", "3")]),
        },
        MergeCase {
            name: "successive calls accumulate, the last value winning",
            initial: Vec::new(),
            calls: vec![
                tags(&[("k1", "v1")]),
                tags(&[("k2", "v2")]),
                tags(&[("k1", "v1-again")]),
            ],
            expected: tags(&[("k1", "v1-again"), ("k2", "v2")]),
        },
        MergeCase {
            // "Tag keys and values are case sensitive" ("Tagging IAM resources", IAM
            // User Guide), so two spellings of one word are two tags.
            name: "keys differing only in case are distinct tags",
            initial: tags(&[("Env", "prod")]),
            calls: vec![tags(&[("env", "dev"), ("ENV", "test")])],
            expected: tags(&[("Env", "prod"), ("env", "dev"), ("ENV", "test")]),
        },
        MergeCase {
            name: "a key repeated inside one request settles on its last value",
            initial: Vec::new(),
            calls: vec![tags(&[("dup", "first"), ("dup", "last")])],
            expected: tags(&[("dup", "last")]),
        },
        MergeCase {
            name: "a value at the published minimum length",
            initial: tags(&[("team", "federation")]),
            calls: vec![tags(&[("empty", "")])],
            expected: tags(&[("team", "federation"), ("empty", "")]),
        },
        MergeCase {
            name: "a held value overwritten with the empty value",
            initial: tags(&[("env", "prod")]),
            calls: vec![tags(&[("env", "")])],
            expected: tags(&[("env", "")]),
        },
        MergeCase {
            name: "a value at the published maximum length",
            initial: Vec::new(),
            calls: vec![tags(&[("long", longest_value.as_str())])],
            expected: tags(&[("long", longest_value.as_str())]),
        },
        MergeCase {
            name: "a key at the published maximum length",
            initial: Vec::new(),
            calls: vec![tags(&[(longest_key.as_str(), "fits")])],
            expected: tags(&[(longest_key.as_str(), "fits")]),
        },
        MergeCase {
            // `tagKeyType`'s pattern is `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$` and
            // `tagValueType`'s the same with `*`, so spaces, non-ASCII letters, digits
            // and every punctuation character of that class belong to the class the rule
            // covers.
            name: "a key and a value spanning the modelled alphabet",
            initial: Vec::new(),
            calls: vec![tags(&[("Cost Center_1.2:3/4=5+6-7@8", "Ünité 9 / A=B+C-D@E")])],
            expected: tags(&[("Cost Center_1.2:3/4=5+6-7@8", "Ünité 9 / A=B+C-D@E")]),
        },
        MergeCase {
            // The whole tag set a provider may carry, arriving in one call on a provider
            // that holds none.
            name: "the published maximum number of tags in one call",
            initial: Vec::new(),
            calls: vec![fixture(MAX_TAGS_PER_ENTITY)],
            expected: fixture(MAX_TAGS_PER_ENTITY),
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let arn = create_provider(&router, &format!("merge-{index}"), &case.initial).await;

        for (call_index, list) in case.calls.iter().enumerate() {
            let (status, body) = tag_provider(&router, &arn, list).await;
            assert_eq!(
                status, 200,
                "case {:?}: TagSAMLProvider call {call_index} must succeed, got {status}: \
                 {body}",
                case.name
            );
        }

        assert_reads(&router, case.name, &arn, &expected_map(&case.expected)).await;
    }
}

/// One row: a provider created with `initial`, then one `UntagSAMLProvider` naming
/// `remove`, then the tag set the provider must hold.
struct RemoveCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
    expected: Vec<(String, String)>,
}

/// RULE: `UntagSAMLProvider` drops exactly the held tags whose keys it names, comparing
/// keys exactly, and leaves every other held tag with its value — so a named key the
/// provider does not hold removes nothing and the call still succeeds. One row per thing
/// the rule generalises over: the held tag count, how many keys one call names, held
/// versus absent keys, the case of a key, a key repeated inside one request, and the
/// whole held set. An EMPTY `TagKeys` is not among them: the list is a required member,
/// so an awsQuery request carrying no `TagKeys.member.N` is refused before the operation
/// runs (asserted in [`a_required_member_absent_is_refused_before_the_operation_runs`]).
#[tokio::test]
async fn untagging_drops_exactly_the_named_keys() {
    let cases = vec![
        RemoveCase {
            name: "one held key of several",
            initial: tags(&[("team", "federation"), ("env", "prod"), ("owner", "idp")]),
            remove: keys(&["env"]),
            expected: tags(&[("team", "federation"), ("owner", "idp")]),
        },
        RemoveCase {
            name: "several held keys at once",
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            remove: keys(&["b", "d"]),
            expected: tags(&[("a", "1"), ("c", "3")]),
        },
        RemoveCase {
            name: "the whole held set",
            initial: tags(&[("a", "1"), ("b", "2")]),
            remove: keys(&["a", "b"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "the provider's only tag",
            initial: tags(&[("solo", "value")]),
            remove: keys(&["solo"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "a key the provider does not hold removes nothing",
            initial: tags(&[("team", "federation")]),
            remove: keys(&["absent"]),
            expected: tags(&[("team", "federation")]),
        },
        RemoveCase {
            name: "a held key beside one the provider does not hold",
            initial: tags(&[("team", "federation"), ("env", "prod")]),
            remove: keys(&["env", "absent"]),
            expected: tags(&[("team", "federation")]),
        },
        RemoveCase {
            // "Tag keys and values are case sensitive" ("Tagging IAM resources"), so a
            // key spelled in another case names a different tag.
            name: "a key spelled in another case names a different tag",
            initial: tags(&[("Env", "prod"), ("env", "dev")]),
            remove: keys(&["env"]),
            expected: tags(&[("Env", "prod")]),
        },
        RemoveCase {
            name: "a key named twice in one request",
            initial: tags(&[("team", "federation"), ("env", "prod")]),
            remove: keys(&["env", "env"]),
            expected: tags(&[("team", "federation")]),
        },
        RemoveCase {
            name: "a key on a provider holding no tags removes nothing",
            initial: Vec::new(),
            remove: keys(&["absent"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "half of the published maximum tag set",
            initial: fixture(MAX_TAGS_PER_ENTITY),
            remove: fixture(MAX_TAGS_PER_ENTITY / 2)
                .into_iter()
                .map(|(key, _)| key)
                .collect(),
            expected: fixture(MAX_TAGS_PER_ENTITY)
                .into_iter()
                .skip(MAX_TAGS_PER_ENTITY / 2)
                .collect(),
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let arn = create_provider(&router, &format!("remove-{index}"), &case.initial).await;

        let (status, body) = untag_provider(&router, &arn, &case.remove).await;
        assert_eq!(
            status, 200,
            "case {:?}: UntagSAMLProvider must succeed, got {status}: {body}",
            case.name
        );

        assert_reads(&router, case.name, &arn, &expected_map(&case.expected)).await;

        // The call is idempotent: repeating it reports the same set, and a key it
        // removed is free to be tagged again.
        let (status, body) = untag_provider(&router, &arn, &case.remove).await;
        assert_eq!(
            status, 200,
            "case {:?}: a repeated UntagSAMLProvider must succeed, got {status}: {body}",
            case.name
        );
        assert_reads(&router, case.name, &arn, &expected_map(&case.expected)).await;

        let regained = tags(&[("regained", "value")]);
        let (status, body) = tag_provider(&router, &arn, &regained).await;
        assert_eq!(
            status, 200,
            "case {:?}: tagging after an untag must succeed, got {status}: {body}",
            case.name
        );
        let mut expected = expected_map(&case.expected);
        expected.extend(regained);
        assert_reads(&router, case.name, &arn, &expected).await;
    }
}

/// RULE: the `aws:` tag-key prefix is reserved ("You cannot create tag keys that begin
/// with `aws:`", "Tagging IAM resources"), so a tag request naming one is refused with
/// the modelled `InvalidInputException` and the ENTIRE request is dropped — no tag of it
/// reaches the provider.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_stores_nothing() {
    let held = tags(&[("team", "federation")]);
    let refused = [
        ("the prefix alone", tags(&[("aws:", "x")])),
        (
            "a reserved key",
            tags(&[("aws:cloudformation:stack-name", "stack")]),
        ),
        // The prefix is reserved however it is capitalised.
        ("an upper-case prefix", tags(&[("AWS:Owner", "aws")])),
        ("a mixed-case prefix", tags(&[("Aws:Owner", "aws")])),
        (
            "a valid tag beside a reserved one",
            tags(&[("owner", "idp"), ("aws:service", "iam")]),
        ),
        (
            "a reserved key before a valid one",
            tags(&[("aws:service", "iam"), ("owner", "idp")]),
        ),
    ];

    let router = router();
    for (index, (case, list)) in refused.iter().enumerate() {
        let arn = create_provider(&router, &format!("reserved-{index}"), &held).await;

        let (status, body) = tag_provider(&router, &arn, list).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a reserved tag key, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        // The whole request failed, so the provider still holds exactly what it was
        // created with: neither the reserved key nor the valid one beside it.
        assert_reads(&router, case, &arn, &expected_map(&held)).await;
    }
}

/// RULE: a resource carries at most [`MAX_TAGS_PER_ENTITY`] tags (AWS's "IAM and STS
/// quotas": "Number of tags per IAM resource: 50"), and the bound is taken against the
/// MERGED set — so overwriting a held key is never refused, while a call that would carry
/// the provider past the bound is refused with the modelled `LimitExceededException` and
/// stores nothing.
#[tokio::test]
async fn the_tag_bound_is_taken_against_the_merged_set() {
    let bound = MAX_TAGS_PER_ENTITY;

    // One row per position relative to the published bound: just inside it, at it, and
    // the first call that would pass it.
    let accepted = [
        ("one below the bound, one new key", bound - 1, tags(&[("new-a", "a")])),
        (
            "two below the bound, two new keys",
            bound - 2,
            tags(&[("new-a", "a"), ("new-b", "b")]),
        ),
        (
            "at the bound, overwriting a held key",
            bound,
            tags(&[("key-00", "overwritten")]),
        ),
        (
            "at the bound, overwriting several held keys",
            bound,
            tags(&[("key-01", "one"), ("key-02", "two")]),
        ),
    ];

    let router = router();
    for (index, (case, held_count, list)) in accepted.iter().enumerate() {
        let held = fixture(*held_count);
        let arn = create_provider(&router, &format!("bound-ok-{index}"), &held).await;

        let (status, body) = tag_provider(&router, &arn, list).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 inside the tag bound, got {status}: {body}"
        );

        let mut expected = expected_map(&held);
        expected.extend(list.iter().cloned());
        assert!(
            expected.len() <= bound,
            "case {case:?}: a provider must never hold more than {bound} tags, got {}",
            expected.len()
        );
        assert_reads(&router, case, &arn, &expected).await;
    }

    let refused = [
        ("at the bound, one new key", bound, 1usize),
        ("one below the bound, two new keys", bound - 1, 2),
        ("two below the bound, three new keys", bound - 2, 3),
    ];

    for (index, (case, held_count, new_keys)) in refused.iter().enumerate() {
        let held = fixture(*held_count);
        let arn = create_provider(&router, &format!("bound-refused-{index}"), &held).await;

        let list: Vec<(String, String)> = (0..*new_keys)
            .map(|position| (format!("new-{position}"), format!("value-{position}")))
            .collect();
        let (status, body) = tag_provider(&router, &arn, &list).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 past the tag bound, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceeded",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        // Nothing of the refused request was stored.
        assert_reads(&router, case, &arn, &expected_map(&held)).await;
    }
}

/// RULE: both calls name their provider by ARN, so a value that is not an ARN is an
/// invalid input (HTTP 400) while a well-formed ARN naming no provider the account holds
/// is the different answer that the entity does not exist (HTTP 404) — and neither
/// refusal changes the tags of the provider the account does hold.
#[tokio::test]
async fn a_non_arn_is_invalid_and_an_unheld_provider_is_not_found() {
    let router = router();
    let held = tags(&[("team", "federation")]);
    let arn = create_provider(&router, "HeldProvider", &held).await;
    let added = tags(&[("owner", "idp")]);
    let removed = keys(&["team"]);

    // Values past `arnType`'s 20-character minimum, so the HANDLER is what refuses them
    // rather than the boundary validator.
    for value in [
        "HeldProviderButNotAnArnAtAll",
        "iam::saml-provider/HeldProvider",
        "arn-but-not-really-one/HeldProvider",
    ] {
        let (status, body) = tag_provider(&router, value, &added).await;
        assert_eq!(
            status, 400,
            "case {value:?}: TagSAMLProvider must refuse a value that is not an ARN with \
             HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {value:?}: expected InvalidInput, got {body}"
        );

        let (status, body) = untag_provider(&router, value, &removed).await;
        assert_eq!(
            status, 400,
            "case {value:?}: UntagSAMLProvider must refuse a value that is not an ARN with \
             HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {value:?}: expected InvalidInput, got {body}"
        );
    }

    // A value SHORTER than `arnType`'s published 20-character minimum is refused by the
    // modelled constraint, before the operation runs at all.
    for short in ["HeldProvider", "arn:aws:iam"] {
        let (status, body) = tag_provider(&router, short, &added).await;
        assert_eq!(
            status, 400,
            "case {short:?}: a value shorter than arnType's minimum must be refused with \
             HTTP 400, got {status}: {body}"
        );
        let (status, body) = untag_provider(&router, short, &removed).await;
        assert_eq!(
            status, 400,
            "case {short:?}: a value shorter than arnType's minimum must be refused with \
             HTTP 400, got {status}: {body}"
        );
    }

    for name in ["NeverCreated", "another.provider-1", "held_provider"] {
        let absent = expected_arn(name);
        // IAM names the ARN it found no provider under, so the message's variable part
        // is the ARN the request carried and its fixed text does not depend on the row.
        let expected_message = format!("Manifest not found for arn {absent}");
        let (status, body) = tag_provider(&router, &absent, &added).await;
        assert_eq!(
            status, 404,
            "case {name:?}: TagSAMLProvider must answer 404 for an ARN the account holds no \
             provider under, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            expected_message,
            "case {name:?}: expected the message to name the ARN, got {body}"
        );

        let (status, body) = untag_provider(&router, &absent, &removed).await;
        assert_eq!(
            status, 404,
            "case {name:?}: UntagSAMLProvider must answer 404 for an ARN the account holds \
             no provider under, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            expected_message,
            "case {name:?}: expected the message to name the ARN, got {body}"
        );
    }

    // Every refusal above left the held provider's tags exactly as its create stored
    // them.
    assert_reads(&router, "HeldProvider", &arn, &expected_map(&held)).await;
}

/// RULE: `Tags` and `TagKeys` are REQUIRED members (`smithy_query(query="operation",
/// operation="TagSAMLProvider,UntagSAMLProvider")`), so a request carrying none of their
/// members is refused before the operation runs and changes nothing.
#[tokio::test]
async fn a_required_member_absent_is_refused_before_the_operation_runs() {
    let router = router();
    let held = tags(&[("team", "federation")]);
    let arn = create_provider(&router, "RequiredMembers", &held).await;

    for action in ["TagSAMLProvider", "UntagSAMLProvider"] {
        let (status, body) = call(&router, action, &[("SAMLProviderArn", arn.as_str())]).await;
        assert_eq!(
            status, 400,
            "case {action:?}: a request carrying no tag list must be refused with HTTP 400, \
             got {status}: {body}"
        );
    }

    assert_reads(&router, "RequiredMembers", &arn, &expected_map(&held)).await;
}
