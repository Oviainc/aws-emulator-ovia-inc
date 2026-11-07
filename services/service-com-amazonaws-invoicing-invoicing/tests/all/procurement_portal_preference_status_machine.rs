// SPDX-License-Identifier: CC0-1.0

//! The STATUS MACHINE of a procurement portal preference, driven end to end over this
//! service's wire protocol through the same `MultiServiceRouter` the production router
//! uses, with the shared harness of `refused_procurement_portal_writes` rather than a copy
//! of it. The one thing read out of state is the issued CODE, which no modelled response
//! carries.
//!
//! The rules under test, each spanned by a table rather than one request:
//!
//! * `SendProcurementPortalValidation` issues the validation code and then reports the
//!   DELIVERY of the test e-invoice carrying it, which this emulator makes to no
//!   procurement portal: the call names the delivery target it could not reach — the test
//!   environment's endpoint when the stated connection-testing method replays into that
//!   environment, the portal's own delivery endpoint otherwise — and states that the
//!   delivery was not confirmed for a preference that configures neither. The code is
//!   outstanding either way, which is what makes that refusal's own remedy work: a caller
//!   that finds the code in the portal's rejected-invoice log submits it and completes the
//!   verification.
//! * `UpdateProcurementPortalPreferenceStatus` applies only a move a CALLER may ask for: a
//!   status the service assigns itself, and a pair the lifecycle does not provide, are
//!   refused with the modelled `ValidationException` whose message states the detail the
//!   request carried and every rule that detail violates — and the refusal writes nothing.
//! * what a reader reports after such a lifecycle is what the ACCEPTED calls left: the
//!   status only an accepted transition produced, and a `Version` counting one per applied
//!   write — the validation writes among them, and nothing for a call refused before it
//!   wrote.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::refused_procurement_portal_writes::{
    accepted_update_round, arn_of, call, create_with_delivery_endpoint, emulator_with_engine,
    error_code, error_text, get_preference, last_update, minimal_create, ok, outstanding_code,
    shifted, undeliverable_sentence, validate_preference, version, without_write_markers,
    DELIVERY_NOT_CONFIRMED, PORTAL_INSTANCE_ENDPOINT, PORTAL_NAMES,
};

/// The statuses of `ProcurementPortalPreferenceStatus` this table names
/// (`smithy_query(query="shape", shape="ProcurementPortalPreferenceStatus")` lists all
/// seven: `PENDING_VERIFICATION`, `VALIDATED`, `TEST_INITIALIZED`,
/// `TEST_INITIALIZATION_FAILED`, `TEST_FAILED`, `ACTIVE`, `SUSPENDED`).
const PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";
const VALIDATED: &str = "VALIDATED";
const TEST_INITIALIZED: &str = "TEST_INITIALIZED";
const TEST_INITIALIZATION_FAILED: &str = "TEST_INITIALIZATION_FAILED";
const TEST_FAILED: &str = "TEST_FAILED";
const ACTIVE: &str = "ACTIVE";
const SUSPENDED: &str = "SUSPENDED";

/// Both values `ConnectionTestingMethod` admits — "the method to use for testing the
/// connection to the procurement portal" (`smithy_query(query="shape",
/// shape="ConnectionTestingMethod,EinvoiceDeliveryPreference")`).
const PROD_ENV_DOLLAR_TEST: &str = "PROD_ENV_DOLLAR_TEST";
const TEST_ENV_REPLAY_TEST: &str = "TEST_ENV_REPLAY_TEST";

/// The endpoint the test environment of a portal delivers e-invoices to
/// (`TestEnvPreference$ProcurementPortalInstanceEndpoint`), inside that member's `^\S+$`.
const TEST_ENV_ENDPOINT: &str = "https://test.portal.example.com/cxml";

/// The two planes of a preference, each as the model spells its status members, with the
/// key the refusal's echoed detail carries them under.
const PLANES: &[(&str, &str, &str, &str, &str)] = &[
    (
        "e-invoice delivery",
        "EinvoiceDeliveryPreferenceStatus",
        "EinvoiceDeliveryPreferenceStatusReason",
        "einvoiceDeliveryPreferenceStatus",
        "einvoiceDeliveryPreferenceStatusReason",
    ),
    (
        "purchase order retrieval",
        "PurchaseOrderRetrievalPreferenceStatus",
        "PurchaseOrderRetrievalPreferenceStatusReason",
        "purchaseOrderRetrievalPreferenceStatus",
        "purchaseOrderRetrievalPreferenceStatusReason",
    ),
];

/// An `EinvoiceDeliveryPreference` stating `method` as its connection-testing method, with
/// every other member the model marks required on it (`smithy_query(query="shape",
/// shape="EinvoiceDeliveryPreference")`).
fn einvoice_delivery_preference(method: &str) -> Value {
    json!({
        "EinvoiceDeliveryDocumentTypes": ["AWS_CLOUD_INVOICE"],
        "Protocol": "CXML",
        "PurchaseOrderDataSources": [{
            "EinvoiceDeliveryDocumentType": "AWS_CLOUD_INVOICE",
            "PurchaseOrderDataSourceType": "PURCHASE_ORDER_NOT_REQUIRED",
        }],
        "ConnectionTestingMethod": method,
        "EinvoiceDeliveryActivationDate": 1_767_225_600,
    })
}

/// A `TestEnvPreference` configuring the test environment's own delivery endpoint, with
/// every member the model marks required on it (`smithy_query(query="shape",
/// shape="TestEnvPreference")`).
fn test_env_preference() -> Value {
    json!({
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": "AN0TEST000001",
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": "AN0TEST000002",
        "ProcurementPortalInstanceEndpoint": TEST_ENV_ENDPOINT,
    })
}

/// One row of the delivery table: what the preference configures, and the endpoint the
/// test e-invoice of its connection test is delivered TO under that configuration — `None`
/// for a configuration that names no such endpoint at all.
struct DeliveryRow {
    label: &'static str,
    /// The connection-testing method stated, or `None` for a preference whose e-invoice
    /// delivery configuration states none.
    method: Option<&'static str>,
    /// Whether the portal's own delivery endpoint is configured.
    portal_endpoint: bool,
    /// Whether the test environment's delivery endpoint is configured.
    test_env_endpoint: bool,
    target: Option<&'static str>,
}

/// The rows: both connection-testing methods and no method at all, crossed with the
/// endpoints a preference can configure — so each method is asserted both with the
/// endpoint its test document goes to and with only the other one.
fn delivery_rows() -> Vec<DeliveryRow> {
    vec![
        DeliveryRow {
            label: "no method, no endpoint",
            method: None,
            portal_endpoint: false,
            test_env_endpoint: false,
            target: None,
        },
        DeliveryRow {
            label: "no method, the portal endpoint",
            method: None,
            portal_endpoint: true,
            test_env_endpoint: false,
            target: Some(PORTAL_INSTANCE_ENDPOINT),
        },
        DeliveryRow {
            label: "no method, the test environment endpoint only",
            method: None,
            portal_endpoint: false,
            test_env_endpoint: true,
            target: None,
        },
        DeliveryRow {
            label: "a dollar test in production, the portal endpoint",
            method: Some(PROD_ENV_DOLLAR_TEST),
            portal_endpoint: true,
            test_env_endpoint: false,
            target: Some(PORTAL_INSTANCE_ENDPOINT),
        },
        DeliveryRow {
            label: "a dollar test in production, the test environment endpoint only",
            method: Some(PROD_ENV_DOLLAR_TEST),
            portal_endpoint: false,
            test_env_endpoint: true,
            target: None,
        },
        DeliveryRow {
            label: "a replay in the test environment, its endpoint",
            method: Some(TEST_ENV_REPLAY_TEST),
            portal_endpoint: false,
            test_env_endpoint: true,
            target: Some(TEST_ENV_ENDPOINT),
        },
        DeliveryRow {
            label: "a replay in the test environment, the portal endpoint only",
            method: Some(TEST_ENV_REPLAY_TEST),
            portal_endpoint: true,
            test_env_endpoint: false,
            target: None,
        },
        DeliveryRow {
            label: "a replay in the test environment, both endpoints",
            method: Some(TEST_ENV_REPLAY_TEST),
            portal_endpoint: true,
            test_env_endpoint: true,
            target: Some(TEST_ENV_ENDPOINT),
        },
    ]
}

/// RULE: the call that starts a portal validation issues the validation code and reports
/// the DELIVERY of the test e-invoice carrying it — which this emulator makes to no
/// procurement portal, so the call is refused rather than claiming a delivery nothing
/// performed: it names the delivery target (the endpoint the document goes to, the test
/// environment's own when the stated connection-testing method replays into that
/// environment and the portal's delivery endpoint otherwise, with the portal's cXML invoice
/// path under it) when the preference configures that endpoint, and states that the
/// delivery was not confirmed when it configures none. The code it issued is outstanding
/// either way: submitting the code the refusal tells the caller to retrieve completes the
/// verification.
#[tokio::test]
async fn a_send_reports_the_delivery_of_the_test_einvoice_it_could_not_make() {
    let (router, engine) = emulator_with_engine();

    for (index, row) in delivery_rows().iter().enumerate() {
        let mut create = minimal_create(
            PORTAL_NAMES[index % PORTAL_NAMES.len()],
            &format!("BUYER-DELIVERY-{index}"),
            &format!("SUPPLIER-DELIVERY-{index}"),
        );
        if let Some(method) = row.method {
            create["EinvoiceDeliveryPreference"] = einvoice_delivery_preference(method);
        }
        if row.portal_endpoint {
            create["ProcurementPortalInstanceEndpoint"] = json!(PORTAL_INSTANCE_ENDPOINT);
        }
        if row.test_env_endpoint {
            create["TestEnvPreference"] = test_env_preference();
        }
        let arn = arn_of(&ok(&router, "CreateProcurementPortalPreference", &create).await);
        let before = get_preference(&router, &arn).await;

        let (status, body) = call(
            &router,
            "SendProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {}: a delivery the service could not make is refused, got {status} {body}",
            row.label
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {}: expected the modelled ValidationException, got {body}",
            row.label
        );
        let expected = match row.target {
            Some(endpoint) => undeliverable_sentence(endpoint),
            None => DELIVERY_NOT_CONFIRMED.to_string(),
        };
        assert_eq!(
            error_text(&body),
            expected,
            "case {}: the refusal carries the sentence this error class states, got {body}",
            row.label
        );

        // The code is issued whatever the call ANSWERED, which is what the refusal's own
        // remedy rests on — and issuing it is one write of the resource, so the version
        // every applied write advances moved by one and no configuration member did.
        let issued = outstanding_code(&engine, &arn).unwrap_or_else(|| {
            panic!(
                "case {}: the send issues the code the flow runs on",
                row.label
            )
        });
        let after = get_preference(&router, &arn).await;
        assert_eq!(
            without_write_markers(&after),
            without_write_markers(&before),
            "case {}: issuing a code writes no configuration, got {after}",
            row.label
        );
        assert_eq!(
            version(&after),
            version(&before) + 1,
            "case {}: the issued code is one version of the resource, got {after}",
            row.label
        );

        // Submitting that code completes the verification, so the flow the refusal points
        // at really does complete.
        let completed = ok(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn, "Code": issued}),
        )
        .await;
        assert_eq!(
            arn_of(&completed),
            arn,
            "case {}: the code the send issued completes the verification, got {completed}",
            row.label
        );
        let verified = get_preference(&router, &arn).await;
        assert_eq!(
            verified
                .get("EinvoiceDeliveryPreferenceStatus")
                .and_then(Value::as_str),
            Some(VALIDATED),
            "case {}: the completed verification carries the plane off the unverified \
             status, got {verified}",
            row.label
        );
    }
}

/// How a caller-requested move can be refused: by the status it asks for being one the
/// service assigns itself, by the lifecycle not providing the move, or by both.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Violation {
    OnlyAws,
    NotAllowed,
}

/// One row of the transition table: the status the plane is carried to first, the move the
/// request then asks for, and the rules that move violates (empty for a move a caller may
/// make).
struct TransitionRow {
    label: &'static str,
    /// Which plane the request states: an index into [`PLANES`].
    plane: usize,
    /// The status that plane holds when the request arrives.
    held: &'static str,
    /// The status the request asks for.
    requested: &'static str,
    /// The reason stated with it, or `None` for a request that states none — the echoed
    /// detail carries one exactly when the request did.
    reason: Option<&'static str>,
    violations: &'static [Violation],
}

/// The rows: out of each status a caller can reach, the moves the lifecycle provides and
/// the moves it does not — including every status the service assigns itself, so each rule
/// is asserted on its own and both rules together.
fn transition_rows() -> Vec<TransitionRow> {
    vec![
        TransitionRow {
            label: "the suspension a caller asked for, out of the unverified status",
            plane: 0,
            held: PENDING_VERIFICATION,
            requested: SUSPENDED,
            reason: Some("a suspension the caller asked for"),
            violations: &[Violation::OnlyAws, Violation::NotAllowed],
        },
        TransitionRow {
            label: "the activation a caller asked for, out of the unverified status",
            plane: 0,
            held: PENDING_VERIFICATION,
            requested: ACTIVE,
            reason: None,
            violations: &[Violation::NotAllowed],
        },
        TransitionRow {
            label: "the validated status a caller asked for, which the validation sets",
            plane: 1,
            held: PENDING_VERIFICATION,
            requested: VALIDATED,
            reason: Some("validated by the caller"),
            violations: &[Violation::OnlyAws, Violation::NotAllowed],
        },
        TransitionRow {
            label: "the unverified status restated, which asks for no move",
            plane: 0,
            held: PENDING_VERIFICATION,
            requested: PENDING_VERIFICATION,
            reason: Some("restated for the status it holds"),
            violations: &[],
        },
        TransitionRow {
            label: "the connection test asked for once the connection is validated",
            plane: 0,
            held: VALIDATED,
            requested: TEST_INITIALIZED,
            reason: Some("the caller asked for the connection test"),
            violations: &[],
        },
        TransitionRow {
            label: "the same move on the purchase-order plane",
            plane: 1,
            held: VALIDATED,
            requested: TEST_INITIALIZED,
            reason: None,
            violations: &[],
        },
        TransitionRow {
            label: "the activation asked for before the test ran",
            plane: 0,
            held: VALIDATED,
            requested: ACTIVE,
            reason: None,
            violations: &[Violation::NotAllowed],
        },
        TransitionRow {
            label: "the test failure a caller asked for",
            plane: 0,
            held: VALIDATED,
            requested: TEST_FAILED,
            reason: Some("the caller called the test failed"),
            violations: &[Violation::OnlyAws, Violation::NotAllowed],
        },
        TransitionRow {
            label: "the initialization failure a caller asked for",
            plane: 1,
            held: VALIDATED,
            requested: TEST_INITIALIZATION_FAILED,
            reason: None,
            violations: &[Violation::OnlyAws, Violation::NotAllowed],
        },
        TransitionRow {
            label: "the activation asked for once the test was initialized",
            plane: 0,
            held: TEST_INITIALIZED,
            requested: ACTIVE,
            reason: Some("activated after the test"),
            violations: &[],
        },
        TransitionRow {
            label: "the unverified status a caller asked for again",
            plane: 0,
            held: TEST_INITIALIZED,
            requested: PENDING_VERIFICATION,
            reason: None,
            violations: &[Violation::OnlyAws, Violation::NotAllowed],
        },
        TransitionRow {
            label: "the suspension of what the caller activated",
            plane: 0,
            held: ACTIVE,
            requested: SUSPENDED,
            reason: Some("the caller wanted it suspended"),
            violations: &[Violation::OnlyAws, Violation::NotAllowed],
        },
        TransitionRow {
            label: "the activation restated, which asks for no move",
            plane: 0,
            held: ACTIVE,
            requested: ACTIVE,
            reason: None,
            violations: &[],
        },
    ]
}

/// The statuses a caller can carry a plane to, in the order the lifecycle reaches them:
/// a completed validation leaves the first, and each of the others is one update.
const CALLER_REACHABLE: &[&str] = &[VALIDATED, TEST_INITIALIZED, ACTIVE];

/// A preference whose `plane` holds `held`, reached the way a caller reaches it: a create
/// with both planes enabled, then (for every status past the unverified one) a completed
/// validation and one update per further move.
async fn preference_holding(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    index: usize,
    plane: usize,
    held: &str,
) -> String {
    let (_, status_member, _, _, _) = PLANES[plane];
    let arn = arn_of(
        &ok(
            router,
            "CreateProcurementPortalPreference",
            &create_with_delivery_endpoint(
                PORTAL_NAMES[index % PORTAL_NAMES.len()],
                &format!("BUYER-MACHINE-{index}"),
                &format!("SUPPLIER-MACHINE-{index}"),
            ),
        )
        .await,
    );
    if held == PENDING_VERIFICATION {
        return arn;
    }
    validate_preference(router, engine, &arn).await;
    let target = CALLER_REACHABLE
        .iter()
        .position(|reachable| *reachable == held)
        .unwrap_or_else(|| panic!("case {held}: a caller cannot carry a plane there"));
    for asked_for in &CALLER_REACHABLE[1..target + 1] {
        let mut update = json!({"ProcurementPortalPreferenceArn": arn});
        update[status_member] = json!(asked_for);
        ok(router, "UpdateProcurementPortalPreferenceStatus", &update).await;
    }
    arn
}

/// The detail a refusal echoes for a request stating ONE plane: the account the preference
/// belongs to, the preference the request named, and the status members it stated, in the
/// model's member order and spelled as the refusal spells them.
fn echoed_detail(arn: &str, plane: usize, requested: &str, reason: Option<&str>) -> String {
    let (_, _, _, status_key, reason_key) = PLANES[plane];
    let mut detail = format!(
        "{{\"awsAccountId\":\"{}\",\"procurementPortalPreferenceArn\":\"{arn}\",\
         \"{status_key}\":\"{requested}\"",
        engine::emulator_account_id()
    );
    if let Some(reason) = reason {
        detail.push_str(&format!(",\"{reason_key}\":\"{reason}\""));
    }
    detail.push('}');
    detail
}

/// The sentence a refused move states: the detail the request carried, then the rules that
/// detail is invalid because of, joined in the order they are stated.
fn invalid_detail_sentence(detail: &str, rules: &[String]) -> String {
    format!(
        "Given detail {detail} in request is invalid because [{}]",
        rules.join(", ")
    )
}

/// One violated rule as the refusal words it.
fn rule(violation: Violation, status_member: &str, held: &str, requested: &str) -> String {
    match violation {
        Violation::OnlyAws => {
            format!("Transition of {status_member} from {held} to {requested} can only be done by AWS")
        }
        Violation::NotAllowed => {
            format!("Transition of {status_member} from {held} to {requested} is not allowed")
        }
    }
}

/// RULE: a status update applies only a move a CALLER may ask for — the status asked for
/// has to be one a caller states at all, and the pair has to be a move the lifecycle
/// provides — and a request that asks for any other move is refused with the modelled
/// `ValidationException` whose message states the detail the request carried and every rule
/// that detail violates, writing nothing at all.
#[tokio::test]
async fn a_status_update_applies_only_the_moves_a_caller_may_ask_for() {
    let (router, engine) = emulator_with_engine();

    for (index, row) in transition_rows().iter().enumerate() {
        let (plane, status_member, reason_member, _, _) = PLANES[row.plane];
        let case = format!("{} ({plane}: {} -> {})", row.label, row.held, row.requested);
        let arn = preference_holding(&router, &engine, index, row.plane, row.held).await;
        let before = get_preference(&router, &arn).await;
        assert_eq!(
            before.get(status_member).and_then(Value::as_str),
            Some(row.held),
            "case {case}: the plane has to hold {} to start with, got {before}",
            row.held
        );

        let mut request = json!({"ProcurementPortalPreferenceArn": arn});
        request[status_member] = json!(row.requested);
        if let Some(reason) = row.reason {
            request[reason_member] = json!(reason);
        }
        let (status, body) = call(
            &router,
            "UpdateProcurementPortalPreferenceStatus",
            &request,
        )
        .await;

        if row.violations.is_empty() {
            assert_eq!(
                status, 200,
                "case {case}: a move a caller may ask for is applied, got {status} {body}"
            );
            let after = get_preference(&router, &arn).await;
            assert_eq!(
                after.get(status_member).and_then(Value::as_str),
                Some(row.requested),
                "case {case}: the plane takes the status the request stated, got {after}"
            );
            assert_eq!(
                after.get(reason_member).and_then(Value::as_str),
                row.reason,
                "case {case}: the plane reports the reason stated with it, got {after}"
            );
            assert_eq!(
                version(&after),
                version(&before) + 1,
                "case {case}: the applied write is one version of the resource, got {after}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case}: a move a caller may not ask for is refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected the modelled ValidationException, got {body}"
        );
        let rules: Vec<String> = row
            .violations
            .iter()
            .map(|violation| rule(*violation, status_member, row.held, row.requested))
            .collect();
        assert_eq!(
            error_text(&body),
            invalid_detail_sentence(
                &echoed_detail(&arn, row.plane, row.requested, row.reason),
                &rules
            ),
            "case {case}: the refusal states the detail and every rule it violates, got {body}"
        );

        // A refusal that never reached the write leaves the resource exactly as it was.
        let after = get_preference(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case}: a refused move writes nothing, got {after}"
        );
        assert_eq!(
            last_update(&after),
            last_update(&before),
            "case {case}: a refused move restamps nothing, got {after}"
        );
    }
}

/// RULE (both planes at once): a request stating a move for each plane is refused with ONE
/// message listing the rules of both, in the order the model declares the two status
/// members, and applies neither plane's move.
#[tokio::test]
async fn a_refusal_states_the_rules_of_every_plane_the_request_names() {
    let (router, engine) = emulator_with_engine();

    let arn = preference_holding(&router, &engine, 0, 0, VALIDATED).await;
    let before = get_preference(&router, &arn).await;

    let request = json!({
        "ProcurementPortalPreferenceArn": arn,
        "EinvoiceDeliveryPreferenceStatus": ACTIVE,
        "PurchaseOrderRetrievalPreferenceStatus": SUSPENDED,
        "PurchaseOrderRetrievalPreferenceStatusReason": "both planes at once",
    });
    let (status, body) = call(
        &router,
        "UpdateProcurementPortalPreferenceStatus",
        &request,
    )
    .await;
    assert_eq!(
        status, 400,
        "case both planes: a request whose moves a caller may not ask for is refused, got \
         {status} {body}"
    );

    let detail = format!(
        "{{\"awsAccountId\":\"{}\",\"procurementPortalPreferenceArn\":\"{arn}\",\
         \"einvoiceDeliveryPreferenceStatus\":\"{ACTIVE}\",\
         \"purchaseOrderRetrievalPreferenceStatus\":\"{SUSPENDED}\",\
         \"purchaseOrderRetrievalPreferenceStatusReason\":\"both planes at once\"}}",
        engine::emulator_account_id()
    );
    let rules = vec![
        rule(Violation::NotAllowed, PLANES[0].1, VALIDATED, ACTIVE),
        rule(Violation::OnlyAws, PLANES[1].1, VALIDATED, SUSPENDED),
        rule(Violation::NotAllowed, PLANES[1].1, VALIDATED, SUSPENDED),
    ];
    assert_eq!(
        error_text(&body),
        invalid_detail_sentence(&detail, &rules),
        "case both planes: the refusal states the detail and the rules of both planes, got \
         {body}"
    );
    let after = get_preference(&router, &arn).await;
    assert_eq!(
        after, before,
        "case both planes: a refused request applies neither plane's move, got {after}"
    );
}

/// One scripted call of a lifecycle: what to send, and whether the service applies a write
/// for it.
struct Call {
    label: &'static str,
    operation: &'static str,
    /// The request, built from the preference's ARN and the code outstanding for it.
    request: fn(&str, Option<&str>) -> Value,
    expected_status: u16,
    writes: bool,
}

fn a_configuration_update(arn: &str, _code: Option<&str>) -> Value {
    accepted_update_round(arn, 1)
}

fn a_validation_start(arn: &str, _code: Option<&str>) -> Value {
    json!({"ProcurementPortalPreferenceArn": arn})
}

fn the_outstanding_code(arn: &str, code: Option<&str>) -> Value {
    json!({"ProcurementPortalPreferenceArn": arn, "Code": code.unwrap_or("000000")})
}

fn a_wrong_code(arn: &str, code: Option<&str>) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": arn,
        "Code": code.map(shifted).unwrap_or_else(|| "000000".to_string()),
    })
}

fn a_caller_suspension(arn: &str, _code: Option<&str>) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": arn,
        "EinvoiceDeliveryPreferenceStatus": SUSPENDED,
        "EinvoiceDeliveryPreferenceStatusReason": "asked for by the caller",
    })
}

fn the_connection_test(arn: &str, _code: Option<&str>) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": arn,
        "EinvoiceDeliveryPreferenceStatus": TEST_INITIALIZED,
    })
}

/// RULE: what a reader reports after a lifecycle is what the ACCEPTED calls left — the
/// status only a transition the service applied produced, and a `Version` counting one per
/// APPLIED write (the validation writes among them, and nothing for a call the service
/// refused before writing).
///
/// The rows are lifecycles differing in which calls they mix and in what order: a
/// configuration write followed by refusals only, the accepted path through the validation
/// and the connection test, and a refusal between accepted writes.
#[tokio::test]
async fn a_read_back_reports_what_the_accepted_calls_left() {
    let (router, engine) = emulator_with_engine();

    // (label, whether the preference configures the endpoint its test e-invoice goes to,
    //  the calls, the status the e-invoice plane ends at)
    let lifecycles: Vec<(&str, bool, Vec<Call>, &str)> = vec![
        (
            "a configuration write, then three refusals",
            false,
            vec![
                Call {
                    label: "the configuration update",
                    operation: "PutProcurementPortalPreference",
                    request: a_configuration_update,
                    expected_status: 200,
                    writes: true,
                },
                Call {
                    label: "the send that has no endpoint to deliver to",
                    operation: "SendProcurementPortalValidation",
                    request: a_validation_start,
                    expected_status: 400,
                    writes: true,
                },
                Call {
                    label: "the submission of a code that is not the issued one",
                    operation: "VerifyProcurementPortalValidation",
                    request: a_wrong_code,
                    expected_status: 400,
                    writes: true,
                },
                Call {
                    label: "the suspension a caller asked for",
                    operation: "UpdateProcurementPortalPreferenceStatus",
                    request: a_caller_suspension,
                    expected_status: 400,
                    writes: false,
                },
            ],
            PENDING_VERIFICATION,
        ),
        (
            "the accepted path through the validation and the connection test",
            true,
            vec![
                Call {
                    label: "the send",
                    operation: "SendProcurementPortalValidation",
                    request: a_validation_start,
                    expected_status: 400,
                    writes: true,
                },
                Call {
                    label: "the submission of the issued code",
                    operation: "VerifyProcurementPortalValidation",
                    request: the_outstanding_code,
                    expected_status: 200,
                    writes: true,
                },
                Call {
                    label: "the connection test the caller asked for",
                    operation: "UpdateProcurementPortalPreferenceStatus",
                    request: the_connection_test,
                    expected_status: 200,
                    writes: true,
                },
            ],
            TEST_INITIALIZED,
        ),
        (
            "a refusal between accepted writes",
            true,
            vec![
                Call {
                    label: "the send",
                    operation: "SendProcurementPortalValidation",
                    request: a_validation_start,
                    expected_status: 400,
                    writes: true,
                },
                Call {
                    label: "the suspension a caller asked for",
                    operation: "UpdateProcurementPortalPreferenceStatus",
                    request: a_caller_suspension,
                    expected_status: 400,
                    writes: false,
                },
                Call {
                    label: "the submission of the issued code",
                    operation: "VerifyProcurementPortalValidation",
                    request: the_outstanding_code,
                    expected_status: 200,
                    writes: true,
                },
                Call {
                    label: "the configuration update",
                    operation: "PutProcurementPortalPreference",
                    request: a_configuration_update,
                    expected_status: 200,
                    writes: true,
                },
            ],
            VALIDATED,
        ),
    ];

    for (index, (label, with_endpoint, calls, ends_at)) in lifecycles.iter().enumerate() {
        let portal = PORTAL_NAMES[index % PORTAL_NAMES.len()];
        let buyer = format!("BUYER-LIFECYCLE-{index}");
        let supplier = format!("SUPPLIER-LIFECYCLE-{index}");
        let create = if *with_endpoint {
            create_with_delivery_endpoint(portal, &buyer, &supplier)
        } else {
            minimal_create(portal, &buyer, &supplier)
        };
        let arn = arn_of(&ok(&router, "CreateProcurementPortalPreference", &create).await);
        // The create is the first write of the resource.
        let mut writes = 1;

        for scripted in calls {
            let code = outstanding_code(&engine, &arn);
            let request = (scripted.request)(&arn, code.as_deref());
            let (status, body) = call(&router, scripted.operation, &request).await;
            assert_eq!(
                status, scripted.expected_status,
                "case {label} / {}: expected HTTP {}, got {status} {body}",
                scripted.label, scripted.expected_status
            );
            if scripted.writes {
                writes += 1;
            }
        }

        let reported = get_preference(&router, &arn).await;
        assert_eq!(
            reported
                .get("EinvoiceDeliveryPreferenceStatus")
                .and_then(Value::as_str),
            Some(*ends_at),
            "case {label}: the read reports the status the accepted calls left, got {reported}"
        );
        assert_eq!(
            version(&reported),
            writes,
            "case {label}: expected Version {writes}, one per applied write, got {reported}"
        );
    }
}
