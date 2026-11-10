// SPDX-License-Identifier: CC0-1.0

//! Addressing one of this service's resources in the WRONG SCOPE, end to end over HTTP.
//!
//! Every resource of this service that is addressed by (appId, environmentName, id) —
//! `CodegenJobResource`, `FormResource`, `ComponentResource` and `ThemeResource` all carry
//! that same identifier triple
//! (`smithy_query(query="shape",shape="CodegenJobResource,FormResource,ComponentResource,ThemeResource")`)
//! — tells TWO failures apart by WHERE the id resolves, and the classification is per
//! failure class, not per resource kind:
//!
//! | the id ...                                 | status | message                                              |
//! |--------------------------------------------|--------|------------------------------------------------------|
//! | resolves under another app environment     | 400    | contains "does not belong to that app or environment"|
//! | resolves nowhere                           | 404    | "The item specified was not found"                   |
//!
//! Each case is run over a TABLE: the rows differ in the resource KIND addressed, in
//! which half of the (appId, environmentName) pair the request gets wrong, and in whether
//! the scope addressed holds resources of its own — none of which may change which class
//! the answer falls in.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment a resource is created in. `AppId` is
/// `@length(min: 1, max: 20)` with pattern `^d[a-z0-9]+$`
/// (`smithy_query(query="shape",shape="AppId")`).
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The fixed phrase the wrong-scope refusal's message contains. Only the phrase is
/// pinned: it names no ids, and with no error type on the wire it is what a client
/// matches on. It is also what `crate::state::WRONG_SCOPE_MESSAGE` holds.
const WRONG_SCOPE_PHRASE: &str = "does not belong to that app or environment";

/// The message a read answers for an id that names no resource at all — the other half of
/// the rule, and what `crate::state::ITEM_NOT_FOUND_MESSAGE` holds.
const READ_NOT_FOUND_MESSAGE: &str = "The item specified was not found";

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

/// A resource kind of this service whose reads are addressed by the identifier triple.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Component,
    Form,
    CodegenJob,
    Theme,
}

impl Kind {
    /// The kinds the table runs over, in the order their collection URIs are listed in
    /// this crate's `OPERATIONS.md`.
    const ALL: [Self; 4] = [Self::Component, Self::Form, Self::CodegenJob, Self::Theme];

    /// The collection element of this kind's REST URIs
    /// (`/app/{appId}/environment/{environmentName}/<collection>/{id}`).
    fn collection(self) -> &'static str {
        match self {
            Self::Component => "components",
            Self::Form => "forms",
            Self::CodegenJob => "codegen-jobs",
            Self::Theme => "themes",
        }
    }

    /// The create payload carrying only the members this kind's create data requires:
    /// `CreateComponentData`'s six, `CreateFormData`'s seven,
    /// `StartCodegenJobData`'s one (`renderConfig`, a union with a single variant) and
    /// `CreateThemeData`'s `name` plus the `values` list it is modelled with. A codegen
    /// job carries no name, so `name` is ignored for it.
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
            Self::CodegenJob => json!({"renderConfig": {"react": {}}}),
            Self::Theme => json!({
                "name": name,
                "values": [{"key": "tokens.colors.brand", "value": {"value": "#047d95"}}],
            }),
        }
    }
}

/// The collection URI of one kind in one app environment.
fn collection_uri(kind: Kind, app_id: &str, environment: &str) -> String {
    format!(
        "/app/{app_id}/environment/{environment}/{}",
        kind.collection()
    )
}

/// The member URI of one resource.
fn member_uri(kind: Kind, app_id: &str, environment: &str, id: &str) -> String {
    format!("{}/{id}", collection_uri(kind, app_id, environment))
}

/// Create (or, for a codegen job, start) one resource of `kind`, and report the id it was
/// given. Panics when the create is refused, so a case cannot pass on an absent resource.
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

/// One wrong scope to address a resource from: which half of the (appId, environmentName)
/// pair the request gets wrong, and whether that scope holds a resource of its own.
struct WrongScope {
    case: &'static str,
    app_id: &'static str,
    environment: &'static str,
    /// True when the case first creates a resource of the same kind in that scope, so the
    /// addressed collection exists and is non-empty: a miss there still has to be
    /// classified by where the id resolves, not by whether the scope is populated.
    populated: bool,
}

/// The wrong scopes the table runs over.
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

/// An id that resolves under ANOTHER app environment is refused as wrong-scope: the
/// modelled `InvalidParameterException` (HTTP 400) whose message carries the fixed phrase
/// — for every kind addressed by the identifier triple, and whichever half of the pair the
/// request got wrong.
#[tokio::test]
async fn an_id_another_app_environment_owns_is_refused_as_wrong_scope() {
    for kind in Kind::ALL {
        for scope in wrong_scopes() {
            // A fresh emulator per row, so one row's resources cannot resolve for the
            // next one's scope.
            let router = emulator();
            let id = create(&router, kind, APP_ID, ENVIRONMENT, "Owned").await;
            if scope.populated {
                create(&router, kind, scope.app_id, scope.environment, "Native").await;
            }

            let answer = read(&router, kind, scope.app_id, scope.environment, &id).await;
            let case = format!("{} on a {kind:?}", scope.case);
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
            assert!(
                answer.error_code().is_empty(),
                "case {case:?}: a refusal must name no error type, got {:?}",
                answer.error_code()
            );
            assert_names_no_request_value(
                &case,
                &answer,
                &[scope.app_id, scope.environment, &id],
            );

            // The refusal reported nothing about the resource and changed nothing: it
            // still reads back under the scope that owns it.
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
        }
    }
}

/// An id that resolves NOWHERE stays absent — the modelled `ResourceNotFoundException`
/// (HTTP 404) with the read path's literal — whatever its form and whether the scope it is
/// asked for in holds resources of its own. This is the other half of the rule: the two
/// classes have to stay apart.
#[tokio::test]
async fn an_id_that_resolves_nowhere_is_reported_absent() {
    // The rows differ along the form of the id: a well-formed version 4 UUID, a one
    // character id and a string that is no UUID at all.
    let unknown_ids = [
        ("a UUID nothing ever created", "11111111-1111-4111-8111-111111111111"),
        ("an id of one character", "x"),
        ("an id that is not a UUID at all", "not-a-resource"),
    ];

    for kind in Kind::ALL {
        let router = emulator();
        // One resource exists in the home scope, so the store holds that collection: an
        // unknown id is still absent, not wrong-scope.
        create(&router, kind, APP_ID, ENVIRONMENT, "Owned").await;

        for (form, unknown) in unknown_ids {
            for (scope, app_id, environment) in [
                ("the populated app environment", APP_ID, ENVIRONMENT),
                ("another app", "d9999zzzz", ENVIRONMENT),
                ("another environment", APP_ID, "production"),
            ] {
                let answer = read(&router, kind, app_id, environment, unknown).await;
                let case = format!("{form} in {scope}, on a {kind:?}");
                assert_eq!(
                    answer.status, 404,
                    "case {case:?}: an id resolving nowhere must answer HTTP 404, got {} with \
                     {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.message(),
                    READ_NOT_FOUND_MESSAGE,
                    "case {case:?}: expected the message {READ_NOT_FOUND_MESSAGE:?}, got {:?}",
                    answer.message()
                );
                assert_names_no_request_value(&case, &answer, &[app_id, environment, unknown]);
            }
        }
    }
}

/// The classification follows the id's OWNER, not the kind: one id created in each kind,
/// read under the same wrong scope, answers the same class for all of them — and the id
/// that was never created answers the other class in that very same scope.
#[tokio::test]
async fn the_two_classes_are_told_apart_in_one_scope() {
    let router = emulator();
    let owned: Vec<(Kind, String)> = {
        let mut owned = Vec::new();
        for kind in Kind::ALL {
            owned.push((kind, create(&router, kind, APP_ID, ENVIRONMENT, "Owned").await));
        }
        owned
    };

    let (app_id, environment) = ("d7777xxxx", "qa");
    for (kind, id) in &owned {
        let wrong_scope = read(&router, *kind, app_id, environment, id).await;
        let case = format!("a {kind:?} owned elsewhere");
        assert_eq!(
            wrong_scope.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            wrong_scope.status, wrong_scope.body
        );
        assert!(
            wrong_scope.message().contains(WRONG_SCOPE_PHRASE),
            "case {case:?}: the message must contain {WRONG_SCOPE_PHRASE:?}, got {:?}",
            wrong_scope.message()
        );

        // The same scope, an id nothing owns: the other class.
        let absent = read(
            &router,
            *kind,
            app_id,
            environment,
            "99999999-9999-4999-8999-999999999999",
        )
        .await;
        let case = format!("a {kind:?} id nothing owns");
        assert_eq!(
            absent.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {:?}",
            absent.status, absent.body
        );
        assert_eq!(
            absent.message(),
            READ_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the message {READ_NOT_FOUND_MESSAGE:?}, got {:?}",
            absent.message()
        );
    }
}
