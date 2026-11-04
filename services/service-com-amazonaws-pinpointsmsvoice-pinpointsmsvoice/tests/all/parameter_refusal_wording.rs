// SPDX-License-Identifier: CC0-1.0

//! How Pinpoint SMS Voice words a request it refuses for a parameter it cannot use, and
//! where it reports that wording on the wire.
//!
//! RULE: a parameter this service cannot use is refused with the one client-input error
//! every operation in the model declares — `BadRequestException`, HTTP 400, `restJson1`
//! `application/json` with `x-amzn-errortype: BadRequestException` — and the detail text
//! goes in the body member `message`, naming the parameter(s) the REQUEST carried and
//! nothing else:
//!
//! * a `@required` parameter bound to the URI that arrived with no value at all is
//!   `Missing required request parameters: [<Member>]`, by the modelled member name, and
//!   is answered BEFORE any lookup — so it is a 400 and not the 404 a name that merely
//!   does not exist gets;
//! * a parameter that reached this service's own validation carrying a value it cannot use
//!   is `The following parameter is null, empty or invalid: ['<parameter>']`, by the
//!   service's own lower-camel spelling of that member. A URI label that is PRESENT binds a
//!   value, so a blank one is this class rather than the one above.
//!
//! One parameter is the exception that the member carries nothing for: a `PageSize` this
//! service cannot read as a page bound is refused with the same code and status and NO
//! detail member at all, so the rows for it assert the member's absence.
//!
//! The same member carries the text for EVERY error this service's model declares, which
//! the `NotFoundException` rows pin, and a refused request changes nothing — the listing
//! that follows one still answers 200 with its `ConfigurationSets` member.
//!
//! Each suite drives the real `restJson1` router, so the requests here are the bytes an
//! AWS SDK `pinpoint-sms-voice` client sends. The rows differ along what the rule
//! generalises over: the operation, the HTTP method, where the parameter travels (a URI
//! path label, a query parameter, a body member), and the FORM of the unusable value
//! (absent, empty, blank, unparseable, out of range, contradictory).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pinpointsmsvoice_pinpointsmsvoice::{
    ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState, EmulatedService,
};

/// The emulator under test: the service mounted on the production router, over the one
/// shared engine the test also reads state from.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// One HTTP answer: the status, the `x-amzn-errortype` header, and the parsed JSON body
/// (`Value::Null` when empty).
struct Answer {
    status: u16,
    error_type_header: String,
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

    /// The detail text under the member this service reports it in.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// Whether the body carries the modelled member name instead — the thing the rename
    /// in `service.rs` exists to prevent, since no Pinpoint SMS Voice client reads it.
    fn has_modelled_member(&self) -> bool {
        self.body.get("Message").is_some()
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
        // This suite is about how a MALFORMED request is worded, which is answered ahead
        // of the authorization decision — the account holds the entitlement this API's
        // resource-scoped actions need so the rows that name a set reach their parameter
        // rather than that decision. The entitlement is pinned in
        // `legacy_surface_authorization.rs`, including that validation still precedes it.
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
            .uri(format!("https://sms-voice.pinpoint.us-east-1.amazonaws.com{path}"))
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
        let error_type_header = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error_type_header,
            body,
        }
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

/// One refusal case: how it is sent, and the sentence it must come back with.
struct Case {
    /// What makes this row different from the others.
    name: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<fn() -> Value>,
    /// The sentence the refusal carries, or `None` where the service carries NO detail
    /// member at all.
    message: Option<&'static str>,
}

/// The sentence for a `@required` URI parameter that arrived with no value.
const MISSING_CONFIGURATION_SET_NAME: &str =
    "Missing required request parameters: [ConfigurationSetName]";

/// The sentences for a parameter this service's validation cannot use, one per parameter
/// the rows below exercise. `PageSize` has none: that refusal carries no detail member
/// (see `ops/list_configuration_sets.rs`), which the `None` rows below pin.
const INVALID_NEXT_TOKEN: &str = "The following parameter is null, empty or invalid: ['nextToken']";
const INVALID_CONFIGURATION_SET_NAME: &str =
    "The following parameter is null, empty or invalid: ['configurationSetName']";
const INVALID_EVENT_DESTINATION_NAME: &str =
    "The following parameter is null, empty or invalid: ['eventDestinationName']";
const INVALID_DESTINATION_PHONE_NUMBER: &str =
    "The following parameter is null, empty or invalid: ['destinationPhoneNumber']";
/// `Content` is the one parameter with a sentence of its own: what is wrong is WHICH of
/// the three message bodies the request chose, so the refusal names them instead of
/// naming the member (see `ops/send_voice_message.rs`).
const INVALID_CONTENT: &str = "Message is invalid. The message must have either a plain \
     text, SSML, or call instructions message";

/// A configuration set the invalid-parameter rows that need one can resolve, so a row
/// cannot pass merely because the set was missing.
const EXISTING_SET: &str = "refusal-wording-set";

/// Every row refuses one request for one parameter. They span three bindings (a URI path
/// label, a query parameter, a body member), five operations and six forms of unusable
/// value — and, for a URI-bound parameter, both layers: a label that bound NO value is the
/// request-binding layer's sentence, one that bound a BLANK value is this service's
/// validation's.
const CASES: &[Case] = &[
    // ---- a @required URI parameter with no value -------------------------------------
    Case {
        name: "GetConfigurationSetEventDestinations with an EMPTY path label",
        method: "GET",
        path: "/v1/sms-voice/configuration-sets//event-destinations",
        body: None,
        message: Some(MISSING_CONFIGURATION_SET_NAME),
    },
    Case {
        name: "CreateConfigurationSetEventDestination with an EMPTY path label",
        method: "POST",
        path: "/v1/sms-voice/configuration-sets//event-destinations",
        body: Some(named_destination),
        message: Some(MISSING_CONFIGURATION_SET_NAME),
    },
    // ---- a @required URI parameter that bound a BLANK value --------------------------
    // A label that is present binds a value, so it reaches this service's own validation
    // rather than failing in the request-binding layer: same error and status, the other
    // sentence.
    Case {
        name: "GetConfigurationSetEventDestinations with a BLANK path label",
        method: "GET",
        path: "/v1/sms-voice/configuration-sets/%20/event-destinations",
        body: None,
        message: Some(INVALID_CONFIGURATION_SET_NAME),
    },
    Case {
        name: "DeleteConfigurationSet with a BLANK path label",
        method: "DELETE",
        path: "/v1/sms-voice/configuration-sets/%20",
        body: None,
        message: Some(INVALID_CONFIGURATION_SET_NAME),
    },
    Case {
        name: "CreateConfigurationSetEventDestination with a BLANK path label",
        method: "POST",
        path: "/v1/sms-voice/configuration-sets/%20%20/event-destinations",
        body: Some(named_destination),
        message: Some(INVALID_CONFIGURATION_SET_NAME),
    },
    // ---- a query parameter this service cannot use -----------------------------------
    Case {
        name: "ListConfigurationSets with a NextToken that is not even hexadecimal",
        method: "GET",
        path: "/v1/sms-voice/configuration-sets?NextToken=not-a-token",
        body: None,
        message: Some(INVALID_NEXT_TOKEN),
    },
    Case {
        name: "ListConfigurationSets with a NextToken this service never minted",
        // Hexadecimal that decodes cleanly to text carrying no cursor marker.
        method: "GET",
        path: "/v1/sms-voice/configuration-sets?NextToken=6e6f70652d6e6f742d6d696e65",
        body: None,
        message: Some(INVALID_NEXT_TOKEN),
    },
    Case {
        name: "ListConfigurationSets with a non-numeric PageSize",
        method: "GET",
        path: "/v1/sms-voice/configuration-sets?PageSize=not-a-number",
        body: None,
        message: None,
    },
    Case {
        name: "ListConfigurationSets with a PageSize of zero, which pages nothing",
        method: "GET",
        path: "/v1/sms-voice/configuration-sets?PageSize=0",
        body: None,
        message: None,
    },
    Case {
        name: "ListConfigurationSets with a negative PageSize",
        method: "GET",
        path: "/v1/sms-voice/configuration-sets?PageSize=-5",
        body: None,
        message: None,
    },
    Case {
        name: "ListConfigurationSets with a fractional PageSize",
        method: "GET",
        path: "/v1/sms-voice/configuration-sets?PageSize=2.5",
        body: None,
        message: None,
    },
    // ---- a body member this service cannot use ---------------------------------------
    Case {
        name: "CreateConfigurationSet with no ConfigurationSetName at all",
        method: "POST",
        path: "/v1/sms-voice/configuration-sets",
        body: Some(empty_object),
        message: Some(INVALID_CONFIGURATION_SET_NAME),
    },
    Case {
        name: "CreateConfigurationSet with an empty ConfigurationSetName",
        method: "POST",
        path: "/v1/sms-voice/configuration-sets",
        body: Some(empty_set_name),
        message: Some(INVALID_CONFIGURATION_SET_NAME),
    },
    Case {
        name: "CreateConfigurationSet with a blank ConfigurationSetName",
        method: "POST",
        path: "/v1/sms-voice/configuration-sets",
        body: Some(blank_set_name),
        message: Some(INVALID_CONFIGURATION_SET_NAME),
    },
    Case {
        name: "CreateConfigurationSetEventDestination with no EventDestinationName",
        method: "POST",
        path: "/v1/sms-voice/configuration-sets/refusal-wording-set/event-destinations",
        body: Some(empty_object),
        message: Some(INVALID_EVENT_DESTINATION_NAME),
    },
    Case {
        name: "CreateConfigurationSetEventDestination with a blank EventDestinationName",
        method: "POST",
        path: "/v1/sms-voice/configuration-sets/refusal-wording-set/event-destinations",
        body: Some(blank_destination_name),
        message: Some(INVALID_EVENT_DESTINATION_NAME),
    },
    Case {
        name: "SendVoiceMessage with no DestinationPhoneNumber",
        method: "POST",
        path: "/v1/sms-voice/voice/message",
        body: Some(message_without_destination),
        message: Some(INVALID_DESTINATION_PHONE_NUMBER),
    },
    Case {
        name: "SendVoiceMessage with a blank DestinationPhoneNumber",
        method: "POST",
        path: "/v1/sms-voice/voice/message",
        body: Some(message_with_blank_destination),
        message: Some(INVALID_DESTINATION_PHONE_NUMBER),
    },
    Case {
        name: "SendVoiceMessage whose Content carries no message body",
        method: "POST",
        path: "/v1/sms-voice/voice/message",
        body: Some(message_without_content),
        message: Some(INVALID_CONTENT),
    },
    Case {
        name: "SendVoiceMessage whose Content carries two message bodies",
        method: "POST",
        path: "/v1/sms-voice/voice/message",
        body: Some(message_with_two_bodies),
        message: Some(INVALID_CONTENT),
    },
];

fn empty_object() -> Value {
    json!({})
}

fn empty_set_name() -> Value {
    json!({ "ConfigurationSetName": "" })
}

fn blank_set_name() -> Value {
    json!({ "ConfigurationSetName": "   " })
}

fn named_destination() -> Value {
    json!({ "EventDestinationName": "events", "EventDestination": { "Enabled": true } })
}

fn blank_destination_name() -> Value {
    json!({ "EventDestinationName": "  ", "EventDestination": { "Enabled": true } })
}

fn message_without_destination() -> Value {
    json!({ "Content": { "PlainTextMessage": { "Text": "hello" } } })
}

fn message_with_blank_destination() -> Value {
    json!({
        "DestinationPhoneNumber": " ",
        "Content": { "PlainTextMessage": { "Text": "hello" } },
    })
}

fn message_without_content() -> Value {
    json!({ "DestinationPhoneNumber": "+15550000001" })
}

fn message_with_two_bodies() -> Value {
    json!({
        "DestinationPhoneNumber": "+15550000001",
        "Content": {
            "PlainTextMessage": { "Text": "hello" },
            "SSMLMessage": { "Text": "<speak>hello</speak>" },
        },
    })
}

#[tokio::test]
async fn a_parameter_this_service_cannot_use_is_refused_in_its_own_wording() {
    let harness = Harness::new();
    // The rows that name an existing set must fail on their parameter, not on a missing
    // configuration set.
    let created = harness
        .call(
            "POST",
            "/v1/sms-voice/configuration-sets",
            Some(json!({ "ConfigurationSetName": EXISTING_SET })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture configuration set must be created, got {} {:?}",
        created.status, created.body
    );

    for case in CASES {
        let answer = harness
            .call(case.method, case.path, case.body.map(|build| build()))
            .await;

        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400, got {} {:?}",
            case.name, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {:?}: expected the modelled BadRequestException, got {:?}",
            case.name,
            answer.body
        );
        assert_eq!(
            answer.error_type_header, "BadRequestException",
            "case {:?}: expected x-amzn-errortype BadRequestException, got {:?}",
            case.name, answer.error_type_header
        );
        match case.message {
            Some(expected) => assert_eq!(
                answer.message(),
                expected,
                "case {:?}: expected the message {expected:?}, got {:?}",
                case.name,
                answer.body
            ),
            // No detail member AT ALL: not an empty one, and not the modelled spelling
            // of it either.
            None => assert!(
                answer.body.get("message").is_none() && !answer.has_modelled_member(),
                "case {:?}: expected no detail member, got {:?}",
                case.name,
                answer.body
            ),
        }
        assert!(
            !answer.has_modelled_member(),
            "case {:?}: the detail text must travel in `message` alone, got {:?}",
            case.name,
            answer.body
        );
    }

    // A refused request changes nothing: only the set created above is stored.
    assert_eq!(
        harness.stored_set_names(),
        vec![EXISTING_SET.to_string()],
        "a refusal must leave the store untouched, got {:?}",
        harness.stored_set_names()
    );
}

/// RULE (the same member, for every error this service's model declares): a configuration
/// set name that is present but names nothing is the modelled `NotFoundException` (404) —
/// not the 400 an unusable parameter gets — and its detail text travels in `message` too.
///
/// The rows differ in the operation and the method, so the member is pinned on more than
/// the one read the refusal rows exercise.
#[tokio::test]
async fn a_name_that_resolves_to_nothing_is_not_found_in_the_same_member() {
    let harness = Harness::new();

    for (case, method, path) in [
        (
            "GetConfigurationSetEventDestinations",
            "GET",
            "/v1/sms-voice/configuration-sets/absent-set/event-destinations",
        ),
        (
            "DeleteConfigurationSet",
            "DELETE",
            "/v1/sms-voice/configuration-sets/absent-set",
        ),
    ] {
        let answer = harness.call(method, path, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "case {case:?}: expected the modelled NotFoundException, got {:?}",
            answer.body
        );
        assert!(
            !answer.message().is_empty(),
            "case {case:?}: expected detail text in `message`, got {:?}",
            answer.body
        );
        assert!(
            !answer.has_modelled_member(),
            "case {case:?}: the detail text must travel in `message` alone, got {:?}",
            answer.body
        );
    }

    // Nothing was created by either read.
    assert!(
        harness.stored_set_names().is_empty(),
        "a not-found read must store nothing, got {:?}",
        harness.stored_set_names()
    );
}

/// RULE (the positive control behind every refusal): a rejected request leaves the account
/// readable — the `ListConfigurationSets` that follows one answers 200 with its
/// `ConfigurationSets` member, listing exactly what is stored.
///
/// The rows differ in which refusal preceded the read, so the control is not tied to one
/// kind of rejected request.
#[tokio::test]
async fn a_listing_after_a_refused_request_still_answers_with_its_sets() {
    let harness = Harness::new();
    for name in ["alpha", "beta"] {
        let created = harness
            .call(
                "POST",
                "/v1/sms-voice/configuration-sets",
                Some(json!({ "ConfigurationSetName": name })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the set must be created, got {} {:?}",
            created.status, created.body
        );
    }

    for (case, path) in [
        (
            "after a garbage NextToken",
            "/v1/sms-voice/configuration-sets?NextToken=not-a-token",
        ),
        (
            "after a non-numeric PageSize",
            "/v1/sms-voice/configuration-sets?PageSize=not-a-number",
        ),
        (
            "after an empty path label",
            "/v1/sms-voice/configuration-sets//event-destinations",
        ),
    ] {
        let refused = harness.call("GET", path, None).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: the request must be refused, got {} {:?}",
            refused.status, refused.body
        );

        let listed = harness
            .call("GET", "/v1/sms-voice/configuration-sets", None)
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the follow-up listing must succeed, got {} {:?}",
            listed.status, listed.body
        );
        let names: Vec<String> = listed
            .body
            .get("ConfigurationSets")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| entry.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names,
            vec!["alpha".to_string(), "beta".to_string()],
            "case {case:?}: expected the stored sets under ConfigurationSets, got {:?}",
            listed.body
        );
    }
}
