// SPDX-License-Identifier: CC0-1.0

//! `UntagOpenIDConnectProvider`, end to end over the awsQuery wire, read back through
//! `GetOpenIDConnectProvider`.
//!
//! RULE under test: an untag call drops from the named resource's stored tag set exactly
//! the tags whose keys it names, comparing keys exactly, and leaves every other held tag
//! with its value and every other member of the resource alone — so a named key the
//! resource does not hold removes nothing and the call still succeeds, which makes it
//! idempotent, and a removed key is free to be tagged again.
//! `UntagOpenIDConnectProvider`'s own documentation: "removes the specified tags from the
//! specified OpenID Connect (OIDC)-compatible identity provider in IAM", with "the tags
//! with matching keys are removed from the specified OIDC provider"
//! (`UntagOpenIDConnectProvider$TagKeys`); `GetOpenIDConnectProvider$Tags`: "the returned
//! list of tags is sorted by tag key".
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of a provider that was
//! PERSISTED rather than a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns, the parameter readers are
//! `tests/all/role_lifecycle.rs`'s, the tag-list builders are `tests/all/role_tags.rs`'s,
//! the key-list builder and the distinct-tag fixture are `tests/all/role_tag_removal.rs`'s
//! and the provider fixtures and readers come from
//! `tests/all/open_id_connect_providers.rs` and
//! `tests/all/open_id_connect_provider_tag_updates.rs`, so a request is built and read
//! exactly one way across this crate.
//!
//! The suites are tables whose rows differ along what the rule generalises over: the held
//! tag count, how many keys one call names, whether a named key is held or not, the case
//! of a key, the number of calls, the alphabet and the published length bounds of
//! `tagKeyType` (1..128), the published per-resource tag quota, and the shape of the URL
//! the provider is named by.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_TAGS_PER_ENTITY;
use smithy_server_runtime::MultiServiceRouter;

use crate::open_id_connect_provider_tag_updates::{
    assert_reads, create_provider, stored_tags, tag_provider,
};
use crate::open_id_connect_providers::{
    absence_message, client_ids, issuer_of, list_members, thumbprint,
};
use crate::role_lifecycle::{borrowed, param};
use crate::role_tag_removal::{fixture, keys, with_key_params};
use crate::role_tags::{expected_map, tags};
use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// One `UntagOpenIDConnectProvider` call: the HTTP status and the XML body.
async fn untag_provider(
    router: &MultiServiceRouter,
    arn: &str,
    removed: &[String],
) -> (u16, String) {
    let params = with_key_params(vec![param("OpenIDConnectProviderArn", arn)], removed);
    call(
        router,
        "UntagOpenIDConnectProvider",
        &borrowed(&params),
    )
    .await
}

/// One row: a provider created with `initial`, then one `UntagOpenIDConnectProvider`
/// naming `remove`, then the tag set the provider must hold.
struct RemoveCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
    expected: Vec<(String, String)>,
}

/// RULE: `UntagOpenIDConnectProvider` drops exactly the held tags whose keys the request
/// names and leaves every other held tag untouched, so `GetOpenIDConnectProvider` reports
/// the remaining set, still sorted by tag key. One row per thing the rule generalises
/// over: the held tag count, how many keys one call names, held versus unheld keys, the
/// case of a key, the empty tag value, the published key-length bounds and alphabet, and
/// the per-resource tag quota.
#[tokio::test]
async fn untagging_removes_exactly_the_named_keys() {
    // `tagKeyType` is length 1..128 (`smithy_query(query="shape", shape="Tag")`), so
    // these are the published bounds a key the request names can sit at.
    let longest_key = "k".repeat(128);
    // `tagKeyType`'s pattern is `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$`, so spaces, non-ASCII
    // letters, digits and that class's punctuation all name tags.
    let alphabet_key = "Cost Center_1.2:3/4=5+6-7@8";
    let quota = MAX_TAGS_PER_ENTITY;

    let cases = vec![
        RemoveCase {
            name: "the only held tag is removed, leaving none",
            initial: tags(&[("team", "federation")]),
            remove: keys(&["team"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "one of several held tags is removed",
            initial: tags(&[("team", "federation"), ("env", "prod"), ("owner", "idp")]),
            remove: keys(&["env"]),
            expected: tags(&[("team", "federation"), ("owner", "idp")]),
        },
        RemoveCase {
            name: "several held keys are removed in one call",
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            remove: keys(&["a", "c"]),
            expected: tags(&[("b", "2"), ("d", "4")]),
        },
        RemoveCase {
            name: "every held key is removed in one call",
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
            remove: keys(&["a", "b", "c"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "a key the provider does not hold removes nothing",
            initial: tags(&[("team", "federation"), ("env", "prod")]),
            remove: keys(&["absent"]),
            expected: tags(&[("team", "federation"), ("env", "prod")]),
        },
        RemoveCase {
            name: "a held key named beside an unheld one",
            initial: tags(&[("team", "federation"), ("env", "prod")]),
            remove: keys(&["team", "absent"]),
            expected: tags(&[("env", "prod")]),
        },
        RemoveCase {
            name: "an unheld key named on a provider holding no tags",
            initial: Vec::new(),
            remove: keys(&["absent"]),
            expected: Vec::new(),
        },
        RemoveCase {
            // "Tag keys and values are case sensitive" ("Tagging IAM resources", IAM
            // User Guide), so a key spelled in another case names another tag.
            name: "keys differing only in case are distinct tags",
            initial: tags(&[("Env", "prod"), ("env", "dev"), ("ENV", "test")]),
            remove: keys(&["env"]),
            expected: tags(&[("Env", "prod"), ("ENV", "test")]),
        },
        RemoveCase {
            name: "a key whose case the provider does not hold removes nothing",
            initial: tags(&[("Env", "prod")]),
            remove: keys(&["env", "ENV"]),
            expected: tags(&[("Env", "prod")]),
        },
        RemoveCase {
            name: "a tag carrying the empty value is removed by its key",
            initial: tags(&[("empty", ""), ("keep", "v")]),
            remove: keys(&["empty"]),
            expected: tags(&[("keep", "v")]),
        },
        RemoveCase {
            name: "a key at the published maximum length",
            initial: tags(&[(longest_key.as_str(), "fits"), ("team", "federation")]),
            remove: keys(&[longest_key.as_str()]),
            expected: tags(&[("team", "federation")]),
        },
        RemoveCase {
            name: "a key at the published minimum length",
            initial: tags(&[("k", "short"), ("team", "federation")]),
            remove: keys(&["k"]),
            expected: tags(&[("team", "federation")]),
        },
        RemoveCase {
            name: "a key spanning the modelled alphabet",
            initial: tags(&[(alphabet_key, "x"), ("keep", "y")]),
            remove: keys(&[alphabet_key]),
            expected: tags(&[("keep", "y")]),
        },
        RemoveCase {
            name: "one key removed from a provider at the tag quota",
            initial: fixture(quota),
            remove: keys(&["key-00"]),
            expected: fixture(quota)[1..].to_vec(),
        },
        RemoveCase {
            // `tagKeyListType` is length 0..50, and the quota is 50 tags per resource,
            // so this names the most keys one call can carry.
            name: "every key of a provider at the tag quota removed in one call",
            initial: fixture(quota),
            remove: fixture(quota).iter().map(|(key, _)| key.clone()).collect(),
            expected: Vec::new(),
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let arn = create_provider(
            &router,
            &format!("https://remove-{index}.example.org/realms/main"),
            &case.initial,
        )
        .await;

        let (status, body) = untag_provider(&router, &arn, &case.remove).await;
        assert_eq!(
            status, 200,
            "case {:?}: UntagOpenIDConnectProvider must succeed, got {status}: {body}",
            case.name
        );

        assert_reads(&router, case.name, &arn, &expected_map(&case.expected)).await;
        let (_, reported_keys) = stored_tags(&router, &arn).await;
        assert_eq!(
            reported_keys.len(),
            case.expected.len(),
            "case {:?}: expected {} stored tags, got {reported_keys:?}",
            case.name,
            case.expected.len()
        );
    }
}

/// RULE: removing a key the resource does not hold removes nothing and is not an error, so
/// repeating an untag call leaves the same tag set however many times it is made.
#[tokio::test]
async fn repeating_an_untag_call_changes_nothing_further() {
    let held = tags(&[("team", "federation"), ("env", "prod"), ("owner", "idp")]);
    let repeated = [
        (
            "one held key",
            keys(&["env"]),
            tags(&[("team", "federation"), ("owner", "idp")]),
        ),
        (
            "two held keys",
            keys(&["env", "owner"]),
            tags(&[("team", "federation")]),
        ),
        ("only unheld keys", keys(&["absent", "other"]), held.clone()),
        (
            "every held key",
            keys(&["team", "env", "owner"]),
            Vec::new(),
        ),
    ];

    let router = router();
    for (index, (case, removed, expected)) in repeated.iter().enumerate() {
        let arn = create_provider(
            &router,
            &format!("https://repeat-{index}.example.org"),
            &held,
        )
        .await;

        // Three identical calls: the first may change the set, the rest must not.
        for attempt in 0..3 {
            let (status, body) = untag_provider(&router, &arn, removed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UntagOpenIDConnectProvider attempt {attempt} must \
                 succeed, got {status}: {body}"
            );
            assert_reads(&router, case, &arn, &expected_map(expected)).await;
        }
    }
}

/// RULE: an untag call touches only the provider's TAGS, so the URL, client IDs and
/// thumbprints the create persisted are reported unchanged afterwards. Rows span the shape
/// of the URL the provider is named by, since the ARN the write is addressed by is derived
/// from it.
#[tokio::test]
async fn untagging_leaves_the_provider_s_other_members_alone() {
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
        let held = tags(&[("env", "prod"), ("owner", "identity")]);
        let arn = create_provider(&router, url, &held).await;

        let (status, body) = untag_provider(&router, &arn, &keys(&["env"])).await;
        assert_eq!(
            status, 200,
            "case {url:?}: UntagOpenIDConnectProvider must succeed, got {status}: {body}"
        );

        let (status, read) = call(
            &router,
            "GetOpenIDConnectProvider",
            &[("OpenIDConnectProviderArn", arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {url:?}: the untagged provider must read back, got {status}: {read}"
        );
        assert_eq!(
            element(&read, "Url"),
            issuer_of(url),
            "case {url:?}: untagging must leave the stored URL alone, got {read}"
        );
        assert_eq!(
            list_members(&read, "ClientIDList"),
            client_ids(1),
            "case {url:?}: untagging must leave the client IDs alone, got {read}"
        );
        assert_eq!(
            list_members(&read, "ThumbprintList"),
            vec![thumbprint(7)],
            "case {url:?}: untagging must leave the thumbprints alone, got {read}"
        );
        assert_reads(
            &router,
            url,
            &arn,
            &expected_map(&tags(&[("owner", "identity")])),
        )
        .await;
    }
}

/// RULE: the call names its provider by ARN, so a value that is not an ARN is an invalid
/// input (HTTP 400) while a well-formed ARN naming no provider the account holds is the
/// different answer that the entity does not exist (HTTP 404) — and neither refusal
/// changes the tags of the provider the account does hold.
#[tokio::test]
async fn a_non_arn_is_invalid_and_an_unheld_provider_is_not_found() {
    let router = router();
    let held = tags(&[("team", "federation"), ("env", "prod")]);
    let url = "https://untag-held.example.org";
    let arn = create_provider(&router, url, &held).await;
    let removed = keys(&["env"]);

    // Values past `arnType`'s 20-character minimum, so the HANDLER is what refuses them
    // rather than the boundary validator.
    for value in [
        "https://untag-held.example.org",
        "untag-held.example.org-provider",
        "aws:iam::000000000000:oidc-provider/untag-held.example.org",
    ] {
        let expected_message =
            format!("Invalid OpenIDConnectProviderArn {value}: the value is not an ARN");
        let (status, body) = untag_provider(&router, value, &removed).await;
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
        assert_eq!(
            element(&body, "Message"),
            expected_message,
            "case {value:?}: expected the message to name the value supplied, got {body}"
        );
    }

    // A value SHORTER than `arnType`'s published 20-character minimum is refused by the
    // modelled constraint, before the operation runs at all.
    for short in ["untag.example.org", "arn:aws:iam"] {
        let (status, body) = untag_provider(&router, short, &removed).await;
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
            "arn:aws:iam::{}:oidc-provider/untag-held.example.org/x",
            account_id()
        ),
        format!("arn:aws:iam::{}:role/untag-held.example.org", account_id()),
        "arn:aws:iam::111122223333:oidc-provider/untag-held.example.org".to_string(),
    ];
    for absent in unheld {
        // IAM names the ARN it found no provider under, so the message's variable part is
        // the ARN the request carried and its fixed text does not depend on the row.
        let expected_message = absence_message(&absent);
        let (status, body) = untag_provider(&router, &absent, &removed).await;
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

/// One row: a provider created with `initial`, then an `UntagOpenIDConnectProvider` naming
/// `removed`, then a `TagOpenIDConnectProvider` supplying `re_tagged`.
struct ReTagCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    removed: Vec<String>,
    re_tagged: Vec<(String, String)>,
}

/// RULE: an untagged key is simply not held any more, so it can be tagged again — and
/// because the per-resource tag quota is taken against the set the resource actually
/// holds, removing a tag frees room inside it.
#[tokio::test]
async fn a_removed_key_can_be_tagged_again() {
    let quota = MAX_TAGS_PER_ENTITY;

    // One row per starting point: a provider holding one tag, one holding several, and one
    // at the published quota, where the re-tag only fits because a key was removed.
    let rows = [
        ReTagCase {
            name: "a provider holding one tag",
            initial: tags(&[("env", "prod")]),
            removed: keys(&["env"]),
            re_tagged: tags(&[("env", "staging")]),
        },
        ReTagCase {
            name: "a provider holding several tags",
            initial: tags(&[("a", "1"), ("b", "2")]),
            removed: keys(&["a", "b"]),
            re_tagged: tags(&[("a", "one"), ("c", "3")]),
        },
        ReTagCase {
            name: "a provider at the published tag quota",
            initial: fixture(quota),
            removed: keys(&["key-00", "key-01"]),
            re_tagged: tags(&[("new-a", "a"), ("new-b", "b")]),
        },
    ];

    let router = router();
    for (index, case) in rows.iter().enumerate() {
        let name = case.name;
        let arn = create_provider(
            &router,
            &format!("https://retag-{index}.example.org"),
            &case.initial,
        )
        .await;

        let (status, body) = untag_provider(&router, &arn, &case.removed).await;
        assert_eq!(
            status, 200,
            "case {name:?}: UntagOpenIDConnectProvider must succeed, got {status}: {body}"
        );

        let (status, body) = tag_provider(&router, &arn, &case.re_tagged).await;
        assert_eq!(
            status, 200,
            "case {name:?}: re-tagging a removed key must succeed, got {status}: {body}"
        );

        // What the provider holds now: the initial set less the removed keys, plus the
        // re-tagged ones.
        let mut expected: BTreeMap<String, String> = expected_map(&case.initial);
        for key in &case.removed {
            expected.remove(key);
        }
        expected.extend(case.re_tagged.iter().cloned());

        assert_reads(&router, name, &arn, &expected).await;
        assert!(
            expected.len() <= quota,
            "case {name:?}: a provider must never hold more than {quota} tags, got {}",
            expected.len()
        );
    }
}

/// RULE: `TagKeys` is a REQUIRED member (`smithy_query(query="operation",
/// operation="UntagOpenIDConnectProvider")`), so a request carrying none of its members is
/// refused before the operation runs and changes nothing.
#[tokio::test]
async fn a_required_member_absent_is_refused_before_the_operation_runs() {
    let router = router();
    let held = tags(&[("team", "federation")]);
    let url = "https://untag-required.example.org";
    let arn = create_provider(&router, url, &held).await;

    let (status, body) = call(
        &router,
        "UntagOpenIDConnectProvider",
        &[("OpenIDConnectProviderArn", arn.as_str())],
    )
    .await;
    assert_eq!(
        status, 400,
        "a request carrying no tag-key list must be refused with HTTP 400, got {status}: \
         {body}"
    );

    assert_reads(&router, url, &arn, &expected_map(&held)).await;
}
