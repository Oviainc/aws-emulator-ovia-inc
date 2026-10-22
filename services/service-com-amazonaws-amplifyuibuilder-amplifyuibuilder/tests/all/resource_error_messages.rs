// SPDX-License-Identifier: CC0-1.0

//! What a refused resource call SAYS: the `message` of each failure class.
//!
//! This service names no error type on the wire (see
//! `src/service/untyped_errors.rs`): a refusal carries no `x-amzn-errortype` header and
//! no `__type` / `code` / `Code` member, so a client's `Error.Code` degrades to the bare
//! status string and the `message` is the only machine-readable name the failure class
//! has. The rule under test is therefore about the message: each class answers ONE FIXED
//! literal that interpolates nothing from the request — no app id, no environment name,
//! no resource id and no resource name — and the literal is per CLASS, not per operation
//! or resource kind:
//!
//! | class                                     | status | message                                              |
//! |-------------------------------------------|--------|------------------------------------------------------|
//! | an id that names no resource, on a read   | 404    | "The item specified was not found"                   |
//! | an id that names no resource, on a delete | 404    | "Unable to find item to delete"                      |
//! | a name a live resource already holds      | 409    | "Item name already exists"                           |
//! | an id another app environment owns        | 400    | contains "does not belong to that app or environment"|
//!
//! The delete literal is deliberately NOT the read one: on an untyped 404 it is the only
//! thing that tells a failed delete from a failed read, which is what idempotent-delete
//! logic keys on.
//!
//! Every test below runs its class over a SET of requests whose app id, environment name,
//! resource id, resource name and resource kind differ, and asserts both halves of the
//! rule: the literal is the expected one, and the answer repeats nothing the request
//! carried.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment a case addresses unless it is about addressing
/// another one.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The message of a not-found refusal on a reporting or updating call.
///
/// Source: this service's recorded refusals — the model declares a `message` member on
/// `ResourceNotFoundException` but no text for it
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), so the literal is
/// the one the service is observed to answer. It is also what
/// `crate::state::ITEM_NOT_FOUND_MESSAGE` holds.
const READ_NOT_FOUND_MESSAGE: &str = "The item specified was not found";

/// The message of a not-found refusal on a DELETE — recorded behaviour, as above, and a
/// different literal from the read path's.
const DELETE_NOT_FOUND_MESSAGE: &str = "Unable to find item to delete";

/// The message of the name-conflict refusal — recorded behaviour, as above.
const DUPLICATE_NAME_MESSAGE: &str = "Item name already exists";

/// The fixed phrase the wrong-scope refusal's message contains — recorded behaviour, as
/// above. Only the phrase is pinned: it is what a client matches on, and it names no ids.
const WRONG_SCOPE_PHRASE: &str = "does not belong to that app or environment";

/// The longest `ComponentName` the model admits
/// (`smithy_query(query="shape",shape="ComponentName")`: `@length(min: 1, max: 255)`).
const NAME_MAX_LENGTH: usize = 255;

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
    /// The component id the answer carries, or the empty string.
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

/// The collection URI of one resource kind in one app environment.
fn collection_uri(kind: &str, app_id: &str, environment: &str) -> String {
    format!("/app/{app_id}/environment/{environment}/{kind}")
}

/// The member URI of one resource.
fn member_uri(kind: &str, app_id: &str, environment: &str, id: &str) -> String {
    format!("{}/{id}", collection_uri(kind, app_id, environment))
}

/// A create payload carrying only the members `CreateComponentData` requires.
fn create_data(name: &str) -> Value {
    json!({
        "name": name,
        "componentType": "View",
        "properties": {},
        "variants": [],
        "overrides": {},
        "bindingProperties": {},
    })
}

async fn create_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    name: &str,
) -> Answer {
    call(
        router,
        "POST",
        &collection_uri("components", app_id, environment),
        Some(create_data(name)),
    )
    .await
}

async fn read(
    router: &MultiServiceRouter,
    kind: &str,
    app_id: &str,
    environment: &str,
    id: &str,
) -> Answer {
    call(router, "GET", &member_uri(kind, app_id, environment, id), None).await
}

async fn update_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    id: &str,
    patch: Value,
) -> Answer {
    call(
        router,
        "PATCH",
        &member_uri("components", app_id, environment, id),
        Some(patch),
    )
    .await
}

async fn delete_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    id: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &member_uri("components", app_id, environment, id),
        None,
    )
    .await
}

/// Assert the answer's message repeats nothing the request carried: a fixed literal is
/// one a client can match, and an interpolated one never is.
fn assert_names_no_request_value(case: &str, answer: &Answer, values: &[&str]) {
    for value in values {
        assert!(
            !answer.message().contains(value),
            "case {case:?}: the message must interpolate no request value, but it carries \
             {value:?}: {:?}",
            answer.message()
        );
    }
}

/// The addressings an absent id can arrive in: the app id, the environment name and the
/// form of the id itself all differ, and none of them may reach the message.
fn absent_addressings() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        (
            "a uuid in the default app environment",
            APP_ID,
            ENVIRONMENT,
            "11111111-1111-4111-8111-111111111111",
        ),
        (
            "a uuid in another app",
            "d9999zzzz",
            ENVIRONMENT,
            "22222222-2222-4222-8222-222222222222",
        ),
        (
            "a uuid in another environment",
            APP_ID,
            "production",
            "33333333-3333-4333-8333-333333333333",
        ),
        ("an id of one character", APP_ID, ENVIRONMENT, "x"),
        (
            "an id that is not a uuid at all",
            "d0mf86cg35xrtv",
            "dev-2",
            "not-a-uuid-at-all",
        ),
    ]
}

/// A read that finds nothing answers the one read literal — whichever resource kind it
/// addressed, and whatever app id, environment name or id form it named.
#[tokio::test]
async fn a_read_that_finds_nothing_answers_one_fixed_literal() {
    let router = emulator();
    // Every reporting operation addressed by (appId, environmentName, id), plus the
    // updates, which answer the read literal too.
    let readers = ["components", "forms", "themes", "codegen-jobs"];

    for (case, app_id, environment, id) in absent_addressings() {
        for kind in readers {
            let answer = read(&router, kind, app_id, environment, id).await;
            let case = format!("{case} on the {kind} read");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: an absent id must answer HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.message(),
                READ_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the message {READ_NOT_FOUND_MESSAGE:?}, got {:?}",
                answer.message()
            );
            assert_names_no_request_value(&case, &answer, &[app_id, environment, id]);
        }

        // The updates answer the read literal too — the class is the same, whichever
        // resource kind the update addressed. Each carries the smallest patch its
        // `Update…Data` admits, so the request reaches the handler rather than being
        // refused on a modelled constraint.
        for (operation, kind, patch) in [
            ("UpdateComponent", "components", json!({"name": "Ghost"})),
            ("UpdateForm", "forms", json!({"name": "Ghost"})),
            (
                "UpdateTheme",
                "themes",
                // `UpdateThemeData$values` is `@required`.
                json!({"values": [{"key": "a", "value": {"value": "b"}}]}),
            ),
        ] {
            let updated = call(
                &router,
                "PATCH",
                &member_uri(kind, app_id, environment, id),
                Some(patch),
            )
            .await;
            let case = format!("{case} on {operation}");
            assert_eq!(
                updated.status, 404,
                "case {case:?}: an absent id must answer HTTP 404, got {} with {:?}",
                updated.status, updated.body
            );
            assert_eq!(
                updated.message(),
                READ_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the message {READ_NOT_FOUND_MESSAGE:?}, got {:?}",
                updated.message()
            );
            assert_names_no_request_value(&case, &updated, &[app_id, environment, id]);
        }
    }
}

/// A delete that finds nothing answers its OWN literal — the same one for every
/// addressing, and never the read path's.
#[tokio::test]
async fn a_delete_that_finds_nothing_answers_its_own_fixed_literal() {
    let router = emulator();

    for (case, app_id, environment, id) in absent_addressings() {
        // Every delete addressed by (appId, environmentName, id): one literal for all
        // of them, whichever resource kind the delete named.
        for kind in ["components", "forms", "themes"] {
            let answer = call(
                &router,
                "DELETE",
                &member_uri(kind, app_id, environment, id),
                None,
            )
            .await;
            let case = format!("{case} on the {kind} delete");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: an absent id must answer HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.message(),
                DELETE_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the message {DELETE_NOT_FOUND_MESSAGE:?}, got {:?}",
                answer.message()
            );
            assert_names_no_request_value(&case, &answer, &[app_id, environment, id]);
        }
    }

    // The two literals stay apart on one id across the whole lifecycle: a component is
    // created, deleted, and then read and deleted again. The read says one thing, the
    // delete the other.
    let id = create_in(&router, APP_ID, ENVIRONMENT, "Ephemeral").await.id();
    let deleted = delete_in(&router, APP_ID, ENVIRONMENT, &id).await;
    assert_eq!(
        deleted.status, 200,
        "deleting {id:?} must succeed, got {} with {:?}",
        deleted.status, deleted.body
    );
    for (case, answer, expected) in [
        (
            "a read-back after the delete",
            read(&router, "components", APP_ID, ENVIRONMENT, &id).await,
            READ_NOT_FOUND_MESSAGE,
        ),
        (
            "a second delete of the same id",
            delete_in(&router, APP_ID, ENVIRONMENT, &id).await,
            DELETE_NOT_FOUND_MESSAGE,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected the message {expected:?}, got {:?}",
            answer.message()
        );
        assert_names_no_request_value(case, &answer, &[APP_ID, ENVIRONMENT, &id]);
    }
}

/// A name a live resource of the app environment already holds answers one fixed literal,
/// whatever the name looks like and whether a create or a rename would introduce the
/// duplicate.
#[tokio::test]
async fn a_duplicate_name_answers_one_fixed_literal() {
    // The rows differ along the form of the name the rule generalises over: length at
    // both published bounds, digits, punctuation, case and non-ASCII.
    let names: Vec<(&str, String)> = vec![
        ("a plain name", "Hero".to_string()),
        ("the shortest name the model admits", "H".to_string()),
        (
            "the longest name the model admits",
            "n".repeat(NAME_MAX_LENGTH),
        ),
        ("digits only", "2024".to_string()),
        ("punctuation", "Hero_Card-v2.1".to_string()),
        ("non-ASCII", "Überschrift".to_string()),
    ];

    let router = emulator();
    for (case, name) in &names {
        let holder = create_in(&router, APP_ID, ENVIRONMENT, name).await;
        assert_eq!(
            holder.status, 200,
            "case {case:?}: the first create of {name:?} must succeed, got {} with {:?}",
            holder.status, holder.body
        );
        // The component a rename moves onto the taken name. Its own name is a fixed
        // short one, so no row's name form decides whether this create is admitted.
        let mover = create_in(&router, APP_ID, ENVIRONMENT, "Mover").await;
        assert_eq!(
            mover.status, 200,
            "case {case:?}: the mover create must succeed, got {} with {:?}",
            mover.status, mover.body
        );

        for (operation, answer) in [
            (
                "CreateComponent",
                create_in(&router, APP_ID, ENVIRONMENT, name).await,
            ),
            (
                "UpdateComponent",
                update_in(
                    &router,
                    APP_ID,
                    ENVIRONMENT,
                    &mover.id(),
                    json!({"name": name}),
                )
                .await,
            ),
        ] {
            let case = format!("{case} on {operation}");
            assert_eq!(
                answer.status, 409,
                "case {case:?}: a taken name must answer HTTP 409, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.message(),
                DUPLICATE_NAME_MESSAGE,
                "case {case:?}: expected the message {DUPLICATE_NAME_MESSAGE:?}, got {:?}",
                answer.message()
            );
            assert_names_no_request_value(&case, &answer, &[APP_ID, ENVIRONMENT, name]);
        }

        // Clear the app environment for the next row.
        delete_in(&router, APP_ID, ENVIRONMENT, &holder.id()).await;
        delete_in(&router, APP_ID, ENVIRONMENT, &mover.id()).await;
    }
}

/// An id that resolves under ANOTHER app environment answers the wrong-scope refusal,
/// whose message carries the fixed phrase and none of the request's ids — on every
/// operation addressed by (appId, environmentName, id).
#[tokio::test]
async fn a_wrong_scope_id_answers_one_fixed_phrase() {
    let router = emulator();
    let id = create_in(&router, APP_ID, ENVIRONMENT, "Owned").await.id();

    // The rows differ in which half of the scope the request gets wrong.
    let scopes = [
        ("another app, the same environment", "d9999zzzz", ENVIRONMENT),
        ("the same app, another environment", APP_ID, "production"),
        ("another app and another environment", "d0mf86cg35xrtv", "dev-2"),
    ];

    for (case, app_id, environment) in scopes {
        for (operation, answer) in [
            (
                "GetComponent",
                read(&router, "components", app_id, environment, &id).await,
            ),
            (
                "UpdateComponent",
                update_in(&router, app_id, environment, &id, json!({"name": "Moved"})).await,
            ),
            (
                "DeleteComponent",
                delete_in(&router, app_id, environment, &id).await,
            ),
        ] {
            let case = format!("{case} on {operation}");
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a wrong-scope id must answer HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.message().contains(WRONG_SCOPE_PHRASE),
                "case {case:?}: the message must contain {WRONG_SCOPE_PHRASE:?}, got {:?}",
                answer.message()
            );
            assert_names_no_request_value(&case, &answer, &[app_id, environment, &id]);
        }
    }

    // None of those refusals touched the component: it still reads in its own scope.
    let read_back = read(&router, "components", APP_ID, ENVIRONMENT, &id).await;
    assert_eq!(
        read_back.status, 200,
        "the component must still be readable in its own scope, got {} with {:?}",
        read_back.status, read_back.body
    );
    assert_eq!(
        read_back.body["name"],
        json!("Owned"),
        "a refused call must not change the stored component, got {:?}",
        read_back.body
    );
}
