// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a component call: the status it answers, and the fact that it
//! names no error type.
//!
//! Three rules are under test, each over a set of inputs rather than one recorded request:
//!
//! 1. A component name identifies a component within its app environment, so a create —
//!    or a rename — that would leave two live components of one app environment sharing a
//!    name is refused as a conflict with the existing resource (HTTP 409). The constraint
//!    is on live state: deleting the component that holds a name frees it, and it is
//!    per app environment, so the same name is free in another one.
//! 2. A component is addressed by (appId, environmentName, id), so an id that resolves
//!    under ANOTHER app environment is a wrong-scope request (HTTP 400), told apart from
//!    an id that names no component anywhere (HTTP 404). Every operation addressed that
//!    way — `GetComponent`, `UpdateComponent`, `DeleteComponent` — answers the same two,
//!    and neither refusal changes the store.
//! 3. No error response of one of this service's own modelled exceptions names its shape
//!    on the wire: no `x-amzn-errortype` header and no `__type` / `code` / `Code` member,
//!    so a client classifies the failure by status alone. The emulator's own signals — the
//!    scaffold's 501 for an operation nobody has written — keep their name, because they
//!    report something no AmplifyUIBuilder exception does.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment the cases address, unless a case is about addressing
/// another one.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The response header restJson1 resolves an error shape from. It must be absent from
/// every refusal this service answers.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The longest `ComponentName` the model admits (`@length(min: 1, max: 255)`).
const NAME_MAX_LENGTH: usize = 255;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status, the JSON body, and the error-type header.
struct Answer {
    status: u16,
    body: Value,
    error_type_header: Option<String>,
}

impl Answer {
    /// The component id the answer carries, or the empty string.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }

    /// The error-type discriminator the body carries — `__type`, or the `code` / `Code`
    /// spellings the same JSON error parsers also accept — or the empty string when it
    /// carries none.
    fn body_error_code(&self) -> &str {
        ["__type", "code", "Code"]
            .iter()
            .find_map(|member| self.body[*member].as_str())
            .unwrap_or_default()
    }

    /// Whether the answer names an error type anywhere a client would read one.
    fn names_an_error_type(&self) -> bool {
        self.error_type_header.is_some() || !self.body_error_code().is_empty()
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
    let error_type_header = response
        .headers()
        .get(ERROR_TYPE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
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
    Answer {
        status,
        body,
        error_type_header,
    }
}

fn components_uri(app_id: &str, environment: &str) -> String {
    format!("/app/{app_id}/environment/{environment}/components")
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
    data: Value,
) -> Answer {
    call(
        router,
        "POST",
        &components_uri(app_id, environment),
        Some(data),
    )
    .await
}

async fn create(router: &MultiServiceRouter, data: Value) -> Answer {
    create_in(router, APP_ID, ENVIRONMENT, data).await
}

async fn get_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    id: &str,
) -> Answer {
    let uri = format!("{}/{id}", components_uri(app_id, environment));
    call(router, "GET", &uri, None).await
}

async fn update_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    id: &str,
    patch: Value,
) -> Answer {
    let uri = format!("{}/{id}", components_uri(app_id, environment));
    call(router, "PATCH", &uri, Some(patch)).await
}

async fn delete_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    id: &str,
) -> Answer {
    let uri = format!("{}/{id}", components_uri(app_id, environment));
    call(router, "DELETE", &uri, None).await
}

/// The ids `ListComponents` reports for one app environment.
async fn listed_ids(router: &MultiServiceRouter, app_id: &str, environment: &str) -> Vec<String> {
    let answer = call(router, "GET", &components_uri(app_id, environment), None).await;
    answer.body["entities"]
        .as_array()
        .map(|entities| {
            entities
                .iter()
                .map(|entity| entity["id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Rule 1: a component name is unique among the live components of one app environment.
///
/// The rows differ along the form of the name the rule generalises over — length at both
/// published bounds, case, digits, punctuation, non-ASCII — and each row runs the whole
/// cycle: take the name, be refused the duplicate, free it by deleting, take it again.
#[tokio::test]
async fn a_component_name_is_unique_within_its_app_environment() {
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
        ("a lowercase spelling of an earlier row's name", "hero".to_string()),
    ];

    let router = emulator();
    for (case, name) in &names {
        let first = create(&router, create_data(name)).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create of {name:?} must succeed, got {} with {:?}",
            first.status, first.body
        );
        let id = first.id();

        let duplicate = create(&router, create_data(name)).await;
        assert_eq!(
            duplicate.status, 409,
            "case {case:?}: a second component named {name:?} in one app environment must be \
             refused with HTTP 409, got {} with {:?}",
            duplicate.status, duplicate.body
        );
        assert!(
            !duplicate.names_an_error_type(),
            "case {case:?}: a refusal must name no error type, got header {:?} and body {:?}",
            duplicate.error_type_header,
            duplicate.body
        );
        assert!(
            duplicate.id().is_empty(),
            "case {case:?}: a refused create must mint no component, got {:?}",
            duplicate.body
        );
        assert_eq!(
            listed_ids(&router, APP_ID, ENVIRONMENT).await,
            vec![id.clone()],
            "case {case:?}: a refused create must leave one component named {name:?}"
        );

        // The same name is free in another app environment: the constraint is per
        // (appId, environmentName).
        for (scope, app_id, environment) in [
            ("another environment of the same app", APP_ID, "production"),
            ("another app, the same environment", "d9999zzzz", ENVIRONMENT),
        ] {
            let elsewhere = create_in(&router, app_id, environment, create_data(name)).await;
            assert_eq!(
                elsewhere.status, 200,
                "case {case:?}/{scope:?}: {name:?} must be free in another app environment, \
                 got {} with {:?}",
                elsewhere.status, elsewhere.body
            );
            delete_in(&router, app_id, environment, &elsewhere.id()).await;
        }

        // Deleting the holder frees the name, and the identical create then succeeds.
        let deleted = delete_in(&router, APP_ID, ENVIRONMENT, &id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: deleting {id:?} must succeed, got {} with {:?}",
            deleted.status, deleted.body
        );
        let again = create(&router, create_data(name)).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: {name:?} must be free once its component is deleted, got {} with \
             {:?}",
            again.status, again.body
        );
        assert_ne!(
            again.id(),
            id,
            "case {case:?}: the retaken name must name a freshly minted component"
        );
        delete_in(&router, APP_ID, ENVIRONMENT, &again.id()).await;
    }
}

/// Rule 1 on a rename: `UpdateComponent` may not give a component the name another live
/// component of its app environment holds, and renaming a component to the name it
/// already has is not a conflict.
#[tokio::test]
async fn a_rename_onto_a_taken_name_is_refused() {
    let router = emulator();
    let taken = create(&router, create_data("Taken")).await.id();
    let mover = create(&router, create_data("Mover")).await.id();

    for (case, name) in [
        ("the name another component holds", "Taken"),
        ("that name again, after the refusal", "Taken"),
    ] {
        let refused = update_in(
            &router,
            APP_ID,
            ENVIRONMENT,
            &mover,
            json!({"name": name}),
        )
        .await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: a rename onto {name:?} must be refused with HTTP 409, got {} with \
             {:?}",
            refused.status, refused.body
        );
        assert!(
            !refused.names_an_error_type(),
            "case {case:?}: a refusal must name no error type, got header {:?} and body {:?}",
            refused.error_type_header,
            refused.body
        );
    }

    // The refused rename changed nothing.
    assert_eq!(
        get_in(&router, APP_ID, ENVIRONMENT, &mover).await.body["name"],
        json!("Mover"),
        "a refused rename must leave the stored name alone"
    );

    // Renaming a component to the name it already holds is a no-op, not a conflict; so is
    // renaming it to a name nothing holds.
    for (case, id, name) in [
        ("its own name", mover.clone(), "Mover"),
        ("a free name", mover.clone(), "Moved"),
        ("the other component's own name", taken.clone(), "Taken"),
    ] {
        let accepted = update_in(&router, APP_ID, ENVIRONMENT, &id, json!({"name": name})).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: renaming to {name:?} must succeed, got {} with {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.body["name"],
            json!(name),
            "case {case:?}: the rename must apply, got {:?}",
            accepted.body
        );
    }
}

/// Rule 2: an id that resolves under another app environment is a wrong-scope request
/// (400); an id that names no component anywhere is absent (404). Every operation
/// addressed by (appId, environmentName, id) answers the same two, and neither refusal
/// touches the store.
#[tokio::test]
async fn a_wrong_scope_id_is_told_apart_from_an_absent_one() {
    let router = emulator();
    let id = create(&router, create_data("Owned")).await.id();

    // Rows differ in which half of the scope the request gets wrong, and in whether the id
    // resolves at all.
    let scopes: Vec<(&str, String, String, String, u16)> = vec![
        (
            "a stored id under another app",
            "d9999zzzz".to_string(),
            ENVIRONMENT.to_string(),
            id.clone(),
            400,
        ),
        (
            "a stored id under another environment",
            APP_ID.to_string(),
            "production".to_string(),
            id.clone(),
            400,
        ),
        (
            "a stored id under another app and environment",
            "d9999zzzz".to_string(),
            "production".to_string(),
            id.clone(),
            400,
        ),
        (
            "an id that was never created",
            APP_ID.to_string(),
            ENVIRONMENT.to_string(),
            "11111111-1111-4111-8111-111111111111".to_string(),
            404,
        ),
        (
            "an id that was never created, in another app",
            "d9999zzzz".to_string(),
            ENVIRONMENT.to_string(),
            "22222222-2222-4222-8222-222222222222".to_string(),
            404,
        ),
    ];

    for (case, app_id, environment, addressed, expected) in &scopes {
        let answers = vec![
            (
                "GetComponent",
                get_in(&router, app_id, environment, addressed).await,
            ),
            (
                "UpdateComponent",
                update_in(
                    &router,
                    app_id,
                    environment,
                    addressed,
                    json!({"name": "Ghost"}),
                )
                .await,
            ),
            (
                "DeleteComponent",
                delete_in(&router, app_id, environment, addressed).await,
            ),
        ];
        for (operation, answer) in answers {
            assert_eq!(
                answer.status, *expected,
                "case {case:?}: {operation} must answer HTTP {expected}, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                !answer.names_an_error_type(),
                "case {case:?}: {operation} must name no error type, got header {:?} and body \
                 {:?}",
                answer.error_type_header,
                answer.body
            );
        }
    }

    // No refusal created, moved or removed anything: the one component is still there,
    // under its own scope, with the name it was created with.
    let read = get_in(&router, APP_ID, ENVIRONMENT, &id).await;
    assert_eq!(
        read.status, 200,
        "the component must still be readable in its own scope, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body["name"],
        json!("Owned"),
        "a refused call must not change the stored component, got {:?}",
        read.body
    );
    assert_eq!(
        listed_ids(&router, APP_ID, ENVIRONMENT).await,
        vec![id.clone()],
        "a refused call must not add or remove a component"
    );
    assert!(
        listed_ids(&router, "d9999zzzz", ENVIRONMENT).await.is_empty(),
        "a wrong-scope call must not create the component it addressed"
    );
}

/// Rule 3: across every refusal this service answers — each status class, each operation —
/// the response names no error type; the scaffold's 501 for an unwritten operation still
/// does, because it reports something no AmplifyUIBuilder exception reports.
#[tokio::test]
async fn no_modelled_refusal_names_its_error_type() {
    let router = emulator();
    let id = create(&router, create_data("Named")).await.id();
    let absent = "33333333-3333-4333-8333-333333333333";

    // Rows differ in the status class and in the operation that answers it.
    let refusals = vec![
        (
            "a duplicate name on CreateComponent (409)",
            create(&router, create_data("Named")).await,
            409,
        ),
        (
            "a reserved property name on CreateComponent (400)",
            create(
                &router,
                json!({
                    "name": "Reserved",
                    "componentType": "View",
                    "properties": {"tags": {"value": "x"}},
                    "variants": [],
                    "overrides": {},
                    "bindingProperties": {},
                }),
            )
            .await,
            400,
        ),
        (
            "an absent id on GetComponent (404)",
            get_in(&router, APP_ID, ENVIRONMENT, absent).await,
            404,
        ),
        (
            "an absent id on UpdateComponent (404)",
            update_in(&router, APP_ID, ENVIRONMENT, absent, json!({"name": "X"})).await,
            404,
        ),
        (
            "an absent id on DeleteComponent (404)",
            delete_in(&router, APP_ID, ENVIRONMENT, absent).await,
            404,
        ),
        (
            "a wrong-scope id on GetComponent (400)",
            get_in(&router, "d9999zzzz", ENVIRONMENT, &id).await,
            400,
        ),
        (
            "an unknown metadata feature on PutMetadataFlag (400)",
            call(
                &router,
                "PUT",
                &format!("/app/{APP_ID}/environment/{ENVIRONMENT}/metadata/features/nope"),
                Some(json!({"newValue": "true"})),
            )
            .await,
            400,
        ),
    ];

    for (case, answer, expected) in refusals {
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            !answer.names_an_error_type(),
            "case {case:?}: a refusal must name no error type, got header {:?} and body {:?}",
            answer.error_type_header,
            answer.body
        );
        assert!(
            answer.body["message"].is_string() || answer.body["Message"].is_string(),
            "case {case:?}: a refusal must still say why, got {:?}",
            answer.body
        );
    }

    // The emulator's own signal keeps its name: an operation nobody has written yet
    // answers 501 `NotImplemented`, which no AmplifyUIBuilder exception stands for.
    // `ExchangeCodeForToken` is one such operation today
    // (`src/ops/exchange_code_for_token.rs` still carries the scaffold marker); when it is
    // implemented, point this row at another operation that still carries one. The rule
    // itself is pinned independently of any operation by the unit tests in
    // `src/service/untyped_errors.rs`.
    //
    // The request has to satisfy the model first — `provider` is the `TokenProviders`
    // value `figma` and the `@httpPayload` body carries the two members
    // `ExchangeCodeForTokenRequestBody` requires
    // (`smithy_query(query="operation", operation="ExchangeCodeForToken")`) — because a
    // refusal the framework makes before the handler would never reach the scaffold.
    let stub = call(
        &router,
        "POST",
        "/tokens/figma",
        Some(json!({"code": "an-access-code", "redirectUri": "https://example.test/callback"})),
    )
    .await;
    assert_eq!(
        stub.status, 501,
        "ExchangeCodeForToken is still scaffolded, so it must answer 501, got {} with {:?}",
        stub.status, stub.body
    );
    assert_eq!(
        stub.body["__type"],
        json!("NotImplemented"),
        "the scaffold's 501 must keep its name, got {:?}",
        stub.body
    );
}
