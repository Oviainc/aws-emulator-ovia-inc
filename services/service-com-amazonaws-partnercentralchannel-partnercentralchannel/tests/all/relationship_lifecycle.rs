// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the program-management-account -> relationship lifecycle.
//!
//! Every case drives the service the way a client does: an awsJson1_0 `POST /` with
//! `X-Amz-Target: PartnerCentralChannel.<Operation>`, dispatched through the same
//! [`MultiServiceRouter`] the emulator binary mounts. Nothing reaches into the store
//! directly, so a response that is not computed from persisted state fails here.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralchannel_partnercentralchannel::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A router with one freshly-engined copy of this service mounted.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Deterministic regardless of the process-wide default: these cases are about
    // the service's behaviour, not about SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation signed for `region`, and return `(http status, parsed body)`.
async fn call_in_region(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    region: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://partnercentral-channel.{region}.amazonaws.com/"
        ))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("PartnerCentralChannel.{operation}"))
        // The router keys on the SigV4 credential scope's service field
        // (`partnercentral-channel`, this service's signing name) and the service
        // reads the region segment out of the same scope. The signature itself is
        // not checked, because authentication is off above.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLEEXAMPLE/20260101/{region}/\
                 partnercentral-channel/aws4_request, SignedHeaders=host;x-amz-target, \
                 Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The region every case signs for unless it is specifically about the region.
const DEFAULT_REGION: &str = "us-east-1";

async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, operation, body, DEFAULT_REGION).await
}

/// Call one operation, requiring success.
async fn call_ok(router: &MultiServiceRouter, operation: &str, body: &Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to succeed with HTTP 200, got {status} and {parsed}"
    );
    parsed
}

/// The awsJson error code an error body carries in `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Assert a call was refused with `expected_code` and `expected_status`.
fn assert_refused(
    (status, body): (u16, Value),
    expected_status: u16,
    expected_code: &str,
    case: &str,
) {
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} and {body}"
    );
    assert_eq!(
        error_code(&body),
        expected_code,
        "case {case:?}: expected error code {expected_code}, got {body}"
    );
}

fn member<'a>(body: &'a Value, name: &str) -> &'a Value {
    body.get(name).unwrap_or(&Value::Null)
}

fn string_member(body: &Value, name: &str) -> String {
    member(body, name).as_str().unwrap_or_default().to_string()
}

/// Is `value` an id of `prefix`'s kind? `ProgramManagementAccountId` is modelled
/// `^pma-[a-z0-9]{13}$` and `RelationshipId` `^rs-[a-z0-9]{13}$`, so both are the
/// prefix plus exactly 13 lowercase-alphanumeric characters.
fn is_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 13
            && suffix
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// The ARN a program management account must carry, from the model's
/// `ProgramManagementAccountIdentifier` pattern
/// `^(arn:[a-z-]+:partnercentral:[a-z0-9-]+:[0-9]{12}:catalog/[a-zA-Z]+/program-management-account/)?pma-[a-z0-9]{13}$`.
/// The account segment is the emulator's canonical placeholder account.
fn expected_pma_arn(region: &str, catalog: &str, id: &str) -> String {
    format!(
        "arn:aws:partnercentral:{region}:{}:catalog/{catalog}/program-management-account/{id}",
        engine::emulator_account_id()
    )
}

// ---------------------------------------------------------------------------
// Creating the parent and its children, then reading them back
// ---------------------------------------------------------------------------

/// Which form of the parent identifier a case sends: both a bare `pma-…` id and
/// the full ARN are modelled, and both must resolve to the same record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParentIdentifier {
    BareId,
    Arn,
}

/// Which form of the relationship identifier a case sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChildIdentifier {
    BareId,
    Arn,
}

#[derive(Debug, Clone)]
struct LifecycleCase {
    case: &'static str,
    catalog: &'static str,
    program: &'static str,
    pma_display_name: &'static str,
    pma_account_id: &'static str,
    association_type: &'static str,
    sector: &'static str,
    resale_account_model: Option<&'static str>,
    associated_account_id: &'static str,
    display_name: &'static str,
    support_plan: Option<Value>,
    parent_identifier: ParentIdentifier,
    child_identifier: ChildIdentifier,
}

/// A `displayName` at the model's upper length bound (`@length(min: 1, max: 30)`).
const DISPLAY_NAME_30: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123";

/// The table every create/read case walks. Its rows differ along what the rule
/// generalises over: the catalog, the program, the association type, the sector,
/// the resale account model, the support-plan union member, the display-name
/// length, and which FORM of each identifier the read uses. Every enum value here
/// is the model's own (`smithy_query(query="shape",shape="Program")` and the
/// `ASSOCIATION_TYPE` / `SECTOR` / `RESALE_ACCOUNT_MODEL` / `COVERAGE` /
/// `PROVIDER` value sets).
fn lifecycle_cases() -> Vec<LifecycleCase> {
    vec![
        LifecycleCase {
            case: "solution-provider/end-customer/commercial",
            catalog: "AWS",
            program: "SOLUTION_PROVIDER",
            pma_display_name: "Acme PMA",
            pma_account_id: "111111111111",
            association_type: "END_CUSTOMER",
            sector: "COMMERCIAL",
            resale_account_model: Some("END_CUSTOMER"),
            associated_account_id: "210987654321",
            display_name: "Acme Customer",
            support_plan: None,
            parent_identifier: ParentIdentifier::BareId,
            child_identifier: ChildIdentifier::BareId,
        },
        LifecycleCase {
            case: "distribution/downstream-seller/government",
            catalog: "AWS",
            program: "DISTRIBUTION",
            pma_display_name: DISPLAY_NAME_30,
            pma_account_id: "222222222222",
            association_type: "DOWNSTREAM_SELLER",
            sector: "GOVERNMENT",
            resale_account_model: Some("DISTRIBUTOR"),
            associated_account_id: "000000000001",
            display_name: DISPLAY_NAME_30,
            support_plan: Some(json!({
                "resoldEnterprise": {
                    "coverage": "ENTIRE_ORGANIZATION",
                    "tamLocation": "us-east-1",
                    "chargeAccountId": "333333333333"
                }
            })),
            parent_identifier: ParentIdentifier::Arn,
            child_identifier: ChildIdentifier::Arn,
        },
        LifecycleCase {
            case: "distribution-seller/internal/government-exception",
            catalog: "Sandbox",
            program: "DISTRIBUTION_SELLER",
            pma_display_name: "X",
            pma_account_id: "333333333333",
            association_type: "INTERNAL",
            sector: "GOVERNMENT_EXCEPTION",
            resale_account_model: None,
            associated_account_id: "444444444444",
            display_name: "X",
            support_plan: Some(json!({
                "partnerLedSupport": {
                    "coverage": "MANAGEMENT_ACCOUNT_ONLY",
                    "provider": "DISTRIBUTOR",
                    "tamLocation": "eu-west-1"
                }
            })),
            parent_identifier: ParentIdentifier::BareId,
            child_identifier: ChildIdentifier::Arn,
        },
        LifecycleCase {
            case: "solution-provider/end-customer/commercial-in-sandbox",
            catalog: "Sandbox",
            program: "SOLUTION_PROVIDER",
            pma_display_name: "Sandbox PMA",
            pma_account_id: "555555555555",
            association_type: "END_CUSTOMER",
            sector: "COMMERCIAL",
            resale_account_model: Some("SOLUTION_PROVIDER"),
            associated_account_id: "666666666666",
            display_name: "Sandbox Customer",
            support_plan: Some(json!({
                "resoldUnifiedOperations": {
                    "coverage": "ENTIRE_ORGANIZATION",
                    "tamLocation": "ap-southeast-2"
                }
            })),
            parent_identifier: ParentIdentifier::Arn,
            child_identifier: ChildIdentifier::BareId,
        },
    ]
}

/// Create the case's program management account and return `(id, arn)`.
async fn create_parent(router: &MultiServiceRouter, case: &LifecycleCase) -> (String, String) {
    let created = call_ok(
        router,
        "CreateProgramManagementAccount",
        &json!({
            "catalog": case.catalog,
            "program": case.program,
            "displayName": case.pma_display_name,
            "accountId": case.pma_account_id,
        }),
        case.case,
    )
    .await;
    let detail = member(&created, "programManagementAccountDetail").clone();
    let id = string_member(&detail, "id");
    let arn = string_member(&detail, "arn");
    assert!(
        is_id(&id, "pma-"),
        "case {:?}: expected a ^pma-[a-z0-9]{{13}}$ id, got {id:?}",
        case.case
    );
    assert_eq!(
        arn,
        expected_pma_arn(DEFAULT_REGION, case.catalog, &id),
        "case {:?}: expected the ARN to be built from the stored id and catalog, got {arn:?}",
        case.case
    );
    (id, arn)
}

/// Create the case's relationship under `parent` and return `(id, arn)`.
async fn create_child(
    router: &MultiServiceRouter,
    case: &LifecycleCase,
    parent: &(String, String),
) -> (String, String) {
    let parent_identifier = match case.parent_identifier {
        ParentIdentifier::BareId => parent.0.clone(),
        ParentIdentifier::Arn => parent.1.clone(),
    };
    let mut request = json!({
        "catalog": case.catalog,
        "associationType": case.association_type,
        "programManagementAccountIdentifier": parent_identifier,
        "associatedAccountId": case.associated_account_id,
        "displayName": case.display_name,
        "sector": case.sector,
    });
    if let Some(model) = case.resale_account_model {
        request["resaleAccountModel"] = json!(model);
    }
    if let Some(plan) = case.support_plan.clone() {
        request["requestedSupportPlan"] = plan;
    }
    let created = call_ok(router, "CreateRelationship", &request, case.case).await;
    let detail = member(&created, "relationshipDetail").clone();
    let id = string_member(&detail, "id");
    let arn = string_member(&detail, "arn");
    assert!(
        is_id(&id, "rs-"),
        "case {:?}: expected a ^rs-[a-z0-9]{{13}}$ id, got {id:?}",
        case.case
    );
    assert_eq!(
        arn,
        format!("{}/relationship/{id}", parent.1),
        "case {:?}: expected the relationship ARN to hang off its parent's, got {arn:?}",
        case.case
    );
    (id, arn)
}

#[tokio::test]
async fn a_created_relationship_reads_back_with_the_values_it_was_created_with() {
    let router = emulator();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    for case in lifecycle_cases() {
        let parent = create_parent(&router, &case).await;
        let child = create_child(&router, &case, &parent).await;
        assert!(
            seen_ids.insert(child.0.clone()),
            "case {:?}: every minted relationship id must be unique, {} repeated",
            case.case,
            child.0
        );

        let identifier = match case.child_identifier {
            ChildIdentifier::BareId => child.0.clone(),
            ChildIdentifier::Arn => child.1.clone(),
        };
        let parent_identifier = match case.parent_identifier {
            ParentIdentifier::BareId => parent.0.clone(),
            ParentIdentifier::Arn => parent.1.clone(),
        };
        let fetched = call_ok(
            &router,
            "GetRelationship",
            &json!({
                "catalog": case.catalog,
                "programManagementAccountIdentifier": parent_identifier,
                "identifier": identifier,
            }),
            case.case,
        )
        .await;
        let detail = member(&fetched, "relationshipDetail").clone();

        for (field, expected) in [
            ("arn", child.1.clone()),
            ("id", child.0.clone()),
            ("revision", "1".to_string()),
            ("catalog", case.catalog.to_string()),
            ("associationType", case.association_type.to_string()),
            ("programManagementAccountId", parent.0.clone()),
            (
                "associatedAccountId",
                case.associated_account_id.to_string(),
            ),
            ("displayName", case.display_name.to_string()),
            ("sector", case.sector.to_string()),
        ] {
            let actual = string_member(&detail, field);
            assert_eq!(
                actual, expected,
                "case {:?}: expected {field} to be {expected:?}, got {actual:?}",
                case.case
            );
        }

        let resale = member(&detail, "resaleAccountModel").clone();
        match case.resale_account_model {
            Some(model) => assert_eq!(
                resale.as_str(),
                Some(model),
                "case {:?}: expected resaleAccountModel {model:?}, got {resale:?}",
                case.case
            ),
            None => assert!(
                resale.is_null(),
                "case {:?}: expected no resaleAccountModel, got {resale:?}",
                case.case
            ),
        }

        // The wire form of a `DateTime` is the protocol's, so these are compared
        // as whatever the serializer emitted rather than as strings.
        let created_at = member(&detail, "createdAt").clone();
        let updated_at = member(&detail, "updatedAt").clone();
        let start_date = member(&detail, "startDate").clone();
        assert!(
            !created_at.is_null(),
            "case {:?}: expected a createdAt timestamp, got {created_at}",
            case.case
        );
        assert_eq!(
            start_date, created_at,
            "case {:?}: a relationship starts when it is created, so startDate must equal \
             createdAt; got {start_date} and {created_at}",
            case.case
        );
        assert_eq!(
            updated_at, created_at,
            "case {:?}: an unmodified relationship's updatedAt must equal its createdAt, got \
             {updated_at} and {created_at}",
            case.case
        );

        // A read must not change what a later caller sees.
        let again = call_ok(
            &router,
            "GetRelationship",
            &json!({
                "catalog": case.catalog,
                "programManagementAccountIdentifier": parent.0.clone(),
                "identifier": child.0.clone(),
            }),
            case.case,
        )
        .await;
        assert_eq!(
            member(&again, "relationshipDetail"),
            &detail,
            "case {:?}: two reads of the same relationship must report the same record",
            case.case
        );
    }
}

#[tokio::test]
async fn an_arn_carries_the_region_the_request_was_signed_for() {
    // The modelled ARN patterns allow any `[a-z0-9-]+` region, and the region a
    // resource's ARN carries is the one its create was signed for.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-1"] {
        let router = emulator();
        let created = call_in_region(
            &router,
            "CreateProgramManagementAccount",
            &json!({
                "catalog": "AWS",
                "program": "SOLUTION_PROVIDER",
                "displayName": "Acme PMA",
                "accountId": "123456789012",
            }),
            region,
        )
        .await;
        assert_eq!(
            created.0, 200,
            "case {region:?}: expected HTTP 200, got {} and {}",
            created.0, created.1
        );
        let detail = member(&created.1, "programManagementAccountDetail").clone();
        let id = string_member(&detail, "id");
        let arn = string_member(&detail, "arn");
        assert_eq!(
            arn,
            expected_pma_arn(region, "AWS", &id),
            "case {region:?}: expected the ARN to carry the signed region, got {arn:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// Create `count` relationships under one parent and return their ids in
/// creation order.
async fn seed_relationships(
    router: &MultiServiceRouter,
    catalog: &str,
    parent_id: &str,
    names: &[&str],
) -> Vec<String> {
    let mut ids = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let created = call_ok(
            router,
            "CreateRelationship",
            &json!({
                "catalog": catalog,
                "associationType": "END_CUSTOMER",
                "programManagementAccountIdentifier": parent_id,
                "associatedAccountId": format!("{:012}", 700000000000u64 + index as u64),
                "displayName": name,
                "sector": "COMMERCIAL",
            }),
            name,
        )
        .await;
        ids.push(string_member(
            member(&created, "relationshipDetail"),
            "id",
        ));
    }
    ids
}

/// Walk every page of `ListRelationships` with `max_results` and return the ids in
/// the order they came back.
async fn page_through(
    router: &MultiServiceRouter,
    catalog: &str,
    max_results: i64,
    case: &str,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut next_token: Option<String> = None;
    // Bounded so a token that never advances fails the assertion below instead of
    // looping forever.
    for _ in 0..64 {
        let mut request = json!({"catalog": catalog, "maxResults": max_results});
        if let Some(token) = next_token.clone() {
            request["nextToken"] = json!(token);
        }
        let page = call_ok(router, "ListRelationships", &request, case).await;
        let items = member(&page, "items").as_array().cloned().unwrap_or_default();
        assert!(
            items.len() as i64 <= max_results,
            "case {case:?}: a page must not exceed maxResults {max_results}, got {}",
            items.len()
        );
        for item in &items {
            ids.push(string_member(item, "id"));
        }
        match member(&page, "nextToken").as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => return ids,
        }
    }
    panic!("case {case:?}: paging did not terminate");
}

#[tokio::test]
async fn list_relationships_pages_through_every_record_exactly_once() {
    let router = emulator();
    let case = LifecycleCase {
        case: "pagination",
        ..lifecycle_cases()[0].clone()
    };
    let parent = create_parent(&router, &case).await;
    let names = ["one", "two", "three", "four", "five"];
    let created = seed_relationships(&router, case.catalog, &parent.0, &names).await;

    // Every page size spans the class: smaller than, equal to and larger than the
    // number of records, plus the modelled default (20) and bound (100).
    for max_results in [1, 2, 3, 5, 20, 100] {
        let paged = page_through(
            &router,
            case.catalog,
            max_results,
            &format!("maxResults={max_results}"),
        )
        .await;
        assert_eq!(
            paged, created,
            "case maxResults={max_results}: expected paging to yield every record once in \
             creation order, got {paged:?} against {created:?}"
        );
    }

    // The default page size the model declares is applied when the request sends
    // none.
    let defaulted = call_ok(
        &router,
        "ListRelationships",
        &json!({"catalog": case.catalog}),
        "no-maxResults",
    )
    .await;
    let ids: Vec<String> = member(&defaulted, "items")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| string_member(item, "id"))
        .collect();
    assert_eq!(
        ids, created,
        "case \"no-maxResults\": expected the modelled default page size to hold all {} \
         records, got {ids:?}",
        created.len()
    );
    assert!(
        member(&defaulted, "nextToken").is_null(),
        "case \"no-maxResults\": the last page must carry no nextToken, got {}",
        member(&defaulted, "nextToken")
    );
}

#[tokio::test]
async fn list_relationships_honours_every_equality_filter() {
    let router = emulator();
    // Two parents in one catalog and one in another, so the catalog scope and the
    // parent filter are both exercised against records that would otherwise match.
    let first_parent = create_parent(
        &router,
        &LifecycleCase {
            case: "filters/first-parent",
            ..lifecycle_cases()[0].clone()
        },
    )
    .await;
    let second_parent = create_parent(
        &router,
        &LifecycleCase {
            case: "filters/second-parent",
            pma_account_id: "999999999999",
            ..lifecycle_cases()[0].clone()
        },
    )
    .await;
    let other_catalog_parent = create_parent(
        &router,
        &LifecycleCase {
            case: "filters/other-catalog",
            ..lifecycle_cases()[3].clone()
        },
    )
    .await;

    // (parent, catalog, associationType, sector, associatedAccountId, displayName)
    let rows = [
        (
            &first_parent,
            "AWS",
            "END_CUSTOMER",
            "COMMERCIAL",
            "100000000001",
            "alpha",
        ),
        (
            &first_parent,
            "AWS",
            "DOWNSTREAM_SELLER",
            "GOVERNMENT",
            "100000000002",
            "beta",
        ),
        (
            &second_parent,
            "AWS",
            "INTERNAL",
            "GOVERNMENT_EXCEPTION",
            "100000000003",
            "gamma",
        ),
        (
            &second_parent,
            "AWS",
            "END_CUSTOMER",
            "COMMERCIAL",
            "100000000001",
            "delta",
        ),
        (
            &other_catalog_parent,
            "Sandbox",
            "END_CUSTOMER",
            "COMMERCIAL",
            "100000000001",
            "epsilon",
        ),
    ];

    let mut ids = Vec::new();
    for (parent, catalog, association_type, sector, account_id, name) in rows {
        let created = call_ok(
            &router,
            "CreateRelationship",
            &json!({
                "catalog": catalog,
                "associationType": association_type,
                "programManagementAccountIdentifier": parent.0.clone(),
                "associatedAccountId": account_id,
                "displayName": name,
                "sector": sector,
            }),
            name,
        )
        .await;
        ids.push((
            name,
            string_member(member(&created, "relationshipDetail"), "id"),
        ));
    }
    let id_of = |name: &str| {
        ids.iter()
            .find(|(stored, _)| *stored == name)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("seeded {name}"))
    };

    // Each row names the filter under test and which seeded relationships it must
    // select. An omitted filter constrains nothing; several filters are ANDed.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "catalog-scopes-the-list",
            json!({"catalog": "AWS"}),
            vec!["alpha", "beta", "gamma", "delta"],
        ),
        (
            "other-catalog",
            json!({"catalog": "Sandbox"}),
            vec!["epsilon"],
        ),
        (
            "catalog-with-no-records",
            json!({"catalog": "Empty"}),
            vec![],
        ),
        (
            "associationTypes-single",
            json!({"catalog": "AWS", "associationTypes": ["END_CUSTOMER"]}),
            vec!["alpha", "delta"],
        ),
        (
            "associationTypes-several",
            json!({"catalog": "AWS", "associationTypes": ["INTERNAL", "DOWNSTREAM_SELLER"]}),
            vec!["beta", "gamma"],
        ),
        (
            "displayNames-single",
            json!({"catalog": "AWS", "displayNames": ["gamma"]}),
            vec!["gamma"],
        ),
        (
            "displayNames-several",
            json!({"catalog": "AWS", "displayNames": ["alpha", "delta", "epsilon"]}),
            vec!["alpha", "delta"],
        ),
        (
            "associatedAccountIds",
            json!({"catalog": "AWS", "associatedAccountIds": ["100000000001"]}),
            vec!["alpha", "delta"],
        ),
        (
            "programManagementAccountIdentifiers-bare-id",
            json!({
                "catalog": "AWS",
                "programManagementAccountIdentifiers": [second_parent.0.clone()],
            }),
            vec!["gamma", "delta"],
        ),
        (
            "programManagementAccountIdentifiers-arn",
            json!({
                "catalog": "AWS",
                "programManagementAccountIdentifiers": [first_parent.1.clone()],
            }),
            vec!["alpha", "beta"],
        ),
        (
            "several-filters-are-anded",
            json!({
                "catalog": "AWS",
                "associationTypes": ["END_CUSTOMER"],
                "programManagementAccountIdentifiers": [first_parent.0.clone()],
            }),
            vec!["alpha"],
        ),
        (
            "filters-that-select-nothing",
            json!({"catalog": "AWS", "associatedAccountIds": ["100000000009"]}),
            vec![],
        ),
    ];

    for (case, request, expected_names) in cases {
        let listed = call_ok(&router, "ListRelationships", &request, case).await;
        let actual: Vec<String> = member(&listed, "items")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| string_member(item, "id"))
            .collect();
        let expected: Vec<String> = expected_names.iter().map(|name| id_of(name)).collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected_names:?} ({expected:?}), got {actual:?}"
        );
    }
}

#[tokio::test]
async fn list_relationships_summaries_are_built_from_the_stored_record() {
    let router = emulator();
    let case = lifecycle_cases()[1].clone();
    let parent = create_parent(&router, &case).await;
    let child = create_child(&router, &case, &parent).await;

    let listed = call_ok(
        &router,
        "ListRelationships",
        &json!({"catalog": case.catalog}),
        case.case,
    )
    .await;
    let items = member(&listed, "items").as_array().cloned().unwrap();
    assert_eq!(
        items.len(),
        1,
        "case {:?}: expected exactly the one created relationship, got {items:?}",
        case.case
    );
    let summary = &items[0];
    let detail = member(
        &call_ok(
            &router,
            "GetRelationship",
            &json!({
                "catalog": case.catalog,
                "programManagementAccountIdentifier": parent.0.clone(),
                "identifier": child.0.clone(),
            }),
            case.case,
        )
        .await,
        "relationshipDetail",
    )
    .clone();

    // `RelationshipSummary` declares every `RelationshipDetail` member except
    // `resaleAccountModel`, so each shared member must report the same value.
    for field in [
        "arn",
        "id",
        "revision",
        "catalog",
        "associationType",
        "programManagementAccountId",
        "associatedAccountId",
        "displayName",
        "sector",
        "createdAt",
        "updatedAt",
        "startDate",
    ] {
        assert_eq!(
            member(summary, field),
            member(&detail, field),
            "case {:?}: the {field} in the list summary must match the one Get reports",
            case.case
        );
    }
}

#[tokio::test]
async fn list_relationships_refuses_a_token_it_did_not_issue() {
    let router = emulator();
    let case = lifecycle_cases()[0].clone();
    let parent = create_parent(&router, &case).await;
    seed_relationships(&router, case.catalog, &parent.0, &["one", "two"]).await;

    let first = call_ok(
        &router,
        "ListRelationships",
        &json!({"catalog": case.catalog, "maxResults": 1}),
        "token-reuse",
    )
    .await;
    let token = string_member(&first, "nextToken");
    assert!(
        !token.is_empty(),
        "case \"token-reuse\": expected a nextToken while a page remains, got {first}"
    );

    // A token is only valid for the filters it was taken with, and a token this
    // service never issued is refused rather than treated as the first page.
    for (case, request) in [
        (
            "token-with-different-filters",
            json!({
                "catalog": case.catalog,
                "maxResults": 1,
                "nextToken": token,
                "displayNames": ["one"],
            }),
        ),
        (
            "token-we-never-issued",
            json!({"catalog": case.catalog, "nextToken": "notatokenweissued"}),
        ),
        (
            "token-for-another-catalog",
            json!({"catalog": "Sandbox", "maxResults": 1, "nextToken": "7a7a7a"}),
        ),
    ] {
        assert_refused(
            call(&router, "ListRelationships", &request).await,
            400,
            "ValidationException",
            case,
        );
    }
}

// ---------------------------------------------------------------------------
// Updating and deleting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_relationship_bumps_the_revision_and_a_later_read_observes_it() {
    let router = emulator();
    let case = lifecycle_cases()[0].clone();
    let parent = create_parent(&router, &case).await;
    let child = create_child(&router, &case, &parent).await;

    // Each row renames the relationship and expects the revision to advance by
    // one; the last two prove an update that names the CURRENT revision succeeds
    // and that one naming the support plan alone leaves the name alone.
    let cases: Vec<(&str, Value, &str, u32)> = vec![
        (
            "rename",
            json!({"displayName": "Acme Renamed"}),
            "Acme Renamed",
            2,
        ),
        (
            "rename-to-the-upper-length-bound",
            json!({"displayName": DISPLAY_NAME_30}),
            DISPLAY_NAME_30,
            3,
        ),
        (
            "rename-with-the-current-revision",
            json!({"displayName": "Acme Again", "revision": "3"}),
            "Acme Again",
            4,
        ),
        (
            "support-plan-only-leaves-the-name-alone",
            json!({"requestedSupportPlan": {"partnerLedSupport": {
                "coverage": "ENTIRE_ORGANIZATION",
                "tamLocation": "us-east-1"
            }}}),
            "Acme Again",
            5,
        ),
    ];

    for (label, patch, expected_name, expected_revision) in cases {
        let mut request = json!({
            "catalog": case.catalog,
            "programManagementAccountIdentifier": parent.0.clone(),
            "identifier": child.0.clone(),
        });
        for (key, value) in patch.as_object().expect("an object patch") {
            request[key] = value.clone();
        }
        let updated = call_ok(&router, "UpdateRelationship", &request, label).await;
        let detail = member(&updated, "relationshipDetail").clone();
        assert_eq!(
            string_member(&detail, "revision"),
            expected_revision.to_string(),
            "case {label:?}: expected revision {expected_revision}, got {detail}"
        );
        assert_eq!(
            string_member(&detail, "displayName"),
            expected_name,
            "case {label:?}: expected displayName {expected_name:?}, got {detail}"
        );
        assert_eq!(
            string_member(&detail, "id"),
            child.0,
            "case {label:?}: expected the update to report the stored id, got {detail}"
        );
        assert_eq!(
            string_member(&detail, "arn"),
            child.1,
            "case {label:?}: expected the update to report the stored ARN, got {detail}"
        );

        // A following read observes the new values.
        let fetched = member(
            &call_ok(
                &router,
                "GetRelationship",
                &json!({
                    "catalog": case.catalog,
                    "programManagementAccountIdentifier": parent.0.clone(),
                    "identifier": child.0.clone(),
                }),
                label,
            )
            .await,
            "relationshipDetail",
        )
        .clone();
        assert_eq!(
            string_member(&fetched, "displayName"),
            expected_name,
            "case {label:?}: a later read must report the updated displayName, got {fetched}"
        );
        assert_eq!(
            string_member(&fetched, "revision"),
            expected_revision.to_string(),
            "case {label:?}: a later read must report the bumped revision, got {fetched}"
        );
    }

    // A revision other than the current one is refused, and changes nothing.
    for label in ["1", "4", "0", "9999999999"] {
        assert_refused(
            call(
                &router,
                "UpdateRelationship",
                &json!({
                    "catalog": case.catalog,
                    "programManagementAccountIdentifier": parent.0.clone(),
                    "identifier": child.0.clone(),
                    "revision": label,
                    "displayName": "Should Not Stick",
                }),
            )
            .await,
            409,
            "ConflictException",
            label,
        );
        let fetched = member(
            &call_ok(
                &router,
                "GetRelationship",
                &json!({
                    "catalog": case.catalog,
                    "programManagementAccountIdentifier": parent.0.clone(),
                    "identifier": child.0.clone(),
                }),
                label,
            )
            .await,
            "relationshipDetail",
        )
        .clone();
        assert_eq!(
            string_member(&fetched, "revision"),
            "5",
            "case {label:?}: a refused update must leave the stored revision alone, got {fetched}"
        );
        assert_eq!(
            string_member(&fetched, "displayName"),
            "Acme Again",
            "case {label:?}: a refused update must leave the stored name alone, got {fetched}"
        );
    }
}

#[tokio::test]
async fn delete_relationship_removes_the_record() {
    let router = emulator();

    // Both identifier forms delete the same record.
    for (label, use_arn) in [("delete-by-id", false), ("delete-by-arn", true)] {
        let router = &router;
        let case = LifecycleCase {
            case: label,
            pma_account_id: if use_arn {
                "888888888888"
            } else {
                "777777777777"
            },
            ..lifecycle_cases()[0].clone()
        };
        let parent = create_parent(router, &case).await;
        let child = create_child(router, &case, &parent).await;
        let identifier = if use_arn {
            child.1.clone()
        } else {
            child.0.clone()
        };

        let deleted = call_ok(
            router,
            "DeleteRelationship",
            &json!({
                "catalog": case.catalog,
                "programManagementAccountIdentifier": parent.0.clone(),
                "identifier": identifier,
            }),
            label,
        )
        .await;
        assert!(
            deleted.as_object().is_none_or(serde_json::Map::is_empty),
            "case {label:?}: DeleteRelationship models no output members, got {deleted}"
        );

        assert_refused(
            call(
                router,
                "GetRelationship",
                &json!({
                    "catalog": case.catalog,
                    "programManagementAccountIdentifier": parent.0.clone(),
                    "identifier": child.0.clone(),
                }),
            )
            .await,
            404,
            "ResourceNotFoundException",
            label,
        );
        let listed = call_ok(
            router,
            "ListRelationships",
            &json!({
                "catalog": case.catalog,
                "programManagementAccountIdentifiers": [parent.0.clone()],
            }),
            label,
        )
        .await;
        let items = member(&listed, "items").as_array().cloned().unwrap_or_default();
        assert!(
            items.is_empty(),
            "case {label:?}: a deleted relationship must be gone from the list, got {items:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_or_mismatched_identifier_is_not_found() {
    let router = emulator();
    let case = lifecycle_cases()[0].clone();
    let parent = create_parent(&router, &case).await;
    let child = create_child(&router, &case, &parent).await;
    let other_parent = create_parent(
        &router,
        &LifecycleCase {
            case: "other-parent",
            pma_account_id: "121212121212",
            ..case.clone()
        },
    )
    .await;

    // Each row is a well-formed request naming something that is not there: an id
    // nothing minted, the right relationship under the wrong parent, and the right
    // relationship in the wrong catalog.
    let cases: Vec<(&str, Value)> = vec![
        (
            "relationship-that-was-never-created",
            json!({
                "catalog": case.catalog,
                "programManagementAccountIdentifier": parent.0.clone(),
                "identifier": "rs-0000000000000",
            }),
        ),
        (
            "parent-that-was-never-created",
            json!({
                "catalog": case.catalog,
                "programManagementAccountIdentifier": "pma-0000000000000",
                "identifier": child.0.clone(),
            }),
        ),
        (
            "relationship-under-the-wrong-parent",
            json!({
                "catalog": case.catalog,
                "programManagementAccountIdentifier": other_parent.0.clone(),
                "identifier": child.0.clone(),
            }),
        ),
        (
            "relationship-in-the-wrong-catalog",
            json!({
                "catalog": "Sandbox",
                "programManagementAccountIdentifier": parent.0.clone(),
                "identifier": child.0.clone(),
            }),
        ),
    ];

    for (label, request) in cases {
        for operation in ["GetRelationship", "UpdateRelationship", "DeleteRelationship"] {
            assert_refused(
                call(&router, operation, &request).await,
                404,
                "ResourceNotFoundException",
                &format!("{label}/{operation}"),
            );
        }
    }

    // Creating a relationship under a parent that does not exist is the same 404.
    assert_refused(
        call(
            &router,
            "CreateRelationship",
            &json!({
                "catalog": case.catalog,
                "associationType": "END_CUSTOMER",
                "programManagementAccountIdentifier": "pma-0000000000000",
                "associatedAccountId": "210987654321",
                "displayName": "Orphan",
                "sector": "COMMERCIAL",
            }),
        )
        .await,
        404,
        "ResourceNotFoundException",
        "create-under-a-missing-parent",
    );
    // …and so is creating one under a parent that exists in another catalog.
    assert_refused(
        call(
            &router,
            "CreateRelationship",
            &json!({
                "catalog": "Sandbox",
                "associationType": "END_CUSTOMER",
                "programManagementAccountIdentifier": parent.0.clone(),
                "associatedAccountId": "210987654321",
                "displayName": "Wrong Catalog",
                "sector": "COMMERCIAL",
            }),
        )
        .await,
        404,
        "ResourceNotFoundException",
        "create-under-a-parent-in-another-catalog",
    );
}

// ---------------------------------------------------------------------------
// Conflicts and idempotency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_program_management_account_for_the_same_triple_is_a_conflict() {
    let router = emulator();
    let first = json!({
        "catalog": "AWS",
        "program": "SOLUTION_PROVIDER",
        "displayName": "Acme PMA",
        "accountId": "123456789012",
    });
    call_ok(
        &router,
        "CreateProgramManagementAccount",
        &first,
        "first-create",
    )
    .await;

    // The same (catalog, program, accountId) triple conflicts, however the rest of
    // the request differs; changing any member OF the triple does not.
    let mut renamed = first.clone();
    renamed["displayName"] = json!("Acme Renamed");
    assert_refused(
        call(&router, "CreateProgramManagementAccount", &renamed).await,
        409,
        "ConflictException",
        "same-triple-different-display-name",
    );
    assert_refused(
        call(&router, "CreateProgramManagementAccount", &first).await,
        409,
        "ConflictException",
        "same-triple-identical-request",
    );

    for (label, member_name, value) in [
        ("different-catalog", "catalog", json!("Sandbox")),
        ("different-program", "program", json!("DISTRIBUTION")),
        ("different-account", "accountId", json!("210987654321")),
    ] {
        let mut request = first.clone();
        request[member_name] = value;
        let created = call_ok(
            &router,
            "CreateProgramManagementAccount",
            &request,
            label,
        )
        .await;
        let id = string_member(member(&created, "programManagementAccountDetail"), "id");
        assert!(
            is_id(&id, "pma-"),
            "case {label:?}: a distinct triple must mint its own account, got {created}"
        );
    }
}

#[tokio::test]
async fn a_replayed_client_token_returns_the_first_result() {
    let router = emulator();
    let pma_request = json!({
        "catalog": "AWS",
        "program": "SOLUTION_PROVIDER",
        "displayName": "Acme PMA",
        "accountId": "123456789012",
        "clientToken": "token-one",
    });
    let first = call_ok(
        &router,
        "CreateProgramManagementAccount",
        &pma_request,
        "first",
    )
    .await;
    let replay = call_ok(
        &router,
        "CreateProgramManagementAccount",
        &pma_request,
        "replay",
    )
    .await;
    assert_eq!(
        member(&first, "programManagementAccountDetail"),
        member(&replay, "programManagementAccountDetail"),
        "case \"replay\": a replayed clientToken must return the first result, not a second \
         account"
    );

    // The same token with different parameters is a conflict.
    let mut changed = pma_request.clone();
    changed["displayName"] = json!("Acme Renamed");
    assert_refused(
        call(&router, "CreateProgramManagementAccount", &changed).await,
        409,
        "ConflictException",
        "same-token-different-parameters",
    );

    let parent_id = string_member(member(&first, "programManagementAccountDetail"), "id");
    let relationship_request = json!({
        "catalog": "AWS",
        "associationType": "END_CUSTOMER",
        "programManagementAccountIdentifier": parent_id.clone(),
        "associatedAccountId": "210987654321",
        "displayName": "Acme Customer",
        "sector": "COMMERCIAL",
        "clientToken": "token-two",
    });
    let created = call_ok(&router, "CreateRelationship", &relationship_request, "create").await;
    let replayed = call_ok(&router, "CreateRelationship", &relationship_request, "replay").await;
    assert_eq!(
        member(&created, "relationshipDetail"),
        member(&replayed, "relationshipDetail"),
        "case \"replay\": a replayed clientToken must return the first relationship, not a second"
    );
    let listed = call_ok(
        &router,
        "ListRelationships",
        &json!({"catalog": "AWS"}),
        "replay",
    )
    .await;
    let items = member(&listed, "items").as_array().cloned().unwrap_or_default();
    assert_eq!(
        items.len(),
        1,
        "case \"replay\": a replayed create must not have stored a second relationship, got \
         {items:?}"
    );

    // `DeleteRelationship` is `@idempotent`: the retry replays the success rather
    // than reporting the record it already removed as missing.
    let delete_request = json!({
        "catalog": "AWS",
        "programManagementAccountIdentifier": parent_id,
        "identifier": string_member(member(&created, "relationshipDetail"), "id"),
        "clientToken": "token-three",
    });
    call_ok(&router, "DeleteRelationship", &delete_request, "delete").await;
    call_ok(
        &router,
        "DeleteRelationship",
        &delete_request,
        "delete-replay",
    )
    .await;
}
