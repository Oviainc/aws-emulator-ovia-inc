// SPDX-License-Identifier: CC0-1.0

//! `ListTrustedTokenIssuers` end to end over the `awsJson1_1` wire — the operation the
//! stub issue reported as an unimplemented scaffold.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! example:
//!
//! 1. the enumeration reports EXACTLY the trusted token issuers stored under the instance
//!    the request names, one item per stored issuer, each carrying the members the model's
//!    `TrustedTokenIssuerMetadata` declares and no others, holding the values
//!    `DescribeTrustedTokenIssuer` reports for the same record, in one stable
//!    `TrustedTokenIssuerArn` order whatever order the creates arrived in — and an instance
//!    that holds none answers an empty list, not a refusal;
//! 2. it TRACKS the store: a create adds exactly its own item, an update changes that
//!    item and leaves every other one, a delete removes it, and an issuer belongs to its
//!    instance, so no listing of one instance shows another instance's issuers and an
//!    instance whose deletion has fallen due holds none;
//! 3. it is a REPORT: repeating it answers identically and changes nothing a later caller
//!    sees;
//! 4. it pages: for every `MaxResults` the model admits, walking the pages yields each
//!    stored issuer exactly once in that same order, no page carries more than the page
//!    size asked for, and the last page carries no `NextToken`;
//! 5. an `InstanceArn` this account holds no instance for is refused
//!    `AccessDeniedException` (403) — the not-found channel this operation's model
//!    declares, the one `CreateTrustedTokenIssuer` answers an unheld `InstanceArn` on —
//!    never an empty list and never the scaffold's HTTP 501;
//! 6. the `NextToken` is this service's own cursor: one it did not issue for THIS
//!    enumeration (including one issued for another instance's issuers or for another
//!    operation) is refused `ValidationException`, and resuming from one it did issue
//!    yields exactly the stored issuers that sort after the key it carries.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, hold_deletion, router_over, seeded_instance_arn, with_store, TEST_REGION,
    UNKNOWN_INSTANCE_ARN,
};

/// The only value the model's `TrustedTokenIssuerType` enum admits.
///
/// SOURCE: `smithy_query(query="shape",shape="TrustedTokenIssuerType",
/// model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`.
const ISSUER_TYPE: &str = "OIDC_JWT";

/// The only value the model's `JwksRetrievalOption` enum admits.
///
/// SOURCE: `smithy_query(query="shape",shape="OidcJwtConfiguration")`, member
/// `JwksRetrievalOption`.
const JWKS_RETRIEVAL_OPTION: &str = "OPEN_ID_DISCOVERY";

/// Every member the model declares for a `TrustedTokenIssuerMetadata` item, and no other.
///
/// SOURCE: `smithy_query(query="shape",shape="TrustedTokenIssuerMetadata")`.
const MODELLED_ITEM_MEMBERS: [&str; 3] = ["Name", "TrustedTokenIssuerArn", "TrustedTokenIssuerType"];

/// The largest page this enumeration can answer: the maximum the request's `MaxResults`
/// declares (`range(min: 1, max: 100)`). The response's `TrustedTokenIssuerList` carries
/// no length of its own, so nothing narrows it further.
///
/// SOURCE: `smithy_query(query="operation",operation="ListTrustedTokenIssuers")` and
/// `smithy_query(query="shape",shape="TrustedTokenIssuerList")`.
const MAX_ITEMS_PER_PAGE: usize = 100;

/// The id of the SECOND instance the instance-scoping rows are asserted against:
/// `ssoins-` + 16 of `[a-zA-Z0-9-.]`, as the model's `InstanceArn` pattern requires.
const SECOND_INSTANCE_ID: &str = "ssoins-6b5a4938271605f4";

/// Its identity store: `d-` + 10 hex digits, per `InstanceIdentityStoreArn`.
const SECOND_IDENTITY_STORE_ID: &str = "d-6b5a493827";

/// One `CreateTrustedTokenIssuer` request: a row of the issuer table.
#[derive(Debug, Clone)]
struct IssuerRow {
    label: &'static str,
    name: String,
    issuer_url: &'static str,
    claim_attribute_path: &'static str,
    identity_store_attribute_path: &'static str,
    tags: Vec<(&'static str, &'static str)>,
}

impl IssuerRow {
    fn request(&self, instance_arn: &str) -> Value {
        let mut body = json!({
            "InstanceArn": instance_arn,
            "Name": self.name,
            "TrustedTokenIssuerType": ISSUER_TYPE,
            "TrustedTokenIssuerConfiguration": {
                "OidcJwtConfiguration": {
                    "IssuerUrl": self.issuer_url,
                    "ClaimAttributePath": self.claim_attribute_path,
                    "IdentityStoreAttributePath": self.identity_store_attribute_path,
                    "JwksRetrievalOption": JWKS_RETRIEVAL_OPTION,
                }
            },
        });
        if !self.tags.is_empty() {
            body["Tags"] = Value::Array(
                self.tags
                    .iter()
                    .map(|(key, value)| json!({"Key": key, "Value": value}))
                    .collect(),
            );
        }
        body
    }
}

/// The rows differ along what rule 1 generalises over: the length and character class of
/// `Name` (`len 1..255`, `^[\w+=,.@-]+$`), the scheme, port and punctuation of `IssuerUrl`,
/// the segment count and separator of the two attribute paths, and the number of `Tags`.
///
/// All constraints are from
/// `smithy_query(query="operation",operation="CreateTrustedTokenIssuer")` and
/// `smithy_query(query="shape",shape="OidcJwtConfiguration")`.
fn issuer_rows() -> Vec<IssuerRow> {
    vec![
        IssuerRow {
            label: "the shortest Name the model admits, no tags",
            name: "a".to_string(),
            issuer_url: "https://example.com/oidc",
            claim_attribute_path: "email",
            identity_store_attribute_path: "email",
            tags: Vec::new(),
        },
        IssuerRow {
            label: "Name at the 255-character limit, three tags",
            name: format!("Tti_0+=,.@-{}", "x".repeat(244)),
            issuer_url: "https://login.example.com/.well-known/openid-configuration",
            claim_attribute_path: "user_email_address",
            identity_store_attribute_path: "emails.primary.value",
            tags: vec![("owner", "platform"), ("Env", "staging"), ("cost-centre", "42")],
        },
        IssuerRow {
            label: "an http IssuerUrl with a port and the punctuation the pattern allows",
            name: "http.issuer-1".to_string(),
            issuer_url: "http://idp.example.com:8443/tenant,one;realm!two~three/jwks",
            claim_attribute_path: "sub",
            identity_store_attribute_path: "userName",
            tags: vec![("single", "tag")],
        },
        IssuerRow {
            label: "a Name spanning the whole allowed character class",
            name: "Issuer+=,.@-_09".to_string(),
            issuer_url: "https://issuer.example.org/%7Euser/openid",
            claim_attribute_path: "claims.email_address",
            identity_store_attribute_path: "addresses.formatted",
            tags: vec![("team", "identity")],
        },
    ]
}

/// A router over an account that holds the ambient instance plus a second one held under
/// an id of its own, and both ARNs.
///
/// An organization instance is enabled through AWS Organizations rather than through this
/// API, so the second one is seeded through the SAME `state::Instance` constructor the
/// handlers use — exactly as the harness's own organization fixture does — and every call
/// a test then makes goes over the wire. Two instances in ONE store is what lets "under the
/// instance the request names" be asserted instead of read off the only instance there is.
async fn two_instance_router() -> (MultiServiceRouter, String, String) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let first = seeded_instance_arn(&router).await;
    let second = with_store(&engine, |store| {
        let instance = state::Instance::active(
            SECOND_INSTANCE_ID,
            SECOND_IDENTITY_STORE_ID,
            state::InstanceType::Organization,
            Some("Second".to_string()),
            aws_smithy_types::DateTime::from(SystemTime::now()),
            TEST_REGION,
        );
        let arn = instance.instance_arn.clone();
        store.instances.insert(arn.clone(), instance);
        arn
    });
    assert_ne!(
        first, second,
        "the fixture must hold two instances of different ids, got {first}"
    );
    (router, first, second)
}

/// One `ListTrustedTokenIssuers` call.
async fn list_page(
    router: &MultiServiceRouter,
    instance_arn: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({"InstanceArn": instance_arn});
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    call(router, "ListTrustedTokenIssuers", &request).await
}

/// The items one answer carries, in the order it listed them.
fn listed_items(case: &str, body: &Value) -> Vec<Value> {
    body["TrustedTokenIssuers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the answer carries no TrustedTokenIssuers: {body}"))
        .clone()
}

/// The `TrustedTokenIssuerArn` of each listed item, in the order they were listed.
fn listed_arns(case: &str, items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            item["TrustedTokenIssuerArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an item carries no ARN: {item}"))
                .to_string()
        })
        .collect()
}

/// Every page of one enumeration, walked the way a client walks it: the items in the order
/// they were listed.
///
/// Asserts RULE 4's page-size and termination invariants on the way: no page carries more
/// than the page size asked for, and only the last page comes back without a `NextToken`.
async fn walk(
    router: &MultiServiceRouter,
    instance_arn: &str,
    max_results: Option<i64>,
    case: &str,
) -> Vec<Value> {
    let limit =
        max_results.map_or(MAX_ITEMS_PER_PAGE, |value| (value as usize).min(MAX_ITEMS_PER_PAGE));
    let mut seen: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let (status, body) = list_page(router, instance_arn, max_results, next_token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a page of the enumeration must answer 200, got {status} {body}"
        );
        let items = listed_items(case, &body);
        assert!(
            items.len() <= limit,
            "case {case:?}: a page must carry at most {limit} items, got {}",
            items.len()
        );
        seen.extend(items.clone());
        pages += 1;
        assert!(
            pages <= state::MAX_RECORDS_PER_STORE,
            "case {case:?}: the enumeration did not terminate after {pages} pages"
        );
        match body["NextToken"].as_str() {
            Some(token) => {
                assert_eq!(
                    items.len(),
                    limit,
                    "case {case:?}: a page that carries a NextToken must be full, got {} items",
                    items.len()
                );
                next_token = Some(token.to_string());
            }
            None => break,
        }
    }
    seen
}

/// `DescribeTrustedTokenIssuer` for `arn`, as a client reads it.
async fn describe(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = call(
        router,
        "DescribeTrustedTokenIssuer",
        &json!({"TrustedTokenIssuerArn": arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeTrustedTokenIssuer of {arn} must answer 200, got {status} {body}"
    );
    body
}

/// Create one issuer from `row` and return its `TrustedTokenIssuerArn`.
async fn create(router: &MultiServiceRouter, instance_arn: &str, row: &IssuerRow) -> String {
    let (status, body) = call(
        router,
        "CreateTrustedTokenIssuer",
        &row.request(instance_arn),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateTrustedTokenIssuer must answer 200, got {status} {body}",
        row.label
    );
    body["TrustedTokenIssuerArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: the create minted no ARN: {body}", row.label))
        .to_string()
}

/// Create a plainly-named issuer and return its `TrustedTokenIssuerArn`.
async fn create_named(router: &MultiServiceRouter, instance_arn: &str, name: &str) -> String {
    create(
        router,
        instance_arn,
        &IssuerRow {
            label: "named",
            name: name.to_string(),
            issuer_url: "https://named.example.com/oidc",
            claim_attribute_path: "email",
            identity_store_attribute_path: "email",
            tags: Vec::new(),
        },
    )
    .await
}

/// RULES 1 and 3 — the enumeration reports exactly the stored issuers, in
/// `TrustedTokenIssuerArn` order, each agreeing member for member with its describe, for
/// every SUBSET of the issuer table and whatever order the creates arrived in; and
/// repeating it changes nothing.
#[tokio::test]
async fn list_reports_exactly_the_issuers_stored_under_the_instance() {
    let rows = issuer_rows();
    // The cases span the whole class the rule generalises over: every subset of the issuer
    // table (so every count from none to all, and every combination of the member shapes),
    // each created in a ROTATED order so the answer's order is the service's and not the
    // order the caller happened to create in.
    for mask in 0..(1u32 << rows.len()) {
        let (router, instance_arn, second) = two_instance_router().await;
        let mut selected: Vec<&IssuerRow> = rows
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, row)| row)
            .collect();
        let rotation = selected.len() / 2;
        selected.rotate_right(rotation);
        let case = format!(
            "subset {mask:#06b}: {:?}",
            selected.iter().map(|row| row.label).collect::<Vec<_>>()
        );

        // One issuer under the OTHER instance, so "under the instance the request names"
        // is asserted rather than read off the only issuers that exist.
        let neighbour = create_named(&router, &second, "neighbour-issuer").await;

        let mut expected: Vec<String> = Vec::new();
        for row in &selected {
            expected.push(create(&router, &instance_arn, row).await);
        }
        expected.sort();

        let items = walk(&router, &instance_arn, None, &case).await;
        assert_eq!(
            listed_arns(&case, &items),
            expected,
            "case {case:?}: the enumeration must report exactly the stored issuers of this \
             instance, in ARN order, got {items:?}"
        );
        assert!(
            !listed_arns(&case, &items).contains(&neighbour),
            "case {case:?}: the other instance's issuer must not be listed, got {items:?}"
        );

        // Every item carries the members the model declares and no others, holding the
        // values the describe of the same record reports.
        for item in &items {
            let arn = item["TrustedTokenIssuerArn"].as_str().unwrap_or_default();
            let mut members: Vec<&str> = item
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: an item is not an object: {item}"))
                .keys()
                .map(String::as_str)
                .collect();
            members.sort_unstable();
            assert_eq!(
                members,
                MODELLED_ITEM_MEMBERS.to_vec(),
                "case {case:?}: an item must carry exactly the modelled members, got {item}"
            );
            let described = describe(&router, arn).await;
            for member in MODELLED_ITEM_MEMBERS {
                assert_eq!(
                    item[member], described[member],
                    "case {case:?}: item member {member} must hold what the describe reports, \
                     got {item} against {described}"
                );
            }
        }

        // RULE 3: the enumeration reported rather than mutated, so a second call answers
        // identically.
        let (status, first_answer) = list_page(&router, &instance_arn, None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the enumeration must answer 200, got {status} {first_answer}"
        );
        let (status, repeated) = list_page(&router, &instance_arn, None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated enumeration must answer 200, got {status} {repeated}"
        );
        assert_eq!(
            repeated, first_answer,
            "case {case:?}: a list must change nothing a later caller sees, got {repeated}"
        );
    }
}

/// RULE 2 — the enumeration tracks the store: an update changes exactly its own item, a
/// delete removes it, an issuer belongs to its instance, and an instance whose deletion has
/// fallen due holds none.
#[tokio::test]
async fn the_list_tracks_the_store_and_its_own_instance() {
    let (router, first, second) = two_instance_router().await;

    let mut under_first: Vec<String> = Vec::new();
    for row in issuer_rows() {
        under_first.push(create(&router, &first, &row).await);
    }
    under_first.sort();
    let under_second = vec![create_named(&router, &second, "second-instance-issuer").await];

    for (instance, expected) in [(&first, &under_first), (&second, &under_second)] {
        let case = format!("instance {instance}");
        let items = walk(&router, instance, None, &case).await;
        assert_eq!(
            &listed_arns(&case, &items),
            expected,
            "case {case:?}: each instance lists exactly its own issuers, got {items:?}"
        );
    }

    // An update of ONE issuer changes that issuer's item and leaves every other one.
    let renamed = under_first[1].clone();
    let (status, body) = call(
        &router,
        "UpdateTrustedTokenIssuer",
        &json!({"TrustedTokenIssuerArn": renamed, "Name": "renamed-by-update"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateTrustedTokenIssuer must answer 200, got {status} {body}"
    );
    let items = walk(&router, &first, None, "after the update").await;
    assert_eq!(
        listed_arns("after the update", &items),
        under_first,
        "an update adds and removes nothing, got {items:?}"
    );
    for item in &items {
        let arn = item["TrustedTokenIssuerArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let described = describe(&router, &arn).await;
        assert_eq!(
            item["Name"], described["Name"],
            "the listed Name must track the store, got {item} against {described}"
        );
        if arn == renamed {
            assert_eq!(
                item["Name"].as_str(),
                Some("renamed-by-update"),
                "the updated issuer's listed Name must be the one the update stored, got {item}"
            );
        }
    }

    // A delete removes exactly its own item.
    let deleted = under_first[0].clone();
    let (status, body) = call(
        &router,
        "DeleteTrustedTokenIssuer",
        &json!({"TrustedTokenIssuerArn": deleted}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteTrustedTokenIssuer must answer 200, got {status} {body}"
    );
    let items = walk(&router, &first, None, "after the delete").await;
    assert_eq!(
        listed_arns("after the delete", &items),
        under_first[1..].to_vec(),
        "a delete removes exactly its own item, got {items:?}"
    );
    let items = walk(&router, &second, None, "the neighbour after the delete").await;
    assert_eq!(
        listed_arns("the neighbour after the delete", &items),
        under_second,
        "the other instance's list must be untouched, got {items:?}"
    );

    // An issuer belongs to its instance: once the removal a `DeleteInstance` owes falls
    // due, the instance is gone and the issuers held inside it with it, so the enumeration
    // is refused rather than answered with an empty page.
    let doomed_engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let doomed_router = router_over(Arc::clone(&doomed_engine));
    let doomed_instance = seeded_instance_arn(&doomed_router).await;
    let doomed_issuer = create_named(&doomed_router, &doomed_instance, "doomed-with-its-instance").await;
    let items = walk(&doomed_router, &doomed_instance, None, "before the delete").await;
    assert_eq!(
        listed_arns("before the delete", &items),
        vec![doomed_issuer],
        "case \"before the delete\": the list must carry the stored issuer, got {items:?}"
    );
    let (status, body) = call(
        &doomed_router,
        "DeleteInstance",
        &json!({"InstanceArn": doomed_instance}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteInstance must answer 200, got {status} {body}"
    );
    hold_deletion(&doomed_engine, SystemTime::now() - Duration::from_secs(60));
    let (status, body) = list_page(&doomed_router, &doomed_instance, None, None).await;
    assert_eq!(
        status, 403,
        "case \"after the deletion fell due\": the list must be refused 403, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "AccessDeniedException",
        "case \"after the deletion fell due\": expected AccessDeniedException, got {body}"
    );
}

/// RULE 4 — the enumeration pages: for every `MaxResults` the model admits, the pages
/// together carry each stored issuer exactly once, in the one stable order.
#[tokio::test]
async fn every_page_size_the_model_admits_walks_the_whole_store_once() {
    let (router, instance_arn, _second) = two_instance_router().await;

    let mut stored: Vec<String> = Vec::new();
    for index in 0..7 {
        stored.push(create_named(&router, &instance_arn, &format!("page-issuer-{index:03}")).await);
    }
    stored.sort();

    // Each published bound of `MaxResults` (`range(min: 1, max: 100)`), sizes either side
    // of the stored count, and the absent member.
    for max_results in [Some(1), Some(2), Some(3), Some(6), Some(7), Some(8), Some(100), None] {
        let case = format!("MaxResults {max_results:?}");
        let items = walk(&router, &instance_arn, max_results, &case).await;
        assert_eq!(
            listed_arns(&case, &items),
            stored,
            "case {case:?}: every page size must walk the whole store once in ARN order, got {} \
             items",
            items.len()
        );
    }

    // A `MaxResults` one past each published bound is refused by the boundary validator.
    for max_results in [0, 101] {
        let case = format!("MaxResults {max_results}");
        let (status, body) = list_page(&router, &instance_arn, Some(max_results), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a MaxResults outside the modelled range must be refused 400, got \
             {status} {body}"
        );
    }
}

/// RULE 5 — an `InstanceArn` this account holds no instance for is refused on the modelled
/// not-found channel, never answered with an empty list and never with the scaffold's
/// HTTP 501.
#[tokio::test]
async fn an_instance_arn_this_account_holds_no_instance_for_is_refused() {
    let (router, instance_arn, _second) = two_instance_router().await;
    // One stored issuer exists, so the refusal is about the instance asked for and not
    // about an empty store.
    let stored = create_named(&router, &instance_arn, "stored-issuer").await;
    let held_id = instance_arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();

    // The rows span the parts of the `InstanceArn` pattern that identify the instance: both
    // `(sso)?ins-` spellings, the id character class and the partition suffixes
    // `(-[a-z]{1,5}){0,3}` admits — the held instance's id under another partition names a
    // DIFFERENT instance.
    let cases: Vec<(&str, String)> = vec![
        (
            "a pattern-valid ssoins- ARN that was never created",
            UNKNOWN_INSTANCE_ARN.to_string(),
        ),
        (
            "a pattern-valid ins- ARN that was never created",
            "arn:aws:sso:::instance/ins-a1b2c3d4e5f60718".to_string(),
        ),
        (
            "an id using the full [a-zA-Z0-9-.] class",
            "arn:aws:sso:::instance/ssoins-A.b-C.d-12345678".to_string(),
        ),
        (
            "the held instance's id in another partition",
            format!("arn:aws-us-gov:sso:::instance/{held_id}"),
        ),
    ];

    for (case, unheld) in cases {
        let (status, body) = list_page(&router, &unheld, None, None).await;
        assert_eq!(
            status, 403,
            "case {case:?}: an unheld InstanceArn must be refused 403, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {body}"
        );
        assert!(
            body["TrustedTokenIssuers"].is_null(),
            "case {case:?}: a refusal lists nothing, got {body}"
        );
    }

    // The refusals changed nothing: the held instance still lists its issuer.
    let items = walk(&router, &instance_arn, None, "after the refusals").await;
    assert_eq!(
        listed_arns("after the refusals", &items),
        vec![stored],
        "a refused list must change nothing, got {items:?}"
    );
}

/// RULE 6 — the `NextToken` is this service's own cursor: a token it did not issue for this
/// enumeration is refused, and its own cursor resumes after the key it carries.
#[tokio::test]
async fn list_refuses_a_next_token_it_did_not_issue_and_resumes_from_its_own() {
    let (router, instance_arn, second) = two_instance_router().await;
    let mut stored: Vec<String> = Vec::new();
    for index in 0..4 {
        stored.push(create_named(&router, &instance_arn, &format!("cursor-issuer-{index}")).await);
    }
    stored.sort();
    for index in 0..2 {
        create_named(&router, &second, &format!("cursor-other-{index}")).await;
    }

    // A token issued for the SECOND instance's enumeration of the same operation: a
    // cursor of another scope, read over the wire.
    let (status, body) = list_page(&router, &second, Some(1), None).await;
    assert_eq!(
        status, 200,
        "the neighbour's first page must answer 200, got {status} {body}"
    );
    let foreign_token = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the neighbour's truncated page must carry a NextToken: {body}"))
        .to_string();

    // The rows span the ways a token can fail to be this enumeration's cursor: not a token
    // of this service at all, a token of another enumeration of the same operation, and a
    // token of another operation's enumeration over the same instance.
    let cases: Vec<(&str, String)> = vec![
        ("a token this service never issued", "not-a-token".to_string()),
        (
            "a token of the same operation under another instance",
            foreign_token,
        ),
        (
            "a token of another operation's enumeration",
            state::encode_page_token(&state::cursor_permission_sets(&instance_arn), &stored[0]),
        ),
        (
            "a cursor of this scope with its version marker changed",
            state::encode_page_token(
                &format!("{}x", state::cursor_trusted_token_issuers(&instance_arn)),
                &stored[0],
            ),
        ),
    ];
    for (case, token) in cases {
        let (status, body) = list_page(&router, &instance_arn, None, Some(&token)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a NextToken this enumeration did not issue must be refused 400, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // Its own cursor resumes exactly after the key it carries, for every prefix length.
    for taken in 1..stored.len() {
        let case = format!("resuming after {taken} items");
        let (status, body) = list_page(&router, &instance_arn, Some(taken as i64), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first page must answer 200, got {status} {body}"
        );
        let first_page = listed_arns(&case, &listed_items(&case, &body));
        assert_eq!(
            first_page,
            stored[..taken].to_vec(),
            "case {case:?}: the first page must carry the first {taken} stored issuers, got \
             {first_page:?}"
        );
        let token = body["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a truncated page must carry a token: {body}"))
            .to_string();
        let (status, body) = list_page(&router, &instance_arn, None, Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: resuming must answer 200, got {status} {body}"
        );
        let resumed = listed_arns(&case, &listed_items(&case, &body));
        assert_eq!(
            resumed,
            stored[taken..].to_vec(),
            "case {case:?}: resuming must yield exactly the issuers after the cursor's key, got \
             {resumed:?}"
        );
    }
}
