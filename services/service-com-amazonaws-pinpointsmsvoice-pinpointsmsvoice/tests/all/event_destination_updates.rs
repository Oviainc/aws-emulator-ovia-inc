// SPDX-License-Identifier: CC0-1.0

//! What `UpdateConfigurationSetEventDestination` and
//! `DeleteConfigurationSetEventDestination` do to a stored event destination, read back
//! through `GetConfigurationSetEventDestinations`.
//!
//! RULES, each asserted over a table rather than one call:
//!
//! * an update changes exactly the members of `EventDestinationDefinition` the request
//!   carries and leaves every other member of the stored destination — and every sibling
//!   destination — as it was, so a caller can change one member without restating the
//!   rest;
//! * a delete removes the one destination its path names, leaving its siblings and the
//!   configuration set itself in place;
//! * both resolve through the configuration set that owns the destination, so a set or a
//!   destination that does not exist is the modelled `NotFoundException` (404) — including
//!   a second delete of the same name.
//!
//! The suite drives the real `restJson1` router, so the requests here are the bytes an AWS
//! SDK `pinpoint-sms-voice` client sends. The account holds the entitlement this API's
//! resource-scoped actions need, which `legacy_surface_authorization.rs` owns.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pinpointsmsvoice_pinpointsmsvoice::{
    ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState, EmulatedService,
};

/// The configuration set every row works inside.
const SET: &str = "updates-set";

/// The emulator under test, over the one shared engine.
struct Harness {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status and the parsed JSON body (`Value::Null` when empty).
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The `__type` discriminator of a `restJson1` error envelope, or `""` on a success.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The `EventDestinations` member of a `GetConfigurationSetEventDestinations` answer.
    fn destinations(&self) -> Vec<Value> {
        self.body
            .get("EventDestinations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite drives unsigned requests; signature verification is the framework's
        // own concern and is covered where it lives.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        // The account holds the entitlement this API's resource-scoped actions need, so
        // the rows below exercise what those actions DO; the entitlement itself is pinned
        // in `legacy_surface_authorization.rs`.
        engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>()
            .lock()
            .legacy_surface_entitled = true;
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        let payload = body.map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(value.to_string())
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://sms-voice.pinpoint.us-east-1.amazonaws.com{path}"
            ))
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/sms-voice/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .header("content-type", "application/json")
            .body(payload)
            .expect("the test request must build");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Answer { status, body }
    }

    async fn create_set(&self, name: &str) -> Answer {
        self.call(
            "POST",
            "/v1/sms-voice/configuration-sets",
            Some(json!({ "ConfigurationSetName": name })),
        )
        .await
    }

    async fn create_destination(&self, name: &str, definition: Value) -> Answer {
        self.call(
            "POST",
            &format!("/v1/sms-voice/configuration-sets/{SET}/event-destinations"),
            Some(json!({ "EventDestinationName": name, "EventDestination": definition })),
        )
        .await
    }

    async fn update_destination(&self, set: &str, name: &str, definition: Value) -> Answer {
        self.call(
            "PUT",
            &format!("/v1/sms-voice/configuration-sets/{set}/event-destinations/{name}"),
            Some(json!({ "EventDestination": definition })),
        )
        .await
    }

    async fn delete_destination(&self, set: &str, name: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("/v1/sms-voice/configuration-sets/{set}/event-destinations/{name}"),
            None,
        )
        .await
    }

    async fn get_destinations(&self, set: &str) -> Answer {
        self.call(
            "GET",
            &format!("/v1/sms-voice/configuration-sets/{set}/event-destinations"),
            None,
        )
        .await
    }
}

/// The definition every update row starts from: all five members of
/// `EventDestinationDefinition` set, so a row can show which of them its update left
/// alone.
fn stored_definition() -> Value {
    json!({
        "Enabled": true,
        "MatchingEventTypes": ["ANSWERED", "RINGING"],
        "SnsDestination": { "TopicArn": "arn:aws:sns:us-east-1:000000000000:voice" },
        "CloudWatchLogsDestination": {
            "IamRoleArn": "arn:aws:iam::000000000000:role/voice-logs",
            "LogGroupArn": "arn:aws:logs:us-east-1:000000000000:log-group:/voice:*",
        },
        "KinesisFirehoseDestination": {
            "DeliveryStreamArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/voice",
            "IamRoleArn": "arn:aws:iam::000000000000:role/voice-firehose",
        },
    })
}

/// RULE: an update changes exactly the members it carries and leaves the rest of the
/// stored destination as it was.
///
/// The rows differ in WHICH member the update states (a boolean, a list, each of the three
/// destination targets, several at once) and in whether it states any at all — the
/// expectation is computed from the stored definition and the update, never written out,
/// so a row cannot pass against a handler that replaced the whole definition.
#[tokio::test]
async fn an_update_changes_the_members_it_carries_and_leaves_the_rest() {
    // (case, the definition the update carries)
    let rows: &[(&str, Value)] = &[
        ("the boolean alone", json!({ "Enabled": false })),
        (
            "the event-type list alone",
            json!({ "MatchingEventTypes": ["BUSY", "FAILED", "NO_ANSWER"] }),
        ),
        (
            "an emptied event-type list",
            json!({ "MatchingEventTypes": [] }),
        ),
        (
            "the SNS target alone",
            json!({ "SnsDestination": { "TopicArn": "arn:aws:sns:us-east-1:000000000000:other" } }),
        ),
        (
            "the CloudWatch Logs target alone",
            json!({ "CloudWatchLogsDestination": {
                "IamRoleArn": "arn:aws:iam::000000000000:role/other-logs",
                "LogGroupArn": "arn:aws:logs:us-east-1:000000000000:log-group:/other:*",
            } }),
        ),
        (
            "the Firehose target alone, dropping one of its members",
            json!({ "KinesisFirehoseDestination": {
                "DeliveryStreamArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/other",
            } }),
        ),
        (
            "two members at once",
            json!({ "Enabled": false, "MatchingEventTypes": ["COMPLETED_CALL"] }),
        ),
        ("no members at all", json!({})),
    ];

    for (case, update) in rows {
        let harness = Harness::new();
        assert_eq!(
            harness.create_set(SET).await.status,
            200,
            "case {case:?}: the set must be created"
        );
        assert_eq!(
            harness
                .create_destination("dest", stored_definition())
                .await
                .status,
            200,
            "case {case:?}: the destination must be created"
        );
        // A sibling, so a row also shows the update reached ONE destination.
        assert_eq!(
            harness
                .create_destination("sibling", json!({ "Enabled": true }))
                .await
                .status,
            200,
            "case {case:?}: the sibling must be created"
        );

        let updated = harness.update_destination(SET, "dest", update.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: the update must answer 200, got {} {}",
            updated.status, updated.body
        );

        // What must be stored: the created definition with the update's members written
        // over it, plus the name it is filed under.
        let mut expected = stored_definition();
        let expected_members = expected
            .as_object_mut()
            .expect("the stored definition is a JSON object");
        for (member, value) in update
            .as_object()
            .expect("an update row is a JSON object")
            .iter()
        {
            expected_members.insert(member.clone(), value.clone());
        }
        expected_members.insert("Name".to_string(), json!("dest"));

        let read = harness.get_destinations(SET).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the read-back must answer 200, got {} {}",
            read.status, read.body
        );
        let destinations = read.destinations();
        assert_eq!(
            destinations.len(),
            2,
            "case {case:?}: an update must not add or remove a destination, got {}",
            read.body
        );
        let read_back = destinations
            .iter()
            .find(|destination| destination.get("Name") == Some(&json!("dest")))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            read_back, expected,
            "case {case:?}: expected {expected}, got {read_back}"
        );
        // The sibling is exactly as it was created.
        let sibling = destinations
            .iter()
            .find(|destination| destination.get("Name") == Some(&json!("sibling")))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            sibling,
            json!({ "Name": "sibling", "Enabled": true }),
            "case {case:?}: an update must leave a sibling destination alone, got {sibling}"
        );
    }
}

/// RULE: a delete removes the one destination its path names and leaves its siblings and
/// the configuration set in place; a second delete of that name is the modelled 404.
///
/// The rows differ in how many destinations the set carries and in which of them is
/// removed, so the rule is not pinned on a set holding exactly one.
#[tokio::test]
async fn a_delete_removes_exactly_the_destination_it_names() {
    // (case, the destinations to create, the one to delete)
    let rows: &[(&str, &[&str], &str)] = &[
        ("the only destination", &["only"], "only"),
        ("the first of three", &["a", "b", "c"], "a"),
        ("the last of three", &["a", "b", "c"], "c"),
        ("the middle of three", &["a", "b", "c"], "b"),
    ];

    for (case, created, doomed) in rows {
        let harness = Harness::new();
        assert_eq!(
            harness.create_set(SET).await.status,
            200,
            "case {case:?}: the set must be created"
        );
        for name in *created {
            assert_eq!(
                harness
                    .create_destination(name, json!({ "Enabled": true }))
                    .await
                    .status,
                200,
                "case {case:?}: creating {name}"
            );
        }

        let deleted = harness.delete_destination(SET, doomed).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the delete must answer 200, got {} {}",
            deleted.status, deleted.body
        );

        let expected: Vec<Value> = created
            .iter()
            .filter(|name| *name != doomed)
            .map(|name| json!({ "Name": name, "Enabled": true }))
            .collect();
        let read = harness.get_destinations(SET).await;
        assert_eq!(
            read.destinations(),
            expected,
            "case {case:?}: expected the siblings alone, got {}",
            read.body
        );

        // A second delete of the same name names nothing.
        let again = harness.delete_destination(SET, doomed).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: a second delete must answer 404, got {} {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {}",
            again.body
        );
    }
}

/// RULE: both operations resolve through the configuration set that owns the destination,
/// so a set that does not exist and a destination that does not exist are each the
/// modelled `NotFoundException` (404), and neither changes anything.
#[tokio::test]
async fn an_update_or_delete_of_something_absent_is_not_found() {
    let harness = Harness::new();
    assert_eq!(harness.create_set(SET).await.status, 200);
    assert_eq!(
        harness
            .create_destination("dest", json!({ "Enabled": true }))
            .await
            .status,
        200
    );

    // (case, the set, the destination)
    let rows: &[(&str, &str, &str)] = &[
        ("a set that was never created", "no-such-set", "dest"),
        ("a set whose name differs in case", "UPDATES-SET", "dest"),
        ("a destination that was never created", SET, "no-such-dest"),
        ("a destination whose name differs in case", SET, "DEST"),
    ];

    for (case, set, destination) in rows {
        let updated = harness
            .update_destination(set, destination, json!({ "Enabled": false }))
            .await;
        assert_eq!(
            updated.status, 404,
            "case {case:?}: the update must answer 404, got {} {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException from the update, got {}",
            updated.body
        );

        let deleted = harness.delete_destination(set, destination).await;
        assert_eq!(
            deleted.status, 404,
            "case {case:?}: the delete must answer 404, got {} {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException from the delete, got {}",
            deleted.body
        );
    }

    // Nothing the refused calls named changed: the one stored destination is as created.
    let read = harness.get_destinations(SET).await;
    assert_eq!(
        read.destinations(),
        vec![json!({ "Name": "dest", "Enabled": true })],
        "a refused update or delete must change nothing, got {}",
        read.body
    );
}
