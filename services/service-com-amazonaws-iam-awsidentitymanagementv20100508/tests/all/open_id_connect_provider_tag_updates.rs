// SPDX-License-Identifier: CC0-1.0

//! `TagOpenIDConnectProvider`, end to end over the awsQuery wire, read back through
//! `GetOpenIDConnectProvider`.
//!
//! RULE under test: a tag call MERGES the supplied tags into the tag set the named
//! provider persists — a supplied key the provider already holds has its value
//! overwritten and a key it does not hold is added — bounded by the per-resource tag
//! quota, with a request carrying any invalid tag storing nothing; the set it leaves
//! behind is what every later read reports, sorted by tag key, and the provider's other
//! members are untouched. `TagOpenIDConnectProvider`'s own documentation: "if a tag with
//! the same key name already exists, then that tag is overwritten with the new value",
//! and "if any one of the tags is invalid or if you exceed the allowed maximum number of
//! tags, then the entire request fails and the resource is not created";
//! `GetOpenIDConnectProvider$Tags`: "the returned list of tags is sorted by tag key".
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of a provider that was
//! PERSISTED rather than a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns, the parameter and tag readers are
//! `tests/all/role_lifecycle.rs`'s, the tag-list builders are `tests/all/role_tags.rs`'s
//! and the provider ARN and fixture builders come from
//! `tests/all/open_id_connect_providers.rs`, and the distinct-tag fixture the quota rows
//! are built from is `tests/all/role_tag_removal.rs`'s, so a request is built and read
//! exactly one way across this crate.
//!
//! The suites are tables whose rows differ along what the rule generalises over: the held
//! tag count, the number of calls, whether a key is new or held, the case of a key, keys
//! repeated inside one request, the alphabet and the published length bounds of
//! `tagKeyType` (1..128) and `tagValueType` (0..256), the published per-resource tag
//! quota, and the shape of the URL the provider is named by.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_TAGS_PER_ENTITY;
use smithy_server_runtime::MultiServiceRouter;

use crate::open_id_connect_providers::{client_ids, expected_arn, issuer_of, list_members, thumbprint};
use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::role_tag_removal::fixture;
use crate::role_tags::{expected_map, tags, with_tag_params};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// Create the provider for `url` carrying `initial`, plus one client ID and one
/// thumbprint so a tag call can be shown to leave the provider's other members alone,
/// and answer the ARN it is held under.
pub(crate) async fn create_provider(
    router: &MultiServiceRouter,
    url: &str,
    initial: &[(String, String)],
) -> String {
    let params = with_tag_params(
        vec![
            param("Url", url),
            param("ClientIDList.member.1", &client_ids(1)[0]),
            param("ThumbprintList.member.1", &thumbprint(7)),
        ],
        initial,
    );
    let (status, body) = call(router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {url:?}: the fixture CreateOpenIDConnectProvider must succeed, got \
         {status}: {body}"
    );
    let arn = element(&body, "OpenIDConnectProviderArn");
    assert_eq!(
        arn,
        expected_arn(url),
        "case {url:?}: the create must hold the provider under the ARN derived from its \
         URL, got {arn:?}"
    );
    arn
}

/// One `TagOpenIDConnectProvider` call: the HTTP status and the XML body.
pub(crate) async fn tag_provider(
    router: &MultiServiceRouter,
    arn: &str,
    list: &[(String, String)],
) -> (u16, String) {
    let params = with_tag_params(vec![param("OpenIDConnectProviderArn", arn)], list);
    call(router, "TagOpenIDConnectProvider", &borrowed(&params)).await
}

/// The tag set `GetOpenIDConnectProvider` reports for `arn`, as a map, plus the tag keys
/// in the order the response carried them — so a duplicated key cannot hide inside a map
/// and the reported ORDER is assertable.
pub(crate) async fn stored_tags(
    router: &MultiServiceRouter,
    arn: &str,
) -> (BTreeMap<String, String>, Vec<String>) {
    let (status, body) = call(
        router,
        "GetOpenIDConnectProvider",
        &[("OpenIDConnectProviderArn", arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {arn:?}: GetOpenIDConnectProvider must report the tagged provider, got \
         {status}: {body}"
    );
    (reported_tags(&body), elements(&body, "Key"))
}

/// Assert the reporting half reports `expected`, sorted by tag key.
pub(crate) async fn assert_reads(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    expected: &BTreeMap<String, String>,
) {
    let sorted: Vec<String> = expected.keys().cloned().collect();
    let (reported, reported_keys) = stored_tags(router, arn).await;
    assert_eq!(
        &reported, expected,
        "case {case:?}: expected GetOpenIDConnectProvider to report the tag set \
         {expected:?}, got {reported:?}"
    );
    assert_eq!(
        reported_keys, sorted,
        "case {case:?}: expected the reported keys sorted by key {sorted:?}, got \
         {reported_keys:?}"
    );
}

/// One row: a provider created with `initial`, then `calls` successive
/// `TagOpenIDConnectProvider` requests, then the tag set the provider must hold.
struct MergeCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    calls: Vec<Vec<(String, String)>>,
    expected: Vec<(String, String)>,
}

/// RULE: `TagOpenIDConnectProvider` merges its tags into the provider's stored set — a
/// held key is overwritten, a new key is added — and the read reports exactly that merged
/// set, sorted by tag key. One row per thing the rule generalises over: the held tag
/// count, the number of calls, new versus held keys, the case of a key, a key repeated
/// inside one request, the modelled alphabet, and the published length bounds of
/// `tagKeyType` and `tagValueType`.
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
        let arn = create_provider(
            &router,
            &format!("https://merge-{index}.example.org/realms/main"),
            &case.initial,
        )
        .await;

        for (call_index, list) in case.calls.iter().enumerate() {
            let (status, body) = tag_provider(&router, &arn, list).await;
            assert_eq!(
                status, 200,
                "case {:?}: TagOpenIDConnectProvider call {call_index} must succeed, got \
                 {status}: {body}",
                case.name
            );
        }

        assert_reads(&router, case.name, &arn, &expected_map(&case.expected)).await;
    }
}

/// RULE: a tag call touches only the provider's TAGS, so the URL, client IDs and
/// thumbprints the create persisted are reported unchanged afterwards, and the provider
/// is still the one its ARN names. Rows span the shape of the URL the provider is named
/// by, since the ARN the write is addressed by is derived from it.
#[tokio::test]
async fn tagging_leaves_the_provider_s_other_members_alone() {
    let router = router();

    // Rows: a bare hostname, a one-segment path, a nested path with digits and a hyphen,
    // and a mixed-case host — the URL forms `CreateOpenIDConnectProvider$Url` publishes
    // ("typically the URL consists of only a hostname … path components are allowed").
    for url in [
        "https://server.example.org",
        "https://example.com/oidc",
        "https://idp-7.example-corp.net/auth/realms/dev-2",
        "https://Accounts.Example.COM/Realms/Main",
    ] {
        let arn = create_provider(&router, url, &tags(&[("env", "prod")])).await;
        let added = tags(&[("owner", "identity"), ("env", "staging")]);
        let (status, body) = tag_provider(&router, &arn, &added).await;
        assert_eq!(
            status, 200,
            "case {url:?}: TagOpenIDConnectProvider must succeed, got {status}: {body}"
        );

        let (status, read) = call(
            &router,
            "GetOpenIDConnectProvider",
            &[("OpenIDConnectProviderArn", arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {url:?}: the tagged provider must read back, got {status}: {read}"
        );
        assert_eq!(
            element(&read, "Url"),
            issuer_of(url),
            "case {url:?}: tagging must leave the stored URL alone, got {read}"
        );
        assert_eq!(
            list_members(&read, "ClientIDList"),
            client_ids(1),
            "case {url:?}: tagging must leave the client IDs alone, got {read}"
        );
        assert_eq!(
            list_members(&read, "ThumbprintList"),
            vec![thumbprint(7)],
            "case {url:?}: tagging must leave the thumbprints alone, got {read}"
        );
        assert_reads(
            &router,
            url,
            &arn,
            &expected_map(&tags(&[("env", "staging"), ("owner", "identity")])),
        )
        .await;
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
        let arn = create_provider(
            &router,
            &format!("https://reserved-{index}.example.org"),
            &held,
        )
        .await;

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
        (
            "one below the bound, one new key",
            bound - 1,
            tags(&[("new-a", "a")]),
        ),
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
        let arn = create_provider(
            &router,
            &format!("https://bound-ok-{index}.example.org"),
            &held,
        )
        .await;

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
        let arn = create_provider(
            &router,
            &format!("https://bound-refused-{index}.example.org"),
            &held,
        )
        .await;

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

/// RULE: the call names its provider by ARN, so a value that is not an ARN is an invalid
/// input (HTTP 400) while a well-formed ARN naming no provider the account holds is the
/// different answer that the entity does not exist (HTTP 404) — and neither refusal
/// changes the tags of the provider the account does hold.
#[tokio::test]
async fn a_non_arn_is_invalid_and_an_unheld_provider_is_not_found() {
    let router = router();
    let held = tags(&[("team", "federation")]);
    let url = "https://held.example.org";
    let arn = create_provider(&router, url, &held).await;
    let added = tags(&[("owner", "idp")]);

    // Values past `arnType`'s 20-character minimum, so the HANDLER is what refuses them
    // rather than the boundary validator.
    for value in [
        "https://held.example.org",
        "held.example.org-provider",
        "aws:iam::000000000000:oidc-provider/held.example.org",
    ] {
        let (status, body) = tag_provider(&router, value, &added).await;
        assert_eq!(
            status, 400,
            "case {value:?}: a value that is not an ARN must be refused with HTTP 400, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {value:?}: expected InvalidInput, got {body}"
        );
    }

    // A value SHORTER than `arnType`'s published 20-character minimum is refused by the
    // modelled constraint, before the operation runs at all.
    for short in ["held.example.org", "arn:aws:iam"] {
        let (status, body) = tag_provider(&router, short, &added).await;
        assert_eq!(
            status, 400,
            "case {short:?}: a value shorter than arnType's minimum must be refused with \
             HTTP 400, got {status}: {body}"
        );
    }

    // Rows: an ARN for a URL nothing holds, a near-miss of the held one, an ARN of
    // another IAM resource type, and one in another account.
    let unheld = [
        format!("arn:aws:iam::{}:oidc-provider/never.example.org", account_id()),
        format!(
            "arn:aws:iam::{}:oidc-provider/held.example.org/x",
            account_id()
        ),
        format!("arn:aws:iam::{}:role/held.example.org", account_id()),
        "arn:aws:iam::111122223333:oidc-provider/held.example.org".to_string(),
    ];
    for absent in unheld {
        // IAM names the ARN it found no provider under, so the message's variable part is
        // the ARN the request carried and its fixed text does not depend on the row.
        let expected_message = format!("OpenIDConnect Provider not found for arn {absent}");
        let (status, body) = tag_provider(&router, &absent, &added).await;
        assert_eq!(
            status, 404,
            "case {absent:?}: an ARN the account holds no provider under must answer 404, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected NoSuchEntity, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            expected_message,
            "case {absent:?}: expected the message to name the ARN, got {body}"
        );
    }

    // Every refusal above left the held provider's tags exactly as its create stored
    // them.
    assert_reads(&router, url, &arn, &expected_map(&held)).await;
}

/// RULE: `Tags` is a REQUIRED member (`smithy_query(query="operation",
/// operation="TagOpenIDConnectProvider")`), so a request carrying none of its members is
/// refused before the operation runs and changes nothing.
#[tokio::test]
async fn a_required_member_absent_is_refused_before_the_operation_runs() {
    let router = router();
    let held = tags(&[("team", "federation")]);
    let url = "https://required.example.org";
    let arn = create_provider(&router, url, &held).await;

    let (status, body) = call(
        &router,
        "TagOpenIDConnectProvider",
        &[("OpenIDConnectProviderArn", arn.as_str())],
    )
    .await;
    assert_eq!(
        status, 400,
        "a request carrying no tag list must be refused with HTTP 400, got {status}: \
         {body}"
    );

    assert_reads(&router, url, &arn, &expected_map(&held)).await;
}
