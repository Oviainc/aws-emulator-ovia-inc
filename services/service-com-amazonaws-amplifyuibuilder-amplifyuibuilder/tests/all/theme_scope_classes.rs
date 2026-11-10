// SPDX-License-Identifier: CC0-1.0

//! Which failure class a THEME call addressed by (appId, environmentName, id) falls in,
//! end to end over HTTP.
//!
//! The RULE: a resource of this service addressed by that identifier triple classifies a
//! lookup miss by WHERE the id resolves, never by the resource kind and never by the
//! operation asking — an id that resolves under another app environment is a wrong-scope
//! request (`InvalidParameterException`, HTTP 400, one fixed sentence) and an id that
//! resolves nowhere is absent (`ResourceNotFoundException`, HTTP 404, the read path's
//! literal on a read or an update and the delete path's own literal on a delete).
//! `ThemeResource` carries the same identifiers as `ComponentResource` and `FormResource`
//! (`smithy_query(query="shape",shape="ThemeResource,ComponentResource,FormResource")`),
//! so the theme read, update and delete answer what the component and form siblings
//! answer for the same addressing.
//!
//! The rows span what the rule generalises over: the OPERATION addressed (`GetTheme`,
//! `UpdateTheme`, `DeleteTheme`), which half of the (appId, environmentName) pair the
//! request gets wrong, whether the scope addressed holds a theme of its own, whether the
//! wrong-scope id is the home theme's asked for elsewhere or another scope's theme asked
//! for at home, and — for the absent class — the FORM of an id that resolves nowhere.
//!
//! Each case also pins that a refusal changes nothing: every theme still reads back under
//! the scope that created it.
//!
//! Each suite in this crate carries its own request harness, so this one does too.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::state::{
    ITEM_NOT_FOUND_MESSAGE, ITEM_TO_DELETE_NOT_FOUND_MESSAGE, WRONG_SCOPE_MESSAGE,
};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment that OWNS the theme every row addresses. `AppId` is
/// `@length(min: 1, max: 20)` with pattern `^d[a-z0-9]+$`
/// (`smithy_query(query="shape",shape="AppId")`).
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
    /// The theme id the answer carries, or the empty string. `CreateTheme`, `GetTheme`
    /// and `UpdateTheme` all bind their theme to `@httpPayload`, so the body IS the
    /// `Theme`.
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

/// The three theme operations addressed by the identifier triple.
#[derive(Debug, Clone, Copy)]
enum Operation {
    Read,
    Update,
    Delete,
}

impl Operation {
    const ALL: [Self; 3] = [Self::Read, Self::Update, Self::Delete];

    /// The message this operation answers for an id that resolves NOWHERE: the read and
    /// the update answer the read literal, the delete its own.
    fn absent_message(self) -> &'static str {
        match self {
            Self::Read | Self::Update => ITEM_NOT_FOUND_MESSAGE,
            Self::Delete => ITEM_TO_DELETE_NOT_FOUND_MESSAGE,
        }
    }
}

fn themes_uri(app_id: &str, environment: &str) -> String {
    format!("/app/{app_id}/environment/{environment}/themes")
}

fn theme_uri(app_id: &str, environment: &str, id: &str) -> String {
    format!("{}/{id}", themes_uri(app_id, environment))
}

/// A `CreateThemeData` carrying its `name` and the `values` list it is modelled with
/// (`smithy_query(query="shape",shape="CreateThemeData")`).
fn theme_data(name: &str) -> Value {
    json!({
        "name": name,
        "values": [{"key": "tokens.colors.brand", "value": {"value": "#047d95"}}],
    })
}

/// Create one theme in an app environment and report the id it was given. Panics when the
/// create is refused, so a case cannot pass on a theme that never existed.
async fn create(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    name: &str,
) -> String {
    let answer = call(
        router,
        "POST",
        &themes_uri(app_id, environment),
        Some(theme_data(name)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "creating the theme {name:?} in {app_id}/{environment} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    let id = answer.id();
    assert!(
        !id.is_empty(),
        "creating the theme {name:?} must report an id, got {:?}",
        answer.body
    );
    id
}

/// Address one theme through one of the three operations. The update carries the smallest
/// patch `UpdateThemeData` admits: `values` is `@required`
/// (`smithy_query(query="shape",shape="UpdateThemeData")`).
async fn address(
    router: &MultiServiceRouter,
    operation: Operation,
    app_id: &str,
    environment: &str,
    id: &str,
) -> Answer {
    let uri = theme_uri(app_id, environment, id);
    match operation {
        Operation::Read => call(router, "GET", &uri, None).await,
        Operation::Update => {
            call(
                router,
                "PATCH",
                &uri,
                Some(json!({
                    "name": "Moved",
                    "values": [{"key": "tokens.colors.brand", "value": {"value": "#be123c"}}],
                })),
            )
            .await
        }
        Operation::Delete => call(router, "DELETE", &uri, None).await,
    }
}

/// Assert a refusal repeats nothing the request carried and names no error type: with no
/// error type on the wire the status and the fixed literal are the whole name of the
/// failure class, which a message interpolating request values would break.
fn assert_refusal_is_matchable(case: &str, answer: &Answer, values: &[&str]) {
    for value in values {
        assert!(
            !answer.message().contains(value),
            "case {case:?}: the message must interpolate no request value, but it carries \
             {value:?}: {:?}",
            answer.message()
        );
    }
    assert!(
        answer.error_code().is_empty(),
        "case {case:?}: a refusal must name no error type, got {:?}",
        answer.error_code()
    );
}

/// One way to address a theme id in the wrong scope.
struct WrongScope {
    case: &'static str,
    /// The app id and environment name the request addresses.
    app_id: &'static str,
    environment: &'static str,
    /// True when the case creates a theme of its own in the addressed scope, so that
    /// collection is non-empty: the class is still decided by where the id resolves.
    populated: bool,
    /// True when the row addresses the HOME theme's id in another scope; false when it
    /// addresses, at home, the id a theme of another scope holds.
    home_id_elsewhere: bool,
}

fn wrong_scopes() -> Vec<WrongScope> {
    vec![
        WrongScope {
            case: "this id under another app, the same environment",
            app_id: "d9999zzzz",
            environment: ENVIRONMENT,
            populated: false,
            home_id_elsewhere: true,
        },
        WrongScope {
            case: "this id under the same app, another environment",
            app_id: APP_ID,
            environment: "prod",
            populated: false,
            home_id_elsewhere: true,
        },
        WrongScope {
            case: "this id under another app and another environment",
            app_id: "d0mf86cg35xrtv",
            environment: "dev-2",
            populated: false,
            home_id_elsewhere: true,
        },
        WrongScope {
            case: "this id under another app that holds a theme of its own",
            app_id: "d8888yyyy",
            environment: ENVIRONMENT,
            populated: true,
            home_id_elsewhere: true,
        },
        WrongScope {
            case: "this id under another environment that holds a theme of its own",
            app_id: APP_ID,
            environment: "integration",
            populated: true,
            home_id_elsewhere: true,
        },
        WrongScope {
            case: "another app's theme id asked for at home",
            app_id: "d7777xxxx",
            environment: ENVIRONMENT,
            populated: true,
            home_id_elsewhere: false,
        },
        WrongScope {
            case: "another environment's theme id asked for at home",
            app_id: APP_ID,
            environment: "qa",
            populated: true,
            home_id_elsewhere: false,
        },
    ]
}

/// An id another app environment owns is a wrong-scope request on the theme READ, UPDATE
/// and DELETE alike: HTTP 400 with the one fixed sentence, never the 404 an id that
/// resolves nowhere gets — and it changes nothing.
#[tokio::test]
async fn a_theme_id_another_app_environment_owns_is_refused_as_wrong_scope() {
    for operation in Operation::ALL {
        for scope in wrong_scopes() {
            // A fresh emulator per row, so one row's themes cannot resolve for the next
            // one's scope.
            let router = emulator();
            let home = create(&router, APP_ID, ENVIRONMENT, "Owned").await;
            let foreign = if scope.populated {
                Some(create(&router, scope.app_id, scope.environment, "Native").await)
            } else {
                None
            };

            // Either the home theme's id addressed in the other scope, or the other
            // scope's theme id addressed at home: both are ids that resolve, outside the
            // scope the request named.
            let (app_id, environment, id) = if scope.home_id_elsewhere {
                (scope.app_id, scope.environment, home.clone())
            } else {
                (
                    APP_ID,
                    ENVIRONMENT,
                    foreign
                        .clone()
                        .expect("a row addressing a foreign id creates that theme"),
                )
            };

            let answer = address(&router, operation, app_id, environment, &id).await;
            let case = format!("{} on the {operation:?}", scope.case);
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a wrong-scope id must answer HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.message(),
                WRONG_SCOPE_MESSAGE,
                "case {case:?}: expected the message {WRONG_SCOPE_MESSAGE:?}, got {:?}",
                answer.message()
            );
            assert_refusal_is_matchable(&case, &answer, &[app_id, environment, &id]);

            // The refusal moved, renamed and removed nothing: the home theme still reads
            // back under the name it was created with, and so does the foreign one.
            let own = call(&router, "GET", &theme_uri(APP_ID, ENVIRONMENT, &home), None).await;
            assert_eq!(
                own.status, 200,
                "case {case:?}: the home theme must still read, got {} with {:?}",
                own.status, own.body
            );
            assert_eq!(
                own.body["name"],
                json!("Owned"),
                "case {case:?}: a refused call must leave the home theme as it was, got {:?}",
                own.body
            );
            if let Some(foreign) = foreign {
                let elsewhere = call(
                    &router,
                    "GET",
                    &theme_uri(scope.app_id, scope.environment, &foreign),
                    None,
                )
                .await;
                assert_eq!(
                    elsewhere.status, 200,
                    "case {case:?}: the theme of the other scope must still read, got {} with \
                     {:?}",
                    elsewhere.status, elsewhere.body
                );
                assert_eq!(
                    elsewhere.body["name"],
                    json!("Native"),
                    "case {case:?}: a refused call must leave the other scope's theme as it \
                     was, got {:?}",
                    elsewhere.body
                );
            }
        }
    }
}

/// An id that resolves NOWHERE is absent on the theme read, update and delete alike: HTTP
/// 404 with the path's own literal, whatever the id's form and whichever app environment
/// it is asked for in — including one that holds a theme of its own.
#[tokio::test]
async fn a_theme_id_that_resolves_nowhere_is_reported_absent() {
    // The rows differ along the form of the id: a well-formed version 4 UUID (the form
    // `Uuid` carries), an id of one character, and a string that is no UUID at all.
    let unknown_ids = [
        (
            "a uuid nothing ever created",
            "11111111-1111-4111-8111-111111111111",
        ),
        ("an id of one character", "x"),
        ("an id that is not a uuid at all", "not-a-theme"),
    ];

    for operation in Operation::ALL {
        let router = emulator();
        // A theme DOES exist in the home scope, so an empty collection is not what
        // decides the answer.
        let home = create(&router, APP_ID, ENVIRONMENT, "Owned").await;

        for (form, unknown) in unknown_ids {
            for (scope, app_id, environment) in [
                ("the populated app environment", APP_ID, ENVIRONMENT),
                ("another app", "d9999zzzz", ENVIRONMENT),
                ("another environment of the same app", APP_ID, "prod"),
            ] {
                let answer = address(&router, operation, app_id, environment, unknown).await;
                let expected = operation.absent_message();
                let case = format!("{form} in {scope}, on the {operation:?}");
                assert_eq!(
                    answer.status, 404,
                    "case {case:?}: an id resolving nowhere must answer HTTP 404, got {} with \
                     {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.message(),
                    expected,
                    "case {case:?}: expected the message {expected:?}, got {:?}",
                    answer.message()
                );
                assert_refusal_is_matchable(&case, &answer, &[app_id, environment, unknown]);

                // Nothing was created under that id either: an update is not an upsert.
                let after = call(&router, "GET", &theme_uri(app_id, environment, unknown), None)
                    .await;
                assert_eq!(
                    after.status, 404,
                    "case {case:?}: the refused call must create nothing, got {} with {:?}",
                    after.status, after.body
                );
            }
        }

        // The home theme is untouched by every refusal above, and once it is deleted its
        // own id falls in the ABSENT class — the 400 class is for an id that still
        // resolves somewhere, not for one that is gone.
        let own = call(&router, "GET", &theme_uri(APP_ID, ENVIRONMENT, &home), None).await;
        assert_eq!(
            own.status, 200,
            "case the {operation:?} rows: the home theme must still read, got {} with {:?}",
            own.status, own.body
        );
        let removed = call(
            &router,
            "DELETE",
            &theme_uri(APP_ID, ENVIRONMENT, &home),
            None,
        )
        .await;
        assert_eq!(
            removed.status, 200,
            "case the {operation:?} rows: the own-scope delete must answer HTTP 200, got {} \
             with {:?}",
            removed.status, removed.body
        );
        let gone = address(&router, operation, APP_ID, ENVIRONMENT, &home).await;
        let expected = operation.absent_message();
        assert_eq!(
            gone.status, 404,
            "case the deleted id on the {operation:?}: expected HTTP 404, got {} with {:?}",
            gone.status, gone.body
        );
        assert_eq!(
            gone.message(),
            expected,
            "case the deleted id on the {operation:?}: expected the message {expected:?}, got \
             {:?}",
            gone.message()
        );
    }
}

/// The control row: the SAME call, addressed in the scope that owns the theme, succeeds —
/// so the two refusal classes above are about the addressing, not about the operation
/// being broken.
#[tokio::test]
async fn a_theme_addressed_in_its_own_scope_is_served() {
    for operation in Operation::ALL {
        let router = emulator();
        let id = create(&router, APP_ID, ENVIRONMENT, "Owned").await;
        // A theme of another app environment holding its own id, so the home call cannot
        // be served by an empty store.
        create(&router, "d9999zzzz", ENVIRONMENT, "Native").await;

        let answer = address(&router, operation, APP_ID, ENVIRONMENT, &id).await;
        assert_eq!(
            answer.status, 200,
            "case the {operation:?} in the owning scope: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );

        let after = call(&router, "GET", &theme_uri(APP_ID, ENVIRONMENT, &id), None).await;
        match operation {
            Operation::Read => {
                assert_eq!(
                    answer.id(),
                    id,
                    "case the Read in the owning scope: expected the theme {id:?}, got {:?}",
                    answer.body
                );
                assert_eq!(
                    after.body["name"],
                    json!("Owned"),
                    "case the Read in the owning scope: a read must change nothing, got {:?}",
                    after.body
                );
            }
            Operation::Update => {
                assert_eq!(
                    after.body["name"],
                    json!("Moved"),
                    "case the Update in the owning scope: the merge must be visible on the \
                     read, got {:?}",
                    after.body
                );
            }
            Operation::Delete => {
                assert_eq!(
                    after.status, 404,
                    "case the Delete in the owning scope: the theme must be gone, got {} with \
                     {:?}",
                    after.status, after.body
                );
            }
        }
    }
}
