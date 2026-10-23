// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a request naming an SAP application it does not hold, and
//! how `GetApplication` refuses a request that names no application at all.
//!
//! Two rules are pinned here:
//!
//! 1. A request naming an application this account and Region holds no registration for
//!    is refused with a message BUILT FROM THE REQUEST, at the status and error shape
//!    that operation models. An application-CONTROL call — one that acts on the
//!    application it names — models `ResourceNotFoundException` and answers
//!    `Application <ApplicationId> doesn't exist.` with it (404); an operation that
//!    models no not-found error answers the same condition with the service's
//!    `ValidationException` wording (400), which
//!    [`crate::application_lifecycle`] owns for the lookup members.
//! 2. `GetApplication`'s missing-identifier refusal fires exactly when the request
//!    carries NONE of the identifier members the model gives it — and a request that
//!    carries any one of them is never refused for a missing identifier.
//!
//! The tables span what each rule generalises over: the operation, the optional members
//! the request carries, the status/shape pair the operation models, which identifier
//! members are present, and the form of the `ApplicationId` itself (shortest and longest,
//! every character class the modelled pattern admits, digits only, a case-differing
//! spelling). The recorded `sim2real-probe-ssm-sap-ctrl-…` id is ONE row. References
//! arriving as an `ApplicationArn` or an `AppRegistryArn` are pinned by
//! [`crate::application_lifecycle`] and are not restated here.
//!
//! Every call goes through `MultiServiceRouter::dispatch` (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the error serialization all run as they do in the
//! emulator binary.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::CONNECTED_ENTITY_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::TRANSITION_SPAN;

use crate::application_lifecycle::{valid_registration, Emulator, Reply};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The modelled error code `StartApplication` and `StopApplication` declare for an
/// application that is not registered (`smithy_query(query="operation",
/// operation="StartApplication")`: `ResourceNotFoundException`, `@httpError 404`).
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The region the harness signs for, so an application ARN this test builds is the one
/// `RegisterApplication` mints.
const REGION: &str = "us-east-1";

/// The account every emulated ARN and account-scoped refusal carries, from the core's
/// single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// An id this suite registers, so the positive rows have a real application to name.
const REGISTERED_ID: &str = "HDB-TEST";

/// What an application-control call reports for an application that is not registered,
/// as the `ssm-sap` application-control recordings show it: the `ApplicationId` the
/// REQUEST carried, inside a fixed template.
fn control_refusal_message(reference: &str) -> String {
    format!("Application {reference} doesn't exist.")
}

/// What an operation modelling no `ResourceNotFoundException` reports for the same
/// condition: the account the call ran in and the reference the request carried. The
/// wording is the one `tests/all/application_lifecycle.rs` pins on the lookup members.
fn validation_refusal_message(reference: &str) -> String {
    format!("No registered application found for account: {ACCOUNT}, and application: {reference}")
}

/// The ARN `RegisterApplication` mints for `id`.
fn application_arn(id: &str) -> String {
    format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:application/{id}")
}

/// An AWS Service Catalog AppRegistry ARN, inside the modelled `AppRegistryArn` pattern
/// `^arn:aws:servicecatalog:[a-z0-9:\/-]+$`. No application in this emulator is
/// registered through AppRegistry, so it resolves to nothing.
const APP_REGISTRY_ARN: &str = "arn:aws:servicecatalog:us-east-1:000000000000:/applications/abc";

/// Register a minimal HANA application and wait out the status change the clock owes it,
/// so a later `StartApplication` is not refused for an operation already under way.
async fn register_settled(emulator: &Emulator, id: &str) {
    let reply = emulator
        .send("/register-application", valid_registration(id, "HANA"))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {id:?}: expected 200 from RegisterApplication, got {reply:?}"
    );
    // The emulator never sleeps; this is the test waiting for the clock, which is the
    // only thing that can make the pending change due.
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(300)).await;
}

/// One operation that refuses an `ApplicationId` naming nothing: its label, its URI path,
/// how the request carrying `reference` is built, the status and error code the model
/// declares for that refusal, and the message that refusal must state.
struct Refusal {
    label: &'static str,
    path: &'static str,
    body: fn(&str) -> Value,
    status: u16,
    code: &'static str,
    message: fn(&str) -> String,
}

/// The rows: every implemented operation that takes an `ApplicationId` and refuses one
/// that names no registered application. The two `StopApplication` rows differ in the
/// optional members they carry, so the refusal is shown to be the reference's and not the
/// switches'.
fn id_refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            label: "StartApplication",
            path: "/start-application",
            body: |reference| json!({ "ApplicationId": reference }),
            status: 404,
            code: RESOURCE_NOT_FOUND_EXCEPTION,
            message: control_refusal_message,
        },
        Refusal {
            label: "StopApplication",
            path: "/stop-application",
            body: |reference| json!({ "ApplicationId": reference }),
            status: 404,
            code: RESOURCE_NOT_FOUND_EXCEPTION,
            message: control_refusal_message,
        },
        Refusal {
            label: "StopApplication with every switch set",
            path: "/stop-application",
            body: |reference| {
                json!({
                    "ApplicationId": reference,
                    // The model's only `ConnectedEntityType` value.
                    "StopConnectedEntity": CONNECTED_ENTITY_TYPE_VALUES[0],
                    "IncludeEc2InstanceShutdown": true,
                })
            },
            status: 404,
            code: RESOURCE_NOT_FOUND_EXCEPTION,
            message: control_refusal_message,
        },
        Refusal {
            // Models no `ResourceNotFoundException`, so the same condition is a 400
            // `ValidationException` here, with the service's wording for it.
            label: "DeregisterApplication",
            path: "/deregister-application",
            body: |reference| json!({ "ApplicationId": reference }),
            status: 400,
            code: VALIDATION_EXCEPTION,
            message: validation_refusal_message,
        },
        Refusal {
            label: "GetApplication by ApplicationId",
            path: "/get-application",
            body: |reference| json!({ "ApplicationId": reference }),
            status: 400,
            code: VALIDATION_EXCEPTION,
            message: validation_refusal_message,
        },
    ]
}

/// References inside the modelled `ApplicationId` class (`@length(1, 60)`,
/// `@pattern ^[\w\d\.-]+$`, from `smithy_query(query="operation",
/// operation="StartApplication")`) that name no registered application: the shortest, the
/// longest, every character class the pattern admits, the recorded probe id, and the
/// registered id spelled in another case.
fn unregistered_ids() -> Vec<String> {
    vec![
        "NO-SUCH".to_string(),
        // The recorded instance: ONE row of the class.
        "sim2real-probe-ssm-sap-ctrl-dd8b8c28".to_string(),
        // The minimum modelled length.
        "a".to_string(),
        // Word characters, digits, a dot, a hyphen and an underscore.
        "app.1-2_3".to_string(),
        // Digits only.
        "0".to_string(),
        // The maximum modelled length.
        "L".repeat(60),
        // The registered id in another case: the lookup is exact, and the refusal echoes
        // what the CALLER sent rather than what is stored.
        REGISTERED_ID.to_lowercase(),
    ]
}

/// RULE 1: a request naming an application this account and Region holds no registration
/// for is refused with a message built from the request, at the status and error shape the
/// operation models — `Application <ApplicationId> doesn't exist.` with the 404
/// `ResourceNotFoundException` the control operations model.
#[tokio::test]
async fn an_unregistered_application_id_is_refused_with_the_request_derived_wording() {
    let emulator = Emulator::new();
    register_settled(&emulator, REGISTERED_ID).await;

    for refusal in id_refusals() {
        for reference in unregistered_ids() {
            let case = format!("{} / {reference:?}", refusal.label);
            let reply = emulator.send(refusal.path, (refusal.body)(&reference)).await;

            assert_eq!(
                reply.status, refusal.status,
                "case {case}: expected {}, got {reply:?}",
                refusal.status
            );
            assert_eq!(
                reply.code(),
                refusal.code,
                "case {case}: expected {}, got {reply:?}",
                refusal.code
            );

            let actual = message(&reply);
            let expected = (refusal.message)(&reference);
            assert_eq!(
                actual, expected,
                "case {case}: expected {expected:?}, got {actual:?}"
            );
            assert!(
                reply.member("OperationId").is_none() && reply.member("Application").is_none(),
                "case {case}: a refused reference may mint or report nothing, got {reply:?}"
            );
        }
    }

    // The registered application is untouched by every refusal above: it is still
    // readable, and it is NOT reported as missing.
    let fetched = emulator
        .send("/get-application", json!({ "ApplicationId": REGISTERED_ID }))
        .await;
    assert_eq!(
        fetched.status, 200,
        "case {REGISTERED_ID:?}: the registered application is still readable, got {fetched:?}"
    );
    let started = emulator
        .send(
            "/start-application",
            json!({ "ApplicationId": REGISTERED_ID }),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "case {REGISTERED_ID:?}: a registered application starts, got {started:?}"
    );
}

/// The wording `GetApplication` reports when the request names no application at all.
/// Recorded AWS behaviour for this operation: no request value goes into it, so the
/// recording is the only evidence of the text, and a constant is the only form it can
/// take.
const MISSING_IDENTIFIER_MESSAGE: &str = "applicationId or applicationArn need to be specified";

/// RULE 2: the missing-identifier refusal fires exactly on the request that carries NONE
/// of `GetApplication`'s identifier members, and never on one that carries any of them.
///
/// The table is every subset of the three identifier members the model gives the
/// operation (`smithy_query(query="operation", operation="GetApplication")`:
/// `ApplicationId`, `ApplicationArn`, `AppRegistryArn`) — all eight.
#[tokio::test]
async fn get_application_refuses_only_the_request_that_names_no_identifier() {
    let emulator = Emulator::new();
    register_settled(&emulator, REGISTERED_ID).await;
    let arn = application_arn(REGISTERED_ID);

    for members in 0u8..8 {
        let mut body = serde_json::Map::new();
        if members & 0b001 != 0 {
            body.insert("ApplicationId".to_string(), json!(REGISTERED_ID));
        }
        if members & 0b010 != 0 {
            body.insert("ApplicationArn".to_string(), json!(arn));
        }
        if members & 0b100 != 0 {
            body.insert("AppRegistryArn".to_string(), json!(APP_REGISTRY_ARN));
        }
        let case = format!("identifier members {members:03b}");
        let reply = emulator.send("/get-application", Value::Object(body)).await;

        if members == 0 {
            assert_eq!(
                reply.status, 400,
                "case {case}: a request naming no application is refused, got {reply:?}"
            );
            assert_eq!(
                reply.code(),
                VALIDATION_EXCEPTION,
                "case {case}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
            );
            let actual = message(&reply);
            assert_eq!(
                actual, MISSING_IDENTIFIER_MESSAGE,
                "case {case}: expected {MISSING_IDENTIFIER_MESSAGE:?}, got {actual:?}"
            );
            continue;
        }

        // Any identifier member present is an identifier the operation considers, so the
        // missing-identifier refusal must not fire.
        let stated = stated_message(&reply);
        assert_ne!(
            stated.as_deref(),
            Some(MISSING_IDENTIFIER_MESSAGE),
            "case {case}: an identifier IS present, got {reply:?}"
        );

        // `ApplicationId` and `ApplicationArn` name the registered application, so those
        // subsets are answered; the AppRegistry-only subset resolves to nothing and is
        // refused as an unresolvable reference, not as a missing one.
        if members & 0b011 != 0 {
            assert_eq!(
                reply.status, 200,
                "case {case}: a resolvable reference is answered, got {reply:?}"
            );
            assert_eq!(
                reply
                    .member("Application")
                    .and_then(|application| application.get("Id"))
                    .and_then(Value::as_str),
                Some(REGISTERED_ID),
                "case {case}: expected the registered application, got {reply:?}"
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case}: an AppRegistry ARN matching nothing is refused, got {reply:?}"
            );
            assert_eq!(
                stated.as_deref(),
                Some(validation_refusal_message(APP_REGISTRY_ARN).as_str()),
                "case {case}: expected the unresolved-reference wording, got {reply:?}"
            );
        }
    }
}

/// The failure message a reply states, if it states one. Unlike the shared `message`,
/// this tolerates a success, so a row that is answered rather than refused is read with
/// the same helper.
fn stated_message(reply: &Reply) -> Option<String> {
    reply
        .body
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| reply.body.get("Message").and_then(Value::as_str))
        .map(str::to_string)
}
