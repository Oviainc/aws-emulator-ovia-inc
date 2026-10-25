// SPDX-License-Identifier: CC0-1.0

//! What a REFUSED call of the procurement-portal-preference plane leaves behind, driven
//! end to end over this service's wire protocol (`POST /` with
//! `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body, through the same
//! `MultiServiceRouter` the production router uses).
//!
//! The rule under test: `ProcurementPortalPreference$Version` is "the version number of
//! the procurement portal preference configuration" and `LastUpdateDate` is "the date and
//! time when the procurement portal preference was last updated"
//! (`smithy_query(query="shape", shape="ProcurementPortalPreference")`), so a call that
//! answers an error wrote no configuration and leaves BOTH exactly as the last successful
//! write left them — however many times it is repeated. The version a reader sees is
//! therefore one plus the number of calls that successfully wrote the resource, and no
//! refusal of any operation on this plane shifts it.
//!
//! Each test is a table rather than one request: the rows span the refusal classes this
//! plane answers (the two validation calls, the configuration update, the status update
//! and a conflicting create — 400 and 409 alike), the portals a preference can route to,
//! and the ORDER refusals and successful writes arrive in.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// Every value `ProcurementPortalName` accepts (`smithy_query(query="shape",
/// shape="ProcurementPortalName")`).
const PORTAL_NAMES: &[&str] = &["ZIP", "SAP_BUSINESS_NETWORK", "JAGGAER", "COUPA"];

/// The portal whose Marketplace PunchOut configuration the model documents as required
/// (`MarketplacePunchOutPreference`: "Required for Coupa when MarketplacePunchOutEnabled
/// is true"), which is what makes one row's update refusable.
const PORTAL_REQUIRING_PUNCH_OUT_CONFIG: &str = "COUPA";

/// A status every plane of a preference may hold: one of the values
/// `ProcurementPortalPreferenceStatus` admits (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`).
const STATUS_ACTIVE: &str = "ACTIVE";

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

/// The wire error code of a failure body (`__type` on the awsJson protocols), without the
/// namespace a backend stamps in front of it.
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

/// One contact, the single element `Contacts` (`@length(min: 1, max: 1)`) admits.
fn contact(name: &str) -> Value {
    json!([{ "Name": name, "Email": format!("{}@example.com", name.to_lowercase().replace(' ', ".")) }])
}

/// A create request for the portal and buyer/supplier pair that identify a preference,
/// carrying only the members the model marks required.
fn minimal_create(portal: &str, buyer: &str, supplier: &str) -> Value {
    json!({
        "ProcurementPortalName": portal,
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": buyer,
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": supplier,
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
        "Contacts": contact("Initial Contact"),
    })
}

/// The preference `arn` names, as `GetProcurementPortalPreference` reports it.
async fn get_preference(router: &MultiServiceRouter, arn: &str) -> Value {
    let body = ok(
        router,
        "GetProcurementPortalPreference",
        &json!({"ProcurementPortalPreferenceArn": arn}),
    )
    .await;
    body.get("ProcurementPortalPreference")
        .cloned()
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreference in {body}"))
}

/// The summary `ListProcurementPortalPreferences` reports for `arn`, so a refusal is
/// checked against both readers of this plane rather than only the single-resource one.
async fn listed_summary(router: &MultiServiceRouter, arn: &str) -> Value {
    let body = ok(router, "ListProcurementPortalPreferences", &json!({})).await;
    body.get("ProcurementPortalPreferences")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ProcurementPortalPreferences in {body}"))
        .iter()
        .find(|summary| {
            summary
                .get("ProcurementPortalPreferenceArn")
                .and_then(Value::as_str)
                == Some(arn)
        })
        .cloned()
        .unwrap_or_else(|| panic!("expected a summary for {arn} in {body}"))
}

/// The reported `Version` of a preference or its summary.
fn version(preference: &Value) -> i64 {
    preference
        .get("Version")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected a Version in {preference}"))
}

/// The reported `LastUpdateDate`, as the epoch-seconds number it travels as.
fn last_update(preference: &Value) -> f64 {
    preference
        .get("LastUpdateDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a LastUpdateDate in {preference}"))
}

/// The preference one row drives its calls against: its ARN and the identity it holds,
/// because a row's request can name either.
struct Subject {
    arn: String,
    portal: &'static str,
    buyer: String,
    supplier: String,
}

/// One class of refused request: which operation refuses it, the request built from the
/// subject preference, and the modelled answer it is refused with.
struct Refusal {
    label: &'static str,
    /// The portal the subject preference routes to, so the rows span the catalog and the
    /// update row reaches the portal whose configuration requirement refuses it.
    portal: &'static str,
    operation: &'static str,
    request: fn(&Subject) -> Value,
    http_status: u16,
    error_code: &'static str,
}

/// A valid configuration update: every member `PutProcurementPortalPreference` marks
/// required (`smithy_query(query="operation",
/// operation="PutProcurementPortalPreference")`), so this one SUCCEEDS.
fn accepted_update(subject: &Subject) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": subject.arn,
        "Contacts": contact("Updated Contact"),
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
    })
}

/// A configuration update that enables Marketplace PunchOut without the configuration the
/// subject's portal requires for it.
fn update_missing_required_punch_out_config(subject: &Subject) -> Value {
    let mut request = accepted_update(subject);
    request["MarketplacePunchOutEnabled"] = json!(true);
    request
}

/// A validation start for the subject preference.
fn send_validation(subject: &Subject) -> Value {
    json!({"ProcurementPortalPreferenceArn": subject.arn})
}

/// The same start, carrying a `ClientToken` — there is no success for a replay of a
/// refused call to answer with.
fn send_validation_with_token(subject: &Subject) -> Value {
    json!({"ProcurementPortalPreferenceArn": subject.arn, "ClientToken": "refused-send-1"})
}

/// A code submitted for the subject preference while none is outstanding.
fn verify_unissued_code(subject: &Subject) -> Value {
    json!({"ProcurementPortalPreferenceArn": subject.arn, "Code": "123456"})
}

/// The same submission with a code of another shape, so nothing here is keyed to one
/// code's form.
fn verify_unissued_code_of_another_shape(subject: &Subject) -> Value {
    json!({"ProcurementPortalPreferenceArn": subject.arn, "Code": "a-code-nothing-issued"})
}

/// A status update that states no plane's status, so it asks for no update.
fn status_update_stating_nothing(subject: &Subject) -> Value {
    json!({"ProcurementPortalPreferenceArn": subject.arn})
}

/// A status update that states a plane's reason without that plane's status.
fn status_update_stating_a_reason_only(subject: &Subject) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": subject.arn,
        "PurchaseOrderRetrievalPreferenceStatusReason": "a reason for nothing",
    })
}

/// A status update naming a status outside the modelled enum.
fn status_update_outside_the_enum(subject: &Subject) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": subject.arn,
        "EinvoiceDeliveryPreferenceStatus": "NOT_A_STATUS",
    })
}

/// A create of the identity the subject preference already holds, which is the conflict a
/// second create of one resource is.
fn conflicting_create(subject: &Subject) -> Value {
    minimal_create(subject.portal, &subject.buyer, &subject.supplier)
}

/// Every refusal class this plane answers for a preference that EXISTS, one row each.
fn refusal_classes() -> Vec<Refusal> {
    vec![
        Refusal {
            label: "a validation start that cannot confirm delivery",
            portal: PORTAL_NAMES[0],
            operation: "SendProcurementPortalValidation",
            request: send_validation,
            http_status: 400,
            error_code: "ValidationException",
        },
        Refusal {
            label: "the same start carrying a ClientToken",
            portal: PORTAL_NAMES[1],
            operation: "SendProcurementPortalValidation",
            request: send_validation_with_token,
            http_status: 400,
            error_code: "ValidationException",
        },
        Refusal {
            label: "a code submitted while none is outstanding",
            portal: PORTAL_NAMES[2],
            operation: "VerifyProcurementPortalValidation",
            request: verify_unissued_code,
            http_status: 400,
            error_code: "ValidationException",
        },
        Refusal {
            label: "a submitted code of another shape",
            portal: PORTAL_NAMES[0],
            operation: "VerifyProcurementPortalValidation",
            request: verify_unissued_code_of_another_shape,
            http_status: 400,
            error_code: "ValidationException",
        },
        Refusal {
            label: "a status update stating no plane",
            portal: PORTAL_NAMES[1],
            operation: "UpdateProcurementPortalPreferenceStatus",
            request: status_update_stating_nothing,
            http_status: 400,
            error_code: "ValidationException",
        },
        Refusal {
            label: "a status update stating a reason without its status",
            portal: PORTAL_NAMES[2],
            operation: "UpdateProcurementPortalPreferenceStatus",
            request: status_update_stating_a_reason_only,
            http_status: 400,
            error_code: "ValidationException",
        },
        Refusal {
            label: "a status update naming a status outside the modelled enum",
            portal: PORTAL_NAMES[3],
            operation: "UpdateProcurementPortalPreferenceStatus",
            request: status_update_outside_the_enum,
            http_status: 400,
            error_code: "ValidationException",
        },
        Refusal {
            label: "a configuration update missing the configuration its portal requires",
            portal: PORTAL_REQUIRING_PUNCH_OUT_CONFIG,
            operation: "PutProcurementPortalPreference",
            request: update_missing_required_punch_out_config,
            http_status: 400,
            error_code: "ValidationException",
        },
        Refusal {
            label: "a create of the identity this preference already holds",
            portal: PORTAL_NAMES[0],
            operation: "CreateProcurementPortalPreference",
            request: conflicting_create,
            http_status: 409,
            error_code: "ConflictException",
        },
    ]
}

/// A preference of this row's own, carrying one successful configuration update on top of
/// the create — so the version under test is not the one a create happens to leave.
async fn subject_at_version_two(
    router: &MultiServiceRouter,
    index: usize,
    portal: &'static str,
) -> Subject {
    let buyer = format!("BUYER-REFUSED-{index}");
    let supplier = format!("SUPPLIER-REFUSED-{index}");
    let arn = arn_of(
        &ok(
            router,
            "CreateProcurementPortalPreference",
            &minimal_create(portal, &buyer, &supplier),
        )
        .await,
    );
    let subject = Subject {
        arn,
        portal,
        buyer,
        supplier,
    };
    ok(
        router,
        "PutProcurementPortalPreference",
        &accepted_update(&subject),
    )
    .await;
    subject
}

/// RULE: a call that answers an error writes no configuration, so it leaves every member
/// a reader reports — `Version` and `LastUpdateDate` included — exactly as the last
/// SUCCESSFUL write left them, however many times that call is repeated.
///
/// The rows are the refusal classes this plane answers for a preference that exists: the
/// two validation calls, the status update's three, the configuration update's, and a
/// conflicting create (a 409, so the rule is not read as being about 400 alone), spread
/// across the portals `ProcurementPortalName` admits.
#[tokio::test]
async fn a_refused_call_leaves_the_preference_where_the_last_successful_write_left_it() {
    let router = emulator();

    for (index, refusal) in refusal_classes().iter().enumerate() {
        let subject = subject_at_version_two(&router, index, refusal.portal).await;
        let before = get_preference(&router, &subject.arn).await;
        let before_summary = listed_summary(&router, &subject.arn).await;
        assert_eq!(
            version(&before),
            2,
            "case {}: the create and one update leave the second version, got {before}",
            refusal.label
        );

        // Repeated, because the defect this pins showed up as a counter that drifted one
        // step per refused call: three refusals are three times nothing, not three writes.
        for round in 1..=3 {
            let request = (refusal.request)(&subject);
            let (status, body) = call(&router, refusal.operation, &request).await;
            assert_eq!(
                status, refusal.http_status,
                "case {} round {round}: expected HTTP {}, got {status} {body}",
                refusal.label, refusal.http_status
            );
            assert_eq!(
                error_code(&body),
                refusal.error_code,
                "case {} round {round}: expected the modelled {}, got {body}",
                refusal.label,
                refusal.error_code
            );

            let after = get_preference(&router, &subject.arn).await;
            assert_eq!(
                after, before,
                "case {} round {round}: a refused call must leave every reported member as \
                 it was, got {after}",
                refusal.label
            );
            let after_summary = listed_summary(&router, &subject.arn).await;
            assert_eq!(
                after_summary, before_summary,
                "case {} round {round}: the listing must report the same summary, got \
                 {after_summary}",
                refusal.label
            );
        }
    }
}

/// One step of a scripted sequence against one preference.
struct Step {
    label: &'static str,
    operation: &'static str,
    request: fn(&Subject) -> Value,
    /// Whether this call SUCCEEDS and writes the configuration — which is exactly when
    /// the reported version advances.
    writes: bool,
}

/// A status update that moves one plane to a status the modelled enum admits, so it
/// SUCCEEDS.
fn accepted_status_update(subject: &Subject) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": subject.arn,
        "EinvoiceDeliveryPreferenceStatus": STATUS_ACTIVE,
    })
}

/// The steps a sequence is built from, each named once here so a row is just an order.
fn step(label: &'static str) -> Step {
    match label {
        "refused send" => Step {
            label,
            operation: "SendProcurementPortalValidation",
            request: send_validation,
            writes: false,
        },
        "refused verify" => Step {
            label,
            operation: "VerifyProcurementPortalValidation",
            request: verify_unissued_code,
            writes: false,
        },
        "refused status update" => Step {
            label,
            operation: "UpdateProcurementPortalPreferenceStatus",
            request: status_update_stating_nothing,
            writes: false,
        },
        "accepted update" => Step {
            label,
            operation: "PutProcurementPortalPreference",
            request: accepted_update,
            writes: true,
        },
        "accepted status update" => Step {
            label,
            operation: "UpdateProcurementPortalPreferenceStatus",
            request: accepted_status_update,
            writes: true,
        },
        other => panic!("case {other}: no such step"),
    }
}

/// RULE: the reported `Version` is one plus the number of calls that SUCCESSFULLY wrote
/// the preference, and `LastUpdateDate` moves only with those calls — so interleaving
/// refusals with writes, in any order, cannot shift either.
///
/// The rows are orders in which refusals and successful writes arrive: refusals first,
/// refusals between the writes, and a sequence with no successful write at all.
#[tokio::test]
async fn the_version_counts_the_successful_writes_only() {
    let router = emulator();

    let sequences: Vec<(&str, Vec<&str>)> = vec![
        (
            "refusals before the writes",
            vec![
                "refused send",
                "refused verify",
                "refused status update",
                "accepted update",
                "accepted status update",
            ],
        ),
        (
            "refusals between the writes",
            vec![
                "accepted update",
                "refused send",
                "accepted status update",
                "refused verify",
                "refused send",
                "accepted update",
            ],
        ),
        (
            "nothing but refusals",
            vec![
                "refused send",
                "refused send",
                "refused verify",
                "refused status update",
            ],
        ),
    ];

    for (index, (label, sequence)) in sequences.iter().enumerate() {
        let portal = PORTAL_NAMES[index % PORTAL_NAMES.len()];
        let buyer = format!("BUYER-SEQUENCE-{index}");
        let supplier = format!("SUPPLIER-SEQUENCE-{index}");
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &minimal_create(portal, &buyer, &supplier),
            )
            .await,
        );
        let subject = Subject {
            arn,
            portal,
            buyer,
            supplier,
        };

        // The create left the first version; every later successful write adds one.
        let mut successful_writes = 0;
        let mut reported = get_preference(&router, &subject.arn).await;
        assert_eq!(
            version(&reported),
            1,
            "case {label}: a create leaves the first version, got {reported}"
        );

        for step_label in sequence {
            let step = step(step_label);
            let previous_last_update = last_update(&reported);
            let request = (step.request)(&subject);
            let (status, body) = call(&router, step.operation, &request).await;
            if step.writes {
                successful_writes += 1;
                assert_eq!(
                    status, 200,
                    "case {label} / {}: expected HTTP 200, got {status} {body}",
                    step.label
                );
            } else {
                assert!(
                    status >= 400,
                    "case {label} / {}: expected a refusal, got {status} {body}",
                    step.label
                );
            }

            reported = get_preference(&router, &subject.arn).await;
            let expected_version = 1 + successful_writes;
            assert_eq!(
                version(&reported),
                expected_version,
                "case {label} / {}: expected Version {expected_version} after \
                 {successful_writes} successful writes, got {reported}",
                step.label
            );
            if step.writes {
                assert!(
                    last_update(&reported) >= previous_last_update,
                    "case {label} / {}: a successful write stamps the instant it changed \
                     at, got {reported}",
                    step.label
                );
            } else {
                assert_eq!(
                    last_update(&reported),
                    previous_last_update,
                    "case {label} / {}: a refused call must not restamp LastUpdateDate, \
                     got {reported}",
                    step.label
                );
            }
            // The listing reports the same resource, so it reports the same version.
            let summary = listed_summary(&router, &subject.arn).await;
            assert_eq!(
                version(&summary),
                expected_version,
                "case {label} / {}: the listing reports the version the read does, got \
                 {summary}",
                step.label
            );
        }
    }
}
