// SPDX-License-Identifier: CC0-1.0

//! Which actions of this API the account this emulator serves may perform at all, and
//! what it is answered when it may not.
//!
//! RULE: this API's RESOURCE-SCOPED actions — every action on a configuration set, on an
//! event destination, or on a voice call — are authorized only for an account holding the
//! entitlement for this legacy surface. For an account without it the service answers HTTP
//! 403, error code `AccessDeniedException` and the message `Access Denied`, before it
//! resolves the named resource and whatever else the request carried, and nothing is
//! created, changed, removed or sent. The ACCOUNT-SCOPED listing names no resource, so it
//! keeps answering what is stored. Request-shape validation still comes first, so a
//! malformed request is the modelled `BadRequestException` (400) even for an account that
//! may perform nothing.
//!
//! The suites drive the real `restJson1` router, so the requests here are the bytes an AWS
//! SDK `pinpoint-sms-voice` client sends. The rows differ along what the rule generalises
//! over: the operation and its method, whether the resource the request names EXISTS,
//! which payload it carries, and — for the ordering suite — where the malformed parameter
//! travels.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pinpointsmsvoice_pinpointsmsvoice::state::{
    ConfigurationSet, EventDestinationRecord,
};
use service_com_amazonaws_pinpointsmsvoice_pinpointsmsvoice::{
    ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState, EmulatedService,
};

/// The HTTP status an authorization refusal carries.
const DENIED_STATUS: u16 = 403;
/// The error code an authorization refusal carries, which a client branches on.
const DENIED_CODE: &str = "AccessDeniedException";
/// The whole message an authorization refusal carries.
const DENIED_MESSAGE: &str = "Access Denied";

/// A configuration set the rows put in the store directly, so a refusal that would
/// otherwise be a 404 cannot be mistaken for one: the resource IS there and the request is
/// still refused.
const EXISTING_SET: &str = "authorization-set";
/// An event destination of [`EXISTING_SET`], there for the same reason.
const EXISTING_DESTINATION: &str = "authorization-dest";
/// A name no request creates, so every action naming it addresses nothing.
const ABSENT_SET: &str = "authorization-absent-set";

/// The emulator under test: the service mounted on the production router, over the one
/// shared engine the test also reads and seeds state through.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// One HTTP answer: the status, the `x-amzn-errortype` header and the parsed JSON body
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
}

impl Harness {
    /// A harness whose account does NOT hold the entitlement — the default state, and
    /// what the gate suites drive.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suites drive unsigned requests; signature verification is the framework's
        // own concern and is covered where it lives.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Grant the account the entitlement this API's resource-scoped actions need. There is
    /// no operation in this model that grants it, so a suite that needs the actions to run
    /// states it in the store, exactly as the handlers read it.
    fn entitle(&self) {
        self.engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>()
            .lock()
            .legacy_surface_entitled = true;
    }

    /// Put a configuration set carrying one event destination in the store WITHOUT going
    /// through the API, so a gated request can name a resource that really exists.
    fn seed_set_with_destination(&self) {
        let mut set = ConfigurationSet::new(EXISTING_SET);
        set.event_destinations.insert(
            EXISTING_DESTINATION.to_string(),
            EventDestinationRecord {
                name: EXISTING_DESTINATION.to_string(),
                enabled: Some(true),
                ..EventDestinationRecord::default()
            },
        );
        self.engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>()
            .lock()
            .configuration_sets
            .insert(EXISTING_SET.to_string(), set);
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

    /// A snapshot of everything a resource-scoped action could have changed: the stored
    /// sets with their destinations, and how many voice messages were accepted.
    fn snapshot(&self) -> (Vec<(String, Vec<String>)>, usize) {
        let state = self
            .engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>();
        let store = state.lock();
        let sets = store
            .configuration_sets
            .iter()
            .map(|(name, set)| (name.clone(), set.event_destinations.keys().cloned().collect()))
            .collect();
        (sets, store.sent_messages.len())
    }
}

/// One request a resource-scoped action is reached by.
struct Action {
    /// What makes this row different from the others.
    name: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<fn() -> Value>,
}

fn sns_destination() -> Value {
    json!({
        "EventDestinationName": "sns-dest",
        "EventDestination": {
            "Enabled": true,
            "MatchingEventTypes": ["ANSWERED"],
            "SnsDestination": { "TopicArn": "arn:aws:sns:us-east-1:000000000000:voice-events" },
        },
    })
}

fn cloudwatch_destination() -> Value {
    json!({
        "EventDestinationName": "logs-dest",
        "EventDestination": {
            "Enabled": true,
            "MatchingEventTypes": ["FAILED"],
            "CloudWatchLogsDestination": {
                "IamRoleArn": "arn:aws:iam::000000000000:role/voice-logs",
                "LogGroupArn": "arn:aws:logs:us-east-1:000000000000:log-group:/voice:*",
            },
        },
    })
}

fn firehose_destination() -> Value {
    json!({
        "EventDestinationName": "firehose-dest",
        "EventDestination": {
            "MatchingEventTypes": ["BUSY"],
            "KinesisFirehoseDestination": {
                "DeliveryStreamArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/voice",
                "IamRoleArn": "arn:aws:iam::000000000000:role/voice-firehose",
            },
        },
    })
}

fn empty_destination() -> Value {
    json!({ "EventDestinationName": "bare-dest" })
}

fn definition_only() -> Value {
    json!({ "EventDestination": { "Enabled": false } })
}

fn no_definition() -> Value {
    json!({})
}

fn well_formed_voice_message() -> Value {
    json!({
        "DestinationPhoneNumber": "+14255550182",
        "OriginationPhoneNumber": "+12065550100",
        "Content": { "PlainTextMessage": { "Text": "a call", "VoiceId": "Joanna" } },
    })
}

fn voice_message_to_a_malformed_number() -> Value {
    json!({
        "DestinationPhoneNumber": "not-a-phone-number",
        "OriginationPhoneNumber": "+12065550100",
        "Content": { "PlainTextMessage": { "Text": "a call", "VoiceId": "Joanna" } },
    })
}

fn voice_message_with_configuration_set() -> Value {
    json!({
        "ConfigurationSetName": EXISTING_SET,
        "DestinationPhoneNumber": "+14255550182",
        "Content": { "CallInstructionsMessage": { "Text": "press 1" } },
    })
}

/// Every resource-scoped action of this API, reached on a resource that EXISTS and on one
/// that never did, with payloads that differ in which destination shape they name and in
/// how the voice message is addressed — the rule holds across all of it, so the rows span
/// the whole gated surface rather than one call.
const GATED: &[Action] = &[
    Action {
        name: "CreateConfigurationSet with a name nothing holds",
        method: "POST",
        path: "/v1/sms-voice/configuration-sets",
        body: Some(create_set_body),
    },
    Action {
        name: "CreateConfigurationSet with a name the store already holds",
        method: "POST",
        path: "/v1/sms-voice/configuration-sets",
        body: Some(create_existing_set_body),
    },
    Action {
        name: "CreateConfigurationSetEventDestination with an SNS destination",
        method: "POST",
        path: "/v1/sms-voice/configuration-sets/authorization-set/event-destinations",
        body: Some(sns_destination),
    },
    Action {
        name: "CreateConfigurationSetEventDestination with a CloudWatch Logs destination",
        method: "POST",
        path: "/v1/sms-voice/configuration-sets/authorization-set/event-destinations",
        body: Some(cloudwatch_destination),
    },
    Action {
        name: "CreateConfigurationSetEventDestination with a Firehose destination",
        method: "POST",
        path: "/v1/sms-voice/configuration-sets/authorization-set/event-destinations",
        body: Some(firehose_destination),
    },
    Action {
        name: "CreateConfigurationSetEventDestination with no definition at all",
        method: "POST",
        path: "/v1/sms-voice/configuration-sets/authorization-set/event-destinations",
        body: Some(empty_destination),
    },
    Action {
        name: "CreateConfigurationSetEventDestination under a set that does not exist",
        method: "POST",
        path: "/v1/sms-voice/configuration-sets/authorization-absent-set/event-destinations",
        body: Some(sns_destination),
    },
    Action {
        name: "UpdateConfigurationSetEventDestination on a destination that exists",
        method: "PUT",
        path: "/v1/sms-voice/configuration-sets/authorization-set/event-destinations/authorization-dest",
        body: Some(definition_only),
    },
    Action {
        name: "UpdateConfigurationSetEventDestination carrying no definition",
        method: "PUT",
        path: "/v1/sms-voice/configuration-sets/authorization-set/event-destinations/authorization-dest",
        body: Some(no_definition),
    },
    Action {
        name: "UpdateConfigurationSetEventDestination on a set that does not exist",
        method: "PUT",
        path: "/v1/sms-voice/configuration-sets/authorization-absent-set/event-destinations/whatever",
        body: Some(definition_only),
    },
    Action {
        name: "DeleteConfigurationSetEventDestination on a destination that exists",
        method: "DELETE",
        path: "/v1/sms-voice/configuration-sets/authorization-set/event-destinations/authorization-dest",
        body: None,
    },
    Action {
        name: "DeleteConfigurationSetEventDestination on a set that does not exist",
        method: "DELETE",
        path: "/v1/sms-voice/configuration-sets/authorization-absent-set/event-destinations/whatever",
        body: None,
    },
    Action {
        name: "GetConfigurationSetEventDestinations on a set that exists",
        method: "GET",
        path: "/v1/sms-voice/configuration-sets/authorization-set/event-destinations",
        body: None,
    },
    Action {
        name: "GetConfigurationSetEventDestinations on a set that does not exist",
        method: "GET",
        path: "/v1/sms-voice/configuration-sets/authorization-absent-set/event-destinations",
        body: None,
    },
    Action {
        name: "DeleteConfigurationSet on a set that exists",
        method: "DELETE",
        path: "/v1/sms-voice/configuration-sets/authorization-set",
        body: None,
    },
    Action {
        name: "DeleteConfigurationSet on a set that does not exist",
        method: "DELETE",
        path: "/v1/sms-voice/configuration-sets/authorization-absent-set",
        body: None,
    },
    Action {
        name: "SendVoiceMessage to a well-formed number",
        method: "POST",
        path: "/v1/sms-voice/voice/message",
        body: Some(well_formed_voice_message),
    },
    Action {
        name: "SendVoiceMessage to a number that is not a phone number",
        method: "POST",
        path: "/v1/sms-voice/voice/message",
        body: Some(voice_message_to_a_malformed_number),
    },
    Action {
        name: "SendVoiceMessage naming a configuration set that exists",
        method: "POST",
        path: "/v1/sms-voice/voice/message",
        body: Some(voice_message_with_configuration_set),
    },
];

fn create_set_body() -> Value {
    json!({ "ConfigurationSetName": "authorization-new-set" })
}

fn create_existing_set_body() -> Value {
    json!({ "ConfigurationSetName": EXISTING_SET })
}

/// RULE: for an account without the entitlement, EVERY resource-scoped action is refused
/// 403 `AccessDeniedException` / `Access Denied`, whether or not the resource it names
/// exists and whatever payload it carries, and the store is untouched.
#[tokio::test]
async fn an_unentitled_account_is_denied_every_resource_scoped_action() {
    let harness = Harness::new();
    harness.seed_set_with_destination();
    let before = harness.snapshot();

    for action in GATED {
        let answer = harness
            .call(action.method, action.path, action.body.map(|build| build()))
            .await;

        assert_eq!(
            answer.status, DENIED_STATUS,
            "case {:?}: expected HTTP {DENIED_STATUS}, got {} {:?}",
            action.name, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            DENIED_CODE,
            "case {:?}: expected the code {DENIED_CODE}, got {:?}",
            action.name,
            answer.body
        );
        assert_eq!(
            answer.error_type_header, DENIED_CODE,
            "case {:?}: expected x-amzn-errortype {DENIED_CODE}, got {:?}",
            action.name, answer.error_type_header
        );
        assert_eq!(
            answer.message(),
            DENIED_MESSAGE,
            "case {:?}: expected the message {DENIED_MESSAGE:?}, got {:?}",
            action.name,
            answer.body
        );
        assert_eq!(
            harness.snapshot(),
            before,
            "case {:?}: a denied action must change nothing",
            action.name
        );
    }
}

/// RULE: the denial is decided BEFORE the resource is resolved, so a request naming a
/// resource that exists and the same request naming one that never did get the identical
/// answer — the refusal tells a caller nothing about what the account holds.
#[tokio::test]
async fn the_denial_does_not_depend_on_whether_the_resource_exists() {
    let harness = Harness::new();
    harness.seed_set_with_destination();

    // (case, method, the path on the seeded resource, the same path on an absent one)
    let pairs: &[(&str, &str, &str, &str)] = &[
        (
            "GetConfigurationSetEventDestinations",
            "GET",
            "/v1/sms-voice/configuration-sets/authorization-set/event-destinations",
            "/v1/sms-voice/configuration-sets/authorization-absent-set/event-destinations",
        ),
        (
            "DeleteConfigurationSet",
            "DELETE",
            "/v1/sms-voice/configuration-sets/authorization-set",
            "/v1/sms-voice/configuration-sets/authorization-absent-set",
        ),
        (
            "DeleteConfigurationSetEventDestination",
            "DELETE",
            "/v1/sms-voice/configuration-sets/authorization-set/event-destinations/authorization-dest",
            "/v1/sms-voice/configuration-sets/authorization-set/event-destinations/nope",
        ),
    ];

    for (case, method, held, absent) in pairs {
        let on_held = harness.call(method, held, None).await;
        let on_absent = harness.call(method, absent, None).await;
        assert_eq!(
            (on_held.status, on_held.error_code(), on_held.message()),
            (
                on_absent.status,
                on_absent.error_code(),
                on_absent.message()
            ),
            "case {case:?}: expected the same refusal either way, got {:?} against {:?}",
            on_held.body,
            on_absent.body
        );
        assert_eq!(
            on_held.status, DENIED_STATUS,
            "case {case:?}: expected HTTP {DENIED_STATUS}, got {} {:?}",
            on_held.status, on_held.body
        );
    }
}

/// RULE: the listing is ACCOUNT-scoped, so it is not gated — it answers 200 with a
/// `ConfigurationSets` member reporting what is stored even while every resource-scoped
/// action is denied, before and after each denial.
#[tokio::test]
async fn the_account_scoped_listing_stays_open_while_every_action_is_denied() {
    let harness = Harness::new();
    harness.seed_set_with_destination();

    for (case, query) in [
        ("no arguments", ""),
        ("with a page bound", "?PageSize=5"),
        ("with a page bound of one", "?PageSize=1"),
    ] {
        let listed = harness
            .call(
                "GET",
                &format!("/v1/sms-voice/configuration-sets{query}"),
                None,
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the listing must answer 200, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.body.get("ConfigurationSets"),
            Some(&json!([EXISTING_SET])),
            "case {case:?}: the listing must report what is stored, got {:?}",
            listed.body
        );

        // A denial in between changes neither the listing nor the store.
        let denied = harness
            .call(
                "POST",
                "/v1/sms-voice/configuration-sets",
                Some(create_set_body()),
            )
            .await;
        assert_eq!(
            denied.status, DENIED_STATUS,
            "case {case:?}: the create must be denied, got {} {:?}",
            denied.status, denied.body
        );
        let after = harness
            .call("GET", "/v1/sms-voice/configuration-sets", None)
            .await;
        assert_eq!(
            after.body.get("ConfigurationSets"),
            Some(&json!([EXISTING_SET])),
            "case {case:?}: the denied create must not appear in the listing, got {:?}",
            after.body
        );
    }
}

/// One request whose SHAPE is wrong, and the sentence its refusal must carry — empty for
/// the one class of refusal that carries no detail member at all.
struct Malformed {
    /// What makes this row different from the others.
    name: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<fn() -> Value>,
    message: &'static str,
}

/// RULE: request-shape validation runs AHEAD of the authorization decision, so a
/// malformed request is the modelled `BadRequestException` (400) — naming the parameter
/// the service could not use — even for an account that may perform no action at all,
/// while the same request with a usable parameter is the 403.
///
/// The rows differ in where the unusable parameter travels (a URI label, a query
/// parameter, a body member) and in the form of the value.
#[tokio::test]
async fn a_malformed_request_is_refused_before_the_authorization_decision() {
    let harness = Harness::new();
    harness.seed_set_with_destination();

    // Each row is one malformed request and the sentence its refusal must carry.
    let rows: &[Malformed] = &[
        Malformed {
            name: "GetConfigurationSetEventDestinations with an empty path label",
            method: "GET",
            path: "/v1/sms-voice/configuration-sets//event-destinations",
            body: None,
            message: "Missing required request parameters: [ConfigurationSetName]",
        },
        Malformed {
            name: "DeleteConfigurationSet with a blank path label",
            method: "DELETE",
            path: "/v1/sms-voice/configuration-sets/%20",
            body: None,
            message: "The following parameter is null, empty or invalid: ['configurationSetName']",
        },
        Malformed {
            name: "DeleteConfigurationSetEventDestination with a blank destination label",
            method: "DELETE",
            path: "/v1/sms-voice/configuration-sets/authorization-set/event-destinations/%20",
            body: None,
            message: "The following parameter is null, empty or invalid: ['eventDestinationName']",
        },
        Malformed {
            name: "CreateConfigurationSet with a name the member's shape does not admit",
            method: "POST",
            path: "/v1/sms-voice/configuration-sets",
            body: Some(name_with_spaces_and_punctuation),
            message: "The following parameter is null, empty or invalid: ['configurationSetName']",
        },
        Malformed {
            name: "CreateConfigurationSet with no name at all",
            method: "POST",
            path: "/v1/sms-voice/configuration-sets",
            body: Some(no_definition),
            message: "The following parameter is null, empty or invalid: ['configurationSetName']",
        },
        Malformed {
            name: "SendVoiceMessage whose Content carries two message bodies",
            method: "POST",
            path: "/v1/sms-voice/voice/message",
            body: Some(voice_message_with_two_bodies),
            message: "Message is invalid. The message must have either a plain text, SSML, \
                 or call instructions message",
        },
        Malformed {
            // The one refusal of this class that carries no detail member at all.
            name: "ListConfigurationSets with a PageSize that is not a page bound",
            method: "GET",
            path: "/v1/sms-voice/configuration-sets?PageSize=not-a-number",
            body: None,
            message: "",
        },
    ];

    for case in rows {
        let answer = harness
            .call(case.method, case.path, case.body.map(|build| build()))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400 ahead of the authorization decision, got {} {:?}",
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
            answer.message(),
            case.message,
            "case {:?}: expected the message {:?}, got {:?}",
            case.name,
            case.message,
            answer.body
        );
    }

    // The ordering is only observable against the other answer: the SAME operations with
    // a usable parameter are the 403.
    for (case, method, path, body) in [
        (
            "CreateConfigurationSet with an admitted name",
            "POST",
            "/v1/sms-voice/configuration-sets",
            Some(create_set_body()),
        ),
        (
            "SendVoiceMessage with one message body",
            "POST",
            "/v1/sms-voice/voice/message",
            Some(well_formed_voice_message()),
        ),
        (
            "GetConfigurationSetEventDestinations with a usable label",
            "GET",
            "/v1/sms-voice/configuration-sets/authorization-set/event-destinations",
            None,
        ),
    ] {
        let answer = harness.call(method, path, body).await;
        assert_eq!(
            answer.status, DENIED_STATUS,
            "case {case:?}: expected HTTP {DENIED_STATUS}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            DENIED_CODE,
            "case {case:?}: expected the code {DENIED_CODE}, got {:?}",
            answer.body
        );
    }
}

/// RULE: an IAM role ANOTHER account holds is one no principal of this account may hand to
/// this service, so an event destination that passes one is refused with the authorization
/// code and status and the sentence for that failure — whatever the account's own
/// entitlement, and whether or not the resources named exist.
///
/// The rows differ in which destination carries the role, in which account holds it, and in
/// the operation that is handed it; the control rows pass a role of THIS account, and an
/// SNS destination passes no role at all, so neither is refused for this reason.
#[tokio::test]
async fn a_role_another_account_holds_cannot_be_passed_to_this_service() {
    /// The sentence this refusal carries, which is not the one the entitlement decision
    /// carries: a caller can act on this one.
    const CROSS_ACCOUNT: &str = "Cross-account pass role is not allowed.";
    /// The account every ARN this emulator mints carries (`engine::emulator_account_id`),
    /// so a role under it is one this service may be handed.
    const THIS_ACCOUNT: &str = "000000000000";

    // (case, the destination definition, the message the refusal must carry)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a CloudWatch Logs role another account holds",
            json!({ "CloudWatchLogsDestination": {
                "IamRoleArn": "arn:aws:iam::123456789012:role/voice-logs",
                "LogGroupArn": "arn:aws:logs:us-east-1:123456789012:log-group:/voice:*",
            } }),
            CROSS_ACCOUNT,
        ),
        (
            "a Firehose role another account holds",
            json!({ "KinesisFirehoseDestination": {
                "DeliveryStreamArn": "arn:aws:firehose:us-east-1:123456789012:deliverystream/voice",
                "IamRoleArn": "arn:aws:iam::123456789012:role/voice-firehose",
            } }),
            CROSS_ACCOUNT,
        ),
        (
            "a role under a path in another account",
            json!({ "CloudWatchLogsDestination": {
                "IamRoleArn": "arn:aws:iam::210987654321:role/service-role/voice-logs",
            } }),
            CROSS_ACCOUNT,
        ),
        (
            "a role of another account in another partition",
            json!({ "KinesisFirehoseDestination": {
                "IamRoleArn": "arn:aws-cn:iam::123456789012:role/voice-firehose",
            } }),
            CROSS_ACCOUNT,
        ),
        (
            "one destination of this account beside one of another",
            json!({
                "CloudWatchLogsDestination": {
                    "IamRoleArn": format!("arn:aws:iam::{THIS_ACCOUNT}:role/voice-logs"),
                },
                "KinesisFirehoseDestination": {
                    "IamRoleArn": "arn:aws:iam::123456789012:role/voice-firehose",
                },
            }),
            CROSS_ACCOUNT,
        ),
        (
            // The control: a role of THIS account passes, so the only refusal left is the
            // account's own entitlement.
            "a CloudWatch Logs role of this account",
            json!({ "CloudWatchLogsDestination": {
                "IamRoleArn": format!("arn:aws:iam::{THIS_ACCOUNT}:role/voice-logs"),
                "LogGroupArn": format!("arn:aws:logs:us-east-1:{THIS_ACCOUNT}:log-group:/voice:*"),
            } }),
            DENIED_MESSAGE,
        ),
        (
            "a role ARN carrying no account at all",
            json!({ "KinesisFirehoseDestination": { "IamRoleArn": "arn:aws:iam:::role/voice" } }),
            DENIED_MESSAGE,
        ),
        (
            // An SNS topic hands over no role, however the topic is addressed.
            "an SNS topic in another account, which passes no role",
            json!({ "SnsDestination": { "TopicArn": "arn:aws:sns:us-east-1:123456789012:voice" } }),
            DENIED_MESSAGE,
        ),
        (
            "no destination target at all",
            json!({ "Enabled": true }),
            DENIED_MESSAGE,
        ),
    ];

    // Both halves of the rule: for an account that is not entitled the pass-role refusal
    // comes FIRST, and for one that is entitled it is the only refusal left — a role of
    // this account is then served.
    for entitled in [false, true] {
        let harness = Harness::new();
        harness.seed_set_with_destination();
        if entitled {
            harness.entitle();
        }

        for (index, (case, definition, when_unentitled)) in rows.iter().enumerate() {
            let cross_account = *when_unentitled == CROSS_ACCOUNT;
            for (operation, method, path, body) in [
                (
                    "CreateConfigurationSetEventDestination",
                    "POST",
                    format!("/v1/sms-voice/configuration-sets/{EXISTING_SET}/event-destinations"),
                    json!({
                        "EventDestinationName": format!("passed-role-{index}"),
                        "EventDestination": definition,
                    }),
                ),
                (
                    "UpdateConfigurationSetEventDestination",
                    "PUT",
                    format!(
                        "/v1/sms-voice/configuration-sets/{EXISTING_SET}/event-destinations/\
                         {EXISTING_DESTINATION}"
                    ),
                    json!({ "EventDestination": definition }),
                ),
            ] {
                let answer = harness.call(method, &path, Some(body)).await;
                if entitled && !cross_account {
                    // Nothing else refuses it: the role is this account's (or there is
                    // none), and the account holds the entitlement.
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?} on {operation} (entitled): expected the action to be \
                         served, got {} {:?}",
                        answer.status, answer.body
                    );
                    continue;
                }
                let expected = if cross_account {
                    CROSS_ACCOUNT
                } else {
                    *when_unentitled
                };
                assert_eq!(
                    answer.status, DENIED_STATUS,
                    "case {case:?} on {operation} (entitled={entitled}): expected HTTP \
                     {DENIED_STATUS}, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    DENIED_CODE,
                    "case {case:?} on {operation} (entitled={entitled}): expected the code \
                     {DENIED_CODE}, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.message(),
                    expected,
                    "case {case:?} on {operation} (entitled={entitled}): expected the message \
                     {expected:?}, got {:?}",
                    answer.body
                );
            }
        }
    }
}

fn name_with_spaces_and_punctuation() -> Value {
    json!({ "ConfigurationSetName": "authorization set !!" })
}

fn voice_message_with_two_bodies() -> Value {
    json!({
        "DestinationPhoneNumber": "+14255550182",
        "Content": {
            "PlainTextMessage": { "Text": "a call", "VoiceId": "Joanna" },
            "SSMLMessage": { "Text": "<speak>a call</speak>", "VoiceId": "Joanna" },
        },
    })
}

/// RULE (the other side of the gate): with the entitlement held, the same resource-scoped
/// actions are authorized and answer their own modelled result — so the denial is the
/// account's entitlement and nothing else, and no action is refused for a reason this
/// suite invented.
#[tokio::test]
async fn an_entitled_account_reaches_the_actions_the_gate_refuses() {
    let harness = Harness::new();
    harness.entitle();

    // The same requests the gate suite drives, now answered by the handlers: a create
    // succeeds, a read of a set that exists succeeds, and an action on a name nothing
    // holds is the modelled 404 rather than the 403.
    let created = harness
        .call(
            "POST",
            "/v1/sms-voice/configuration-sets",
            Some(create_existing_set_body()),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "an entitled create must answer 200, got {} {:?}",
        created.status, created.body
    );

    let read = harness
        .call(
            "GET",
            "/v1/sms-voice/configuration-sets/authorization-set/event-destinations",
            None,
        )
        .await;
    assert_eq!(
        read.status, 200,
        "an entitled read of a set that exists must answer 200, got {} {:?}",
        read.status, read.body
    );

    for (case, method, path) in [
        (
            "GetConfigurationSetEventDestinations",
            "GET",
            "/v1/sms-voice/configuration-sets/authorization-absent-set/event-destinations",
        ),
        (
            "DeleteConfigurationSet",
            "DELETE",
            "/v1/sms-voice/configuration-sets/authorization-absent-set",
        ),
        (
            "DeleteConfigurationSetEventDestination",
            "DELETE",
            "/v1/sms-voice/configuration-sets/authorization-absent-set/event-destinations/nope",
        ),
    ] {
        let answer = harness.call(method, path, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an entitled action on {ABSENT_SET} must answer the modelled 404, \
             got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "case {case:?}: expected the modelled NotFoundException, got {:?}",
            answer.body
        );
    }

    let sent = harness
        .call(
            "POST",
            "/v1/sms-voice/voice/message",
            Some(well_formed_voice_message()),
        )
        .await;
    assert_eq!(
        sent.status, 200,
        "an entitled send must answer 200, got {} {:?}",
        sent.status, sent.body
    );
    assert!(
        sent.body
            .get("MessageId")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty()),
        "an accepted send must report a MessageId, got {:?}",
        sent.body
    );
}
