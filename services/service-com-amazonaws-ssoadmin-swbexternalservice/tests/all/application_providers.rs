// SPDX-License-Identifier: CC0-1.0

//! `ListApplicationProviders` end to end over the `awsJson1_1` wire.
//!
//! The rule under test: an application provider is one of AWS's own — the modelled
//! `ApplicationProviderArn` pins the owner segment to the literal `aws`
//! (`^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$`) and no
//! operation of this service creates one — so `ListApplicationProviders` REPORTS the
//! service's own catalogue ([`state::BUILT_IN_APPLICATION_PROVIDERS`], the same records
//! `DescribeApplicationProvider` answers from): every entry exactly once, in
//! `ApplicationProviderArn` order, each member projected from the catalogue record,
//! paged by ANY `MaxResults` the model's `range(1..100)` admits with a `NextToken` that
//! resumes strictly after the last entry of the previous page and is ABSENT on the last
//! page. A `NextToken` this enumeration did not issue is refused with the modelled
//! `ValidationException` (400), and it writes nothing.
//!
//! The tables span what the rule generalises over: the page size (both published bounds,
//! one past each bound, the catalogue size, sizes that divide it and sizes that do not,
//! and the size omitted entirely), the catalogue ENTRY (every row of the catalogue, read
//! from the crate so the test tracks it rather than restating it — `custom` is the entry
//! the recorded cases cite, the rest are entries they do not), and the ways a cursor can
//! be foreign.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints
//! and the `X-Amz-Target` routing are exercised rather than bypassed. The harness is the
//! one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{call, error_code, router, seeded_instance_arn};

/// The model's `MaxResults` bounds for this operation: `range(min: 1, max: 100)`
/// (`smithy_query(query="operation", operation="ListApplicationProviders")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// Whether `arn` has the shape the model's `ApplicationProviderArn` pattern requires:
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$`, length
/// 10..1224. The account segment is the literal `aws`: the provider belongs to AWS.
fn is_application_provider_arn(arn: &str) -> bool {
    let Some(resource) = arn.strip_prefix("arn:aws") else {
        return false;
    };
    let Some((partition_suffix, id)) = resource.split_once(":sso::aws:applicationProvider/") else {
        return false;
    };
    let partition_parts: Vec<&str> = partition_suffix.split('-').skip(1).collect();
    let partition_ok = (partition_suffix.is_empty() || partition_suffix.starts_with('-'))
        && partition_parts.len() <= 3
        && partition_parts
            .iter()
            .all(|part| (1..=5).contains(&part.len()) && part.chars().all(|c| c.is_ascii_lowercase()));
    partition_ok
        && !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '/')
        && (10..=1224).contains(&arn.len())
}

/// Whether `token` satisfies the model's `Token` constraints: 0..2048 of
/// `^[-a-zA-Z0-9+=/_]*$`.
fn is_token(token: &str) -> bool {
    token.len() <= 2048
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-+=/_".contains(c))
}

/// One `ListApplicationProviders` call: `(status, body)`.
async fn list(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "ListApplicationProviders", request).await
}

/// The ARNs of the catalogue, ascending — what EVERY enumeration of it must report, in
/// this order. Built from the crate's own catalogue, so a provider added to it is
/// covered here without editing this test.
fn expected_arns() -> Vec<String> {
    let mut arns: Vec<String> = state::BUILT_IN_APPLICATION_PROVIDERS
        .iter()
        .map(|provider| format!("arn:aws:sso::aws:applicationProvider/{}", provider.id))
        .collect();
    arns.sort();
    arns
}

/// Every provider of the catalogue, read in one unpaged call (the catalogue is far
/// smaller than the model's 100 ceiling).
async fn whole_catalogue(router: &MultiServiceRouter) -> Vec<Value> {
    let (status, body) = list(router, &json!({})).await;
    assert_eq!(
        status, 200,
        "ListApplicationProviders expected 200, got {status} {body}"
    );
    assert_eq!(
        body["NextToken"],
        Value::Null,
        "the catalogue fits one default page, so no NextToken may be reported, got {body}"
    );
    body["ApplicationProviders"]
        .as_array()
        .unwrap_or_else(|| panic!("no ApplicationProviders in {body}"))
        .clone()
}

/// The ARN of each provider in `providers`, in the order reported.
fn arns(providers: &[Value]) -> Vec<String> {
    providers
        .iter()
        .map(|provider| {
            provider["ApplicationProviderArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("a provider without an ApplicationProviderArn: {provider}")
                })
                .to_string()
        })
        .collect()
}

/// RULE — one unpaged read reports exactly the catalogue, once per entry, ascending by
/// ARN.
#[tokio::test]
async fn the_catalogue_is_reported_once_per_entry_in_arn_order() {
    let router = router();
    let reported = arns(&whole_catalogue(&router).await);
    assert_eq!(
        reported,
        expected_arns(),
        "the enumeration must report exactly the catalogue, ascending by ARN, got {reported:?}"
    );
}

/// RULE — over every page size the model admits: paging enumerates exactly the
/// catalogue, once per entry, in ARN order, and only a FULL page carries a `NextToken`.
#[tokio::test]
async fn paging_enumerates_the_whole_catalogue_once_for_every_page_size() {
    let router = router();
    let expected = expected_arns();
    assert!(
        expected.len() >= 2,
        "the catalogue must hold more than one entry for paging to be observable, got {expected:?}"
    );

    // The rows differ along the page size: both published bounds, a size that divides
    // the catalogue, sizes that do not, the exact catalogue size, one either side of it,
    // and the size omitted entirely.
    let count = expected.len() as i64;
    let sizes: Vec<Option<i64>> = vec![
        None,
        Some(MAX_RESULTS_MIN),
        Some(2),
        Some(3),
        Some(count - 1),
        Some(count),
        Some(count + 1),
        Some(MAX_RESULTS_MAX),
    ];

    for size in sizes {
        let case = format!("MaxResults = {size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = json!({});
            if let Some(size) = size {
                request["MaxResults"] = json!(size);
            }
            if let Some(token) = next_token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let (status, body) = list(&router, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: page {pages} expected 200, got {status} {body}"
            );
            let page = body["ApplicationProviders"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no ApplicationProviders in {body}"))
                .clone();
            let limit = size.unwrap_or(MAX_RESULTS_MAX) as usize;
            assert!(
                page.len() <= limit,
                "case {case:?}: a page holds at most MaxResults providers, got {} in {body}",
                page.len()
            );
            seen.extend(arns(&page));
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );

            match body["NextToken"].as_str() {
                Some(token) => {
                    assert!(
                        is_token(token),
                        "case {case:?}: NextToken must satisfy the modelled Token pattern, \
                         got {token:?}"
                    );
                    assert_eq!(
                        page.len(),
                        limit,
                        "case {case:?}: a page that reports a NextToken is full, got {} of \
                         {limit} in {body}",
                        page.len()
                    );
                    next_token = Some(token.to_string());
                }
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: paging must report exactly the catalogue, in ARN order, got {seen:?}"
        );
    }
}

/// RULE — each listed entry carries the catalogue record's own values, and the listing
/// agrees member for member with `DescribeApplicationProvider` on the same ARN: one
/// catalogue, two projections. The table is every entry of the catalogue.
#[tokio::test]
async fn every_listed_entry_matches_the_catalogue_and_its_describe() {
    let router = router();
    let listed = whole_catalogue(&router).await;

    for provider in state::BUILT_IN_APPLICATION_PROVIDERS {
        let arn = format!("arn:aws:sso::aws:applicationProvider/{}", provider.id);
        let entry = listed
            .iter()
            .find(|entry| entry["ApplicationProviderArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the enumeration must list {arn}, got {:?}",
                    provider.id,
                    arns(&listed)
                )
            });
        assert!(
            is_application_provider_arn(arn.as_str()),
            "case {:?}: expected a modelled ApplicationProviderArn, got {arn:?}",
            provider.id
        );
        assert_eq!(
            entry["FederationProtocol"].as_str(),
            Some(provider.federation_protocol),
            "case {:?}: expected FederationProtocol {:?}, got {entry}",
            provider.id,
            provider.federation_protocol
        );
        // The model's `FederationProtocol` enum is exactly `SAML | OAUTH`.
        assert!(
            ["SAML", "OAUTH"].contains(&provider.federation_protocol),
            "case {:?}: {:?} is not a modelled FederationProtocol",
            provider.id,
            provider.federation_protocol
        );
        assert_eq!(
            entry["DisplayData"]["DisplayName"].as_str(),
            Some(provider.display_name),
            "case {:?}: expected DisplayName {:?}, got {entry}",
            provider.id,
            provider.display_name
        );
        // `DisplayData$DisplayName` is `Name`, 1..100 characters.
        assert!(
            (1..=100).contains(&provider.display_name.len()),
            "case {:?}: DisplayName is 1..100 characters, got {:?}",
            provider.id,
            provider.display_name
        );
        // Nothing holds an icon, a blurb or a resource server for these providers, so
        // none may be invented on the listing either.
        for absent in ["IconUrl", "Description"] {
            assert_eq!(
                entry["DisplayData"][absent],
                Value::Null,
                "case {:?}: no {absent} is held, so none may be reported, got {entry}",
                provider.id
            );
        }
        assert_eq!(
            entry["ResourceServerConfig"],
            Value::Null,
            "case {:?}: no resource server is held, so none may be reported, got {entry}",
            provider.id
        );

        // The two reads of ONE record must agree member for member.
        let (status, described) = call(
            &router,
            "DescribeApplicationProvider",
            &json!({"ApplicationProviderArn": arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeApplicationProvider expected 200, got {status} {described}",
            provider.id
        );
        for member in [
            "ApplicationProviderArn",
            "FederationProtocol",
            "DisplayData",
            "ResourceServerConfig",
        ] {
            assert_eq!(
                entry[member], described[member],
                "case {:?}: member {member} must agree with DescribeApplicationProvider, \
                 got {:?} vs {:?}",
                provider.id, entry[member], described[member]
            );
        }
    }

    assert_eq!(
        listed.len(),
        state::BUILT_IN_APPLICATION_PROVIDERS.len(),
        "the enumeration must report the catalogue and nothing else, got {:?}",
        arns(&listed)
    );
}

/// A `MaxResults` outside the model's `range(1..100)` is a BAD REQUEST, refused before
/// the handler runs. The rows are one past each published bound, plus a negative; each
/// bound itself is accepted.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_a_validation_error() {
    let router = router();

    for (case, max_results) in [
        ("one below the minimum", MAX_RESULTS_MIN - 1),
        ("one above the maximum", MAX_RESULTS_MAX + 1),
        ("a negative page size", -5),
    ] {
        let (status, body) = list(&router, &json!({"MaxResults": max_results})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for MaxResults {max_results}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException for MaxResults {max_results}, got {body}"
        );
    }

    for max_results in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let (status, body) = list(&router, &json!({"MaxResults": max_results})).await;
        assert_eq!(
            status, 200,
            "MaxResults {max_results} is inside the modelled range, got {status} {body}"
        );
    }
}

/// RULE — a `NextToken` this enumeration did not issue is refused with the modelled
/// `ValidationException` (400), never answered with a page: an empty page would end a
/// caller's enumeration early. The rows span the ways a cursor can be foreign.
#[tokio::test]
async fn a_next_token_this_enumeration_did_not_issue_is_refused() {
    let router = router();

    // A cursor from ANOTHER enumeration of this same service, built with the service's
    // OWN encoder so the row is a real cursor and only its scope is wrong.
    let instance_arn = seeded_instance_arn(&router).await;
    let foreign_tokens = [
        (
            "a cursor of the ListInstances enumeration",
            state::encode_page_token(state::CURSOR_INSTANCES, &instance_arn),
        ),
        (
            "a cursor of a ListPermissionSets enumeration",
            state::encode_page_token(
                &state::cursor_permission_sets(&instance_arn),
                "arn:aws:sso:::permissionSet/ssoins-0000000000000000/ps-0000000000000000",
            ),
        ),
    ];

    // A cursor this enumeration DID issue, then mutilated: still pattern-valid, no
    // longer a cursor of this service.
    let (status, body) = list(&router, &json!({"MaxResults": 1})).await;
    assert_eq!(
        status, 200,
        "ListApplicationProviders expected 200, got {status} {body}"
    );
    let issued = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a first page of one provider must issue a cursor: {body}"))
        .to_string();
    let mutilated = issued[..issued.len() - 1].to_string();

    let mut rows: Vec<(&str, String)> = vec![
        ("a token no service issued", "deadbeef".to_string()),
        ("a token that is not even hex", "zzzz".to_string()),
        ("a cursor of this enumeration, mutilated", mutilated),
    ];
    rows.extend(foreign_tokens);

    for (case, token) in rows {
        assert!(
            is_token(&token),
            "case {case:?}: the row must be a model-valid Token so the HANDLER refuses it, \
             got {token:?}"
        );
        let (status, body) = list(&router, &json!({"NextToken": token})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {token:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException for {token:?}, got {body}"
        );
    }
}

/// `ListApplicationProviders` is a READ: listing twice reports the identical catalogue,
/// paging through it consumes nothing, and an unrelated write leaves it untouched.
#[tokio::test]
async fn listing_providers_changes_nothing() {
    let router = router();

    let (status, first) = list(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "ListApplicationProviders expected 200, got {status} {first}"
    );
    let (status, second) = list(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "ListApplicationProviders expected 200, got {status} {second}"
    );
    assert_eq!(
        first, second,
        "two reads of the catalogue must report the identical records, got {first} then {second}"
    );

    // Paging through the catalogue consumes nothing either.
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({"MaxResults": 1});
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let (status, body) = list(&router, &request).await;
        assert_eq!(status, 200, "a page expected 200, got {status} {body}");
        match body["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }

    // A write to an unrelated store does not alter the AWS-owned catalogue.
    let instance_arn = seeded_instance_arn(&router).await;
    let (status, body) = call(
        &router,
        "CreateTrustedTokenIssuer",
        &json!({
            "InstanceArn": instance_arn,
            "Name": "CatalogueStability",
            "TrustedTokenIssuerType": "OIDC_JWT",
            "TrustedTokenIssuerConfiguration": {
                "OidcJwtConfiguration": {
                    "IssuerUrl": "https://example.com/oidc",
                    "ClaimAttributePath": "email",
                    "IdentityStoreAttributePath": "email",
                    "JwksRetrievalOption": "OPEN_ID_DISCOVERY",
                }
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateTrustedTokenIssuer expected 200, got {status} {body}"
    );

    let (status, after) = list(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "ListApplicationProviders expected 200, got {status} {after}"
    );
    assert_eq!(
        first, after,
        "a read must not change what a later caller sees, got {first} then {after}"
    );
}
