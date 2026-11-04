// SPDX-License-Identifier: CC0-1.0

//! One name-conflict rule, over every resource kind of this service, end to end over HTTP.
//!
//! The rule under test: a resource name identifies a resource of its kind within its app
//! environment, so a call that would leave two live resources of one kind in one
//! (appId, environmentName) sharing a name — a create taking a taken name, or a rename onto
//! one — is refused as a conflict with the existing resource (HTTP 409, "Item name already
//! exists"), while the name stays free in another app environment and is freed again by
//! deleting the resource that holds it.
//!
//! The rows are the three resource kinds the service names — components, forms and themes —
//! crossed with the form of the name, so no kind and no spelling is a special case. Each
//! assertion goes through the router, so a case exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.
//!
//! Each suite in this crate carries its own request harness (see `component_refusals.rs`),
//! so this one does too.

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

/// The message every refusal of a taken name carries.
const DUPLICATE_NAME_MESSAGE: &str = "Item name already exists";

/// The length bounds every name of this service admits: `ComponentName`, `FormName` and
/// `ThemeName` are each `@length(min: 1, max: 255)`, from
/// `smithy_query(query="shape", shape="ComponentName,FormName,ThemeName")`.
const NAME_MIN_LENGTH: usize = 1;
const NAME_MAX_LENGTH: usize = 255;

/// One resource kind of this service: the collection segment its operations are bound
/// under, and the payloads a create and a rename of that kind carry.
struct Kind {
    /// The collection in the `@http` uri of the kind's operations, e.g.
    /// `/app/{appId}/environment/{environmentName}/<collection>`.
    collection: &'static str,
    /// A create payload carrying only the members the kind's create data requires, under
    /// the given name.
    create: fn(&str) -> Value,
    /// An update payload that renames the resource, carrying whatever else the kind's
    /// update data requires.
    rename: fn(&str) -> Value,
}

/// Every resource kind this service names a resource of, from
/// `smithy_query(query="digest")`: the three collections `CreateComponent`, `CreateForm`
/// and `CreateTheme` are bound under.
fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            collection: "components",
            create: |name| {
                // The members `CreateComponentData` requires, from
                // `smithy_query(query="shape", shape="CreateComponentData")`.
                json!({
                    "name": name,
                    "componentType": "View",
                    "properties": {},
                    "variants": [],
                    "overrides": {},
                    "bindingProperties": {},
                })
            },
            rename: |name| json!({"name": name}),
        },
        Kind {
            collection: "forms",
            create: |name| {
                // The members `CreateFormData` requires, from
                // `smithy_query(query="shape", shape="CreateFormData")`.
                json!({
                    "name": name,
                    "formActionType": "create",
                    "dataType": {"dataSourceType": "DataStore", "dataTypeName": "Post"},
                    "fields": {},
                    "style": {},
                    "sectionalElements": {},
                    "schemaVersion": "1.0",
                })
            },
            rename: |name| json!({"name": name}),
        },
        Kind {
            collection: "themes",
            create: |name| {
                // The members `CreateThemeData` requires, from
                // `smithy_query(query="shape", shape="CreateThemeData")`.
                json!({"name": name, "values": []})
            },
            // `UpdateThemeData$values` is `@required`, so a rename of a theme carries the
            // list too.
            rename: |name| json!({"name": name, "values": []}),
        },
    ]
}

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
    /// The resource id the answer carries, or the empty string. Every create, read and
    /// update of this service binds its entity to `@httpPayload`, so the body IS the
    /// resource.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }

    /// The resource name the answer carries, or the empty string.
    fn name(&self) -> String {
        self.body["name"].as_str().unwrap_or_default().to_string()
    }

    /// The message the answer carries, or the empty string.
    fn message(&self) -> String {
        self.body["message"].as_str().unwrap_or_default().to_string()
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

fn collection_uri(kind: &Kind, app_id: &str, environment: &str) -> String {
    format!(
        "/app/{app_id}/environment/{environment}/{}",
        kind.collection
    )
}

async fn create_in(
    router: &MultiServiceRouter,
    kind: &Kind,
    app_id: &str,
    environment: &str,
    name: &str,
) -> Answer {
    let uri = collection_uri(kind, app_id, environment);
    call(router, "POST", &uri, Some((kind.create)(name))).await
}

async fn create(router: &MultiServiceRouter, kind: &Kind, name: &str) -> Answer {
    create_in(router, kind, APP_ID, ENVIRONMENT, name).await
}

async fn rename(router: &MultiServiceRouter, kind: &Kind, id: &str, name: &str) -> Answer {
    let uri = format!("{}/{id}", collection_uri(kind, APP_ID, ENVIRONMENT));
    call(router, "PATCH", &uri, Some((kind.rename)(name))).await
}

async fn get(router: &MultiServiceRouter, kind: &Kind, id: &str) -> Answer {
    let uri = format!("{}/{id}", collection_uri(kind, APP_ID, ENVIRONMENT));
    call(router, "GET", &uri, None).await
}

async fn delete_in(
    router: &MultiServiceRouter,
    kind: &Kind,
    app_id: &str,
    environment: &str,
    id: &str,
) -> Answer {
    let uri = format!("{}/{id}", collection_uri(kind, app_id, environment));
    call(router, "DELETE", &uri, None).await
}

/// The ids the kind's list operation reports for one app environment.
async fn listed_ids(
    router: &MultiServiceRouter,
    kind: &Kind,
    app_id: &str,
    environment: &str,
) -> Vec<String> {
    let uri = collection_uri(kind, app_id, environment);
    let answer = call(router, "GET", &uri, None).await;
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

/// The name forms the rule generalises over: both published length bounds, digits,
/// punctuation, case and non-ASCII. The recorded case is the plain row; the others are
/// what the rule has to hold for as well.
fn name_forms() -> Vec<(&'static str, String)> {
    vec![
        ("a plain name", "Contact".to_string()),
        (
            "the shortest name the model admits",
            "n".repeat(NAME_MIN_LENGTH),
        ),
        (
            "the longest name the model admits",
            "n".repeat(NAME_MAX_LENGTH),
        ),
        ("digits only", "2024".to_string()),
        ("punctuation", "Contact_Form-v2.1".to_string()),
        ("non-ASCII", "Kontaktformular-Ü".to_string()),
        ("lowercase letters only", "contact".to_string()),
    ]
}

/// A create may not take a name a live resource of the same kind in the same app
/// environment already holds — on every resource kind and whatever the name looks like.
/// The constraint is on live state (deleting the holder frees the name) and per app
/// environment (the same name is free in another one).
#[tokio::test]
async fn a_create_may_not_take_a_name_a_live_resource_of_that_app_environment_holds() {
    let router = emulator();

    for kind in kinds() {
        for (form, name) in name_forms() {
            let case = format!("{} / {form}", kind.collection);

            let holder = create(&router, &kind, &name).await;
            assert_eq!(
                holder.status, 200,
                "case {case:?}: the first create of {name:?} must succeed, got {} with {:?}",
                holder.status, holder.body
            );
            let holder_id = holder.id();

            let duplicate = create(&router, &kind, &name).await;
            assert_eq!(
                duplicate.status, 409,
                "case {case:?}: a second resource named {name:?} in one app environment must \
                 be refused with HTTP 409, got {} with {:?}",
                duplicate.status, duplicate.body
            );
            assert_eq!(
                duplicate.message(),
                DUPLICATE_NAME_MESSAGE,
                "case {case:?}: expected the message {DUPLICATE_NAME_MESSAGE:?}, got {:?}",
                duplicate.message()
            );
            assert!(
                duplicate.id().is_empty(),
                "case {case:?}: a refused create must mint nothing, got {:?}",
                duplicate.body
            );
            assert_eq!(
                listed_ids(&router, &kind, APP_ID, ENVIRONMENT).await,
                vec![holder_id.clone()],
                "case {case:?}: a refused create must leave one resource named {name:?}"
            );

            // The same name is free in another app environment: the constraint is per
            // (appId, environmentName), and both halves of that key separate it.
            for (scope, app_id, environment) in [
                ("another environment of the same app", APP_ID, "production"),
                ("another app, the same environment", "d9999zzzz", ENVIRONMENT),
            ] {
                let elsewhere = create_in(&router, &kind, app_id, environment, &name).await;
                assert_eq!(
                    elsewhere.status, 200,
                    "case {case:?} / {scope:?}: {name:?} must be free in another app \
                     environment, got {} with {:?}",
                    elsewhere.status, elsewhere.body
                );
                delete_in(&router, &kind, app_id, environment, &elsewhere.id()).await;
            }

            // Deleting the holder frees the name, and the identical create then succeeds
            // with a resource of its own.
            let deleted = delete_in(&router, &kind, APP_ID, ENVIRONMENT, &holder_id).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: deleting {holder_id:?} must succeed, got {} with {:?}",
                deleted.status, deleted.body
            );
            let again = create(&router, &kind, &name).await;
            assert_eq!(
                again.status, 200,
                "case {case:?}: {name:?} must be free once its holder is deleted, got {} with \
                 {:?}",
                again.status, again.body
            );
            assert_ne!(
                again.id(),
                holder_id,
                "case {case:?}: the retaken name must name a freshly minted resource"
            );

            // Clear the app environment for the next row.
            delete_in(&router, &kind, APP_ID, ENVIRONMENT, &again.id()).await;
        }
    }
}

/// A rename may not move a resource onto a name ANOTHER live resource of its kind and app
/// environment holds — on every resource kind and whatever the name looks like — and the
/// refused rename leaves the stored record alone. Renaming a resource to the name it
/// already holds, or to a free one, is accepted; the name it leaves behind becomes free.
#[tokio::test]
async fn a_rename_may_not_move_a_resource_onto_a_name_another_live_resource_holds() {
    let router = emulator();

    for kind in kinds() {
        for (form, name) in name_forms() {
            let case = format!("{} / {form}", kind.collection);

            let holder = create(&router, &kind, &name).await;
            assert_eq!(
                holder.status, 200,
                "case {case:?}: the create of {name:?} must succeed, got {} with {:?}",
                holder.status, holder.body
            );
            // The resource a rename moves onto the taken name. Its own name is a fixed
            // short one, so no row's name form decides whether this create is admitted.
            let mover = create(&router, &kind, "Mover").await;
            assert_eq!(
                mover.status, 200,
                "case {case:?}: the mover create must succeed, got {} with {:?}",
                mover.status, mover.body
            );
            let mover_id = mover.id();

            // Twice: the refusal is about the stored set, so it does not wear off.
            for attempt in 0..2 {
                let refused = rename(&router, &kind, &mover_id, &name).await;
                assert_eq!(
                    refused.status, 409,
                    "case {case:?}: attempt {attempt} of a rename onto {name:?} must be \
                     refused with HTTP 409, got {} with {:?}",
                    refused.status, refused.body
                );
                assert_eq!(
                    refused.message(),
                    DUPLICATE_NAME_MESSAGE,
                    "case {case:?}: attempt {attempt} expected the message \
                     {DUPLICATE_NAME_MESSAGE:?}, got {:?}",
                    refused.message()
                );
            }

            // A refused rename changes neither record.
            assert_eq!(
                get(&router, &kind, &mover_id).await.name(),
                "Mover",
                "case {case:?}: a refused rename must leave the stored name alone"
            );
            assert_eq!(
                get(&router, &kind, &holder.id()).await.name(),
                name,
                "case {case:?}: a refused rename must leave the name's holder alone"
            );

            // A rename onto the name the resource itself already holds is not a conflict,
            // and neither is one onto a name nothing holds.
            for (accepted_case, id, target) in [
                ("its own name", mover_id.clone(), "Mover".to_string()),
                ("a free name", mover_id.clone(), "Moved".to_string()),
                ("the holder's own name", holder.id(), name.clone()),
            ] {
                let accepted = rename(&router, &kind, &id, &target).await;
                assert_eq!(
                    accepted.status, 200,
                    "case {case:?} / {accepted_case:?}: renaming to {target:?} must succeed, \
                     got {} with {:?}",
                    accepted.status, accepted.body
                );
                assert_eq!(
                    accepted.name(),
                    target,
                    "case {case:?} / {accepted_case:?}: the rename must store {target:?}, got \
                     {:?}",
                    accepted.name()
                );
            }

            // The mover vacated "Mover", so that name is free for another resource now.
            let successor = create(&router, &kind, "Mover").await;
            assert_eq!(
                successor.status, 200,
                "case {case:?}: a name a rename vacated must be free, got {} with {:?}",
                successor.status, successor.body
            );

            // Clear the app environment for the next row.
            for id in [holder.id(), mover_id, successor.id()] {
                delete_in(&router, &kind, APP_ID, ENVIRONMENT, &id).await;
            }
        }
    }
}

/// Distinct names coexist: the rule refuses a SHARED name, not a second resource. Every
/// name form of the table is taken at once in one app environment, and every one of them
/// is read back under the resource that took it.
#[tokio::test]
async fn resources_with_distinct_names_coexist_in_one_app_environment() {
    let router = emulator();

    for kind in kinds() {
        let mut taken: Vec<(String, String)> = Vec::new();
        for (form, name) in name_forms() {
            let case = format!("{} / {form}", kind.collection);
            let created = create(&router, &kind, &name).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: a distinct name must be admitted, got {} with {:?}",
                created.status, created.body
            );
            taken.push((created.id(), name));
        }

        for (id, name) in &taken {
            let read = get(&router, &kind, id).await;
            assert_eq!(
                read.name(),
                *name,
                "case {}: {id:?} must still be named {name:?}, got {:?}",
                kind.collection,
                read.name()
            );
        }
        assert_eq!(
            listed_ids(&router, &kind, APP_ID, ENVIRONMENT).await.len(),
            taken.len(),
            "case {}: every distinctly named resource must be live, got {:?}",
            kind.collection,
            listed_ids(&router, &kind, APP_ID, ENVIRONMENT).await
        );
    }
}
