// SPDX-License-Identifier: CC0-1.0

//! How AppIntegrations refuses a request it will not read.
//!
//! The rule under test: a request BODY that breaks a modelled constraint is refused by
//! this service's front door — HTTP 400, error code `BadRequestException` (a shape this
//! service's model declares nowhere) and the fixed sentence `"Invalid request body"`,
//! which names neither the member nor the value — while a constraint on a value bound
//! to the request URI (`@httpLabel`, `@httpQuery`) is refused by the service itself as
//! the framework's `ValidationException`, which states what failed.
//!
//! The table spans the kinds of body violation the model can express (absent required
//! member, `@pattern`, `@length` over and under, a violation nested inside a structure)
//! across several operations, and both the body-bound and the URI-bound side of the
//! same member name. The `CreateApplication` namespace the issue recorded is one row.

use serde_json::{json, Value};

use super::harness::{Emulator, Reply, REGION};

/// The code and the sentence a front-door rejection carries
/// (`service_common::front_door::BAD_REQUEST_EXCEPTION` / `INVALID_REQUEST_BODY`,
/// restated here so this suite asserts the wire values rather than the constants).
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// The code the framework names for a refusal this service does not word itself.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// One request, and whether the constraint it breaks is carried in the body.
struct Case {
    label: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Requests whose BODY breaks a modelled constraint.
fn body_violations() -> Vec<Case> {
    vec![
        Case {
            // The recorded case: `Namespace` is @pattern ^[a-zA-Z0-9\/\._\-]+$.
            label: "CreateApplication: a Namespace outside its pattern",
            method: "POST",
            uri: "/applications".to_string(),
            body: Some(json!({
                "Name": "ok-name",
                "Namespace": "invalid namespace!",
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": { "AccessUrl": "https://example.com/bad" },
                },
            })),
        },
        Case {
            label: "CreateApplication: a Namespace one past its @length maximum (211)",
            method: "POST",
            uri: "/applications".to_string(),
            body: Some(json!({
                "Name": "ok-name",
                "Namespace": "n".repeat(212),
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": { "AccessUrl": "https://example.com/ok" },
                },
            })),
        },
        Case {
            label: "CreateApplication: an absent required member",
            method: "POST",
            uri: "/applications".to_string(),
            body: Some(json!({ "Name": "ok-name" })),
        },
        Case {
            label: "CreateEventIntegration: a Name outside its pattern",
            method: "POST",
            uri: "/eventIntegrations".to_string(),
            body: Some(json!({
                "Name": "not a valid name!",
                "EventBridgeBus": "default",
                "EventFilter": { "Source": "aws.partner/demo" },
            })),
        },
        Case {
            label: "CreateEventIntegration: an empty Name, under its @length minimum",
            method: "POST",
            uri: "/eventIntegrations".to_string(),
            body: Some(json!({
                "Name": "",
                "EventBridgeBus": "default",
                "EventFilter": { "Source": "aws.partner/demo" },
            })),
        },
        Case {
            label: "CreateEventIntegration: a violation nested in EventFilter",
            method: "POST",
            uri: "/eventIntegrations".to_string(),
            body: Some(json!({
                "Name": "nested-violation",
                "EventBridgeBus": "default",
                "EventFilter": { "Source": "" },
            })),
        },
        Case {
            label: "CreateDataIntegration: a blank KmsKey, against @pattern \\S",
            method: "POST",
            uri: "/dataIntegrations".to_string(),
            body: Some(json!({ "Name": "kms-blank", "KmsKey": " " })),
        },
        Case {
            label: "CreateDataIntegration: a SourceURI outside its pattern",
            method: "POST",
            uri: "/dataIntegrations".to_string(),
            body: Some(json!({
                "Name": "bad-source",
                "KmsKey": "alias/aws/appintegrations",
                "SourceURI": "not a uri",
            })),
        },
        Case {
            label: "UpdateEventIntegration: a Description past its @length maximum (1000)",
            method: "PATCH",
            uri: "/eventIntegrations/some-integration".to_string(),
            body: Some(json!({ "Description": "d".repeat(1001) })),
        },
    ]
}

/// Requests whose violated constraint is bound to the request URI, not the body.
fn uri_violations() -> Vec<Case> {
    vec![
        Case {
            label: "GetEventIntegration: a Name label outside its pattern",
            method: "GET",
            uri: "/eventIntegrations/invalid%20name%21".to_string(),
            body: None,
        },
        Case {
            label: "DeleteEventIntegration: an empty Name label",
            method: "DELETE",
            uri: "/eventIntegrations/%20".to_string(),
            body: None,
        },
        Case {
            label: "ListEventIntegrations: maxResults under its range minimum (1)",
            method: "GET",
            uri: "/eventIntegrations?maxResults=0".to_string(),
            body: None,
        },
        Case {
            label: "ListDataIntegrations: maxResults past its range maximum (50)",
            method: "GET",
            uri: "/dataIntegrations?maxResults=51".to_string(),
            body: None,
        },
    ]
}

async fn answer(emulator: &Emulator, case: &Case) -> Reply {
    emulator
        .send(case.method, &case.uri, REGION, case.body.clone())
        .await
}

#[tokio::test]
async fn a_body_that_breaks_a_modelled_constraint_is_a_front_door_bad_request() {
    let emulator = Emulator::new();
    for case in body_violations() {
        let label = case.label;
        let reply = answer(&emulator, &case).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, BAD_REQUEST_EXCEPTION,
            "case {label:?}: expected {BAD_REQUEST_EXCEPTION}, got {:?} / {}",
            reply.error_type, reply.body
        );
        assert_eq!(
            reply.message().as_deref(),
            Some(INVALID_REQUEST_BODY),
            "case {label:?}: expected {INVALID_REQUEST_BODY:?}, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_constraint_on_a_value_bound_to_the_uri_keeps_the_validation_error() {
    let emulator = Emulator::new();
    for case in uri_violations() {
        let label = case.label;
        let reply = answer(&emulator, &case).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, VALIDATION_EXCEPTION,
            "case {label:?}: a URI-bound violation is not a body rejection, so it keeps \
             {VALIDATION_EXCEPTION}; got {:?} / {}",
            reply.error_type, reply.body
        );
        assert_ne!(
            reply.message().as_deref(),
            Some(INVALID_REQUEST_BODY),
            "case {label:?}: a URI-bound violation must say what failed, not the front \
             door's fixed sentence"
        );
    }
}

#[tokio::test]
async fn a_request_within_every_modelled_constraint_is_not_refused() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    // The same operations as the table above, with every member inside its constraints:
    // the front door must let these through, so the rule above is about the constraint
    // and not about the operation.
    let accepted: Vec<Case> = vec![
        Case {
            label: "CreateEventIntegration: every member within bounds",
            method: "POST",
            uri: "/eventIntegrations".to_string(),
            body: Some(json!({
                "Name": "front-door-ok",
                "EventBridgeBus": "default",
                "EventFilter": { "Source": "aws.partner/demo" },
            })),
        },
        Case {
            label: "CreateDataIntegration: every member within bounds",
            method: "POST",
            uri: "/dataIntegrations".to_string(),
            body: Some(json!({
                "Name": "front-door-ok",
                "KmsKey": kms_key,
                "SourceURI": "s3://feed-bucket/accounts",
            })),
        },
        Case {
            label: "ListEventIntegrations: maxResults at its range maximum",
            method: "GET",
            uri: "/eventIntegrations?maxResults=50".to_string(),
            body: None,
        },
    ];

    for case in accepted {
        let label = case.label;
        let reply = answer(&emulator, &case).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected HTTP 200, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "",
            "case {label:?}: expected no error type, got {:?}",
            reply.error_type
        );
    }
}
