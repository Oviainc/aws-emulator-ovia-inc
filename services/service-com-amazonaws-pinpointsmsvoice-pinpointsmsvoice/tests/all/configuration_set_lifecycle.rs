// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the Pinpoint SMS Voice configuration-set lifecycle and the
//! voice send, driven over the wire through the real `restJson1` router: the requests
//! here are the bytes an AWS SDK `pinpoint-sms-voice` client sends.
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded call:
//!
//! * a configuration set is created once and listed by name until it is deleted;
//! * an event destination reads back exactly the definition it was created with;
//! * `PageSize` bounds a page and `NextToken` resumes the listing after it;
//! * every accepted voice message gets its own `MessageId` and is recorded in state.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pinpointsmsvoice_pinpointsmsvoice::ops::types::EVENT_TYPE_VALUES;
use service_com_amazonaws_pinpointsmsvoice_pinpointsmsvoice::{
    ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState, EmulatedService,
};

/// The emulator under test: the service mounted on the production router, over the one
/// shared engine the test also reads state from.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
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
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite drives unsigned requests; signature verification is the framework's
        // own concern and is covered where it lives.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        // This suite is about what the operations DO, so the account holds the entitlement
        // this API's resource-scoped actions need. The entitlement itself — and that an
        // account without it reaches none of this behaviour — is pinned in
        // `legacy_surface_authorization.rs`.
        engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>()
            .lock()
            .legacy_surface_entitled = true;
        Self { router, engine }
    }

    /// Send one request and read the answer back.
    ///
    /// The `Authorization` credential scope names `sms-voice`, this service's sigv4
    /// signing name, which is how the router picks the service that claims it.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        let payload = body.map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(value.to_string())
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://sms-voice.us-east-1.amazonaws.com{path}"))
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

    async fn list_sets(&self, query: &str) -> Answer {
        self.call("GET", &format!("/v1/sms-voice/configuration-sets{query}"), None)
            .await
    }

    async fn create_destination(&self, set: &str, name: &str, definition: Value) -> Answer {
        self.call(
            "POST",
            &format!("/v1/sms-voice/configuration-sets/{set}/event-destinations"),
            Some(json!({
                "EventDestinationName": name,
                "EventDestination": definition,
            })),
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

    async fn delete_set(&self, set: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("/v1/sms-voice/configuration-sets/{set}"),
            None,
        )
        .await
    }

    async fn send_voice(&self, request: Value) -> Answer {
        self.call("POST", "/v1/sms-voice/voice/message", Some(request))
            .await
    }

    /// The configuration set names currently in the shared store.
    fn stored_set_names(&self) -> Vec<String> {
        self.engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>()
            .lock()
            .configuration_sets
            .keys()
            .cloned()
            .collect()
    }
}

/// The `ConfigurationSets` member of a `ListConfigurationSets` answer.
fn listed_names(answer: &Answer) -> Vec<String> {
    answer
        .body
        .get("ConfigurationSets")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .map(|name| name.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// CreateConfigurationSet + ListConfigurationSets
// ---------------------------------------------------------------------------

/// RULE: `ListConfigurationSets` reports exactly the names `CreateConfigurationSet`
/// stored, whatever they are and however many there are — never a canned list.
///
/// The rows differ in how many names are created and in the shape of the names
/// themselves (the member's shape is `WordCharactersWithDelimiters`: word characters
/// plus delimiters), and the creation order is deliberately not the listing order.
#[tokio::test]
async fn list_reports_exactly_the_names_create_stored() {
    let cases: &[&[&str]] = &[
        &["only-set"],
        &["zeta", "alpha", "mu"],
        &["set_with_underscores", "set.with.dots", "set-with-dashes"],
        &["Set1", "set1", "SET1x", "9lives"],
    ];

    for names in cases {
        let harness = Harness::new();
        for name in *names {
            let answer = harness.create_set(name).await;
            assert_eq!(
                answer.status, 200,
                "case {names:?}: creating {name} must answer 200, got {} {}",
                answer.status, answer.body
            );
        }

        let mut expected: Vec<String> = names.iter().map(|n| (*n).to_string()).collect();
        expected.sort();

        let answer = harness.list_sets("").await;
        assert_eq!(
            answer.status, 200,
            "case {names:?}: list must answer 200, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            listed_names(&answer),
            expected,
            "case {names:?}: list must report the stored names, got {}",
            answer.body
        );
        assert_eq!(
            harness.stored_set_names(),
            expected,
            "case {names:?}: the names must be in the shared store, not synthesised per call"
        );
        assert!(
            answer.body.get("NextToken").is_none(),
            "case {names:?}: a single-page listing must omit NextToken, got {}",
            answer.body
        );
    }
}

/// RULE: a configuration set name is unique per account and region, so creating one
/// that already exists is a conflict (`AlreadyExistsException`, HTTP 409) and leaves the
/// stored set untouched.
#[tokio::test]
async fn creating_an_existing_configuration_set_conflicts() {
    for name in ["demo-set", "x", "a_very_long_configuration_set_name_0123456789"] {
        let harness = Harness::new();
        assert_eq!(harness.create_set(name).await.status, 200, "case {name:?}");

        let answer = harness.create_set(name).await;
        assert_eq!(
            answer.status, 409,
            "case {name:?}: a duplicate create must answer 409, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "AlreadyExistsException",
            "case {name:?}: expected AlreadyExistsException, got {}",
            answer.body
        );
        assert_eq!(
            harness.stored_set_names(),
            vec![name.to_string()],
            "case {name:?}: a refused create must not change the store"
        );
    }
}

/// RULE: a create that names no configuration set is refused as invalid input
/// (HTTP 400) and stores nothing.
///
/// `BadRequestException` is this service's own code for "the input you provided is
/// invalid"; a JSON `null` where the model declares a string is refused one step
/// earlier, by the protocol codec, which answers the framework's
/// `ValidationException` with the same 400.
#[tokio::test]
async fn creating_a_configuration_set_without_a_name_is_a_bad_request() {
    // Each row is a body that carries no usable name: absent, null, empty, blank.
    let cases: &[(&str, Value, &str)] = &[
        ("absent", json!({}), "BadRequestException"),
        (
            "explicit null",
            json!({ "ConfigurationSetName": null }),
            "ValidationException",
        ),
        (
            "empty string",
            json!({ "ConfigurationSetName": "" }),
            "BadRequestException",
        ),
        (
            "whitespace",
            json!({ "ConfigurationSetName": "   " }),
            "BadRequestException",
        ),
    ];

    for (case, body, expected_code) in cases {
        let harness = Harness::new();
        let answer = harness
            .call(
                "POST",
                "/v1/sms-voice/configuration-sets",
                Some(body.clone()),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {}",
            answer.body
        );
        assert!(
            harness.stored_set_names().is_empty(),
            "case {case:?}: a refused create must store nothing, got {:?}",
            harness.stored_set_names()
        );
    }
}

/// RULE: a configuration set is stored under exactly the name the caller sent, so the
/// listing reports that name and every operation that resolves the set by its path
/// label finds it.
///
/// The rows differ in case and in which delimiters the name carries — a name a
/// normalising store would have rewritten (lower-cased, or with its delimiters folded
/// together) no longer resolves on the child and delete paths. A name carrying anything
/// the member's shape does not admit is refused rather than stored, which
/// `configuration_set_names.rs` pins.
#[tokio::test]
async fn a_configuration_set_keeps_exactly_the_name_it_was_created_with() {
    // (name, the same name as a URI path segment)
    let cases: &[(&str, &str)] = &[
        ("demo-set", "demo-set"),
        ("DEMO-SET", "DEMO-SET"),
        ("Mixed_Case-9", "Mixed_Case-9"),
        ("dots.and_dashes-9", "dots.and_dashes-9"),
        ("a", "a"),
    ];

    for (name, path_segment) in cases {
        let harness = Harness::new();
        let created = harness.create_set(name).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: create must answer 200, got {} {}",
            created.status, created.body
        );

        // Reported and stored verbatim.
        assert_eq!(
            listed_names(&harness.list_sets("").await),
            vec![(*name).to_string()],
            "case {name:?}: the listing must report the name as sent"
        );
        assert_eq!(
            harness.stored_set_names(),
            vec![(*name).to_string()],
            "case {name:?}: the store must key the set by the name as sent"
        );

        // And the caller's own name resolves on the paths that take it as a label.
        let created_destination = harness
            .create_destination(path_segment, "dest1", json!({ "Enabled": true }))
            .await;
        assert_eq!(
            created_destination.status, 200,
            "case {name:?}: the set must resolve by its own name, got {} {}",
            created_destination.status, created_destination.body
        );
        let read = harness.get_destinations(path_segment).await;
        assert_eq!(
            read.body.get("EventDestinations"),
            Some(&json!([{ "Name": "dest1", "Enabled": true }])),
            "case {name:?}: expected the destination back, got {}",
            read.body
        );
        let deleted = harness.delete_set(path_segment).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: delete must resolve by the same name, got {} {}",
            deleted.status, deleted.body
        );
        assert!(
            harness.stored_set_names().is_empty(),
            "case {name:?}: the set must be gone, got {:?}",
            harness.stored_set_names()
        );
    }
}

// ---------------------------------------------------------------------------
// Event destinations
// ---------------------------------------------------------------------------

/// RULE: `GetConfigurationSetEventDestinations` reports each stored destination as the
/// definition it was created with, member for member, under the name it was filed as —
/// an omitted member stays omitted.
///
/// The rows differ in which sub-structures are present, in how many event types are
/// matched, and in whether `Enabled` was given at all. Every `IamRoleArn` names a role of
/// the account the emulator serves, because a role another account holds is one this
/// service may not be handed at all (`legacy_surface_authorization.rs`).
#[tokio::test]
async fn event_destinations_round_trip_every_sub_structure() {
    let cases: &[(&str, Value)] = &[
        (
            "sns only",
            json!({
                "Enabled": true,
                "MatchingEventTypes": ["COMPLETED_CALL"],
                "SnsDestination": { "TopicArn": "arn:aws:sns:us-east-1:111122223333:voice-events" },
            }),
        ),
        (
            "cloudwatch only",
            json!({
                "Enabled": false,
                "MatchingEventTypes": ["INITIATED_CALL", "RINGING"],
                "CloudWatchLogsDestination": {
                    "IamRoleArn": "arn:aws:iam::000000000000:role/voice-logs",
                    "LogGroupArn": "arn:aws:logs:us-east-1:111122223333:log-group:/voice:*",
                },
            }),
        ),
        (
            "firehose only",
            json!({
                "MatchingEventTypes": ["BUSY", "FAILED", "NO_ANSWER"],
                "KinesisFirehoseDestination": {
                    "DeliveryStreamArn": "arn:aws:firehose:us-east-1:111122223333:deliverystream/voice",
                    "IamRoleArn": "arn:aws:iam::000000000000:role/voice-firehose",
                },
            }),
        ),
        (
            "all three destinations",
            json!({
                "Enabled": true,
                "MatchingEventTypes": ["ANSWERED", "COMPLETED_CALL"],
                "SnsDestination": { "TopicArn": "arn:aws:sns:us-east-1:111122223333:all" },
                "CloudWatchLogsDestination": { "LogGroupArn": "arn:aws:logs:us-east-1:111122223333:log-group:/all:*" },
                "KinesisFirehoseDestination": { "IamRoleArn": "arn:aws:iam::000000000000:role/all" },
            }),
        ),
        ("no members at all", json!({})),
    ];

    for (case, definition) in cases {
        let harness = Harness::new();
        assert_eq!(harness.create_set("demo-set").await.status, 200, "case {case:?}");

        let created = harness
            .create_destination("demo-set", "dest1", definition.clone())
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: creating the destination must answer 200, got {} {}",
            created.status, created.body
        );

        let read = harness.get_destinations("demo-set").await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: reading destinations must answer 200, got {} {}",
            read.status, read.body
        );
        let destinations = read
            .body
            .get("EventDestinations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            destinations.len(),
            1,
            "case {case:?}: expected exactly one destination, got {}",
            read.body
        );

        // The reported destination is the definition that was sent, plus `Name`.
        let mut expected = definition.clone();
        expected
            .as_object_mut()
            .expect("the definition rows are JSON objects")
            .insert("Name".to_string(), json!("dest1"));
        assert_eq!(
            destinations[0], expected,
            "case {case:?}: expected {expected}, got {}",
            destinations[0]
        );
    }
}

/// RULE: every value of the model's `EventType` enum is a valid `MatchingEventTypes`
/// entry, and anything outside that enum is refused as invalid input (HTTP 400).
///
/// The accepted set is `EVENT_TYPE_VALUES`, generated from
/// `com.amazonaws.pinpointsmsvoice#EventType` — the test walks every entry rather than
/// naming a few.
#[tokio::test]
async fn matching_event_types_accepts_the_model_enum_and_refuses_anything_else() {
    let harness = Harness::new();
    assert_eq!(harness.create_set("demo-set").await.status, 200);

    for (index, event_type) in EVENT_TYPE_VALUES.iter().enumerate() {
        let answer = harness
            .create_destination(
                "demo-set",
                &format!("dest-{index}"),
                json!({ "MatchingEventTypes": [event_type] }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {event_type:?}: a modelled event type must be accepted, got {} {}",
            answer.status, answer.body
        );
    }

    // Values just outside the enum: wrong case, a near-miss, an empty string, and a
    // value from the newer v2 API that this one does not model.
    for rejected in ["completed_call", "COMPLETED", "", "TEXT_DELIVERED"] {
        let answer = harness
            .create_destination(
                "demo-set",
                "rejected-dest",
                json!({ "MatchingEventTypes": ["COMPLETED_CALL", rejected] }),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {rejected:?}: an unmodelled event type must answer 400, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {rejected:?}: expected BadRequestException, got {}",
            answer.body
        );
    }

    // And nothing was filed under the refused name.
    let read = harness.get_destinations("demo-set").await;
    let names: BTreeSet<String> = read
        .body
        .get("EventDestinations")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|dest| {
                    dest.get("Name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !names.contains("rejected-dest"),
        "a refused create must store nothing, got {names:?}"
    );
}

/// RULE: a destination lives inside a configuration set — an unknown set is
/// `NotFoundException` (404) for both the create and the read, and a set with no
/// destinations reads back as an empty list rather than a 404.
#[tokio::test]
async fn event_destinations_resolve_through_their_configuration_set() {
    let harness = Harness::new();
    assert_eq!(harness.create_set("demo-set").await.status, 200);

    for unknown in ["no-such-set", "demo-set-2", "DEMO-SET"] {
        let created = harness
            .create_destination(unknown, "dest1", json!({ "Enabled": true }))
            .await;
        assert_eq!(
            created.status, 404,
            "case {unknown:?}: creating under an unknown set must answer 404, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.error_code(),
            "NotFoundException",
            "case {unknown:?}: expected NotFoundException, got {}",
            created.body
        );

        let read = harness.get_destinations(unknown).await;
        assert_eq!(
            read.status, 404,
            "case {unknown:?}: reading an unknown set must answer 404, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.error_code(),
            "NotFoundException",
            "case {unknown:?}: expected NotFoundException, got {}",
            read.body
        );
    }

    let read = harness.get_destinations("demo-set").await;
    assert_eq!(
        read.status, 200,
        "a set with no destinations must answer 200, got {} {}",
        read.status, read.body
    );
    assert_eq!(
        read.body.get("EventDestinations"),
        Some(&json!([])),
        "a set with no destinations must report an empty list, got {}",
        read.body
    );
}

/// RULE: a destination name identifies the destination within its set, so a second
/// create under the same name is a conflict (409) and leaves the stored one unchanged.
#[tokio::test]
async fn creating_an_existing_event_destination_conflicts() {
    let harness = Harness::new();
    assert_eq!(harness.create_set("demo-set").await.status, 200);
    assert_eq!(
        harness
            .create_destination("demo-set", "dest1", json!({ "Enabled": true }))
            .await
            .status,
        200
    );

    // The same name with a DIFFERENT definition is still a conflict: a create never
    // becomes an update.
    for definition in [
        json!({ "Enabled": true }),
        json!({ "Enabled": false }),
        json!({ "MatchingEventTypes": ["BUSY"] }),
    ] {
        let answer = harness
            .create_destination("demo-set", "dest1", definition.clone())
            .await;
        assert_eq!(
            answer.status, 409,
            "case {definition}: a duplicate destination must answer 409, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "AlreadyExistsException",
            "case {definition}: expected AlreadyExistsException, got {}",
            answer.body
        );
    }

    let read = harness.get_destinations("demo-set").await;
    assert_eq!(
        read.body.get("EventDestinations"),
        Some(&json!([{ "Name": "dest1", "Enabled": true }])),
        "the stored destination must be untouched by the refused creates, got {}",
        read.body
    );
}

/// RULE: a destination create that names no destination is refused as invalid input (400).
///
/// As above, a JSON `null` where the model declares a string is refused by the codec
/// with the framework's `ValidationException`, still 400.
#[tokio::test]
async fn creating_an_event_destination_without_a_name_is_a_bad_request() {
    let harness = Harness::new();
    assert_eq!(harness.create_set("demo-set").await.status, 200);

    let cases: &[(&str, Value, &str)] = &[
        (
            "absent",
            json!({ "EventDestination": { "Enabled": true } }),
            "BadRequestException",
        ),
        (
            "explicit null",
            json!({ "EventDestinationName": null }),
            "ValidationException",
        ),
        (
            "empty string",
            json!({ "EventDestinationName": "" }),
            "BadRequestException",
        ),
        (
            "whitespace",
            json!({ "EventDestinationName": "  " }),
            "BadRequestException",
        ),
    ];

    for (case, body, expected_code) in cases {
        let answer = harness
            .call(
                "POST",
                "/v1/sms-voice/configuration-sets/demo-set/event-destinations",
                Some(body.clone()),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {}",
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// RULE: `PageSize` bounds one page of `ListConfigurationSets`, the returned
/// `NextToken` resumes the listing immediately after that page, and the last page omits
/// the token — so walking the pages yields every stored name exactly once.
///
/// The rows vary the total number of sets and the page bound, including a page larger
/// than the whole collection and a page of one.
#[tokio::test]
async fn page_size_bounds_a_page_and_next_token_resumes_the_listing() {
    let cases: &[(usize, usize)] = &[(1, 1), (3, 1), (3, 2), (5, 5), (5, 9), (7, 3)];

    for (total, page_size) in cases.iter().copied() {
        let harness = Harness::new();
        let mut expected: Vec<String> = (0..total).map(|i| format!("set-{i:02}")).collect();
        for name in &expected {
            assert_eq!(
                harness.create_set(name).await.status,
                200,
                "case total={total} page_size={page_size}: creating {name}"
            );
        }
        expected.sort();

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            assert!(
                pages <= total + 1,
                "case total={total} page_size={page_size}: the listing did not terminate"
            );
            let query = match &token {
                Some(next) => format!("?PageSize={page_size}&NextToken={next}"),
                None => format!("?PageSize={page_size}"),
            };
            let answer = harness.list_sets(&query).await;
            assert_eq!(
                answer.status, 200,
                "case total={total} page_size={page_size}: expected 200, got {} {}",
                answer.status, answer.body
            );
            let page = listed_names(&answer);
            assert!(
                page.len() <= page_size,
                "case total={total} page_size={page_size}: page of {} exceeds the bound",
                page.len()
            );
            walked.extend(page);
            match answer.body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            walked, expected,
            "case total={total} page_size={page_size}: walking the pages must yield every name once, got {walked:?}"
        );
    }
}

/// RULE: `PageSize` is modelled as a string, so a value that is not a positive whole
/// number of items is invalid input (400), and a `NextToken` this service did not mint
/// is too.
#[tokio::test]
async fn invalid_pagination_arguments_are_bad_requests() {
    let harness = Harness::new();
    for name in ["set-a", "set-b"] {
        assert_eq!(harness.create_set(name).await.status, 200);
    }

    // PageSize rows: zero, negative, fractional, non-numeric, and one past the
    // representable range.
    for page_size in ["0", "-1", "1.5", "two", "99999999999999999999999999"] {
        let answer = harness.list_sets(&format!("?PageSize={page_size}")).await;
        assert_eq!(
            answer.status, 400,
            "case PageSize={page_size:?}: expected 400, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case PageSize={page_size:?}: expected BadRequestException, got {}",
            answer.body
        );
    }

    // NextToken rows: not this service's token at all. The first three are not even
    // the token's encoding; the last three ARE well-formed hex that decodes to UTF-8
    // but carries none of this service's token marker, so they are not cursors either.
    for token in [
        "zzz".to_string(),
        "not-a-token".to_string(),
        "0g".to_string(),
        hex::encode("set-a"),
        hex::encode("pinpointsmsvoice:configuration-sets:0:set-a"),
        "6161".to_string(),
    ] {
        let answer = harness.list_sets(&format!("?NextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case NextToken={token:?}: expected 400, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case NextToken={token:?}: expected BadRequestException, got {}",
            answer.body
        );
    }

    // A valid page size still lists, after all those refusals.
    let answer = harness.list_sets("?PageSize=10").await;
    assert_eq!(answer.status, 200, "got {} {}", answer.status, answer.body);
    assert_eq!(
        listed_names(&answer),
        vec!["set-a".to_string(), "set-b".to_string()],
        "got {}",
        answer.body
    );
}

/// RULE: the server bounds a page itself — an omitted `PageSize` applies the server's
/// own default and reports a `NextToken` for the rest, and a `PageSize` above the
/// server's maximum is clamped to it rather than widening the response.
///
/// The bounds are `DEFAULT_PAGE_SIZE` (50) and `MAX_PAGE_SIZE` (1000) in
/// `ops/list_configuration_sets.rs`; the row below straddles the default so both the
/// bound and the resumption are exercised, and the clamped row asks for far more than
/// the maximum.
#[tokio::test]
async fn the_server_bounds_a_page_even_when_the_caller_does_not() {
    const SERVER_DEFAULT_PAGE_SIZE: usize = 50;
    const SERVER_MAX_PAGE_SIZE: usize = 1_000;
    let total = SERVER_DEFAULT_PAGE_SIZE + 5;

    let harness = Harness::new();
    let mut expected: Vec<String> = (0..total).map(|i| format!("set-{i:03}")).collect();
    for name in &expected {
        assert_eq!(harness.create_set(name).await.status, 200, "creating {name}");
    }
    expected.sort();

    // No PageSize: the server applies its default and hands back a token.
    let first = harness.list_sets("").await;
    assert_eq!(first.status, 200, "got {} {}", first.status, first.body);
    let first_page = listed_names(&first);
    assert_eq!(
        first_page.len(),
        SERVER_DEFAULT_PAGE_SIZE,
        "an omitted PageSize must be bounded by the server default, got {} names",
        first_page.len()
    );
    let token = first
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "a bounded first page must report a NextToken, got {}",
        first.body
    );

    let second = harness.list_sets(&format!("?NextToken={token}")).await;
    assert_eq!(second.status, 200, "got {} {}", second.status, second.body);
    let mut walked = first_page;
    walked.extend(listed_names(&second));
    assert_eq!(
        walked, expected,
        "the default-bounded walk must still yield every name once, got {walked:?}"
    );
    assert!(
        second.body.get("NextToken").is_none(),
        "the last page must omit NextToken, got {}",
        second.body
    );

    // A PageSize above the server maximum is clamped, not honoured — and since the
    // clamp is still larger than the store, one page covers it.
    for asked in [
        SERVER_MAX_PAGE_SIZE,
        SERVER_MAX_PAGE_SIZE + 1,
        SERVER_MAX_PAGE_SIZE * 1_000,
    ] {
        let answer = harness.list_sets(&format!("?PageSize={asked}")).await;
        assert_eq!(
            answer.status, 200,
            "case PageSize={asked}: got {} {}",
            answer.status, answer.body
        );
        let page = listed_names(&answer);
        assert!(
            page.len() <= SERVER_MAX_PAGE_SIZE,
            "case PageSize={asked}: a page must never exceed the server maximum, got {}",
            page.len()
        );
        assert_eq!(
            page, expected,
            "case PageSize={asked}: the clamped page still covers this store"
        );
    }
}

// ---------------------------------------------------------------------------
// SendVoiceMessage
// ---------------------------------------------------------------------------

/// RULE: every accepted `SendVoiceMessage` is given its own `MessageId` — never a
/// constant — and the message it accepted is recorded in state.
///
/// The rows differ in the kind of message body (the three `VoiceMessageContent`
/// alternatives, plus a request with no content), and each is sent more than once so a
/// repeated identical request still yields distinct ids.
#[tokio::test]
async fn each_accepted_voice_message_gets_its_own_id_and_is_recorded() {
    let bodies: &[(&str, Value)] = &[
        (
            "plain text",
            json!({ "PlainTextMessage": { "Text": "hello", "LanguageCode": "en-US", "VoiceId": "Matthew" } }),
        ),
        (
            "ssml",
            json!({ "SSMLMessage": { "Text": "<speak>hi</speak>", "LanguageCode": "en-GB", "VoiceId": "Amy" } }),
        ),
        (
            "call instructions",
            json!({ "CallInstructionsMessage": { "Text": "press 1" } }),
        ),
        (
            "plain text with no voice chosen",
            json!({ "PlainTextMessage": { "Text": "bare" } }),
        ),
    ];

    let harness = Harness::new();
    assert_eq!(harness.create_set("demo-set").await.status, 200);

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut sent = 0usize;
    for (case, content) in bodies {
        for attempt in 0..3 {
            let answer = harness
                .send_voice(json!({
                    "CallerId": "+12065550111",
                    "ConfigurationSetName": "demo-set",
                    "DestinationPhoneNumber": "+12065550100",
                    "OriginationPhoneNumber": "+12065550199",
                    "Content": content,
                }))
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} attempt {attempt}: expected 200, got {} {}",
                answer.status, answer.body
            );
            let message_id = answer
                .body
                .get("MessageId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                !message_id.is_empty(),
                "case {case:?} attempt {attempt}: MessageId must be non-empty, got {}",
                answer.body
            );
            assert!(
                seen.insert(message_id.clone()),
                "case {case:?} attempt {attempt}: MessageId {message_id} was already issued"
            );
            sent += 1;
        }
    }

    // The sends are observable in state, with the ids that were reported.
    let store = harness
        .engine
        .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>();
    let recorded = store.lock();
    assert_eq!(
        recorded.sent_messages.len(),
        sent,
        "every accepted message must be recorded, got {}",
        recorded.sent_messages.len()
    );
    let recorded_ids: BTreeSet<String> = recorded
        .sent_messages
        .iter()
        .map(|message| message.message_id.clone())
        .collect();
    assert_eq!(
        recorded_ids, seen,
        "the recorded ids must be exactly the reported ones"
    );
    let first = &recorded.sent_messages[0];
    assert_eq!(
        (
            first.destination_phone_number.as_deref(),
            first.origination_phone_number.as_deref(),
            first.caller_id.as_deref(),
            first.configuration_set_name.as_deref(),
        ),
        (
            Some("+12065550100"),
            Some("+12065550199"),
            Some("+12065550111"),
            Some("demo-set"),
        ),
        "the recorded message must carry what was sent, got {first:?}"
    );
    assert_eq!(
        first
            .content
            .plain_text
            .as_ref()
            .and_then(|body| body.text.as_deref()),
        Some("hello"),
        "the recorded content must carry the message body, got {:?}",
        first.content
    );
}

/// RULE: a voice message with no recipient, or whose `Content` does not carry exactly
/// one of the three `VoiceMessageContent` bodies, is refused as invalid input (400) and
/// recorded nowhere.
///
/// A JSON `null` where the model declares a string is refused by the codec with the
/// framework's `ValidationException`, still 400.
#[tokio::test]
async fn unsendable_voice_messages_are_bad_requests() {
    let plain = json!({ "PlainTextMessage": { "Text": "a" } });
    let ssml = json!({ "SSMLMessage": { "Text": "<speak>b</speak>" } });
    let instructions = json!({ "CallInstructionsMessage": { "Text": "c" } });

    let cases: &[(&str, Value, &str)] = &[
        (
            "no destination",
            json!({ "Content": plain }),
            "BadRequestException",
        ),
        (
            "null destination",
            json!({ "DestinationPhoneNumber": null, "Content": plain }),
            "ValidationException",
        ),
        (
            "empty destination",
            json!({ "DestinationPhoneNumber": "", "Content": plain }),
            "BadRequestException",
        ),
        (
            "blank destination",
            json!({ "DestinationPhoneNumber": "   ", "Content": plain }),
            "BadRequestException",
        ),
        (
            "no Content member",
            json!({ "DestinationPhoneNumber": "+12065550100" }),
            "BadRequestException",
        ),
        (
            "empty Content",
            json!({ "DestinationPhoneNumber": "+12065550100", "Content": {} }),
            "BadRequestException",
        ),
        (
            "two bodies",
            json!({
                "DestinationPhoneNumber": "+12065550100",
                "Content": { "PlainTextMessage": plain["PlainTextMessage"], "SSMLMessage": ssml["SSMLMessage"] },
            }),
            "BadRequestException",
        ),
        (
            "three bodies",
            json!({
                "DestinationPhoneNumber": "+12065550100",
                "Content": {
                    "PlainTextMessage": plain["PlainTextMessage"],
                    "SSMLMessage": ssml["SSMLMessage"],
                    "CallInstructionsMessage": instructions["CallInstructionsMessage"],
                },
            }),
            "BadRequestException",
        ),
    ];

    let harness = Harness::new();
    for (case, request, expected_code) in cases {
        let answer = harness.send_voice(request.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {}",
            answer.body
        );
    }

    assert!(
        harness
            .engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>()
            .lock()
            .sent_messages
            .is_empty(),
        "a refused send must record nothing"
    );
}

// ---------------------------------------------------------------------------
// DeleteConfigurationSet
// ---------------------------------------------------------------------------

/// RULE: deleting a configuration set removes it and the event destinations it owns, so
/// afterwards it is absent from the listing and every operation that resolves it answers
/// `NotFoundException` (404) — including a second delete.
///
/// The rows differ in how many destinations the deleted set carried.
#[tokio::test]
async fn delete_removes_the_set_and_its_event_destinations() {
    for destinations in [0usize, 1, 3] {
        let harness = Harness::new();
        for name in ["keep-me", "demo-set"] {
            assert_eq!(
                harness.create_set(name).await.status,
                200,
                "case destinations={destinations}: creating {name}"
            );
        }
        for index in 0..destinations {
            assert_eq!(
                harness
                    .create_destination(
                        "demo-set",
                        &format!("dest-{index}"),
                        json!({ "Enabled": true, "MatchingEventTypes": ["COMPLETED_CALL"] }),
                    )
                    .await
                    .status,
                200,
                "case destinations={destinations}: creating dest-{index}"
            );
        }

        let deleted = harness.delete_set("demo-set").await;
        assert_eq!(
            deleted.status, 200,
            "case destinations={destinations}: delete must answer 200, got {} {}",
            deleted.status, deleted.body
        );

        // Gone from the listing, and gone from the store — the sibling set survives.
        let listed = harness.list_sets("").await;
        assert_eq!(
            listed_names(&listed),
            vec!["keep-me".to_string()],
            "case destinations={destinations}: the deleted name must be absent, got {}",
            listed.body
        );
        assert_eq!(
            harness.stored_set_names(),
            vec!["keep-me".to_string()],
            "case destinations={destinations}: the deleted set must be out of the store"
        );

        let read = harness.get_destinations("demo-set").await;
        assert_eq!(
            read.status, 404,
            "case destinations={destinations}: reading a deleted set must answer 404, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.error_code(),
            "NotFoundException",
            "case destinations={destinations}: expected NotFoundException, got {}",
            read.body
        );

        let again = harness.delete_set("demo-set").await;
        assert_eq!(
            again.status, 404,
            "case destinations={destinations}: a second delete must answer 404, got {} {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "NotFoundException",
            "case destinations={destinations}: expected NotFoundException, got {}",
            again.body
        );

        // Re-creating the name starts clean: the old destinations went with the set.
        assert_eq!(
            harness.create_set("demo-set").await.status,
            200,
            "case destinations={destinations}: the name must be reusable after delete"
        );
        let read = harness.get_destinations("demo-set").await;
        assert_eq!(
            read.body.get("EventDestinations"),
            Some(&json!([])),
            "case destinations={destinations}: a re-created set must inherit no destinations, got {}",
            read.body
        );
    }
}

/// RULE: `UpdateConfigurationSetEventDestination` changes exactly the members its
/// definition carries and leaves the rest of the stored destination alone, and
/// `DeleteConfigurationSetEventDestination` removes one destination from its set and
/// nothing else — both read back through `GetConfigurationSetEventDestinations`.
///
/// Those two operations' own behaviour is pinned over a table in
/// `event_destination_updates.rs`; what this row adds is that they take part in the
/// lifecycle this suite drives.
#[tokio::test]
async fn the_remaining_event_destination_operations_edit_one_destination() {
    let harness = Harness::new();
    assert_eq!(harness.create_set("demo-set").await.status, 200);
    for name in ["dest1", "dest2"] {
        assert_eq!(
            harness
                .create_destination("demo-set", name, json!({ "Enabled": true }))
                .await
                .status,
            200,
            "creating {name}"
        );
    }

    let updated = harness
        .call(
            "PUT",
            "/v1/sms-voice/configuration-sets/demo-set/event-destinations/dest1",
            Some(json!({ "EventDestination": { "Enabled": false } })),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "the update must answer 200, got {} {}",
        updated.status, updated.body
    );

    let deleted = harness
        .call(
            "DELETE",
            "/v1/sms-voice/configuration-sets/demo-set/event-destinations/dest2",
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "the delete must answer 200, got {} {}",
        deleted.status, deleted.body
    );

    // The update is visible and the delete removed only its own destination.
    let read = harness.get_destinations("demo-set").await;
    assert_eq!(
        read.body.get("EventDestinations"),
        Some(&json!([{ "Name": "dest1", "Enabled": false }])),
        "expected the updated destination alone, got {}",
        read.body
    );
}
