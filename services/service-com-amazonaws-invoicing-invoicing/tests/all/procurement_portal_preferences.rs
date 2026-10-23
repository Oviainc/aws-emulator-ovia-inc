// SPDX-License-Identifier: CC0-1.0

//! The procurement-portal-preference plane, driven end to end over this service's wire
//! protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router
//! uses: `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0
//! body. Nothing reaches into the state struct, so what these tests see is exactly
//! what an SDK client sees.
//!
//! The tables below span the rules this slice implements rather than one recorded
//! request: a create/list round trip over differently shaped inputs (both portals, each
//! combination of the two enable flags, with and without the optional sub-structures),
//! the `@paginated` walk at every page size, the published `MaxResults` bounds and one
//! past each, a `NextToken` this walk did not mint, the `@idempotent` replay of a
//! `ClientToken`, the conflict a second create of one preference identity is, the
//! documented Marketplace PunchOut requirement, and the tag plane over the new resource
//! kind.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The status a freshly created preference plane holds: one of the values
/// `ProcurementPortalPreferenceStatus` admits (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`).
const PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";

/// Whether `arn` satisfies the `@pattern` of `ProcurementPortalPreferenceArnString`,
/// the shape `CreateProcurementPortalPreference` returns
/// (`smithy_query(query="operation", operation="CreateProcurementPortalPreference")`):
/// `^arn:aws:invoicing::[0-9]{12}:procurement-portal-preference/[-a-zA-Z0-9]+$`.
fn is_preference_arn(arn: &str) -> bool {
    let Some(resource) = arn.strip_prefix("arn:aws:invoicing::") else {
        return false;
    };
    let Some((account, resource)) = resource.split_once(':') else {
        return false;
    };
    let Some(id) = resource.strip_prefix("procurement-portal-preference/") else {
        return false;
    };
    account.len() == 12
        && account.chars().all(|c| c.is_ascii_digit())
        && !id.is_empty()
        && id.chars().all(|c| c == '-' || c.is_ascii_alphanumeric())
        && arn.len() <= 256
}

/// A router carrying only this service, over one shared engine — the same assembly
/// the production router performs, minus the other services.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        engine,
    ));
    router
}

/// One awsJson1_0 request, returning the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("Invoicing.{operation}"))
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same call, asserting it succeeded.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} {parsed}"
    );
    parsed
}

/// The wire error code of a failure body (`__type` on the awsJson protocols), without
/// the namespace a backend stamps in front of it.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `ProcurementPortalPreferenceArn` of a response that carries one.
fn arn_of(body: &Value) -> String {
    body.get("ProcurementPortalPreferenceArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreferenceArn in {body}"))
        .to_string()
}

/// `ListProcurementPortalPreferences`, one page.
async fn list_page(router: &MultiServiceRouter, request: &Value) -> (Vec<Value>, Option<String>) {
    let body = ok(router, "ListProcurementPortalPreferences", request).await;
    let summaries = body
        .get("ProcurementPortalPreferences")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ProcurementPortalPreferences in {body}"))
        .clone();
    let next = body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (summaries, next)
}

/// Every summary `ListProcurementPortalPreferences` reports, walked at `max_results`
/// (or in one page when `max_results` is `None`).
async fn list_all(router: &MultiServiceRouter, max_results: Option<u32>) -> Vec<Value> {
    let mut all = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let (page, next) = list_page(router, &request).await;
        if let Some(max) = max_results {
            assert!(
                page.len() <= max as usize,
                "case MaxResults={max}: a page holds at most that many, got {}",
                page.len()
            );
        }
        all.extend(page);
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    all
}

/// The summary of `arn` among `summaries`.
fn summary_of<'s>(summaries: &'s [Value], arn: &str) -> &'s Value {
    summaries
        .iter()
        .find(|summary| {
            summary
                .get("ProcurementPortalPreferenceArn")
                .and_then(Value::as_str)
                == Some(arn)
        })
        .unwrap_or_else(|| panic!("expected {arn} among the reported preferences"))
}

/// The members a summary reports back exactly as the create request stated them
/// (`smithy_query(query="shape", shape="ProcurementPortalPreferenceSummary")`).
const ECHOED_MEMBERS: &[&str] = &[
    "ProcurementPortalName",
    "BuyerDomain",
    "BuyerIdentifier",
    "SupplierDomain",
    "SupplierIdentifier",
    "Selector",
    "EinvoiceDeliveryEnabled",
    "PurchaseOrderRetrievalEnabled",
];

/// Members of the create request that the summary shape does NOT declare — the
/// `@sensitive` ones among them must not travel in a list response.
const WITHHELD_MEMBERS: &[&str] = &[
    "ProcurementPortalSharedSecret",
    "Contacts",
    "TestEnvPreference",
    "EinvoiceDeliveryPreference",
    "MarketplacePunchOutPreference",
    "ResourceTags",
    "ClientToken",
];

/// One contact, the single element `Contacts` (`@length(min: 1, max: 1)`) admits.
fn contacts() -> Value {
    json!([{ "Name": "Portal Admin", "Email": "portal.admin@example.com" }])
}

/// A create request for the portal and buyer/supplier pair that identify a preference,
/// with both planes enabled and nothing optional.
fn minimal_create(portal: &str, buyer: &str, supplier: &str) -> Value {
    json!({
        "ProcurementPortalName": portal,
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": buyer,
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": supplier,
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
        "Contacts": contacts(),
    })
}

/// The create requests the round-trip table spans: both portals, each combination of
/// the two enable flags, Marketplace PunchOut present and absent, and the optional
/// sub-structures present and absent.
fn create_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "both planes enabled, nothing optional",
            minimal_create("SAP_BUSINESS_NETWORK", "BUYER-1", "SUPPLIER-1"),
        ),
        (
            "e-invoice only, every optional structure supplied, Coupa punch-out",
            json!({
                "ProcurementPortalName": "COUPA",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "BUYER-2",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "SUPPLIER-2",
                "EinvoiceDeliveryEnabled": true,
                "PurchaseOrderRetrievalEnabled": false,
                "MarketplacePunchOutEnabled": true,
                "MarketplacePunchOutPreference": {
                    "ApprovalRequestRedirectUrl": "https://example.com/approve"
                },
                "Selector": {
                    "InvoiceUnitArns": ["arn:aws:invoicing::000000000000:invoice-unit/000000000001"],
                    "SellerOfRecords": ["SOR-1", "SOR-2"]
                },
                "ProcurementPortalSharedSecret": "s3cret-value",
                "ProcurementPortalInstanceEndpoint": "https://portal.example.com/cxml",
                "TestEnvPreference": {
                    "BuyerDomain": "NetworkID",
                    "BuyerIdentifier": "BUYER-2-TEST",
                    "SupplierDomain": "NetworkID",
                    "SupplierIdentifier": "SUPPLIER-2-TEST",
                    "ProcurementPortalSharedSecret": "test-s3cret",
                    "ProcurementPortalInstanceEndpoint": "https://test.portal.example.com/cxml"
                },
                "EinvoiceDeliveryPreference": {
                    "EinvoiceDeliveryDocumentTypes": ["AWS_CLOUD_INVOICE", "AWS_MARKETPLACE_INVOICE"],
                    "EinvoiceDeliveryAttachmentTypes": ["INVOICE_PDF"],
                    "Protocol": "CXML",
                    "PurchaseOrderDataSources": [{
                        "EinvoiceDeliveryDocumentType": "AWS_CLOUD_INVOICE",
                        "PurchaseOrderDataSourceType": "PURCHASE_ORDER_NOT_REQUIRED"
                    }],
                    "ConnectionTestingMethod": "TEST_ENV_REPLAY_TEST",
                    "EinvoiceDeliveryActivationDate": 1_761_172_800
                },
                "Contacts": contacts(),
                "ResourceTags": [{ "Key": "env", "Value": "test" }],
            }),
        ),
        (
            "purchase-order retrieval only",
            json!({
                "ProcurementPortalName": "SAP_BUSINESS_NETWORK",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "BUYER-3",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "SUPPLIER-3",
                "EinvoiceDeliveryEnabled": false,
                "PurchaseOrderRetrievalEnabled": true,
                "Selector": { "SellerOfRecords": ["SOR-3"] },
                "Contacts": contacts(),
            }),
        ),
        (
            "neither plane enabled",
            json!({
                "ProcurementPortalName": "COUPA",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "BUYER-4",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "SUPPLIER-4",
                "EinvoiceDeliveryEnabled": false,
                "PurchaseOrderRetrievalEnabled": false,
                "Contacts": contacts(),
            }),
        ),
    ]
}

/// What a summary reports for a plane: the pending status while the plane is enabled,
/// and nothing at all while it is not.
fn expected_status(enabled: bool) -> Value {
    if enabled {
        json!(PENDING_VERIFICATION)
    } else {
        Value::Null
    }
}

/// Every created preference is reported by `ListProcurementPortalPreferences`, with
/// every member computed from what the create persisted.
#[tokio::test]
async fn list_reports_each_created_preference() {
    let router = emulator();

    // A list before anything is created reports an empty page, not a 404 and not a
    // missing member.
    let (empty, token) = list_page(&router, &json!({})).await;
    assert!(
        empty.is_empty() && token.is_none(),
        "case empty account: expected no preferences and no NextToken, got {empty:?} / {token:?}"
    );

    let cases = create_cases();
    let mut created = Vec::new();
    for (label, request) in &cases {
        let response = ok(&router, "CreateProcurementPortalPreference", request).await;
        let arn = arn_of(&response);
        assert!(
            is_preference_arn(&arn),
            "case {label}: expected a ProcurementPortalPreferenceArnString, got {arn:?}"
        );
        created.push((label, request.clone(), arn));
    }

    let summaries = list_all(&router, None).await;
    assert_eq!(
        summaries.len(),
        cases.len(),
        "case all created: expected {} preferences, got {}",
        cases.len(),
        summaries.len()
    );

    for (label, request, arn) in &created {
        let summary = summary_of(&summaries, arn);
        for member in ECHOED_MEMBERS {
            if let Some(sent) = request.get(member) {
                let actual = summary.get(member).unwrap_or(&Value::Null);
                assert_eq!(
                    actual, sent,
                    "case {label}: expected {member} {sent}, got {actual}"
                );
            }
        }
        for member in WITHHELD_MEMBERS {
            assert!(
                summary.get(member).is_none(),
                "case {label}: {member} is not a member of the summary shape, got {}",
                summary[member]
            );
        }

        let account = summary["AwsAccountId"].as_str().unwrap_or_default();
        assert!(
            account.len() == 12 && account.chars().all(|c| c.is_ascii_digit()),
            "case {label}: expected a 12-digit AwsAccountId, got {account:?}"
        );
        assert!(
            arn.contains(account),
            "case {label}: expected the ARN {arn} to carry the account {account}"
        );

        // `MarketplacePunchOutEnabled` "defaults to false if not provided".
        let expected_punch_out = request
            .get("MarketplacePunchOutEnabled")
            .cloned()
            .unwrap_or(json!(false));
        assert_eq!(
            summary["MarketplacePunchOutEnabled"], expected_punch_out,
            "case {label}: expected MarketplacePunchOutEnabled {expected_punch_out}, got {}",
            summary["MarketplacePunchOutEnabled"]
        );

        for (member, flag) in [
            ("EinvoiceDeliveryPreferenceStatus", "EinvoiceDeliveryEnabled"),
            (
                "PurchaseOrderRetrievalPreferenceStatus",
                "PurchaseOrderRetrievalEnabled",
            ),
        ] {
            let enabled = request[flag].as_bool().unwrap_or_default();
            let expected = expected_status(enabled);
            let actual = summary.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {label}: expected {member} {expected}, got {actual}"
            );
            let reason = format!("{member}Reason");
            assert!(
                summary.get(&reason).is_none(),
                "case {label}: nothing has given a reason yet, got {}",
                summary[&reason]
            );
        }

        assert_eq!(
            summary["Version"],
            json!(1),
            "case {label}: expected the first Version, got {}",
            summary["Version"]
        );
        let created_at = summary["CreateDate"].as_f64().unwrap_or_default();
        let updated_at = summary["LastUpdateDate"].as_f64().unwrap_or_default();
        assert!(
            created_at > 0.0,
            "case {label}: expected a CreateDate, got {}",
            summary["CreateDate"]
        );
        assert_eq!(
            created_at, updated_at,
            "case {label}: nothing has updated the preference, so LastUpdateDate must equal \
             CreateDate ({created_at} vs {updated_at})"
        );
    }

    // A list is a read: a second one answers exactly the same, in the same order.
    let again = list_all(&router, None).await;
    assert_eq!(
        again, summaries,
        "case repeated list: a read must not change what a later caller sees"
    );
}

/// The `@paginated` walk reports every preference exactly once, in the same order,
/// whatever page size the caller asks for.
#[tokio::test]
async fn pagination_walks_every_preference_once_at_every_page_size() {
    let router = emulator();
    let total = 7;
    let mut arns = Vec::new();
    for index in 0..total {
        let request = minimal_create(
            "SAP_BUSINESS_NETWORK",
            &format!("BUYER-{index}"),
            &format!("SUPPLIER-{index}"),
        );
        arns.push(arn_of(
            &ok(&router, "CreateProcurementPortalPreference", &request).await,
        ));
    }

    let whole = list_all(&router, None).await;
    let whole_arns: Vec<String> = whole
        .iter()
        .map(|summary| summary["ProcurementPortalPreferenceArn"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut sorted_created = arns.clone();
    sorted_created.sort();
    let mut sorted_reported = whole_arns.clone();
    sorted_reported.sort();
    assert_eq!(
        sorted_reported, sorted_created,
        "case one page: expected every created preference, got {sorted_reported:?}"
    );

    // 1 and the published maximum are the bounds of `MaxResults`; the sizes between
    // them, and one larger than the collection, walk the same preferences.
    for max in [1_u32, 2, 3, 6, 7, 8, 100] {
        let paged = list_all(&router, Some(max)).await;
        let paged_arns: Vec<String> = paged
            .iter()
            .map(|summary| summary["ProcurementPortalPreferenceArn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            paged_arns, whole_arns,
            "case MaxResults={max}: expected the same preferences in the same order as one \
             page, got {paged_arns:?}"
        );
        assert_eq!(
            paged, whole,
            "case MaxResults={max}: a paged walk reports the same summaries as one page"
        );
    }

    // The page that reports the last preference carries no token, and a page that stops
    // short of the end carries one.
    let (first, next) = list_page(&router, &json!({ "MaxResults": 1 })).await;
    assert_eq!(
        first.len(),
        1,
        "case MaxResults=1: expected one summary, got {}",
        first.len()
    );
    assert!(
        next.is_some(),
        "case MaxResults=1: {total} preferences do not fit in one page, so a NextToken is owed"
    );
    let (_, no_next) = list_page(&router, &json!({ "MaxResults": total })).await;
    assert!(
        no_next.is_none(),
        "case MaxResults={total}: every preference fits, so no NextToken is owed, got {no_next:?}"
    );
}

/// `MaxResults` is refused outside its published `@range(min: 1, max: 100)` and
/// accepted on each bound.
#[tokio::test]
async fn max_results_is_accepted_on_its_bounds_and_refused_past_them() {
    let router = emulator();
    ok(
        &router,
        "CreateProcurementPortalPreference",
        &minimal_create("COUPA", "BUYER-B", "SUPPLIER-B"),
    )
    .await;

    for (max, expected) in [(1, 200), (2, 200), (99, 200), (100, 200), (0, 400), (101, 400)] {
        let (status, body) = call(
            &router,
            "ListProcurementPortalPreferences",
            &json!({ "MaxResults": max }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case MaxResults={max}: expected HTTP {expected}, got {status} {body}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case MaxResults={max}: expected a ValidationException, got {body}"
            );
        }
    }
}

/// A `NextToken` this walk did not mint is refused with the model's own
/// `invalidNextToken` reason, rather than resuming from arbitrary bytes.
#[tokio::test]
async fn a_next_token_this_walk_did_not_mint_is_refused() {
    let router = emulator();
    // TWO preferences, so a page of one leaves a second behind and the walk really owes
    // a token: the positive half of this rule is then forced to run.
    for (buyer, supplier) in [("BUYER-C1", "SUPPLIER-C1"), ("BUYER-C2", "SUPPLIER-C2")] {
        ok(
            &router,
            "CreateProcurementPortalPreference",
            &minimal_create("COUPA", buyer, supplier),
        )
        .await;
    }

    let foreign_walk = hex::encode("invoice-units:arn:aws:invoicing::000000000000:invoice-unit/1");
    let plain_arn = hex::encode("arn:aws:invoicing::000000000000:procurement-portal-preference/1");
    for (label, token) in [
        ("not hex at all", "zzzz".to_string()),
        ("hex of unrelated bytes", hex::encode("whatever")),
        ("another walk's token", foreign_walk),
        ("the position without the prefix", plain_arn),
    ] {
        let (status, body) = call(
            &router,
            "ListProcurementPortalPreferences",
            &json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            body.get("reason").and_then(Value::as_str),
            Some("invalidNextToken"),
            "case {label}: expected the invalidNextToken reason, got {body}"
        );
    }

    // A token this walk DID mint resumes it, and reports the preference the first page
    // stopped before.
    let (first, minted) = list_page(&router, &json!({ "MaxResults": 1 })).await;
    let minted = minted.expect("two preferences do not fit in a page of one, so a token is owed");
    let resumed = ok(
        &router,
        "ListProcurementPortalPreferences",
        &json!({ "NextToken": minted }),
    )
    .await;
    let resumed = resumed["ProcurementPortalPreferences"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        resumed.len(),
        1,
        "case a minted token: expected the one preference the first page stopped before, got \
         {resumed:?}"
    );
    assert_ne!(
        resumed.first(),
        first.first(),
        "case a minted token: the resumed page must not repeat the first page's preference"
    );
}

/// A create past the account's maximum is refused with the modelled
/// `ServiceQuotaExceededException` (HTTP 402), and refuses nothing before it.
///
/// The maximum is the emulator's own
/// (`service_com_amazonaws_invoicing_invoicing::state::MAX_PROCUREMENT_PORTAL_PREFERENCES`),
/// so the test reads it rather than restating a number.
#[tokio::test]
async fn a_create_past_the_maximum_is_refused_with_the_modelled_quota_error() {
    let maximum =
        service_com_amazonaws_invoicing_invoicing::state::MAX_PROCUREMENT_PORTAL_PREFERENCES;
    let router = emulator();
    for index in 0..maximum {
        let request = minimal_create(
            "SAP_BUSINESS_NETWORK",
            &format!("BUYER-Q{index}"),
            &format!("SUPPLIER-Q{index}"),
        );
        let (status, body) = call(&router, "CreateProcurementPortalPreference", &request).await;
        assert_eq!(
            status, 200,
            "case preference {index} of {maximum}: every create up to the maximum succeeds, got \
             {status} {body}"
        );
    }

    // The one past it, and the one after that: the refusal is the state of the account,
    // not a one-off.
    for attempt in 0..2 {
        let request = minimal_create(
            "COUPA",
            &format!("BUYER-OVER{attempt}"),
            &format!("SUPPLIER-OVER{attempt}"),
        );
        let (status, body) = call(&router, "CreateProcurementPortalPreference", &request).await;
        assert_eq!(
            status, 402,
            "case attempt {attempt} past the maximum: expected HTTP 402, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case attempt {attempt} past the maximum: expected the modelled quota error, got {body}"
        );
    }

    // A refused create persists nothing: the account still holds exactly the maximum.
    let summaries = list_all(&router, Some(100)).await;
    assert_eq!(
        summaries.len(),
        maximum,
        "case after the refusals: expected {maximum} preferences, got {}",
        summaries.len()
    );
}

/// `@idempotent`: one `ClientToken` replayed against the same preference answers with
/// the ARN the first call minted and creates nothing more; the same identity without
/// that token is the conflict a second create of one resource is.
#[tokio::test]
async fn a_client_token_replays_one_creation_and_a_second_create_conflicts() {
    let router = emulator();
    let mut request = minimal_create("COUPA", "BUYER-D", "SUPPLIER-D");
    request["ClientToken"] = json!("token-1");

    let first = arn_of(&ok(&router, "CreateProcurementPortalPreference", &request).await);
    let replay = arn_of(&ok(&router, "CreateProcurementPortalPreference", &request).await);
    assert_eq!(
        replay, first,
        "case replayed ClientToken: expected the first ARN {first}, got {replay}"
    );
    let summaries = list_all(&router, None).await;
    assert_eq!(
        summaries.len(),
        1,
        "case replayed ClientToken: a replay creates nothing more, got {} preferences",
        summaries.len()
    );

    // The same identity under a different token is a second create of one resource.
    let mut other_token = request.clone();
    other_token["ClientToken"] = json!("token-2");
    let (status, body) = call(&router, "CreateProcurementPortalPreference", &other_token).await;
    assert_eq!(
        status, 409,
        "case duplicate identity: expected HTTP 409, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case duplicate identity: expected a ConflictException, got {body}"
    );
    assert_eq!(
        body.get("resourceId").and_then(Value::as_str),
        Some(first.as_str()),
        "case duplicate identity: expected the existing ARN in resourceId, got {body}"
    );

    // One token is scoped to the request it was sent with: another preference under the
    // same token is a different request and creates.
    let mut elsewhere = minimal_create("COUPA", "BUYER-E", "SUPPLIER-E");
    elsewhere["ClientToken"] = json!("token-1");
    let second = arn_of(&ok(&router, "CreateProcurementPortalPreference", &elsewhere).await);
    assert_ne!(
        second, first,
        "case the same token against another preference: expected a new ARN, got {second}"
    );

    // Each part of the identity is part of it: changing any ONE of them creates rather
    // than conflicts.
    let base = minimal_create("COUPA", "BUYER-F", "SUPPLIER-F");
    ok(&router, "CreateProcurementPortalPreference", &base).await;
    let mut minted = BTreeMap::new();
    for (member, value) in [
        ("ProcurementPortalName", json!("SAP_BUSINESS_NETWORK")),
        ("BuyerIdentifier", json!("BUYER-F2")),
        ("SupplierIdentifier", json!("SUPPLIER-F2")),
    ] {
        let mut varied = base.clone();
        varied[member] = value.clone();
        let (status, body) = call(&router, "CreateProcurementPortalPreference", &varied).await;
        assert_eq!(
            status, 200,
            "case varied {member}: a different identity is a different preference, got \
             {status} {body}"
        );
        minted.insert(member, arn_of(&body));
    }
    let distinct: std::collections::BTreeSet<&String> = minted.values().collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "case varied identities: expected one ARN each, got {minted:?}"
    );
}

/// `MarketplacePunchOutPreference` is "required for Coupa when
/// MarketplacePunchOutEnabled is true" — and required nowhere else.
#[tokio::test]
async fn coupa_punch_out_requires_its_preference() {
    let router = emulator();
    let punch_out = json!({ "ApprovalRequestRedirectUrl": "https://example.com/approve" });
    for (label, portal, enabled, preference, expected) in [
        ("Coupa, enabled, no preference", "COUPA", Some(true), None, 400),
        (
            "Coupa, enabled, preference supplied",
            "COUPA",
            Some(true),
            Some(punch_out.clone()),
            200,
        ),
        ("Coupa, disabled", "COUPA", Some(false), None, 200),
        ("Coupa, omitted", "COUPA", None, None, 200),
        (
            "the other portal, enabled, no preference",
            "SAP_BUSINESS_NETWORK",
            Some(true),
            None,
            200,
        ),
    ] {
        // One identity per row, so a conflict never stands in for the refusal.
        let buyer = format!("BUYER-{}", label.replace([' ', ','], "-"));
        let mut request = minimal_create(portal, &buyer, "SUPPLIER-P");
        if let Some(enabled) = enabled {
            request["MarketplacePunchOutEnabled"] = json!(enabled);
        }
        if let Some(preference) = preference {
            request["MarketplacePunchOutPreference"] = preference;
        }
        let (status, body) = call(&router, "CreateProcurementPortalPreference", &request).await;
        assert_eq!(
            status, expected,
            "case {label}: expected HTTP {expected}, got {status} {body}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label}: expected a ValidationException, got {body}"
            );
        }
    }
}

/// The tag plane serves a procurement portal preference the same way it serves an
/// invoice unit: tags given at creation are reported for its ARN, and an ARN no
/// resource carries is a 404.
#[tokio::test]
async fn tags_given_at_creation_are_reported_for_the_preference() {
    let router = emulator();
    let mut request = minimal_create("COUPA", "BUYER-T", "SUPPLIER-T");
    request["ResourceTags"] = json!([
        { "Key": "env", "Value": "test" },
        { "Key": "team", "Value": "billing" },
    ]);
    let arn = arn_of(&ok(&router, "CreateProcurementPortalPreference", &request).await);

    let tags = ok(&router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
    assert_eq!(
        tags["ResourceTags"], request["ResourceTags"],
        "case tagged at creation: expected {}, got {}",
        request["ResourceTags"], tags["ResourceTags"]
    );

    let absent = "arn:aws:invoicing::000000000000:procurement-portal-preference/ffffffffffff";
    let (status, body) = call(
        &router,
        "ListTagsForResource",
        &json!({ "ResourceArn": absent }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case no such preference: expected HTTP 404, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case no such preference: expected a ResourceNotFoundException, got {body}"
    );

    // A preference that carries no tags has an empty tag list, not a 404.
    let untagged = arn_of(
        &ok(
            &router,
            "CreateProcurementPortalPreference",
            &minimal_create("COUPA", "BUYER-U", "SUPPLIER-U"),
        )
        .await,
    );
    let none = ok(
        &router,
        "ListTagsForResource",
        &json!({ "ResourceArn": untagged }),
    )
    .await;
    assert_eq!(
        none["ResourceTags"],
        json!([]),
        "case untagged preference: expected an empty tag list, got {}",
        none["ResourceTags"]
    );
}
