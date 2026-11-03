// SPDX-License-Identifier: CC0-1.0

//! `ListApplicationProviders` end to end over the `awsJson1_1` wire.
//!
//! The rule under test: an application provider is one of AWS's own — the modelled
//! `ApplicationProviderArn` pins the owner segment to the literal `aws`
//! (`^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$`) and no
//! operation of this service creates one — so `ListApplicationProviders` REPORTS the
//! service's application-provider catalogue
//! ([`state::BUILT_IN_APPLICATION_PROVIDERS`], the same records
//! `DescribeApplicationProvider` answers from): every entry exactly once, in the
//! CATALOGUE'S OWN order (the order of a service-published catalogue is the service's
//! data, not something this read imposes), every entry carrying the COMPLETE `DisplayData`
//! the shape declares (`DisplayName`, `Description` and an `https` `IconUrl`) and a
//! `ResourceServerConfig` exactly when the entry publishes access scopes. A caller that
//! names no `MaxResults` asked for no page size, so the whole catalogue comes back in ONE
//! page; any `MaxResults` the model's `range(1..100)` admits pages it, with a `NextToken`
//! reported whenever entries remain and ABSENT on the last page. A `NextToken` this
//! enumeration did not issue is refused with the modelled `ValidationException` (400),
//! and it writes nothing.
//!
//! The tables span what the rule generalises over: the page size (both published bounds,
//! one past each bound, sizes that divide the catalogue and sizes that do not, and the
//! size omitted entirely), the catalogue ENTRY (every row of the catalogue, read from the
//! crate so the test tracks it rather than restating it — `custom` and
//! `catalog/Statuspage` are the entries the recorded cases cite, the rest are entries
//! they do not), and the ways a cursor can be foreign.
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
        && partition_parts.iter().all(|part| {
            (1..=5).contains(&part.len()) && part.chars().all(|c| c.is_ascii_lowercase())
        });
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

/// The ARNs of the catalogue, in the order the catalogue publishes them — what EVERY
/// enumeration of it must report, in this order. Built from the crate's own catalogue, so
/// a provider added to it is covered here without editing this test. Shared with
/// `application_provider_resolution.rs`, which asserts the same projection over a single
/// unparameterised call.
pub(crate) fn expected_arns() -> Vec<String> {
    state::BUILT_IN_APPLICATION_PROVIDERS
        .iter()
        .map(|provider| format!("arn:aws:sso::aws:applicationProvider/{}", provider.id))
        .collect()
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

/// Every entry of the catalogue, read by paging the enumeration to its end with
/// `max_results` per page (or in the single page a call that names no `MaxResults` is
/// answered with), plus the number of pages it took.
async fn enumerate(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    case: &str,
) -> (Vec<Value>, usize) {
    let mut entries: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({});
        if let Some(size) = max_results {
            request["MaxResults"] = json!(size);
        }
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let (status, body) = list(router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {pages} expected 200, got {status} {body}"
        );
        let page = body["ApplicationProviders"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no ApplicationProviders in {body}"))
            .clone();
        if let Some(limit) = max_results.map(|size| size as usize) {
            assert!(
                page.len() <= limit,
                "case {case:?}: a page holds at most {limit} providers, got {} in {body}",
                page.len()
            );
        }
        entries.extend(page.iter().cloned());
        pages += 1;
        assert!(
            pages <= state::BUILT_IN_APPLICATION_PROVIDERS.len() + 1,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
        match body["NextToken"].as_str() {
            Some(token) => {
                assert!(
                    is_token(token),
                    "case {case:?}: NextToken must satisfy the modelled Token pattern, \
                     got {token:?}"
                );
                let limit = max_results.unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: a call that names no MaxResults must answer the \
                         whole catalogue in one page, got a NextToken: {body}"
                    )
                }) as usize;
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
    (entries, pages)
}

/// RULE — a single call that names no `MaxResults` reports exactly the catalogue, once
/// per entry, in the catalogue's own order, in ONE page.
#[tokio::test]
async fn the_whole_catalogue_is_reported_once_per_entry_in_catalogue_order() {
    let router = router();
    let (entries, pages) = enumerate(&router, None, "no MaxResults").await;
    let reported = arns(&entries);
    assert_eq!(
        reported,
        expected_arns(),
        "the enumeration must report exactly the catalogue, in the catalogue's own order, \
         got {} entries",
        reported.len()
    );
    assert_eq!(
        pages, 1,
        "a call that names no MaxResults must answer the whole catalogue in one page, got \
         {pages} pages for {} entries",
        reported.len()
    );
}

/// RULE — the catalogue holds BOTH id forms the service publishes: AWS managed providers
/// under a service slug, and the pre-integrated applications of the application catalogue
/// under `catalog/<Name>`; and it is bigger than one page, so paging is observable.
#[tokio::test]
async fn the_catalogue_holds_both_provider_forms_and_more_than_one_page() {
    let router = router();
    let (entries, pages) = enumerate(&router, Some(MAX_RESULTS_MAX), "the largest page").await;
    let reported = arns(&entries);

    let catalog_entries: Vec<&String> = reported
        .iter()
        .filter(|arn| arn.contains(":applicationProvider/catalog/"))
        .collect();
    let service_entries: Vec<&String> = reported
        .iter()
        .filter(|arn| !arn.contains(":applicationProvider/catalog/"))
        .collect();
    assert!(
        catalog_entries.len() > 1,
        "the catalogue must publish the pre-integrated applications under \
         applicationProvider/catalog/<Name>, got {catalog_entries:?}"
    );
    assert!(
        service_entries.len() > 1,
        "the catalogue must publish the AWS managed providers under their service slug, \
         got {service_entries:?}"
    );
    assert!(
        pages > 1,
        "the catalogue must span more than one page of {MAX_RESULTS_MAX} for pagination \
         to be observable, got {pages} page(s) for {} entries",
        reported.len()
    );

    // Entries the recorded cases cite, and entries they do not: both forms resolve.
    for id in ["custom", "glue", "catalog/Statuspage", "catalog/Zoom"] {
        let arn = format!("arn:aws:sso::aws:applicationProvider/{id}");
        assert!(
            reported.contains(&arn),
            "case {id:?}: the catalogue must hold {arn}"
        );
    }
}

/// RULE — over every page size the model admits: paging enumerates exactly the
/// catalogue, once per entry, in the catalogue's own order, and only a FULL page carries
/// a `NextToken`.
#[tokio::test]
async fn paging_enumerates_the_whole_catalogue_once_for_every_page_size() {
    let router = router();
    let expected = expected_arns();
    assert!(
        expected.len() >= 2,
        "the catalogue must hold more than one entry for paging to be observable"
    );

    // The rows differ along the page size: both published bounds, a size that divides the
    // catalogue, sizes that do not, and the size omitted entirely.
    let count = expected.len() as i64;
    let divides = (2..=MAX_RESULTS_MAX)
        .find(|size| count % size == 0)
        .unwrap_or(MAX_RESULTS_MIN);
    let sizes: Vec<Option<i64>> = vec![
        None,
        Some(MAX_RESULTS_MIN),
        Some(7),
        Some(divides),
        Some(MAX_RESULTS_MAX - 1),
        Some(MAX_RESULTS_MAX),
    ];

    for size in sizes {
        let case = format!("MaxResults = {size:?}");
        let (entries, _) = enumerate(&router, size, &case).await;
        assert_eq!(
            arns(&entries),
            expected,
            "case {case:?}: paging must report exactly the catalogue, in the catalogue's \
             own order"
        );
    }
}

/// RULE — each listed entry carries the catalogue record's own values and a COMPLETE
/// `DisplayData`, and the listing agrees member for member with
/// `DescribeApplicationProvider` on the same ARN: one catalogue, two projections. The
/// table is every entry of the catalogue.
#[tokio::test]
async fn every_listed_entry_matches_the_catalogue_and_its_describe() {
    let router = router();
    let (listed, _) = enumerate(&router, None, "no MaxResults").await;

    for provider in state::BUILT_IN_APPLICATION_PROVIDERS {
        let arn = format!("arn:aws:sso::aws:applicationProvider/{}", provider.id);
        let entry = listed
            .iter()
            .find(|entry| entry["ApplicationProviderArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {:?}: the enumeration must list {arn}", provider.id));
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
        assert_display_data(&entry["DisplayData"], provider);
        assert_resource_server_config(&entry["ResourceServerConfig"], provider);

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
        "the enumeration must report the catalogue and nothing else, got {} entries",
        listed.len()
    );
}

/// RULE — `DisplayData` is reported COMPLETE for every provider: the record's display
/// name, its display name as an application as the description, and the provider's static
/// portal icon URL, each inside the member's modelled length (`DisplayName` 1..100,
/// `Description` 1..128, `IconUrl` 1..768 matching `^(http|https)://.*$`).
///
/// The description rule holds for EVERY entry, whichever of the two id forms it carries:
/// `application_provider_display_data.rs` owns the suite that states it and the
/// whole-provider structure the describe reports it in.
pub(crate) fn assert_display_data(
    display_data: &Value,
    provider: &state::BuiltInApplicationProvider,
) {
    let id = provider.id;
    assert_eq!(
        display_data["DisplayName"].as_str(),
        Some(provider.display_name),
        "case {id:?}: expected DisplayName {:?}, got {display_data}",
        provider.display_name
    );
    assert!(
        (1..=100).contains(&provider.display_name.chars().count()),
        "case {id:?}: DisplayName is 1..100 characters, got {:?}",
        provider.display_name
    );

    let description = display_data["Description"].as_str().unwrap_or_else(|| {
        panic!("case {id:?}: a provider must carry a Description, got {display_data}")
    });
    assert_eq!(
        description,
        provider.description(),
        "case {id:?}: expected the record's Description, got {description:?}"
    );
    assert!(
        (1..=128).contains(&description.chars().count()),
        "case {id:?}: Description is 1..128 characters, got {description:?}"
    );
    // Every provider's portal description is its display name as an application.
    assert_eq!(
        description,
        format!("{} Application", provider.display_name),
        "case {id:?}: a provider's Description is its display name as an application, got \
         {description:?}"
    );

    let icon_url = display_data["IconUrl"].as_str().unwrap_or_else(|| {
        panic!("case {id:?}: a provider must carry an IconUrl, got {display_data}")
    });
    assert!(
        icon_url.starts_with("https://"),
        "case {id:?}: IconUrl must match ^(http|https)://.*$, got {icon_url:?}"
    );
    assert!(
        (1..=768).contains(&icon_url.chars().count()),
        "case {id:?}: IconUrl is 1..768 characters, got {icon_url:?}"
    );
    assert_eq!(
        icon_url,
        provider.icon_url(),
        "case {id:?}: expected the record's IconUrl, got {icon_url:?}"
    );
    // The provider's static portal icon path: its own `app-<id>` under the portal's
    // default icon file.
    let path = icon_url
        .strip_prefix("https://")
        .and_then(|rest| rest.split_once('/'))
        .map(|(_host, path)| path)
        .unwrap_or_else(|| panic!("case {id:?}: expected a path in the IconUrl, got {icon_url:?}"));
    let segment = path
        .split('/')
        .next()
        .unwrap_or_else(|| panic!("case {id:?}: expected an app segment, got {icon_url:?}"));
    assert!(
        segment.starts_with("app-") && segment.len() > "app-".len(),
        "case {id:?}: expected the provider's app-<id> segment, got {icon_url:?}"
    );
    assert!(
        path.ends_with("/icons/default.png"),
        "case {id:?}: expected the portal's default icon file, got {icon_url:?}"
    );
}

/// RULE — `ResourceServerConfig` is reported exactly when the record publishes access
/// scopes, with each scope's own title and description.
pub(crate) fn assert_resource_server_config(
    resource_server_config: &Value,
    provider: &state::BuiltInApplicationProvider,
) {
    let id = provider.id;
    if provider.scopes.is_empty() {
        assert_eq!(
            resource_server_config,
            &Value::Null,
            "case {id:?}: no resource server is published, so none may be reported, got \
             {resource_server_config}"
        );
        return;
    }
    for scope in provider.scopes {
        let details = &resource_server_config["Scopes"][scope.name];
        assert_eq!(
            details["DetailedTitle"].as_str(),
            Some(scope.detailed_title),
            "case {id:?}: expected scope {:?} with its title, got {resource_server_config}",
            scope.name
        );
        assert_eq!(
            details["LongDescription"].as_str(),
            Some(scope.long_description),
            "case {id:?}: expected scope {:?} with its description, got \
             {resource_server_config}",
            scope.name
        );
    }
}

/// RULE — a provider's icon URL is derived from the provider, so it is STABLE across
/// reads and DISTINCT per provider: no two entries of the catalogue share one.
#[tokio::test]
async fn every_provider_has_its_own_stable_icon_url() {
    let router = router();
    let (first, _) = enumerate(&router, None, "first read").await;
    let (second, _) = enumerate(&router, None, "second read").await;
    assert_eq!(
        first, second,
        "two reads of the catalogue must report the identical icon URLs"
    );

    let mut urls: Vec<&str> = first
        .iter()
        .map(|entry| {
            entry["DisplayData"]["IconUrl"]
                .as_str()
                .unwrap_or_else(|| panic!("an entry without an IconUrl: {entry}"))
        })
        .collect();
    let total = urls.len();
    urls.sort_unstable();
    urls.dedup();
    assert_eq!(
        urls.len(),
        total,
        "each provider must have its own IconUrl, got {} distinct for {total} providers",
        urls.len()
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
        "two reads of the catalogue must report the identical records"
    );

    // Paging through the catalogue consumes nothing either.
    let (_, pages) = enumerate(&router, Some(1), "one entry per page").await;
    assert_eq!(
        pages,
        state::BUILT_IN_APPLICATION_PROVIDERS.len(),
        "a page of one per entry takes one page per entry, got {pages}"
    );

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
        "a read must not change what a later caller sees"
    );
}
