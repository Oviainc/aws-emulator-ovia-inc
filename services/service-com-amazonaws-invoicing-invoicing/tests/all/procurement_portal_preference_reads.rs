// SPDX-License-Identifier: CC0-1.0

//! `GetProcurementPortalPreference`, driven end to end over this service's wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body. Nothing
//! reaches into the state struct, so what these tests see is exactly what an SDK client
//! sees.
//!
//! The rule under test: a read of one preference reports what state holds for the ARN asked
//! about and nothing else — every member as the create stored it, the members the service
//! assigns (the account, the ARN, the documented `MarketplacePunchOutEnabled` default, a
//! status for each plane the request enabled, the first version, the create instant), the
//! same answer on a second read, the same values the listing reports for that preference,
//! and the modelled `ResourceNotFoundException` for an ARN this service does not hold.
//!
//! The tables below span that class rather than one recorded request: both portals, each
//! combination of the two enable flags, requests carrying only the required members and
//! requests carrying every optional one, and several ARNs the service does not hold.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The status each enabled plane of a freshly created preference carries: one of the values
/// `ProcurementPortalPreferenceStatus` admits (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`), because validation is driven by
/// `SendProcurementPortalValidation` / `VerifyProcurementPortalValidation`.
const PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";

/// The message a 404 of the PROCUREMENT-PORTAL-PREFERENCE plane carries is built from the
/// request (see `procurement_portal_error_message_key.rs`, which pins that rule over a
/// table), so this suite asserts only that it names the ARN asked about.
///
/// The key it travels under is this plane's `Message`; a reader that accepts either case
/// keeps this suite about the READ rather than about the key.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An instant every `CreateDate` this suite observes must be at or after: 2025-01-01T00:00Z
/// in epoch seconds, so a date read back is the clock's rather than a zero or a default.
const EARLIEST_PLAUSIBLE_CREATE_DATE: f64 = 1_735_689_600.0;

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services.
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

/// The wire error code of a failure body (`__type` on the awsJson protocols).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `CreateProcurementPortalPreference`, asserting it succeeded, and the ARN it minted.
async fn create(router: &MultiServiceRouter, request: &Value) -> String {
    let created = ok(router, "CreateProcurementPortalPreference", request).await;
    created
        .get("ProcurementPortalPreferenceArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreferenceArn in {created}"))
        .to_string()
}

/// `GetProcurementPortalPreference` for `arn`, asserting it succeeded, and the
/// `ProcurementPortalPreference` it reported.
async fn get(router: &MultiServiceRouter, arn: &str) -> Value {
    let read = ok(
        router,
        "GetProcurementPortalPreference",
        &json!({"ProcurementPortalPreferenceArn": arn}),
    )
    .await;
    read.get("ProcurementPortalPreference")
        .cloned()
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreference in {read}"))
}

/// `value` with every number rendered as the same JSON type, so two bodies that state one
/// number two ways (an integer timestamp and the same value with a fractional part) compare
/// equal on the value rather than on the spelling.
fn normalized(value: &Value) -> Value {
    match value {
        Value::Number(number) => json!(number.as_f64().unwrap_or_default()),
        Value::Array(items) => Value::Array(items.iter().map(normalized).collect()),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(name, member)| (name.clone(), normalized(member)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The `ProcurementPortalPreference` a read owes for `request`, by the rule under test:
/// every member the create carried, exactly as it carried it, plus the members the service
/// itself assigns — the requester's account, the minted ARN, `MarketplacePunchOutEnabled`'s
/// documented default ("defaults to false if not provided"), `PENDING_VERIFICATION` for each
/// plane the request enabled and no status at all for one it did not, and the first version.
/// `CreateDate` and `LastUpdateDate` are clock values and are asserted separately.
fn expected_preference(request: &Value, arn: &str) -> Value {
    let mut expected = Map::new();
    expected.insert(
        "AwsAccountId".to_string(),
        json!(engine::emulator_account_id()),
    );
    expected.insert("ProcurementPortalPreferenceArn".to_string(), json!(arn));
    // Every member a create carries through unchanged.
    for member in [
        "ProcurementPortalName",
        "BuyerDomain",
        "BuyerIdentifier",
        "SupplierDomain",
        "SupplierIdentifier",
        "Selector",
        "ProcurementPortalSharedSecret",
        "ProcurementPortalInstanceEndpoint",
        "TestEnvPreference",
        "EinvoiceDeliveryEnabled",
        "EinvoiceDeliveryPreference",
        "PurchaseOrderRetrievalEnabled",
        "MarketplacePunchOutPreference",
        "Contacts",
    ] {
        if let Some(value) = request.get(member) {
            expected.insert(member.to_string(), value.clone());
        }
    }
    expected.insert(
        "MarketplacePunchOutEnabled".to_string(),
        request
            .get("MarketplacePunchOutEnabled")
            .cloned()
            .unwrap_or(Value::Bool(false)),
    );
    for (flag, status) in [
        ("EinvoiceDeliveryEnabled", "EinvoiceDeliveryPreferenceStatus"),
        (
            "PurchaseOrderRetrievalEnabled",
            "PurchaseOrderRetrievalPreferenceStatus",
        ),
    ] {
        if request.get(flag).and_then(Value::as_bool) == Some(true) {
            expected.insert(status.to_string(), json!(PENDING_VERIFICATION));
        }
    }
    expected.insert("Version".to_string(), json!(1));
    Value::Object(expected)
}

/// The one contact a request carries (`Contacts` is modelled `length 1..1`), with an
/// `Email` of the modelled `EmailString` form.
fn contacts(local_part: &str) -> Value {
    json!([{"Name": "Portal Administrator", "Email": format!("{local_part}@example.com")}])
}

/// An `EinvoiceDeliveryPreference`, with every member the shape requires
/// (`smithy_query(query="shape", shape="EinvoiceDeliveryPreference")`).
fn einvoice_delivery_preference() -> Value {
    json!({
        "EinvoiceDeliveryDocumentTypes": ["AWS_CLOUD_INVOICE", "AWS_CLOUD_CREDIT_MEMO"],
        "EinvoiceDeliveryAttachmentTypes": ["INVOICE_PDF"],
        "Protocol": "CXML",
        "PurchaseOrderDataSources": [{
            "EinvoiceDeliveryDocumentType": "AWS_CLOUD_INVOICE",
            "PurchaseOrderDataSourceType": "ASSOCIATED_PURCHASE_ORDER_REQUIRED",
        }],
        "ConnectionTestingMethod": "TEST_ENV_REPLAY_TEST",
        // A `Timestamp`, which awsJson1_0 carries as epoch seconds.
        "EinvoiceDeliveryActivationDate": 1_767_225_600_u64,
    })
}

/// One `CreateProcurementPortalPreference` request shape, as a table row.
#[derive(Debug, Clone)]
struct ReadCase {
    case: &'static str,
    request: Value,
}

/// The preferences the read rule is asserted over: the two portals, each combination of the
/// two enable flags, the required-members-only shape and the every-optional-member shape,
/// Marketplace PunchOut on and off, a selector by invoice unit and by seller of record, and
/// a test environment.
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            case: "sap, required members only, both planes enabled",
            request: json!({
                "ProcurementPortalName": "SAP_BUSINESS_NETWORK",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "AN01000000001",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "AN01000000002",
                "EinvoiceDeliveryEnabled": true,
                "PurchaseOrderRetrievalEnabled": true,
                "Contacts": contacts("sap-required"),
            }),
        },
        ReadCase {
            case: "coupa, every optional member, punch-out enabled with its preference",
            request: json!({
                "ProcurementPortalName": "COUPA",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "coupa-buyer-7",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "coupa-supplier-7",
                "Selector": {
                    "InvoiceUnitArns": [
                        "arn:aws:invoicing::000000000000:invoice-unit/000000000001",
                        "arn:aws:invoicing::000000000000:invoice-unit/000000000002",
                    ],
                    "SellerOfRecords": ["AWS-EMEA-SARL"],
                },
                "ProcurementPortalInstanceEndpoint": "https://portal.example.com:8443/cxml?v=1",
                "TestEnvPreference": {
                    "BuyerDomain": "NetworkID",
                    "BuyerIdentifier": "coupa-buyer-7-test",
                    "SupplierDomain": "NetworkID",
                    "SupplierIdentifier": "coupa-supplier-7-test",
                    "ProcurementPortalInstanceEndpoint": "https://test.portal.example.com/cxml",
                },
                "EinvoiceDeliveryEnabled": true,
                "EinvoiceDeliveryPreference": einvoice_delivery_preference(),
                "PurchaseOrderRetrievalEnabled": true,
                "MarketplacePunchOutEnabled": true,
                "MarketplacePunchOutPreference": {
                    "ApprovalRequestRedirectUrl": "https://portal.example.com/approvals",
                },
                "Contacts": contacts("coupa-full"),
            }),
        },
        ReadCase {
            case: "e-invoice delivery only",
            request: json!({
                "ProcurementPortalName": "SAP_BUSINESS_NETWORK",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "AN01000000011",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "AN01000000012",
                "EinvoiceDeliveryEnabled": true,
                "EinvoiceDeliveryPreference": einvoice_delivery_preference(),
                "PurchaseOrderRetrievalEnabled": false,
                "Contacts": contacts("einvoice-only"),
            }),
        },
        ReadCase {
            case: "purchase order retrieval only",
            request: json!({
                "ProcurementPortalName": "COUPA",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "coupa-buyer-21",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "coupa-supplier-21",
                "EinvoiceDeliveryEnabled": false,
                "PurchaseOrderRetrievalEnabled": true,
                "MarketplacePunchOutEnabled": false,
                "Contacts": contacts("po-only"),
            }),
        },
        ReadCase {
            case: "both planes disabled, punch-out flag absent",
            request: json!({
                "ProcurementPortalName": "SAP_BUSINESS_NETWORK",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "AN01000000031",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "AN01000000032",
                "EinvoiceDeliveryEnabled": false,
                "PurchaseOrderRetrievalEnabled": false,
                "Contacts": contacts("both-disabled"),
            }),
        },
    ]
}

/// A read reports the preference the create stored, member for member, over every row of
/// [`read_cases`] — and a second read of the same ARN reports exactly the same thing,
/// because reading one changes nothing.
#[tokio::test]
async fn a_read_reports_the_preference_the_create_stored() {
    let router = emulator();
    for ReadCase { case, request } in read_cases() {
        let arn = create(&router, &request).await;

        let read = get(&router, &arn).await;
        let mut actual = read.clone();
        let members = actual
            .as_object_mut()
            .expect("a ProcurementPortalPreference is a structure");
        let create_date = members
            .remove("CreateDate")
            .and_then(|date| date.as_f64())
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreateDate in {read}"));
        let last_update_date = members
            .remove("LastUpdateDate")
            .and_then(|date| date.as_f64())
            .unwrap_or_else(|| panic!("case {case:?}: expected a LastUpdateDate in {read}"));
        assert!(
            create_date >= EARLIEST_PLAUSIBLE_CREATE_DATE,
            "case {case:?}: expected a CreateDate at or after \
             {EARLIEST_PLAUSIBLE_CREATE_DATE}, got {create_date}"
        );
        assert_eq!(
            last_update_date, create_date,
            "case {case:?}: nothing has updated the preference, so expected \
             LastUpdateDate == CreateDate ({create_date}), got {last_update_date}"
        );

        let expected = expected_preference(&request, &arn);
        assert_eq!(
            normalized(&actual),
            normalized(&expected),
            "case {case:?}: expected {expected}, got {actual}"
        );

        let again = get(&router, &arn).await;
        assert_eq!(
            normalized(&again),
            normalized(&read),
            "case {case:?}: reading a preference changes nothing, so expected {read}, \
             got {again}"
        );
    }
}

/// Two readers of one preference cannot disagree: every member the listing's summary states
/// holds the value the single-preference read reports for that same ARN.
#[tokio::test]
async fn a_read_and_the_listing_agree_about_every_member_they_both_report() {
    let router = emulator();
    let mut arns: Vec<(&str, String)> = Vec::new();
    for ReadCase { case, request } in read_cases() {
        arns.push((case, create(&router, &request).await));
    }

    let listed = ok(
        &router,
        "ListProcurementPortalPreferences",
        &json!({"MaxResults": 100}),
    )
    .await;
    let summaries = listed
        .get("ProcurementPortalPreferences")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ProcurementPortalPreferences in {listed}"))
        .clone();
    assert_eq!(
        summaries.len(),
        arns.len(),
        "expected one summary per created preference ({}), got {}",
        arns.len(),
        summaries.len()
    );

    for (case, arn) in &arns {
        let summary = summaries
            .iter()
            .find(|summary| {
                summary
                    .get("ProcurementPortalPreferenceArn")
                    .and_then(Value::as_str)
                    == Some(arn.as_str())
            })
            .unwrap_or_else(|| panic!("case {case:?}: expected {arn:?} in {listed}"));
        let read = get(&router, arn).await;
        for (member, summarised) in summary
            .as_object()
            .expect("a summary is a structure")
            .iter()
        {
            let Some(reported) = read.get(member) else {
                continue;
            };
            assert_eq!(
                normalized(reported),
                normalized(summarised),
                "case {case:?}: the listing and the read disagree about {member}: \
                 the read says {reported}, the listing says {summarised}"
            );
        }
    }
}

/// A read of an ARN this service does not hold is the modelled 404, whatever well-formed ARN
/// is asked for — never a preference invented for it.
#[tokio::test]
async fn a_read_of_a_preference_the_service_does_not_hold_is_the_modelled_404() {
    let router = emulator();
    // One preference exists, so the 404 is about the ARN asked for rather than about an
    // empty registry.
    let held = create(
        &router,
        &json!({
            "ProcurementPortalName": "COUPA",
            "BuyerDomain": "NetworkID",
            "BuyerIdentifier": "held-buyer",
            "SupplierDomain": "NetworkID",
            "SupplierIdentifier": "held-supplier",
            "EinvoiceDeliveryEnabled": true,
            "PurchaseOrderRetrievalEnabled": true,
            "Contacts": contacts("held"),
        }),
    )
    .await;

    for case in [
        "arn:aws:invoicing::000000000000:procurement-portal-preference/ffffffffffff",
        "arn:aws:invoicing::000000000000:procurement-portal-preference/deleted-1",
        "arn:aws:invoicing::000000000000:procurement-portal-preference/A",
    ] {
        assert_ne!(
            case, held,
            "case {case:?}: the table must name an ARN the service does not hold"
        );
        let (status, parsed) = call(
            &router,
            "GetProcurementPortalPreference",
            &json!({"ProcurementPortalPreferenceArn": case}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {parsed}"
        );
        let code = error_code(&parsed);
        assert!(
            code.ends_with("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {code:?} {parsed}"
        );
        assert!(
            error_message(&parsed).contains(case),
            "case {case:?}: expected a message naming the ARN asked about, got {parsed}"
        );
    }
}
