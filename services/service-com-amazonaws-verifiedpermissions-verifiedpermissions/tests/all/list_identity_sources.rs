// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `ListIdentitySources`: what it reports about each
//! persisted identity source, how it pages, how `filters` selects and what it
//! refuses.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so
//! a listed identity source is only visible if `CreateIdentitySource` really
//! reached state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, error_code, ok, store_with_validation_off as store, user_pool_arn, SIGNING_REGION,
};

/// A Cognito user pool this emulator can resolve: one of its own account, in the
/// region the harness signs for, whose pool id names that same region.
fn pool_arn() -> String {
    user_pool_arn(SIGNING_REGION, &format!("{SIGNING_REGION}_1a2b3c4d5"))
}

/// Create one Cognito identity source whose principals carry
/// `principal_entity_type`, and return its `identitySourceId`.
async fn create_source(
    router: &MultiServiceRouter,
    store_id: &str,
    principal_entity_type: &str,
) -> String {
    let created = ok(
        router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "principalEntityType": principal_entity_type,
            "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": pool_arn() } },
        }),
    )
    .await;
    created["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string()
}

/// List one page and return the parsed response, requiring success.
async fn list(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "ListIdentitySources", request).await
}

/// Walk every page of a request, `page_size` at a time, and return the listed
/// items in the order they arrived. Fails the calling test if the walk does not
/// terminate within `bound` pages.
async fn walk(
    router: &MultiServiceRouter,
    request: &Value,
    page_size: usize,
    bound: usize,
    case: &str,
) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut page_request = request.clone();
        page_request["maxResults"] = json!(page_size);
        if let Some(token) = token.as_deref() {
            page_request["nextToken"] = json!(token);
        }
        let page = list(router, &page_request).await;
        let page_items = page["identitySources"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            page_items.len() <= page_size,
            "case {case:?}: a page may not exceed maxResults {page_size}, got {page}"
        );
        items.extend(page_items);
        pages += 1;
        assert!(
            pages <= bound,
            "case {case:?}: pagination did not terminate within {bound} pages, got {page}"
        );
        match page["nextToken"].as_str() {
            Some(next) => {
                assert!(
                    next.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_=+/.".contains(c)),
                    "case {case:?}: nextToken must match ^[A-Za-z0-9-_=+/\\.]*$, got {next:?}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    items
}

// ---------------------------------------------------------------------------
// What an IdentitySourceItem reports
// ---------------------------------------------------------------------------

/// A listed `IdentitySourceItem` reports exactly what `GetIdentitySource`
/// reports about the same stored identity source — the two shapes carry the same
/// members — across the whole range of configurations a create may carry: both
/// arms of the `Configuration` union, both arms of
/// `OpenIdConnectTokenSelection`, with and without the optional client ids,
/// group configuration, entity-id prefix and `principalEntityType`.
#[tokio::test]
async fn a_listed_identity_source_reports_what_get_identity_source_reports() {
    let arn = pool_arn();
    // (case, request members beside policyStoreId)
    let cases: Vec<(&str, Value)> = vec![
        (
            // No principalEntityType: the create's optional member resolves to
            // the service's default, because every read requires one.
            "cognito-minimal",
            json!({ "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": arn } } }),
        ),
        (
            "cognito-clients-groups-and-entity-type",
            json!({
                "configuration": {
                    "cognitoUserPoolConfiguration": {
                        "userPoolArn": arn,
                        "clientIds": ["a1b2c3d4e5f6g7h8i9j0kalbmc", "zyxwvutsrq9876543210"],
                        "groupConfiguration": { "groupEntityType": "MyCorp::Group" },
                    }
                },
                "principalEntityType": "MyCorp::User",
            }),
        ),
        (
            "oidc-access-token-only",
            json!({
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://auth.example.com",
                        "tokenSelection": {
                            "accessTokenOnly": {
                                "principalIdClaim": "sub",
                                "audiences": ["https://myapp.example.com"],
                            }
                        },
                    }
                },
                "principalEntityType": "MyCorp::Person",
            }),
        ),
        (
            "oidc-identity-token-with-prefix-and-groups",
            json!({
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://oidc.example.org/tenant",
                        "entityIdPrefix": "MyOIDCProvider",
                        "groupConfiguration": {
                            "groupClaim": "groups",
                            "groupEntityType": "MyCorp::UserGroup",
                        },
                        "tokenSelection": {
                            "identityTokenOnly": {
                                "principalIdClaim": "sub",
                                "clientIds": ["clientone", "clienttwo"],
                            }
                        },
                    }
                },
                "principalEntityType": "MyCorp::Robot",
            }),
        ),
    ];

    let router = super::harness::router();
    let store_id = store(&router).await;
    let mut created_ids: Vec<(String, String)> = Vec::new();
    for (case, members) in &cases {
        let mut request = members.clone();
        request["policyStoreId"] = json!(store_id);
        let created = ok(&router, "CreateIdentitySource", &request).await;
        created_ids.push((
            (*case).to_string(),
            created["identitySourceId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        ));
    }

    let listed = list(&router, &json!({ "policyStoreId": store_id })).await;
    let items = listed["identitySources"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        items.len(),
        cases.len(),
        "every created identity source must be listed, got {listed}"
    );

    for (case, id) in &created_ids {
        let item = items
            .iter()
            .find(|item| item["identitySourceId"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the created identity source {id} is missing from {listed}")
            });
        let got = ok(
            &router,
            "GetIdentitySource",
            &json!({ "policyStoreId": store_id, "identitySourceId": id }),
        )
        .await;
        assert_eq!(
            item, &got,
            "case {case:?}: a listed item must report what GetIdentitySource reports {got}, got {item}"
        );
        assert_eq!(
            item["policyStoreId"], json!(store_id),
            "case {case:?}: an item must name its store, got {item}"
        );
        assert!(
            !item["createdDate"].is_null()
                && !item["lastUpdatedDate"].is_null()
                && item["principalEntityType"].is_string(),
            "case {case:?}: createdDate, lastUpdatedDate and principalEntityType are required on an IdentitySourceItem, got {item}"
        );
    }

    // `@readonly`: listing twice reports the same thing, and a list changes
    // nothing a later reader sees.
    let again = list(&router, &json!({ "policyStoreId": store_id })).await;
    assert_eq!(
        again, listed,
        "a read must not change what a later caller sees, got {again} after {listed}"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// `maxResults` truncates, `nextToken` resumes exactly where the page stopped,
/// and the last page carries no token — for every page size, over a store whose
/// identity sources were created one at a time.
#[tokio::test]
async fn list_identity_sources_pages_over_every_persisted_source() {
    let router = super::harness::router();
    let store_id = store(&router).await;

    let total = 4usize;
    let mut created_ids: Vec<String> = Vec::new();
    for n in 0..total {
        created_ids.push(create_source(&router, &store_id, &format!("MyCorp::U{n}")).await);
    }
    created_ids.sort();

    // Rows differ along page size, which is what the rule generalises over:
    // one at a time, a page that divides the total, one that does not, exactly
    // the total, and one page size past it.
    for page_size in [1usize, 2, 3, 4, 5] {
        let case = format!("page_size={page_size}");
        let items = walk(
            &router,
            &json!({ "policyStoreId": store_id }),
            page_size,
            total + 1,
            &case,
        )
        .await;
        let mut walked: Vec<String> = items
            .iter()
            .map(|item| {
                item["identitySourceId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        walked.sort();
        assert_eq!(
            walked, created_ids,
            "case {case:?}: walking every page must yield each identity source exactly once, got {walked:?}"
        );
    }

    // An empty store is an empty list and no cursor, not a refusal.
    let empty_store = store(&router).await;
    let empty = list(&router, &json!({ "policyStoreId": empty_store })).await;
    assert_eq!(
        empty["identitySources"],
        json!([]),
        "case \"store with no identity sources\": identitySources is required and empty, got {empty}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "case \"store with no identity sources\": the last page carries no nextToken, got {empty}"
    );
}

/// `ListIdentitySources` is `@paginated`, so an absent `maxResults` takes the
/// default page size its own documentation states ("the operation defaults to 10
/// identity sources per response") and an over-large one is clamped to the
/// documented cap ("a maximum of 50 identity sources per response"): one small
/// request can never materialise an unbounded response.
#[tokio::test]
async fn list_identity_sources_bounds_the_page_it_builds() {
    /// `DEFAULT_PAGE_SIZE` in `src/state.rs`, the documented default of 10.
    const DEFAULT_PAGE_SIZE: usize = 10;
    /// `MAX_PAGE_SIZE` in `src/state.rs`, the documented maximum of 50.
    const MAX_PAGE_SIZE: usize = 50;

    let router = super::harness::router();
    let store_id = store(&router).await;
    let total = MAX_PAGE_SIZE + 5;
    for n in 0..total {
        create_source(&router, &store_id, &format!("MyCorp::U{n}")).await;
    }

    // Rows differ along what the caller asked for: nothing, below the default,
    // between the default and the cap, exactly the cap, and past it.
    let cases: &[(&str, Value, usize, bool)] = &[
        ("no maxResults", Value::Null, DEFAULT_PAGE_SIZE, true),
        ("maxResults below the default", json!(7), 7, true),
        (
            "maxResults between the default and the cap",
            json!(23),
            23,
            true,
        ),
        (
            "maxResults at the cap",
            json!(MAX_PAGE_SIZE),
            MAX_PAGE_SIZE,
            true,
        ),
        (
            "maxResults above the cap",
            json!(2_000),
            MAX_PAGE_SIZE,
            true,
        ),
    ];
    for (case, max_results, expected_len, expect_token) in cases {
        let mut request = json!({ "policyStoreId": store_id });
        if !max_results.is_null() {
            request["maxResults"] = max_results.clone();
        }
        let page = list(&router, &request).await;
        assert_eq!(
            page["identitySources"].as_array().map(Vec::len),
            Some(*expected_len),
            "case {case:?}: unexpected page length, got {}",
            page["identitySources"].as_array().map_or(0, Vec::len)
        );
        assert_eq!(
            page["nextToken"].is_string(),
            *expect_token,
            "case {case:?}: nextToken presence should be {expect_token}, got {page}"
        );
    }

    // A page is never padded: a store holding fewer sources than the page size
    // reports what it holds, and that page is the last one.
    let small_store = store(&router).await;
    for n in 0..3 {
        create_source(&router, &small_store, &format!("MyCorp::S{n}")).await;
    }
    let page = list(
        &router,
        &json!({ "policyStoreId": small_store, "maxResults": 2_000 }),
    )
    .await;
    assert_eq!(
        page["identitySources"].as_array().map(Vec::len),
        Some(3),
        "case \"a store smaller than the page size\": expected the three it holds, got {page}"
    );
    assert!(
        page["nextToken"].is_null(),
        "case \"a store smaller than the page size\": the last page carries no nextToken, got {page}"
    );
}

// ---------------------------------------------------------------------------
// filters
// ---------------------------------------------------------------------------

/// `filters` is a LIST of characteristics, each a structure of optional members:
/// a source is reported when it matches ANY one filter, and it matches a filter
/// when every member that filter sets agrees with the source. An absent list, an
/// empty list and a filter that sets no member all constrain nothing.
#[tokio::test]
async fn list_identity_sources_selects_the_sources_its_filters_name() {
    let router = super::harness::router();
    let store_id = store(&router).await;

    let entity_types = ["MyCorp::User", "MyCorp::Admin", "MyCorp::Robot"];
    let mut by_entity_type: Vec<(String, String)> = Vec::new();
    for entity_type in entity_types {
        let id = create_source(&router, &store_id, entity_type).await;
        by_entity_type.push((entity_type.to_string(), id));
    }

    // Rows differ along the number of filters, the number of members each sets,
    // and how many sources they select.
    let cases: &[(&str, Value, &[&str])] = &[
        ("no filters", Value::Null, &entity_types),
        ("an empty filter list", json!([]), &entity_types),
        ("a filter that sets no member", json!([{}]), &entity_types),
        (
            "one filter naming one principalEntityType",
            json!([{ "principalEntityType": "MyCorp::User" }]),
            &["MyCorp::User"],
        ),
        (
            "one filter naming a principalEntityType no source carries",
            json!([{ "principalEntityType": "MyCorp::Nobody" }]),
            &[],
        ),
        (
            "two filters naming two principalEntityTypes",
            json!([
                { "principalEntityType": "MyCorp::User" },
                { "principalEntityType": "MyCorp::Robot" },
            ]),
            &["MyCorp::User", "MyCorp::Robot"],
        ),
        (
            "a matching and a non-matching filter",
            json!([
                { "principalEntityType": "MyCorp::Admin" },
                { "principalEntityType": "MyCorp::Nobody" },
            ]),
            &["MyCorp::Admin"],
        ),
    ];

    for (case, filters, expected) in cases {
        let mut request = json!({ "policyStoreId": store_id });
        if !filters.is_null() {
            request["filters"] = filters.clone();
        }
        let page = list(&router, &request).await;
        let mut selected: Vec<String> = page["identitySources"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| {
                item["principalEntityType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        selected.sort();
        let mut wanted: Vec<String> = expected.iter().map(|t| (*t).to_string()).collect();
        wanted.sort();
        assert_eq!(
            selected, wanted,
            "case {case:?}: expected the sources carrying {wanted:?}, got {selected:?} from {page}"
        );

        // Paging a filtered list yields the same selection: the filter is
        // applied before the page is cut, not to the page.
        let walked = walk(&router, &request, 1, entity_types.len() + 1, case).await;
        let mut paged: Vec<String> = walked
            .iter()
            .map(|item| {
                item["principalEntityType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        paged.sort();
        assert_eq!(
            paged, wanted,
            "case {case:?}: walking one page at a time must select the same sources, got {paged:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What it refuses, and which store references it resolves
// ---------------------------------------------------------------------------

/// A `policyStoreId` that names no live store is the model's 404, and a
/// `nextToken` this service did not mint is a 400 naming the member — not an
/// empty page, which would hide the client's mistake.
#[tokio::test]
async fn list_identity_sources_refuses_an_unknown_store_and_a_forged_cursor() {
    let router = super::harness::router();
    let store_id = store(&router).await;
    create_source(&router, &store_id, "MyCorp::User").await;

    // Rows differ along WHICH reference is unknown: a store id, and an alias
    // name — the two forms a `policyStoreId` member may carry.
    let unknown: &[(&str, &str, &str)] = &[
        ("unknown store id", "PSnotastore", "POLICY_STORE"),
        (
            "unknown alias name",
            "policy-store-alias/nope",
            "POLICY_STORE_ALIAS",
        ),
    ];
    for (case, reference, resource_type) in unknown {
        let (status, body) = call(
            &router,
            "ListIdentitySources",
            &json!({ "policyStoreId": reference }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(reference.trim_start_matches("policy-store-alias/")),
            "case {case:?}: the exception must name the missing resource, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some(*resource_type),
            "case {case:?}: unexpected resourceType, got {body}"
        );
    }

    // Rows differ along HOW the cursor is wrong: not this service's encoding at
    // all, an odd number of hex digits, and hex that decodes to bytes which are
    // not text.
    for (case, token) in [
        ("not a cursor", "notacursor"),
        ("odd-length hex", "953"),
        ("hex that is not text", "ff"),
    ] {
        let (status, body) = call(
            &router,
            "ListIdentitySources",
            &json!({ "policyStoreId": store_id, "nextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            body["fieldList"][0]["path"].as_str(),
            Some("/nextToken"),
            "case {case:?}: fieldList must name the offending member, got {body}"
        );
    }
}

/// A `policyStoreId` may name its store by ID or by alias name, so both reach
/// the same store's identity sources.
#[tokio::test]
async fn list_identity_sources_resolves_a_store_by_id_or_by_alias() {
    let router = super::harness::router();
    let store_id = store(&router).await;
    let source_id = create_source(&router, &store_id, "MyCorp::User").await;

    // The prefix every alias name carries, from
    // `CreatePolicyStoreAliasInput$aliasName`'s model documentation: "The alias
    // name must always be prefixed with `policy-store-alias/`."
    let alias_name = "policy-store-alias/the-store";
    ok(
        &router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": alias_name, "policyStoreId": store_id }),
    )
    .await;

    for (case, reference) in [("by id", store_id.as_str()), ("by alias name", alias_name)] {
        let page = list(&router, &json!({ "policyStoreId": reference })).await;
        assert_eq!(
            page["identitySources"][0]["identitySourceId"].as_str(),
            Some(source_id.as_str()),
            "case {case:?}: both references must reach the same store's identity sources, got {page}"
        );
        assert_eq!(
            page["identitySources"][0]["policyStoreId"].as_str(),
            Some(store_id.as_str()),
            "case {case:?}: an item names the store's ID, whichever reference was used, got {page}"
        );
    }
}
