// SPDX-License-Identifier: CC0-1.0

//! Which names `CreateConfigurationSet` files a configuration set under.
//!
//! RULE: `CreateConfigurationSetRequest$ConfigurationSetName` targets the model's
//! `WordCharactersWithDelimiters`
//! (`smithy_query(query="shape",shape="WordCharactersWithDelimiters",model_path="services/service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice/smithy-model/com-amazonaws-pinpointsmsvoice-pinpointsmsvoice.json")`),
//! so a name is words — ASCII letters, digits and `_` — written with the delimiters `-`
//! and `.`, and nothing else; a name spelling no word at all (empty, or delimiters alone)
//! is not one either. A name carrying anything else is refused with
//! the modelled `BadRequestException` (HTTP 400) whose message names the member, and
//! nothing is stored under it; a name the shape admits is stored verbatim and listed.
//!
//! The suite drives the real `restJson1` router, so the requests here are the bytes an AWS
//! SDK `pinpoint-sms-voice` client sends. The rows differ along what the rule generalises
//! over: which character class the name carries, where in the name it appears, and the
//! length of the name.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pinpointsmsvoice_pinpointsmsvoice::{
    ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState, EmulatedService,
};

/// The sentence a refused name comes back with: this service's own wording for a
/// parameter its validation cannot use, naming the member the request carried.
const INVALID_CONFIGURATION_SET_NAME: &str =
    "The following parameter is null, empty or invalid: ['configurationSetName']";

/// The emulator under test, over the one shared engine the test also reads state from.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite drives unsigned requests; signature verification is the framework's
        // own concern and is covered where it lives.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        // The account holds the entitlement this API's resource-scoped actions need, so a
        // name the shape admits reaches the create rather than the authorization refusal
        // (`legacy_surface_authorization.rs` owns that half, including that this
        // validation precedes it).
        engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>()
            .lock()
            .legacy_surface_entitled = true;
        Self { router, engine }
    }

    /// `(status, body)` of a `CreateConfigurationSet` for `name`.
    async fn create_set(&self, name: &str) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://sms-voice.pinpoint.us-east-1.amazonaws.com/v1/sms-voice/configuration-sets")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/sms-voice/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(
                json!({ "ConfigurationSetName": name }).to_string(),
            ))
            .expect("the test request must build");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, body)
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

/// Names made only of word characters and the two delimiters, so each must be accepted and
/// stored exactly as sent. The rows walk the classes the rule admits rather than one
/// example of them: lower case, upper case, digits, `_`, `-`, `.`, a digit first, a single
/// character, and a long name mixing all of it.
const ADMITTED: &[&str] = &[
    "lowercase",
    "UPPERCASE",
    "MixedCase",
    "0123456789",
    "9lives",
    "with_underscores",
    "with-dashes",
    "with.dots",
    "a",
    "_",
    "a_b-c.d9",
    "voice.events_2026-config",
];

/// Names carrying a character the rule does not admit, which must each be refused. The
/// rows differ in the class of the offending character (whitespace, punctuation, a
/// separator, a non-ASCII letter), in where it sits (leading, inner, trailing), and
/// include the degenerate names: empty and all-whitespace.
const REFUSED: &[(&str, &str)] = &[
    ("an inner space", "voice events"),
    ("a leading space", " voice-events"),
    ("a trailing space", "voice-events "),
    ("a tab", "voice\tevents"),
    ("exclamation marks", "voice-events!!"),
    ("spaces and punctuation together", "sim2real behav cfgset !!"),
    ("a slash", "voice/events"),
    ("a colon", "voice:events"),
    ("a plus", "voice+events"),
    ("an at sign", "voice@events"),
    ("a comma", "voice,events"),
    ("an asterisk", "voice*"),
    ("a percent sign", "voice%20events"),
    ("a non-ASCII letter", "vöice-events"),
    ("the empty name", ""),
    ("an all-whitespace name", "   "),
    // Delimiters separate words, so a name that is nothing but delimiters spells none.
    ("a single dash", "-"),
    ("a single dot", "."),
    ("delimiters alone", "-.-"),
];

/// RULE: every name the shape admits is accepted, stored under exactly that name, and
/// listed.
#[tokio::test]
async fn a_name_of_word_characters_with_delimiters_is_accepted_and_stored_verbatim() {
    let harness = Harness::new();
    let mut expected: Vec<String> = Vec::new();
    for name in ADMITTED {
        let (status, body) = harness.create_set(name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: an admitted name must be created, got {status} {body}"
        );
        expected.push((*name).to_string());
    }
    expected.sort();
    assert_eq!(
        harness.stored_set_names(),
        expected,
        "every admitted name must be stored exactly as sent"
    );
}

/// RULE: a name carrying anything the shape does not admit is refused as invalid input,
/// with the sentence that names the member, and nothing is stored under it.
#[tokio::test]
async fn a_name_carrying_anything_else_is_a_bad_request() {
    let harness = Harness::new();
    for (case, name) in REFUSED {
        let (status, body) = harness.create_set(name).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({name:?}): expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            body.get("__type").and_then(Value::as_str),
            Some("BadRequestException"),
            "case {case:?} ({name:?}): expected the modelled BadRequestException, got {body}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(INVALID_CONFIGURATION_SET_NAME),
            "case {case:?} ({name:?}): expected the sentence naming the member, got {body}"
        );
    }
    assert!(
        harness.stored_set_names().is_empty(),
        "a refused name must store nothing, got {:?}",
        harness.stored_set_names()
    );
}
