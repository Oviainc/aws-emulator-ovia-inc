// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal an update answers for an `id` it cannot resolve.
//!
//! The RULE: on an operation addressed by an `@httpLabel` resource id, the refusal class
//! is decided by whether that id resolves ANYWHERE in the store, never by the operation
//! being an update rather than a read. An id that names no resource at all is reported
//! absent — `ResourceNotFoundException`, HTTP 404, the same miss the kind's `Get…` and
//! `Delete…` report — and `InvalidParameterException` (HTTP 400) is reserved for an id
//! that DOES resolve, under another app environment than the one addressed.
//!
//! `UpdateComponent`, `UpdateForm` and `UpdateTheme` each declare only
//! `InternalServerException`, `InvalidParameterException` and `ResourceConflictException`
//! (`smithy_query(query="operation",operation="UpdateComponent,UpdateForm,UpdateTheme")`),
//! so each widens this service's own `ResourceNotFoundException` in through
//! `extra_errors` to answer the absent case the way its `Get…` sibling does — the shape
//! `GetComponent` declares for exactly this lookup.
//!
//! Themes are the one kind that answers the absent class for a wrong-scope id too: the
//! three theme operations agree on which themes exist (see the note in
//! `src/ops/delete_theme.rs`), so this suite carries that classification per KIND rather
//! than assuming one answer for all three.
//!
//! The rows span what the rule generalises over: the resource KIND updated (all three
//! kinds with an update on the identifier triple), the FORM of the unresolvable id (uuid,
//! one character, not a uuid at all), which half of the (appId, environmentName) pair a
//! wrong-scope row gets wrong, and — as the control row — the in-scope update that must
//! still succeed and be visible through the kind's own read.
//!
//! Each suite in this crate carries its own request harness, so this one does too.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::state::{
    ITEM_NOT_FOUND_MESSAGE, WRONG_SCOPE_MESSAGE,
};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment that OWNS every resource a row creates.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

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

/// A resource kind with a create, a read and an update on
/// (`appId`, `environmentName`, `id`).
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
                "values": [{"key": "tokens.colors.brand", "value": {"value": "#1d4ed8"}}],
            }),
        }
    }

    /// A patch renaming this kind's resource. `UpdateThemeData$values` is `@required`
    /// (`smithy_query(query="shape",shape="UpdateThemeData")`), so the theme patch
    /// carries the list it is to hold.
    fn update_payload(self, name: &str) -> Value {
        match self {
            Self::Component | Self::Form => json!({"name": name}),
            Self::Theme => json!({
                "name": name,
                "values": [{"key": "tokens.colors.brand", "value": {"value": "#be123c"}}],
            }),
        }
    }

    /// The status and message this kind answers for an id ANOTHER app environment owns.
    /// Components and forms refuse it as wrong-scope; the theme operations agree instead
    /// on which themes exist and report it absent.
    fn wrong_scope_answer(self) -> (u16, &'static str) {
        match self {
            Self::Component | Self::Form => (400, WRONG_SCOPE_MESSAGE),
            Self::Theme => (404, ITEM_NOT_FOUND_MESSAGE),
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

async fn update(
    router: &MultiServiceRouter,
    kind: Kind,
    app_id: &str,
    environment: &str,
    id: &str,
    name: &str,
) -> Answer {
    call(
        router,
        "PATCH",
        &member_uri(kind, app_id, environment, id),
        Some(kind.update_payload(name)),
    )
    .await
}

async fn read(
    router: &MultiServiceRouter,
    kind: Kind,
    app_id: &str,
    environment: &str,
    id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &member_uri(kind, app_id, environment, id),
        None,
    )
    .await
}

/// The forms an id that resolves NOWHERE can arrive in: the id's own shape differs, and
/// every one of them is the same class to the rule.
fn unresolvable_ids() -> [(&'static str, &'static str); 4] {
    [
        (
            "a well-formed uuid nothing was created with",
            "11111111-1111-4111-8111-111111111111",
        ),
        (
            "another well-formed uuid",
            "22222222-2222-4222-8222-222222222222",
        ),
        ("an id of one character", "x"),
        ("an id that is not a uuid at all", "not-a-uuid-at-all"),
    ]
}

/// An update against an id that resolves nowhere is reported ABSENT (404), on every kind
/// and whatever the id looks like — and the update's own sibling read agrees.
#[tokio::test]
async fn an_update_against_an_unresolvable_id_is_reported_absent() {
    for kind in Kind::ALL {
        let router = emulator();
        // A resource DOES exist in the addressed scope, so an empty store is not what
        // decides the answer.
        let live = create(&router, kind, "Owned").await;

        for (case, id) in unresolvable_ids() {
            let answer = update(&router, kind, APP_ID, ENVIRONMENT, id, "Ghost").await;
            let case = format!("{case} on the {kind:?} update");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.message(),
                ITEM_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the message {ITEM_NOT_FOUND_MESSAGE:?}, got {:?}",
                answer.message()
            );

            // The read sibling classifies the same id the same way: the class is the
            // id's, not the operation's.
            let reported = read(&router, kind, APP_ID, ENVIRONMENT, id).await;
            assert_eq!(
                reported.status, 404,
                "case {case:?}: the {kind:?} read must agree, got {} with {:?}",
                reported.status, reported.body
            );

            // An update is not an upsert: nothing was created under that id.
            let after = read(&router, kind, APP_ID, ENVIRONMENT, id).await;
            assert_eq!(
                after.status, 404,
                "case {case:?}: the refused update must create nothing, got {} with {:?}",
                after.status, after.body
            );
        }

        // The live resource is untouched by every refusal above.
        let own = read(&router, kind, APP_ID, ENVIRONMENT, &live).await;
        assert_eq!(
            own.status, 200,
            "case {kind:?}: the live resource must still read, got {} with {:?}",
            own.status, own.body
        );
        assert_eq!(
            own.body["name"],
            json!("Owned"),
            "case {kind:?}: a refused update must not change the stored resource, got {:?}",
            own.body
        );
    }
}

/// An update against an id that DOES resolve, under another app environment, is the other
/// class: components and forms are refused as wrong-scope (400), themes report the absent
/// class their own read reports. Either way it is NOT the same answer as an id that
/// resolves nowhere for the kinds that distinguish them.
#[tokio::test]
async fn an_update_against_a_wrong_scope_id_answers_that_kinds_scope_class() {
    // The rows differ in which half of the scope the request gets wrong.
    let scopes = [
        ("another app, the same environment", "d9999zzzz", ENVIRONMENT),
        ("the same app, another environment", APP_ID, "production"),
        (
            "another app and another environment",
            "d0mf86cg35xrtv",
            "dev-2",
        ),
    ];

    for kind in Kind::ALL {
        let (expected_status, expected_message) = kind.wrong_scope_answer();
        for (scope, app_id, environment) in scopes {
            let router = emulator();
            let id = create(&router, kind, "Owned").await;

            let answer = update(&router, kind, app_id, environment, &id, "Moved").await;
            let case = format!("{scope} on the {kind:?} update");
            assert_eq!(
                answer.status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.message(),
                expected_message,
                "case {case:?}: expected the message {expected_message:?}, got {:?}",
                answer.message()
            );

            // The refusal moved nothing: the resource still reads in its own scope under
            // the name it was created with.
            let own = read(&router, kind, APP_ID, ENVIRONMENT, &id).await;
            assert_eq!(
                own.status, 200,
                "case {case:?}: the resource must still read in its own scope, got {} with {:?}",
                own.status, own.body
            );
            assert_eq!(
                own.body["name"],
                json!("Owned"),
                "case {case:?}: a refused update must not rename the resource, got {:?}",
                own.body
            );
        }
    }
}

/// The control row: the SAME update, addressed in the scope that owns the id, succeeds
/// and the merge is visible through the kind's own read.
#[tokio::test]
async fn an_update_in_the_owning_scope_succeeds_and_round_trips() {
    for kind in Kind::ALL {
        let router = emulator();
        let id = create(&router, kind, "Owned").await;

        let answer = update(&router, kind, APP_ID, ENVIRONMENT, &id, "Renamed").await;
        assert_eq!(
            answer.status, 200,
            "case {kind:?}: an in-scope update must succeed, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["name"],
            json!("Renamed"),
            "case {kind:?}: the update must report the merged name, got {:?}",
            answer.body
        );

        let own = read(&router, kind, APP_ID, ENVIRONMENT, &id).await;
        assert_eq!(
            own.status, 200,
            "case {kind:?}: the updated resource must read back, got {} with {:?}",
            own.status, own.body
        );
        assert_eq!(
            own.body["name"],
            json!("Renamed"),
            "case {kind:?}: the read must report what the update left behind, got {:?}",
            own.body
        );
        assert_eq!(
            own.body["id"],
            json!(id),
            "case {kind:?}: the update must not renumber the resource, got {:?}",
            own.body
        );
    }
}
