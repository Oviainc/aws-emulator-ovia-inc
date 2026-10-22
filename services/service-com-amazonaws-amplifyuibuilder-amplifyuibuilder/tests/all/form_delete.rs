// SPDX-License-Identifier: CC0-1.0

//! The Amplify UI Builder form DELETE path, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rules under test, each asserted over a SET of inputs rather than on one recorded
//! request:
//!
//! 1. `DeleteForm` removes the ONE record (appId, environmentName, id) addresses and
//!    nothing else: the deleted form stops being readable, every other form of that app
//!    environment and of every other one still reads back unchanged, and the answer is an
//!    empty 200 (`DeleteFormResponse` targets `Unit`, so the status is the whole answer).
//! 2. A delete that resolves no form in the addressed app environment is refused and
//!    changes nothing: an id that names no form anywhere answers the modelled
//!    `ResourceNotFoundException` (404) with the DELETE path's own message, and an id a
//!    different app environment owns answers the modelled `InvalidParameterException`
//!    (400) — the two classes `GetComponent`/`DeleteComponent` already tell apart, now
//!    told apart on the form resource by `GetForm`, `UpdateForm` and `DeleteForm` alike.
//! 3. A delete is not an upsert and a repeat of it is not a success: deleting the same id
//!    twice answers the first delete's 200 and then the not-found refusal.
//! 4. The client token that created a form is released with it, so the same
//!    `@idempotencyToken` used again after a delete mints a NEW form instead of reporting
//!    the removed one.
//!
//! Each suite in this crate carries its own request harness (see `form_lifecycle.rs`), so
//! this one does too.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to the
/// amplifyuibuilder service.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment a case addresses unless it is about addressing another
/// one.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The app environment that holds the bystander form of every case: it must never be
/// touched by a delete addressed elsewhere.
const OTHER_APP_ID: &str = "d9999zzzz";
const OTHER_ENVIRONMENT: &str = "production";

/// The message of a not-found refusal on a reporting call — recorded behaviour, and what
/// `crate::state::ITEM_NOT_FOUND_MESSAGE` holds. The model declares a `message` member on
/// `ResourceNotFoundException` but no text for it
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`).
const READ_NOT_FOUND_MESSAGE: &str = "The item specified was not found";

/// The message of a not-found refusal on a DELETE — recorded behaviour, and a deliberately
/// different literal from the read path's, which is what tells a failed delete from a
/// failed read apart on an otherwise identical untyped 404.
const DELETE_NOT_FOUND_MESSAGE: &str = "Unable to find item to delete";

/// The fixed phrase the wrong-scope refusal's message carries — recorded behaviour. Only
/// the phrase is pinned: it names no ids.
const WRONG_SCOPE_PHRASE: &str = "does not belong to that app or environment";

/// The longest `FormName` the model admits (`@length(min: 1, max: 255)` on
/// `CreateFormData$name`, from `smithy_query(query="shape", shape="FormName")`).
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
    /// The form id the answer carries, or the empty string. `CreateForm` and `GetForm`
    /// both bind their form to `@httpPayload`, so the body IS the `Form`.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }

    /// The form name the answer carries, or the empty string.
    fn name(&self) -> String {
        self.body["name"].as_str().unwrap_or_default().to_string()
    }

    /// The `message` the answer carries, in either spelling a JSON error body uses, or the
    /// empty string when it carries none.
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

fn forms_uri(app_id: &str, environment: &str) -> String {
    format!("/app/{app_id}/environment/{environment}/forms")
}

fn form_uri(app_id: &str, environment: &str, id: &str) -> String {
    format!("{}/{id}", forms_uri(app_id, environment))
}

async fn create_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    data: Value,
) -> Answer {
    call(router, "POST", &forms_uri(app_id, environment), Some(data)).await
}

async fn create_in_with_token(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    data: Value,
    token: &str,
) -> Answer {
    let uri = format!("{}?clientToken={token}", forms_uri(app_id, environment));
    call(router, "POST", &uri, Some(data)).await
}

async fn get_in(router: &MultiServiceRouter, app_id: &str, environment: &str, id: &str) -> Answer {
    call(router, "GET", &form_uri(app_id, environment, id), None).await
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
        &form_uri(app_id, environment, id),
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
    call(router, "DELETE", &form_uri(app_id, environment, id), None).await
}

/// The seven members `CreateFormData` requires, as a starting point for a case that is
/// about something other than those values.
fn minimal_create(name: &str) -> Value {
    json!({
        "name": name,
        "formActionType": "create",
        "dataType": {"dataSourceType": "DataStore", "dataTypeName": "Post"},
        "fields": {},
        "style": {},
        "sectionalElements": {},
        "schemaVersion": "1.0",
    })
}

/// A create payload carrying the optional members too — fields, sectional elements, a
/// style, a CTA and tags — so a case does not only cover the smallest form the model
/// admits.
fn rich_create(name: &str) -> Value {
    json!({
        "name": name,
        "formActionType": "update",
        "dataType": {"dataSourceType": "Custom", "dataTypeName": "Order"},
        "fields": {
            "title": {
                "label": "Title",
                "position": {"fixed": "first"},
                "inputType": {"type": "TextField", "required": true},
                "validations": [{"type": "Required", "validationMessage": "Title is required"}],
            },
        },
        "style": {"horizontalGap": {"value": "16px"}, "outerPadding": {"tokenReference": "space.large"}},
        "sectionalElements": {"heading": {"type": "Heading", "level": 1, "text": "Order"}},
        "schemaVersion": "2.1",
        "tags": {"env": "test", "team": "studio"},
        "cta": {
            "position": "top_and_bottom",
            "clear": {"excluded": true},
            "submit": {"excluded": false, "children": "Save"},
        },
        "labelDecorator": "required",
    })
}

/// Assert a refusal's message repeats nothing the request carried: a fixed literal is one
/// a client can match, an interpolated one never is.
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

/// One row of the "removes only what it names" table: the app environment the forms live
/// in, the create payload of each form in creation order, and which of them the case
/// deletes.
struct RemovalCase {
    case: &'static str,
    app_id: &'static str,
    environment: &'static str,
    forms: Vec<(&'static str, Value)>,
    victim: usize,
}

/// The removal rows, differing along what rule 1 generalises over: how many forms the app
/// environment holds, where in creation order the deleted one sits, how populated the
/// stored record is, the published bounds of `name`, and which half of the (appId,
/// environmentName) key the case addresses.
fn removal_cases() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            case: "the only form of the app environment",
            app_id: APP_ID,
            environment: ENVIRONMENT,
            forms: vec![("Solo", minimal_create("Solo"))],
            victim: 0,
        },
        RemovalCase {
            case: "the first of three in creation order",
            app_id: APP_ID,
            environment: ENVIRONMENT,
            forms: vec![
                ("First", minimal_create("First")),
                ("Second", minimal_create("Second")),
                ("Third", minimal_create("Third")),
            ],
            victim: 0,
        },
        RemovalCase {
            case: "the middle of three in creation order",
            app_id: APP_ID,
            environment: ENVIRONMENT,
            forms: vec![
                ("First", minimal_create("First")),
                ("Second", minimal_create("Second")),
                ("Third", minimal_create("Third")),
            ],
            victim: 1,
        },
        RemovalCase {
            case: "the last of three in creation order",
            app_id: APP_ID,
            environment: ENVIRONMENT,
            forms: vec![
                ("First", minimal_create("First")),
                ("Second", minimal_create("Second")),
                ("Third", minimal_create("Third")),
            ],
            victim: 2,
        },
        RemovalCase {
            case: "a fully populated form beside a minimal one",
            app_id: APP_ID,
            environment: ENVIRONMENT,
            forms: vec![
                ("Rich", rich_create("Rich")),
                ("Plain", minimal_create("Plain")),
            ],
            victim: 0,
        },
        RemovalCase {
            case: "the shortest name the model admits",
            app_id: APP_ID,
            environment: ENVIRONMENT,
            forms: vec![("f", minimal_create("f"))],
            victim: 0,
        },
        RemovalCase {
            case: "the longest name the model admits",
            app_id: "d0mf86cg35xrtv",
            environment: "dev-2",
            forms: vec![("longest", minimal_create(&"n".repeat(NAME_MAX_LENGTH)))],
            victim: 0,
        },
        RemovalCase {
            case: "another app environment of two forms",
            app_id: "d0mf86cg35xrtv",
            environment: "dev-2",
            forms: vec![
                ("Alpha", minimal_create("Alpha")),
                ("Beta", rich_create("Beta")),
            ],
            victim: 1,
        },
    ]
}

/// Rule 1 and rule 3: a delete removes the one record it addresses, answers an empty 200,
/// leaves every other form of that app environment and of another one readable, and a
/// second delete of the same id is refused rather than succeeding again.
#[tokio::test]
async fn deleting_a_form_removes_that_record_and_leaves_every_other_form() {
    for RemovalCase {
        case,
        app_id,
        environment,
        forms,
        victim,
    } in removal_cases()
    {
        let router = emulator();

        // A form of ANOTHER app environment, so each case also asserts the delete reaches
        // no further than the key it addresses.
        let bystander = create_in(
            &router,
            OTHER_APP_ID,
            OTHER_ENVIRONMENT,
            minimal_create("Bystander"),
        )
        .await;
        assert_eq!(
            bystander.status, 200,
            "case {case:?}: the bystander create must succeed, got {} with {:?}",
            bystander.status, bystander.body
        );

        let mut created: Vec<(&str, String)> = Vec::new();
        for (label, data) in &forms {
            let answer = create_in(&router, app_id, environment, data.clone()).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: creating {label:?} must succeed, got {} with {:?}",
                answer.status, answer.body
            );
            created.push((label, answer.id()));
        }

        let (victim_label, victim_id) = created[victim].clone();
        let stored = get_in(&router, app_id, environment, &victim_id).await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: {victim_label:?} must read back before the delete, got {} with {:?}",
            stored.status, stored.body
        );

        let deleted = delete_in(&router, app_id, environment, &victim_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: deleting {victim_label:?} must answer HTTP 200, got {} with {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body,
            Value::Null,
            "case {case:?}: DeleteForm answers Unit, so the body must be empty, got {:?}",
            deleted.body
        );

        // The deleted record is gone from the app environment that held it.
        let gone = get_in(&router, app_id, environment, &victim_id).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: GetForm must answer HTTP 404 for the deleted {victim_label:?}, got \
             {} with {:?}",
            gone.status, gone.body
        );
        assert_eq!(
            gone.message(),
            READ_NOT_FOUND_MESSAGE,
            "case {case:?}: the read-back after the delete must answer \
             {READ_NOT_FOUND_MESSAGE:?}, got {:?}",
            gone.message()
        );

        // Rule 3: the delete is not idempotent-as-success — the second one is refused, and
        // with the DELETE literal rather than the read one.
        let again = delete_in(&router, app_id, environment, &victim_id).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: a second delete of {victim_label:?} must answer HTTP 404, got {} \
             with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.message(),
            DELETE_NOT_FOUND_MESSAGE,
            "case {case:?}: a second delete must answer {DELETE_NOT_FOUND_MESSAGE:?}, got {:?}",
            again.message()
        );
        assert_names_no_request_value(case, &again, &[app_id, environment, &victim_id]);

        // Every sibling of the deleted form still reads back, unchanged.
        for (index, (label, id)) in created.iter().enumerate() {
            if index == victim {
                continue;
            }
            let survivor = get_in(&router, app_id, environment, id).await;
            assert_eq!(
                survivor.status, 200,
                "case {case:?}: the sibling {label:?} must survive the delete, got {} with {:?}",
                survivor.status, survivor.body
            );
            assert_eq!(
                &survivor.id(),
                id,
                "case {case:?}: the sibling {label:?} must still report its own id, got {:?}",
                survivor.id()
            );
            assert_eq!(
                survivor.name(),
                forms[index].1["name"].as_str().unwrap_or_default(),
                "case {case:?}: the sibling {label:?} must keep its stored name, got {:?}",
                survivor.name()
            );
        }

        // And so does the form of the other app environment.
        let other = get_in(
            &router,
            OTHER_APP_ID,
            OTHER_ENVIRONMENT,
            &bystander.id(),
        )
        .await;
        assert_eq!(
            other.status, 200,
            "case {case:?}: the form of another app environment must survive, got {} with {:?}",
            other.status, other.body
        );
        assert_eq!(
            other.name(),
            "Bystander",
            "case {case:?}: the other app environment's form must be unchanged, got {:?}",
            other.body
        );
    }
}

/// The addressings an id that names no form anywhere can arrive in: the app id, the
/// environment name and the form of the id itself all differ, and none of them may reach
/// the message.
fn absent_addressings() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        (
            "a uuid in the app environment that holds a form",
            APP_ID,
            ENVIRONMENT,
            "11111111-1111-4111-8111-111111111111",
        ),
        (
            "a uuid in an app environment that holds nothing",
            "d5555wwww",
            "qa",
            "22222222-2222-4222-8222-222222222222",
        ),
        (
            "a uuid in another environment of the same app",
            APP_ID,
            "dev-2",
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

/// Rule 2, first half: a delete whose id names no form anywhere answers the delete path's
/// own 404 literal, whatever the addressing, and leaves the store alone.
#[tokio::test]
async fn a_delete_that_names_no_form_anywhere_answers_the_delete_literal() {
    let router = emulator();
    let live = create_in(&router, APP_ID, ENVIRONMENT, minimal_create("Live")).await;
    assert_eq!(
        live.status, 200,
        "the live form must be created, got {} with {:?}",
        live.status, live.body
    );

    for (case, app_id, environment, id) in absent_addressings() {
        let answer = delete_in(&router, app_id, environment, id).await;
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
        assert_names_no_request_value(case, &answer, &[app_id, environment, id]);
    }

    // None of those refusals touched the one form the store holds.
    let read_back = get_in(&router, APP_ID, ENVIRONMENT, &live.id()).await;
    assert_eq!(
        read_back.status, 200,
        "a refused delete must leave the live form readable, got {} with {:?}",
        read_back.status, read_back.body
    );
    assert_eq!(
        read_back.name(),
        "Live",
        "a refused delete must not change the stored form, got {:?}",
        read_back.body
    );
}

/// Rule 2, second half: an id that resolves under ANOTHER app environment is a wrong-scope
/// request — the modelled `InvalidParameterException` (400) carrying the fixed phrase and
/// none of the request's values — on every form operation addressed by (appId,
/// environmentName, id), and it removes nothing.
#[tokio::test]
async fn a_cross_scope_form_call_is_refused_and_removes_nothing() {
    let router = emulator();
    let owned = create_in(&router, APP_ID, ENVIRONMENT, rich_create("Owned")).await;
    assert_eq!(
        owned.status, 200,
        "the owned form must be created, got {} with {:?}",
        owned.status, owned.body
    );
    let id = owned.id();

    // Rows differ in which half of the scope the request gets wrong.
    let scopes = [
        ("another app, the same environment", OTHER_APP_ID, ENVIRONMENT),
        ("the same app, another environment", APP_ID, OTHER_ENVIRONMENT),
        (
            "another app and another environment",
            "d0mf86cg35xrtv",
            "dev-2",
        ),
    ];

    for (scope_case, app_id, environment) in scopes {
        for (operation, answer) in [
            ("GetForm", get_in(&router, app_id, environment, &id).await),
            (
                "UpdateForm",
                update_in(
                    &router,
                    app_id,
                    environment,
                    &id,
                    json!({"name": "Moved"}),
                )
                .await,
            ),
            (
                "DeleteForm",
                delete_in(&router, app_id, environment, &id).await,
            ),
        ] {
            let case = format!("{scope_case} on {operation}");
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

    // None of those refusals touched the form: it still reads in its own scope, with the
    // name and the populated members the create stored.
    let read_back = get_in(&router, APP_ID, ENVIRONMENT, &id).await;
    assert_eq!(
        read_back.status, 200,
        "the form must still be readable in its own scope, got {} with {:?}",
        read_back.status, read_back.body
    );
    assert_eq!(
        read_back.name(),
        "Owned",
        "a refused call must not change the stored form, got {:?}",
        read_back.body
    );
    assert_eq!(
        read_back.body["cta"],
        rich_create("Owned")["cta"],
        "a refused call must not change the stored CTA, got {:?}",
        read_back.body["cta"]
    );
}

/// Rule 4: the `clientToken` that created a form names that form only while it exists, so
/// the same token used again after the delete mints a NEW form rather than reporting the
/// removed one — whatever the token looks like.
#[tokio::test]
async fn a_client_token_is_released_with_the_form_it_created() {
    // Rows differ in the token's spelling and in the shape of the form it creates.
    let cases: Vec<(&str, &str, Value)> = vec![
        ("a plain token", "token-a", minimal_create("Tokened")),
        (
            "a uuid-shaped token",
            "f0a1c2d3-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
            rich_create("TokenedRich"),
        ),
        ("a one-character token", "t", minimal_create("Tokened")),
        (
            "a token with punctuation",
            "retry.1_of-2",
            minimal_create("Tokened"),
        ),
    ];

    for (case, token, data) in cases {
        let router = emulator();

        let first = create_in_with_token(&router, APP_ID, ENVIRONMENT, data.clone(), token).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create must succeed, got {} with {:?}",
            first.status, first.body
        );
        // The token names that form while it lives: a retry reports the same id.
        let retried = create_in_with_token(&router, APP_ID, ENVIRONMENT, data.clone(), token).await;
        assert_eq!(
            retried.id(),
            first.id(),
            "case {case:?}: a retry with the same token must report the first form {:?}, got {:?}",
            first.id(),
            retried.id()
        );

        let deleted = delete_in(&router, APP_ID, ENVIRONMENT, &first.id()).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the delete must answer HTTP 200, got {} with {:?}",
            deleted.status, deleted.body
        );

        let after = create_in_with_token(&router, APP_ID, ENVIRONMENT, data.clone(), token).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: a create with the released token must succeed, got {} with {:?}",
            after.status, after.body
        );
        assert_ne!(
            after.id(),
            first.id(),
            "case {case:?}: the token must not revive the deleted form {:?}",
            first.id()
        );

        // The new form is readable and the deleted id stays gone.
        let fresh = get_in(&router, APP_ID, ENVIRONMENT, &after.id()).await;
        assert_eq!(
            fresh.status, 200,
            "case {case:?}: the newly minted form must read back, got {} with {:?}",
            fresh.status, fresh.body
        );
        let gone = get_in(&router, APP_ID, ENVIRONMENT, &first.id()).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: the deleted id must stay gone, got {} with {:?}",
            gone.status, gone.body
        );
    }
}
