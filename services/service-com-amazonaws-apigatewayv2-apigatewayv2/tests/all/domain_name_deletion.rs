// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteDomainName` and `DeleteApiMapping`.
//!
//! The rules these cases assert:
//!
//! * **A delete removes exactly the resource its URI names and answers the status the
//!   service answers it with** ("Deletes a domain name", "Deletes an API mapping"): the
//!   resource stops being
//!   reported by its own reader and by the collection it was in, every sibling is left
//!   exactly as it was, and a second delete of the same resource is the 404
//!   `NotFoundException` of a resource the account no longer holds. `DeleteApiMapping`
//!   answers the `204 No Content` its model binds; `DeleteDomainName` answers
//!   `202 Accepted`, the status of a teardown that continues after the answer.
//! * **A domain name OWNS its api mappings**: deleting the domain takes its mappings with
//!   it, so neither the domain nor any mapping under it is reported afterwards, and the
//!   name is free for a later `CreateDomainName`.
//! * **An api mapping is addressed under its domain**: a delete naming a domain the account
//!   does not hold, or a mapping id that domain does not hold, is 404 and removes nothing.
//!
//! The rows vary what the rules generalise over: how many mappings the domain holds (one,
//! several), whether the mapping deleted is the one at the domain's root or one under a
//! key, which of several domains the request names, and whether the resource was deleted
//! directly or as part of its parent.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    api_mapping_not_found_message, INVALID_DOMAIN_NAME_IDENTIFIER,
};

use crate::api_lifecycle::{as_str, emulator, error_code};
use crate::domain_name_lifecycle::{call, certificate_arn, get_domain_name};

/// Create one custom domain name, asserting the 201 the model binds to
/// `CreateDomainName`.
pub(crate) async fn seed_domain(emulator: &MultiServiceRouter, case: &str, domain: &str) {
    let (status, body) = call(
        emulator,
        "POST",
        "/v2/domainnames",
        Some(json!({
            "domainName": domain,
            "domainNameConfigurations": [{ "certificateArn": certificate_arn(domain) }]
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateDomainName for {domain:?}, got {status} {body}"
    );
}

/// Create one quick-created HTTP api, which holds the `$default` stage a mapping points
/// at, and answer its `ApiId`.
async fn seed_api(emulator: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, body) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": name,
            "protocolType": "HTTP",
            "target": "https://example.com/backend"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {body}"
    );
    as_str(&body, "apiId")
}

/// Map `api_id`'s `$default` stage onto `domain`, under `key` when the row names one, and
/// answer the `ApiMappingId`.
async fn seed_mapping(
    emulator: &MultiServiceRouter,
    case: &str,
    domain: &str,
    api_id: &str,
    key: Option<&str>,
) -> String {
    let mut request = json!({ "apiId": api_id, "stage": "$default" });
    if let Some(key) = key {
        request["apiMappingKey"] = json!(key);
    }
    let (status, body) = call(
        emulator,
        "POST",
        &format!("/v2/domainnames/{domain}/apimappings"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from CreateApiMapping, got {status} {body}"
    );
    as_str(&body, "apiMappingId")
}

/// `GetApiMapping` for one mapping of one domain.
async fn get_mapping(
    emulator: &MultiServiceRouter,
    domain: &str,
    api_mapping_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/domainnames/{domain}/apimappings/{api_mapping_id}"),
        None,
    )
    .await
}

/// The ids `GetApiMappings` lists for one domain.
async fn listed_mappings(emulator: &MultiServiceRouter, case: &str, domain: &str) -> Vec<String> {
    let (status, body) = call(
        emulator,
        "GET",
        &format!("/v2/domainnames/{domain}/apimappings"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetApiMappings, got {status} {body}"
    );
    body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| as_str(item, "apiMappingId"))
        .collect()
}

/// Assert a refusal is the 404 `NotFoundException` whose message names the resource kind
/// the request addressed.
pub(crate) fn assert_not_found(case: &str, status: u16, body: &Value, expected_message: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "NotFoundException",
        "case {case:?}: expected NotFoundException, got {body}"
    );
    assert_eq!(
        as_str(body, "message"),
        expected_message,
        "case {case:?}: expected the refusal to name the resource kind, got {body}"
    );
}

/// One row: which mapping of a domain holding several is deleted.
struct Row {
    case: &'static str,
    /// The `ApiMappingKey` of each mapping the domain holds, in creation order.
    keys: &'static [Option<&'static str>],
    /// Which of them the case deletes.
    delete: usize,
}

const ROWS: &[Row] = &[
    Row {
        case: "the only mapping, at the domain's root",
        keys: &[None],
        delete: 0,
    },
    Row {
        case: "the root mapping of a domain that also holds two keyed ones",
        keys: &[None, Some("orders"), Some("catalog")],
        delete: 0,
    },
    Row {
        case: "one keyed mapping of three",
        keys: &[None, Some("orders"), Some("catalog")],
        delete: 1,
    },
    Row {
        case: "the last keyed mapping of three",
        keys: &[None, Some("orders"), Some("catalog")],
        delete: 2,
    },
];

#[tokio::test]
async fn deleting_an_api_mapping_removes_only_that_mapping() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let domain = format!("mapping-{index}.example.com");
        seed_domain(&emulator, case, &domain).await;
        let api_id = seed_api(&emulator, case, &format!("mapping-api-{index}")).await;
        let mut ids = Vec::new();
        for key in row.keys {
            ids.push(seed_mapping(&emulator, case, &domain, &api_id, *key).await);
        }
        let deleted = ids[row.delete].clone();

        let (status, body) = call(
            &emulator,
            "DELETE",
            &format!("/v2/domainnames/{domain}/apimappings/{deleted}"),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected the modelled 204 from DeleteApiMapping, got {status} {body}"
        );

        // The mapping is gone from its own reader and from the collection; every sibling
        // is still there.
        let (status, body) = get_mapping(&emulator, &domain, &deleted).await;
        assert_not_found(case, status, &body, &api_mapping_not_found_message(&deleted));
        let listed = listed_mappings(&emulator, case, &domain).await;
        assert!(
            !listed.contains(&deleted),
            "case {case:?}: expected the deleted mapping gone from the listing, got {listed:?}"
        );
        for (position, id) in ids.iter().enumerate() {
            if position == row.delete {
                continue;
            }
            assert!(
                listed.contains(id),
                "case {case:?}: expected the sibling mapping {id:?} untouched, got {listed:?}"
            );
            let (status, sibling) = get_mapping(&emulator, &domain, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the sibling mapping readable, got {status} {sibling}"
            );
        }
        // The domain itself is untouched.
        let (status, held) = get_domain_name(&emulator, &domain).await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting a mapping must leave its domain, got {status} {held}"
        );

        // A second delete of the same mapping is the 404 of a mapping the domain no
        // longer holds.
        let (status, body) = call(
            &emulator,
            "DELETE",
            &format!("/v2/domainnames/{domain}/apimappings/{deleted}"),
            None,
        )
        .await;
        assert_not_found(case, status, &body, &api_mapping_not_found_message(&deleted));
    }
}

#[tokio::test]
async fn deleting_a_domain_name_takes_its_api_mappings_with_it() {
    let case = "a domain holding two mappings, beside a second domain";
    let emulator = emulator();
    let domain = "deleted.example.com";
    let other = "kept.example.com";
    seed_domain(&emulator, case, domain).await;
    seed_domain(&emulator, case, other).await;
    let api_id = seed_api(&emulator, case, "domain-deletion-api").await;
    let root = seed_mapping(&emulator, case, domain, &api_id, None).await;
    let keyed = seed_mapping(&emulator, case, domain, &api_id, Some("orders")).await;
    let kept = seed_mapping(&emulator, case, other, &api_id, None).await;

    let (status, body) = call(
        &emulator,
        "DELETE",
        &format!("/v2/domainnames/{domain}"),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202 from DeleteDomainName, got {status} {body}"
    );

    // The domain is gone, and so is every mapping that was addressed under it.
    let (status, body) = get_domain_name(&emulator, domain).await;
    assert_not_found(case, status, &body, INVALID_DOMAIN_NAME_IDENTIFIER);
    for mapping_id in [&root, &keyed] {
        let (status, body) = get_mapping(&emulator, domain, mapping_id).await;
        assert_not_found(case, status, &body, INVALID_DOMAIN_NAME_IDENTIFIER);
    }
    // The second domain and its own mapping are untouched: a deletion removes one
    // resource, not a kind of resource.
    let (status, held) = get_domain_name(&emulator, other).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the other domain untouched, got {status} {held}"
    );
    let (status, held) = get_mapping(&emulator, other, &kept).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the other domain's mapping untouched, got {status} {held}"
    );
    // The api the mappings pointed at is not a child of the domain, so it is still there.
    let (status, api) = call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: deleting a domain must leave the api it mapped, got {status} {api}"
    );

    // The name is free again: a later create takes it, with its own configuration.
    seed_domain(&emulator, case, domain).await;
    let (status, recreated) = get_domain_name(&emulator, domain).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the name free for a later create, got {status} {recreated}"
    );
    assert!(
        listed_mappings(&emulator, case, domain).await.is_empty(),
        "case {case:?}: a recreated domain holds none of the deleted domain's mappings"
    );

    // And a second delete of the deleted name, and a delete of a name nothing minted, are
    // each the 404 of a domain the account does not hold.
    for target in ["never.example.com", "also-never.example.com"] {
        let (status, body) = call(
            &emulator,
            "DELETE",
            &format!("/v2/domainnames/{target}"),
            None,
        )
        .await;
        assert_not_found(case, status, &body, INVALID_DOMAIN_NAME_IDENTIFIER);
    }
}

/// A mapping is addressed under its own domain: a delete naming another domain removes
/// nothing.
#[tokio::test]
async fn an_api_mapping_is_deleted_only_under_its_own_domain() {
    let case = "a mapping deleted under another domain";
    let emulator = emulator();
    let domain = "owner.example.com";
    let other = "stranger.example.com";
    seed_domain(&emulator, case, domain).await;
    seed_domain(&emulator, case, other).await;
    let api_id = seed_api(&emulator, case, "cross-domain-api").await;
    let mapping_id = seed_mapping(&emulator, case, domain, &api_id, None).await;

    let (status, body) = call(
        &emulator,
        "DELETE",
        &format!("/v2/domainnames/{other}/apimappings/{mapping_id}"),
        None,
    )
    .await;
    assert_not_found(
        case,
        status,
        &body,
        &api_mapping_not_found_message(&mapping_id),
    );
    let (status, body) = call(
        &emulator,
        "DELETE",
        &format!("/v2/domainnames/never.example.com/apimappings/{mapping_id}"),
        None,
    )
    .await;
    assert_not_found(case, status, &body, INVALID_DOMAIN_NAME_IDENTIFIER);

    // The mapping is still there, under the domain that owns it.
    let (status, held) = get_mapping(&emulator, domain, &mapping_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a refused delete must leave the mapping, got {status} {held}"
    );
}
