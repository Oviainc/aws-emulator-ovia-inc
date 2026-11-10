// SPDX-License-Identifier: CC0-1.0

//! The EXACT sentence a wrong-scope refusal carries, end to end over HTTP.
//!
//! The sibling `resource_error_messages` and `read_and_delete_scope_classes` suites pin
//! the fixed PHRASE a wrong-scope refusal contains; this one pins the whole sentence,
//! because the message is the only machine-readable name this failure class has: the
//! service names no error type on the wire (see `src/service/untyped_errors.rs`), so a
//! client matching the class matches the text.
//!
//! The RULE: an id that resolves under another app environment than the one addressed is
//! refused with ONE fixed sentence, which names no resource kind, no operation, no app,
//! no environment and no id — so the SAME text answers every resource kind and every
//! operation addressed by the identifier triple.
//!
//! The rows span what the rule generalises over: the resource KIND addressed (components,
//! forms and themes, the three kinds with a read, an update and a delete on the triple),
//! the operation, and which half of the (appId, environmentName) pair the request gets
//! wrong.
//!
//! Each suite in this crate carries its own request harness, so this one does too.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::state::WRONG_SCOPE_MESSAGE;
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment that OWNS every resource a row creates.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The whole sentence a wrong-scope refusal answers, written out here rather than read
/// off the service's own constant so a change of wording is caught rather than followed.
///
/// Source: this service's recorded refusals. The model declares no text for the member
/// (`smithy_query(query="shape",shape="InvalidParameterException")`), so the literal is
/// the one the service is observed to answer.
const WRONG_SCOPE_SENTENCE: &str = "The item requests does not belong to that app or environment";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The resource id the answer carries, or the empty string.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }

    /// The `message` the answer carries, in either spelling a JSON error body uses, or
    /// the empty string when it carries none.
    fn message(&self) -> &str {
        ["message", "Message"]
            .iter()
            .find_map(|member| self.body[*member].as_str())
            .unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "amplifyuibuilder.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_AMPLIFYUIBUILDER);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A resource kind addressed by the identifier triple, with a read, an update and a
/// delete on it.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Component,
    Form,
    Theme,
}

impl Kind {
    const ALL: [Self; 3] = [Self::Component, Self::Form, Self::Theme];

    /// The collection element of this kind's REST URIs
    /// (`/app/{appId}/environment/{environmentName}/<collection>/{id}`).
    fn collection(self) -> &'static str {
        match self {
            Self::Component => "components",
            Self::Form => "forms",
            Self::Theme => "themes",
        }
    }

    /// A create payload carrying only the members this kind's create data requires.
    fn create_payload(self, name: &str) -> Value {
        match self {
            Self::Component => json!({
                "name": name,
                "componentType": "View",
                "properties": {},
                "variants": [],
                "overrides": {},
                "bindingProperties": {},
            }),
            Self::Form => json!({
                "name": name,
                "formActionType": "create",
                "dataType": {"dataSourceType": "DataStore", "dataTypeName": "Post"},
                "fields": {},
                "style": {},
                "sectionalElements": {},
                "schemaVersion": "1.0",
            }),
            Self::Theme => json!({
                "name": name,
                "values": [{"key": "tokens.colors.brand", "value": {"value": "#047d95"}}],
            }),
        }
    }

    /// An update payload this kind's update data accepts. `UpdateThemeData$values` is
    /// `@required` (`smithy_query(query="shape",shape="UpdateThemeData")`), so the theme
    /// patch carries the list it is to hold.
    fn update_payload(self) -> Value {
        match self {
            Self::Component | Self::Form => json!({"name": "Moved"}),
            Self::Theme => json!({
                "name": "Moved",
                "values": [{"key": "tokens.colors.brand", "value": {"value": "#be123c"}}],
            }),
        }
    }
}

fn collection_uri(kind: Kind, app_id: &str, environment: &str) -> String {
    format!(
        "/app/{app_id}/environment/{environment}/{}",
        kind.collection()
    )
}

fn member_uri(kind: Kind, app_id: &str, environment: &str, id: &str) -> String {
    format!("{}/{id}", collection_uri(kind, app_id, environment))
}

/// Create one resource of `kind` in the home scope and report the id it was given.
async fn create(router: &MultiServiceRouter, kind: Kind, name: &str) -> String {
    let answer = call(
        router,
        "POST",
        &collection_uri(kind, APP_ID, ENVIRONMENT),
        Some(kind.create_payload(name)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "creating a {kind:?} named {name:?} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    let id = answer.id();
    assert!(
        !id.is_empty(),
        "creating a {kind:?} named {name:?} must report an id, got {:?}",
        answer.body
    );
    id
}

#[tokio::test]
async fn a_wrong_scope_refusal_carries_one_whole_sentence_on_every_kind_and_operation() {
    // The service's own constant is the sentence, so no handler can answer a second
    // wording of this class.
    assert_eq!(
        WRONG_SCOPE_MESSAGE, WRONG_SCOPE_SENTENCE,
        "case the service constant: expected {WRONG_SCOPE_SENTENCE:?}, got \
         {WRONG_SCOPE_MESSAGE:?}"
    );

    // The rows differ in which half of the scope the request gets wrong.
    let scopes = [
        ("another app, the same environment", "d9999zzzz", ENVIRONMENT),
        ("the same app, another environment", APP_ID, "production"),
        ("another app and another environment", "d0mf86cg35xrtv", "dev-2"),
    ];

    for kind in Kind::ALL {
        for (scope, app_id, environment) in scopes {
            let router = emulator();
            let id = create(&router, kind, "Owned").await;

            for (operation, answer) in [
                (
                    "the read",
                    call(
                        &router,
                        "GET",
                        &member_uri(kind, app_id, environment, &id),
                        None,
                    )
                    .await,
                ),
                (
                    "the update",
                    call(
                        &router,
                        "PATCH",
                        &member_uri(kind, app_id, environment, &id),
                        Some(kind.update_payload()),
                    )
                    .await,
                ),
                (
                    "the delete",
                    call(
                        &router,
                        "DELETE",
                        &member_uri(kind, app_id, environment, &id),
                        None,
                    )
                    .await,
                ),
            ] {
                let case = format!("{scope} on {operation} of a {kind:?}");
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: a wrong-scope id must answer HTTP 400, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.message(),
                    WRONG_SCOPE_SENTENCE,
                    "case {case:?}: expected the message {WRONG_SCOPE_SENTENCE:?}, got {:?}",
                    answer.message()
                );
            }

            // None of those refusals touched the resource: it still reads in its own scope.
            let own = call(
                &router,
                "GET",
                &member_uri(kind, APP_ID, ENVIRONMENT, &id),
                None,
            )
            .await;
            assert_eq!(
                own.status, 200,
                "case {scope:?} on a {kind:?}: the resource must still be readable in its own \
                 scope, got {} with {:?}",
                own.status, own.body
            );
        }
    }
}
