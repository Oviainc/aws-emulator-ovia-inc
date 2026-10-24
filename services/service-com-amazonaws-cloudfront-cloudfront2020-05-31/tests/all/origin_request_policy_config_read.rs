// SPDX-License-Identifier: CC0-1.0

//! `GetOriginRequestPolicyConfig` end to end over HTTP.
//!
//! The rule under test: a config read REPORTS the stored record of the resource an id
//! names — the configuration exactly as the last mutation stored it, beside the version
//! token that mutation handed back — it writes nothing, and an id no stored resource holds
//! is that resource's "no such" error. For an origin request policy that means the
//! `OriginRequestPolicyConfig` payload and `ETag` header are the same ones
//! `GetOriginRequestPolicy` reports around the policy's identity, a list wrapper the create
//! left out stays absent while one the create sent empty stays present with a `Quantity` of
//! zero, and an unknown id is `NoSuchOriginRequestPolicy` (HTTP 404).
//!
//! The rows span what the rule generalises over: which of the three optional name lists the
//! configuration carries at all, each list's length (none, one, many), the behaviour enum
//! values, and whether the comment is absent, empty, ASCII or non-ASCII. The shared table
//! and the XML reader are the crate's own, in `origin_request_policy_lifecycle.rs` and
//! `distribution_lifecycle.rs`; the recorded case is one row.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::origin_request_policy_lifecycle::{
    assert_config, create_policy, get_policy, policy_table, strings, Policy,
};
use crate::response_headers_policy_lifecycle::text;

/// `GET /2020-05-31/origin-request-policy/{Id}/config`.
async fn get_policy_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-request-policy/{id}/config"),
        None,
        None,
    )
    .await
}

/// The config read answers the stored configuration and the stored version token: the same
/// ones the create handed back and `GetOriginRequestPolicy` reports, over and over, because
/// a read changes nothing a later caller sees.
#[tokio::test]
async fn the_config_read_reports_the_stored_configuration_and_its_version() {
    let router = emulator();

    for policy in policy_table() {
        let case = policy.case;
        let created = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201 from the create, got {} with {}",
            created.status, created.raw
        );
        let id = text(created.root(), "Id", case, "the create", &created.raw);
        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token from the create, got {etag:?}"
        );

        let read = get_policy_config(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from the config read, got {} with {}",
            read.status, read.raw
        );
        // `OriginRequestPolicyConfig` is this output's `@httpPayload`, so the configuration
        // IS the document's root element.
        assert_config(read.root(), &policy, "the config read", &read.raw);
        assert_eq!(
            read.header("etag"),
            etag,
            "case {case}: expected the config read to report the version the create handed \
             back ({etag:?}), got {:?}",
            read.header("etag")
        );

        // Parity with `GetOriginRequestPolicy`: both reads answer from the ONE record.
        let whole = get_policy(&router, &id).await;
        assert_eq!(
            whole.status, 200,
            "case {case}: expected HTTP 200 from GetOriginRequestPolicy, got {} with {}",
            whole.status, whole.raw
        );
        let nested = whole
            .root()
            .at("OriginRequestPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: GetOriginRequestPolicy carried no OriginRequestPolicyConfig: {}",
                    whole.raw
                )
            });
        assert_config(nested, &policy, "GetOriginRequestPolicy parity", &whole.raw);
        assert_eq!(
            whole.header("etag"),
            etag,
            "case {case}: expected one version token across both reads, got {:?} and {etag:?}",
            whole.header("etag")
        );

        // A read writes nothing: the same request answers the same document and version.
        let again = get_policy_config(&router, &id).await;
        assert_eq!(
            again.status, 200,
            "case {case}: expected HTTP 200 from the repeated config read, got {} with {}",
            again.status, again.raw
        );
        assert_eq!(
            again.raw, read.raw,
            "case {case}: expected a repeated config read to answer the same document, got \
             {} then {}",
            read.raw, again.raw
        );
        assert_eq!(
            again.header("etag"),
            etag,
            "case {case}: expected the version to survive a read, got {:?}",
            again.header("etag")
        );
    }
}

/// A wrapper the create LEFT OUT and one it sent EMPTY are different configurations, and
/// the config read reports each as submitted: absent stays absent, empty stays a wrapper
/// whose `Quantity` is zero. Asserted over the full cross of the three name lists against
/// absent / empty / one item, so no single list carries the rule.
#[tokio::test]
async fn absent_and_empty_list_wrappers_both_survive_the_config_read() {
    let router = emulator();

    // Absent, present-but-empty, and present with one item: the three states a CloudFront
    // list wrapper can reach through this surface.
    let variants: [(&str, Option<Vec<String>>); 3] = [
        ("absent", None),
        ("empty", Some(Vec::new())),
        ("one", Some(strings(&["x-one"]))),
    ];

    for (headers_case, headers) in &variants {
        for (cookies_case, cookies) in &variants {
            for (query_strings_case, query_strings) in &variants {
                let name =
                    format!("wrappers-{headers_case}-{cookies_case}-{query_strings_case}");
                let policy = Policy {
                    case: "the cross of absent, empty and one-item wrappers",
                    name: name.clone(),
                    comment: None,
                    header_behavior: "whitelist".to_string(),
                    headers: headers.clone(),
                    cookie_behavior: "whitelist".to_string(),
                    cookies: cookies.clone(),
                    query_string_behavior: "whitelist".to_string(),
                    query_strings: query_strings.clone(),
                };

                let created = create_policy(&router, &policy.xml()).await;
                assert_eq!(
                    created.status, 201,
                    "case {name}: expected HTTP 201 from the create, got {} with {}",
                    created.status, created.raw
                );
                let id = text(created.root(), "Id", &name, "the create", &created.raw);

                let read = get_policy_config(&router, &id).await;
                assert_eq!(
                    read.status, 200,
                    "case {name}: expected HTTP 200 from the config read, got {} with {}",
                    read.status, read.raw
                );
                assert_config(read.root(), &policy, &name, &read.raw);
            }
        }
    }
}

/// An id no policy holds is `NoSuchOriginRequestPolicy` (HTTP 404), whatever shape the id
/// takes — and the refusals leave the policy that does exist readable.
#[tokio::test]
async fn the_config_read_refuses_an_id_no_policy_holds() {
    let router = emulator();
    let present = Policy {
        case: "the one policy that exists",
        name: "config-read-present".to_string(),
        ..policy_table()[0].clone()
    };
    let created = create_policy(&router, &present.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from the create, got {} with {}",
        present.case, created.status, created.raw
    );
    let id = text(created.root(), "Id", present.case, "the create", &created.raw);
    let etag = created.header("etag").to_string();

    // Rows: a UUID never minted, the existing id with one character changed, the existing
    // id in upper case, a token that is no UUID, and the policy's name rather than its id.
    let mut flipped: Vec<char> = id.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'a' { 'b' } else { 'a' };
    let absent: [(&str, String); 5] = [
        (
            "a UUID never minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "the existing id with one character changed",
            flipped.into_iter().collect(),
        ),
        ("the existing id in upper case", id.to_uppercase()),
        ("a token that is no UUID", "not-a-policy-id".to_string()),
        ("a policy name rather than an id", present.name.clone()),
    ];

    for (case, candidate) in absent {
        let answer = get_policy_config(&router, &candidate).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchOriginRequestPolicy",
            "case {case}: expected NoSuchOriginRequestPolicy for {candidate:?}, got {}",
            answer.raw
        );
    }

    let read = get_policy_config(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case the existing policy: expected HTTP 200 after the refusals, got {} with {}",
        read.status, read.raw
    );
    assert_config(read.root(), &present, "after the refusals", &read.raw);
    assert_eq!(
        read.header("etag"),
        etag,
        "case the existing policy: expected the version token to survive the refusals, got {:?}",
        read.header("etag")
    );
}
