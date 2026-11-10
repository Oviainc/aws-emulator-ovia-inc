// SPDX-License-Identifier: CC0-1.0

//! Which failure class a member-addressed call falls in, on the READ and on the DELETE of
//! one resource, end to end over HTTP.
//!
//! A resource of this service is addressed by the identifier triple (appId,
//! environmentName, id) — `ComponentResource`, `FormResource` and `ThemeResource` all
//! carry it
//! (`smithy_query(query="shape",shape="ComponentResource,FormResource,ThemeResource")`) —
//! and a call whose id does not resolve in the scope it addressed falls in one of TWO
//! classes, told apart by WHERE the id resolves and not by which operation asked, nor by
//! which resource kind it addressed:
//!
//! | the id ...                             | status | class                       | message                            |
//! |----------------------------------------|--------|-----------------------------|------------------------------------|
//! | resolves under another app environment | 400    | `InvalidParameterException` | contains "does not belong to that app or environment" |
//! | resolves nowhere, on a read            | 404    | `ResourceNotFoundException` | "The item specified was not found" |
//! | resolves nowhere, on a delete          | 404    | `ResourceNotFoundException` | "Unable to find item to delete"    |
//!
//! The reads of these kinds are covered kind-by-kind in `codegen_job_scope.rs`; this suite
//! is about the PAIR — the same ids put through the read AND the delete of the same
//! resource, so neither operation can drift onto the other class on its own, and the 404
//! branch keeps the delete path's own message. With no error type on the wire (see
//! `src/service/untyped_errors.rs`) the status and that literal are the whole name of the
//! failure class.
//!
//! The rows differ along what the rule generalises over: the resource KIND addressed,
//! which half of the (appId, environmentName) pair the request gets wrong, whether the
//! addressed scope holds resources of its own, and the FORM of an id that resolves
//! nowhere.
//!
//! Each suite in this crate carries its own request harness, so this one does too.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment that OWNS every resource a case creates. `AppId` is
/// `@length(min: 1, max: 20)` with pattern `^d[a-z0-9]+$`
/// (`smithy_query(query="shape",shape="AppId")`).
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The fixed phrase the wrong-scope refusal's message carries — what
/// `crate::state::WRONG_SCOPE_MESSAGE` holds. Only the phrase is pinned: it names no ids,
/// and with no error type on the wire it is what a client matches on.
const WRONG_SCOPE_PHRASE: &str = "does not belong to that app or environment";

/// The message a READ answers for an id that resolves nowhere — recorded behaviour, and
/// what `crate::state::ITEM_NOT_FOUND_MESSAGE` holds.
const READ_NOT_FOUND_MESSAGE: &str = "The item specified was not found";

/// The message a DELETE answers for an id that resolves nowhere: the delete path's own
/// literal, and what `crate::state::ITEM_TO_DELETE_NOT_FOUND_MESSAGE` holds.
const DELETE_NOT_FOUND_MESSAGE: &str = "Unable to find item to delete";

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

    /// The error-type discriminator the body carries, or the empty string: this service
    /// names no exception on the wire, so every refusal below must carry none.
    fn error_code(&self) -> &str {
        ["__type", "code", "Code"]
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

/// A resource kind whose read and delete are both addressed by the identifier triple.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Component,
    Form,
    Theme,
}

impl Kind {
    /// The kinds the table runs over: the ones with BOTH a read and a delete on the
    /// triple (a codegen job has no delete).
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

    /// A create payload carrying only the members this kind's create data requires:
    /// `CreateComponentData`'s six, `CreateFormData`'s seven and `CreateThemeData`'s one
    /// plus the `values` list it is modelled with.
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

/// Create one resource of `kind` and report the id it was given. Panics when the create is
/// refused, so a case cannot pass on a resource that never existed.
async fn create(
    router: &MultiServiceRouter,
    kind: Kind,
    app_id: &str,
    environment: &str,
    name: &str,
) -> String {
    let answer = call(
        router,
        "POST",
        &collection_uri(kind, app_id, environment),
        Some(kind.create_payload(name)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "creating a {kind:?} named {name:?} in {app_id}/{environment} must succeed, got {} \
         with {:?}",
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

async fn read(
    router: &MultiServiceRouter,
    kind: Kind,
    app_id: &str,
    environment: &str,
    id: &str,
) -> Answer {
    call(router, "GET", &member_uri(kind, app_id, environment, id), None).await
}

async fn delete(
    router: &MultiServiceRouter,
    kind: Kind,
    app_id: &str,
    environment: &str,
    id: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &member_uri(kind, app_id, environment, id),
        None,
    )
    .await
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

/// Assert the refusal names no error type: with none on the wire, the status and the
/// message literal are the whole name of the failure class, which is what makes the
/// literal part of the contract.
fn assert_names_no_error_type(case: &str, answer: &Answer) {
    assert!(
        answer.error_code().is_empty(),
        "case {case:?}: a refusal must name no error type, got {:?}",
        answer.error_code()
    );
}

/// One wrong scope to address a resource from: which half of the (appId, environmentName)
/// pair the request gets wrong, and whether that scope holds a resource of its own.
struct WrongScope {
    case: &'static str,
    app_id: &'static str,
    environment: &'static str,
    /// True when the case first creates a resource of the same kind in that scope, so the
    /// addressed collection is non-empty: a miss there is still classified by where the id
    /// resolves, not by whether the scope is populated.
    populated: bool,
}

fn wrong_scopes() -> Vec<WrongScope> {
    vec![
        WrongScope {
            case: "another app, the same environment",
            app_id: "d9999zzzz",
            environment: ENVIRONMENT,
            populated: false,
        },
        WrongScope {
            case: "the same app, another environment",
            app_id: APP_ID,
            environment: "production",
            populated: false,
        },
        WrongScope {
            case: "another app and another environment",
            app_id: "d0mf86cg35xrtv",
            environment: "dev-2",
            populated: false,
        },
        WrongScope {
            case: "another app that holds resources of its own",
            app_id: "d8888yyyy",
            environment: ENVIRONMENT,
            populated: true,
        },
        WrongScope {
            case: "another environment that holds resources of its own",
            app_id: APP_ID,
            environment: "integration",
            populated: true,
        },
    ]
}

/// An id another app environment owns is a wrong-scope request on the READ and on the
/// DELETE alike: the modelled `InvalidParameterException` (HTTP 400) with the fixed phrase
/// — never the 404 an absent id gets — and it removes nothing, so the resource is still
/// readable and still deletable in the scope that owns it.
#[tokio::test]
async fn an_id_another_app_environment_owns_is_refused_as_wrong_scope_on_read_and_delete() {
    for kind in Kind::ALL {
        for scope in wrong_scopes() {
            // A fresh emulator per row, so one row's resources cannot resolve for the
            // next one's scope.
            let router = emulator();
            let id = create(&router, kind, APP_ID, ENVIRONMENT, "Owned").await;
            if scope.populated {
                create(&router, kind, scope.app_id, scope.environment, "Native").await;
            }

            for (operation, answer) in [
                (
                    "the read",
                    read(&router, kind, scope.app_id, scope.environment, &id).await,
                ),
                (
                    "the delete",
                    delete(&router, kind, scope.app_id, scope.environment, &id).await,
                ),
            ] {
                let case = format!("{} on {operation} of a {kind:?}", scope.case);
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
                assert_names_no_request_value(
                    &case,
                    &answer,
                    &[scope.app_id, scope.environment, &id],
                );
                assert_names_no_error_type(&case, &answer);
            }

            // Neither refusal touched the resource: it still reads back under the scope
            // that owns it, and the delete addressed THERE succeeds.
            let case = format!("{} on a {kind:?}", scope.case);
            let own = read(&router, kind, APP_ID, ENVIRONMENT, &id).await;
            assert_eq!(
                own.status, 200,
                "case {case:?}: the resource must still be readable in its own scope, got {} \
                 with {:?}",
                own.status, own.body
            );
            assert_eq!(
                own.id(),
                id,
                "case {case:?}: the own-scope read must report the same id, got {:?}",
                own.body
            );

            let removed = delete(&router, kind, APP_ID, ENVIRONMENT, &id).await;
            assert_eq!(
                removed.status, 200,
                "case {case:?}: the own-scope delete must answer HTTP 200, got {} with {:?}",
                removed.status, removed.body
            );
            let gone = read(&router, kind, APP_ID, ENVIRONMENT, &id).await;
            assert_eq!(
                gone.status, 404,
                "case {case:?}: the deleted id must read as absent, got {} with {:?}",
                gone.status, gone.body
            );
        }
    }
}

/// An id that resolves NOWHERE is absent on the READ and on the DELETE alike: HTTP 404,
/// each path with its own fixed literal — whatever the id's form, and whichever app
/// environment the request addresses it in, including one that holds a resource of its
/// own.
#[tokio::test]
async fn an_id_that_resolves_nowhere_is_reported_absent_on_read_and_delete() {
    // The rows differ along the form of the id: a well-formed version 4 UUID (the form
    // `Uuid` carries), an id of one character, and a string that is no UUID at all.
    let unknown_ids = [
        (
            "a UUID nothing ever created",
            "11111111-1111-4111-8111-111111111111",
        ),
        ("an id of one character", "x"),
        ("an id that is not a UUID at all", "not-a-resource"),
    ];

    for kind in Kind::ALL {
        let router = emulator();
        // One resource exists in the home scope, so the store holds that collection and
        // one id of this kind resolves: an unknown id is still absent, not wrong-scope.
        let owned = create(&router, kind, APP_ID, ENVIRONMENT, "Owned").await;

        for (form, unknown) in unknown_ids {
            for (scope, app_id, environment) in [
                ("the populated app environment", APP_ID, ENVIRONMENT),
                ("another app", "d9999zzzz", ENVIRONMENT),
                ("another environment of the same app", APP_ID, "production"),
            ] {
                for (operation, expected, answer) in [
                    (
                        "the read",
                        READ_NOT_FOUND_MESSAGE,
                        read(&router, kind, app_id, environment, unknown).await,
                    ),
                    (
                        "the delete",
                        DELETE_NOT_FOUND_MESSAGE,
                        delete(&router, kind, app_id, environment, unknown).await,
                    ),
                ] {
                    let case = format!("{form} in {scope}, on {operation} of a {kind:?}");
                    assert_eq!(
                        answer.status, 404,
                        "case {case:?}: an id resolving nowhere must answer HTTP 404, got {} \
                         with {:?}",
                        answer.status, answer.body
                    );
                    assert_eq!(
                        answer.message(),
                        expected,
                        "case {case:?}: expected the message {expected:?}, got {:?}",
                        answer.message()
                    );
                    assert_names_no_request_value(&case, &answer, &[app_id, environment, unknown]);
                    assert_names_no_error_type(&case, &answer);
                }
            }
        }

        // None of those refusals touched the resource the home scope owns, and once it is
        // deleted its id falls in the absent class on both paths — the 400 class is for
        // an id that still resolves SOMEWHERE, not for one that is gone.
        let still_there = read(&router, kind, APP_ID, ENVIRONMENT, &owned).await;
        assert_eq!(
            still_there.status, 200,
            "a refused call must leave the {kind:?} readable, got {} with {:?}",
            still_there.status, still_there.body
        );
        let removed = delete(&router, kind, APP_ID, ENVIRONMENT, &owned).await;
        assert_eq!(
            removed.status, 200,
            "the delete of the {kind:?} the home scope owns must answer HTTP 200, got {} with \
             {:?}",
            removed.status, removed.body
        );
        for (operation, expected, answer) in [
            (
                "the read",
                READ_NOT_FOUND_MESSAGE,
                read(&router, kind, APP_ID, ENVIRONMENT, &owned).await,
            ),
            (
                "the delete",
                DELETE_NOT_FOUND_MESSAGE,
                delete(&router, kind, APP_ID, ENVIRONMENT, &owned).await,
            ),
        ] {
            let case = format!("the deleted id on {operation} of a {kind:?}");
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
        }
    }
}
