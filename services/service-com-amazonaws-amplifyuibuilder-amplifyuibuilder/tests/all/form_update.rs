// SPDX-License-Identifier: CC0-1.0

//! The Amplify UI Builder form update path, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test: `UpdateForm` merges an `UpdateFormData` patch into
//! the one stored form the (appId, environmentName, id) addresses — a member the patch
//! carries replaces what was stored, a member it leaves out keeps its stored value, and
//! the identifying members never move — so `GetForm` afterwards reports exactly what the
//! update left behind.
//!
//! Each suite in this crate carries its own request harness (see `form_lifecycle.rs`), so
//! this one does too.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to the
/// amplifyuibuilder service.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment the cases address, unless a case is about addressing
/// another one.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The fixed phrase the wrong-scope refusal's message carries — recorded behaviour, and
/// what `crate::state::WRONG_SCOPE_MESSAGE` holds. Only the phrase is pinned: it names no
/// ids.
const WRONG_SCOPE_PHRASE: &str = "does not belong to that app or environment";

/// The longest `FormName` the model admits (`@length(min: 1, max: 255)` on
/// `UpdateFormData$name`, from `smithy_query(query="shape", shape="UpdateFormData")`).
const NAME_MAX_LENGTH: usize = 255;

/// Every value `FormActionType` admits, from
/// `smithy_query(query="shape", shape="FormActionType")`.
const FORM_ACTION_TYPES: [&str; 2] = ["create", "update"];

/// Every value `FormDataSourceType` admits, from
/// `smithy_query(query="shape", shape="FormDataSourceType")`.
const FORM_DATA_SOURCE_TYPES: [&str; 2] = ["DataStore", "Custom"];

/// Every value `LabelDecorator` admits, from
/// `smithy_query(query="shape", shape="LabelDecorator")`.
const LABEL_DECORATORS: [&str; 3] = ["required", "optional", "none"];

/// The members `UpdateFormData` carries, from
/// `smithy_query(query="shape", shape="UpdateFormData")`. `tags` is NOT among them, so no
/// update can move a form's tags.
const UPDATABLE_MEMBERS: [&str; 9] = [
    "name",
    "dataType",
    "formActionType",
    "fields",
    "style",
    "sectionalElements",
    "schemaVersion",
    "cta",
    "labelDecorator",
];

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
    /// The form id the answer carries, or the empty string.
    ///
    /// `CreateForm`, `UpdateForm` and `GetForm` all bind their form to `@httpPayload`, so
    /// the body IS the `Form`.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
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

async fn create_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    data: Value,
) -> Answer {
    call(router, "POST", &forms_uri(app_id, environment), Some(data)).await
}

async fn create(router: &MultiServiceRouter, data: Value) -> Answer {
    create_in(router, APP_ID, ENVIRONMENT, data).await
}

async fn update_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    id: &str,
    patch: Value,
) -> Answer {
    let uri = format!("{}/{id}", forms_uri(app_id, environment));
    call(router, "PATCH", &uri, Some(patch)).await
}

async fn update(router: &MultiServiceRouter, id: &str, patch: Value) -> Answer {
    update_in(router, APP_ID, ENVIRONMENT, id, patch).await
}

async fn update_with_token(
    router: &MultiServiceRouter,
    id: &str,
    patch: Value,
    token: &str,
) -> Answer {
    let uri = format!("{}/{id}?clientToken={token}", forms_uri(APP_ID, ENVIRONMENT));
    call(router, "PATCH", &uri, Some(patch)).await
}

async fn get_in(router: &MultiServiceRouter, app_id: &str, environment: &str, id: &str) -> Answer {
    let uri = format!("{}/{id}", forms_uri(app_id, environment));
    call(router, "GET", &uri, None).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    get_in(router, APP_ID, ENVIRONMENT, id).await
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// A one-entry JSON object, for a case whose key is computed rather than written out.
fn one_entry(key: &str, value: Value) -> Value {
    let mut map = Map::new();
    map.insert(key.to_string(), value);
    Value::Object(map)
}

/// The form every update case starts from, under the name the case gives it: every
/// `UpdateFormData` member carries a value here, plus the `tags` no update can move, so a
/// patch always replaces something.
///
/// A form name is unique among the live forms of one app environment, so a suite that
/// creates one form per row names each row's form for itself.
fn baseline_named(name: &str) -> Value {
    let mut data = json!({
        "name": "OrderForm",
        "formActionType": "create",
        "dataType": {"dataSourceType": "DataStore", "dataTypeName": "Order"},
        "fields": {
            "title": {
                "label": "Title",
                "position": {"fixed": "first"},
                "inputType": {"type": "TextField", "required": true},
                "validations": [{"type": "Required", "validationMessage": "Title is required"}],
            },
        },
        "style": {"horizontalGap": {"value": "16px"}},
        "sectionalElements": {"heading": {"type": "Heading", "level": 1, "text": "Order"}},
        "schemaVersion": "1.0",
        "cta": {"position": "bottom", "submit": {"children": "Save"}},
        "tags": {"env": "test", "team": "studio"},
        "labelDecorator": "required",
    });
    data["name"] = json!(name);
    data
}

/// The baseline form under the one name a case that creates a single form uses.
fn baseline() -> Value {
    baseline_named("OrderForm")
}

/// The patches the merge cases drive, differing along what the rule generalises over:
/// which member is carried, how many are carried at once, which value or union member a
/// replacement carries, how many entries a replacement map holds, how its keys are spelled
/// and the published bounds of `name`.
fn patch_cases() -> Vec<(&'static str, Value)> {
    let mut cases = vec![
        ("an empty patch, which changes nothing", json!({})),
        ("just the name", json!({"name": "RenamedForm"})),
        (
            "the longest name the model admits",
            json!({"name": "n".repeat(NAME_MAX_LENGTH)}),
        ),
        ("the shortest name the model admits", json!({"name": "f"})),
        ("a new schemaVersion", json!({"schemaVersion": "2.1"})),
        (
            "a fields map of two entries, replacing the stored one",
            json!({"fields": {
                "title": {"label": "Headline", "position": {"fixed": "first"}},
                "rating": {
                    "label": "Rating",
                    "position": {"below": "title"},
                    "excluded": false,
                    "inputType": {
                        "type": "SliderField",
                        "minValue": 0.0,
                        "maxValue": 10.0,
                        "step": 0.5,
                    },
                    "validations": [{"type": "LessThanNum", "numValues": [10]}],
                },
            }}),
        ),
        ("an empty fields map", json!({"fields": {}})),
        (
            "a sectionalElements map of two entries",
            json!({"sectionalElements": {
                "divider": {"type": "Divider", "orientation": "horizontal", "excluded": false},
                "note": {"type": "Text", "text": "note", "position": {"rightOf": "divider"}},
            }}),
        ),
        (
            "an empty sectionalElements map",
            json!({"sectionalElements": {}}),
        ),
        (
            "every style union member replaced",
            json!({"style": {
                "horizontalGap": {"tokenReference": "space.small"},
                "verticalGap": {"value": "8px"},
                "outerPadding": {"tokenReference": "space.large"},
            }}),
        ),
        ("an empty style", json!({"style": {}})),
        (
            "a new cta",
            json!({"cta": {
                "position": "top_and_bottom",
                "clear": {"excluded": true},
                "cancel": {"children": "Back", "position": {"fixed": "first"}},
                "submit": {"excluded": false, "children": "Submit"},
            }}),
        ),
        (
            "every updatable member at once",
            json!({
                "name": "FullyPatched",
                "formActionType": "update",
                "dataType": {"dataSourceType": "Custom", "dataTypeName": "Invoice"},
                "fields": {"total": {"label": "Total", "inputType": {"type": "NumberField"}}},
                "style": {"verticalGap": {"value": "4px"}},
                "sectionalElements": {"spacer": {"type": "Divider"}},
                "schemaVersion": "3.0",
                "cta": {"position": "top"},
                "labelDecorator": "none",
            }),
        ),
    ];

    // One row per published value of each enumerated member, so the catalog is covered
    // rather than only the value the recorded request happened to carry.
    for action in FORM_ACTION_TYPES {
        cases.push((
            "every formActionType the model admits",
            json!({"formActionType": action}),
        ));
    }
    for source in FORM_DATA_SOURCE_TYPES {
        cases.push((
            "every dataSourceType the model admits",
            json!({"dataType": {"dataSourceType": source, "dataTypeName": "Invoice"}}),
        ));
    }
    for decorator in LABEL_DECORATORS {
        cases.push((
            "every labelDecorator the model admits",
            json!({"labelDecorator": decorator}),
        ));
    }

    // A replacement map is stored key by key, whatever its keys are.
    for key in ["plain", "address.line_1", "t", "42", "título"] {
        cases.push((
            "a fields map keyed any way the protocol admits",
            json!({"fields": one_entry(key, json!({"label": key}))}),
        ));
    }

    // And a map that grows: every entry is kept, not just the last.
    for size in [1usize, 2, 7] {
        let mut fields = Map::new();
        for index in 0..size {
            fields.insert(
                format!("field{index}"),
                json!({"label": format!("Field {index}")}),
            );
        }
        cases.push((
            "a fields map of several entries",
            json!({"fields": Value::Object(fields)}),
        ));
    }

    cases
}

/// The `Form` an update of `stored` with `patch` must leave behind: every member the patch
/// carries takes the patch's value, every other member keeps the stored one.
fn merged(stored: &Value, patch: &Value) -> Value {
    let mut expected = stored.clone();
    let target = expected
        .as_object_mut()
        .expect("the stored form is a JSON object");
    for (member, value) in patch.as_object().expect("a patch is a JSON object") {
        target.insert(member.clone(), value.clone());
    }
    expected
}

/// `UpdateForm` replaces the members its patch carries, keeps the rest, never moves the
/// identifying members or the tags `UpdateFormData` cannot carry, and persists the result:
/// a following `GetForm` answers from that one record.
#[tokio::test]
async fn update_form_replaces_the_patched_members_and_keeps_the_others() {
    let router = emulator();

    for (row, (case, patch)) in patch_cases().into_iter().enumerate() {
        let created = create(&router, baseline_named(&format!("OrderForm-{row}"))).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateForm, got {} with {:?}",
            created.status, created.body
        );
        let id = created.id();

        let updated = update(&router, &id, patch.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateForm, got {} with {:?}",
            updated.status, updated.body
        );

        let expected = merged(&created.body, &patch);
        for member in expected
            .as_object()
            .expect("the expected form is a JSON object")
            .keys()
        {
            let observed = at(&updated.body, &format!("/{member}"));
            assert_eq!(
                observed,
                at(&expected, &format!("/{member}")),
                "case {case:?}: member {member:?} must be {:?} after the update, got {observed:?}",
                at(&expected, &format!("/{member}"))
            );
        }

        // The identifying members, and the tags no `UpdateFormData` member can reach,
        // never move.
        for member in ["appId", "environmentName", "id", "tags"] {
            assert_eq!(
                at(&updated.body, &format!("/{member}")),
                at(&created.body, &format!("/{member}")),
                "case {case:?}: member {member:?} must be untouched by an update, got {:?}",
                at(&updated.body, &format!("/{member}"))
            );
        }

        // What the update answered is what the store holds.
        let read = get(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetForm, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, updated.body,
            "case {case:?}: GetForm must report exactly what UpdateForm persisted, got {:?} \
             for {:?}",
            read.body, updated.body
        );
    }
}

/// A patch carrying one member leaves every OTHER member of the stored form exactly as it
/// was: the rule generalises over which single member is patched.
#[tokio::test]
async fn a_single_member_patch_leaves_every_other_member_alone() {
    let router = emulator();

    // One row per member `UpdateFormData` carries, each with a value different from the
    // baseline's.
    let single_member_patches: Vec<(&str, Value)> = vec![
        ("name", json!({"name": "OnlyTheName"})),
        (
            "dataType",
            json!({"dataType": {"dataSourceType": "Custom", "dataTypeName": "Receipt"}}),
        ),
        ("formActionType", json!({"formActionType": "update"})),
        ("fields", json!({"fields": {"only": {"label": "Only"}}})),
        ("style", json!({"style": {"verticalGap": {"value": "2px"}}})),
        (
            "sectionalElements",
            json!({"sectionalElements": {"only": {"type": "Divider"}}}),
        ),
        ("schemaVersion", json!({"schemaVersion": "9.9"})),
        ("cta", json!({"cta": {"position": "top"}})),
        ("labelDecorator", json!({"labelDecorator": "optional"})),
    ];

    for (member, patch) in single_member_patches {
        let created = create(&router, baseline_named(&format!("OrderForm-{member}"))).await;
        let id = created.id();
        let updated = update(&router, &id, patch.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {member:?}: expected HTTP 200 from UpdateForm, got {} with {:?}",
            updated.status, updated.body
        );

        assert_eq!(
            at(&updated.body, &format!("/{member}")),
            at(&patch, &format!("/{member}")),
            "case {member:?}: the patched member must carry the patch's value, got {:?}",
            at(&updated.body, &format!("/{member}"))
        );
        for other in UPDATABLE_MEMBERS.iter().filter(|other| **other != member) {
            assert_eq!(
                at(&updated.body, &format!("/{other}")),
                at(&created.body, &format!("/{other}")),
                "case {member:?}: member {other:?} was not patched, so it must keep {:?}, got \
                 {:?}",
                at(&created.body, &format!("/{other}")),
                at(&updated.body, &format!("/{other}"))
            );
        }
    }
}

/// Applying one patch again changes nothing further (the merge is idempotent), a
/// `clientToken` is accepted on the update, and one form's update leaves the other forms of
/// the same app environment alone.
#[tokio::test]
async fn repeating_an_update_is_idempotent_and_touches_only_the_form_addressed() {
    let router = emulator();

    // Rows differ in how often the same patch is re-applied and in whether the request
    // carries the `@idempotencyToken`.
    let cases: Vec<(&str, Value, usize, Option<&str>)> = vec![
        ("one member, applied twice", json!({"name": "Twice"}), 2, None),
        (
            "several members, applied four times",
            json!({"schemaVersion": "4.0", "fields": {"a": {"label": "A"}}}),
            4,
            None,
        ),
        (
            "an empty patch, applied three times",
            json!({}),
            3,
            Some("token-a"),
        ),
        (
            "every updatable member, applied twice with a client token",
            json!({
                "name": "Tokened",
                "formActionType": "update",
                "dataType": {"dataSourceType": "Custom", "dataTypeName": "Note"},
                "fields": {},
                "style": {},
                "sectionalElements": {},
                "schemaVersion": "1.1",
                "cta": {"position": "bottom"},
                "labelDecorator": "none",
            }),
            2,
            Some("token-b"),
        ),
    ];

    for (row, (case, patch, attempts, token)) in cases.into_iter().enumerate() {
        let target = create(&router, baseline_named(&format!("Target-{row}")))
            .await
            .id();
        let bystander = create(&router, baseline_named(&format!("Bystander-{row}"))).await;
        let bystander_id = bystander.id();

        let mut bodies = Vec::new();
        for attempt in 0..attempts {
            let answer = match token {
                Some(token) => update_with_token(&router, &target, patch.clone(), token).await,
                None => update(&router, &target, patch.clone()).await,
            };
            assert_eq!(
                answer.status, 200,
                "case {case:?}: attempt {attempt} expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            bodies.push(answer.body);
        }
        assert!(
            bodies.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case:?}: re-applying one patch must leave the same form, got {bodies:?}"
        );

        let read_bystander = get(&router, &bystander_id).await;
        assert_eq!(
            read_bystander.body, bystander.body,
            "case {case:?}: updating one form must leave another form of the same app \
             environment untouched, got {:?} for {:?}",
            read_bystander.body, bystander.body
        );
    }
}

/// An update is not an upsert: an id that names no form is refused with HTTP 404 and
/// nothing is stored, and an id a DIFFERENT app environment owns is refused as the
/// wrong-scope class the service answers for every (appId, environmentName, id)-addressed
/// operation — HTTP 400 carrying the fixed phrase (see the module doc of
/// `resource_error_messages.rs`: the refusal classes are per CLASS, not per resource kind,
/// and the component path has answered this one since it was recorded).
#[tokio::test]
async fn update_form_refuses_an_id_that_names_no_form_in_the_scope_addressed() {
    let router = emulator();

    // An id nothing created is not found, whatever its spelling.
    for (case, id) in [
        (
            "a well-formed uuid nothing minted",
            "9fb4bb01-2f14-47c2-aef7-40ff219ed1c1",
        ),
        ("an id that is not a uuid", "not-a-uuid"),
        ("the id of no form, one character", "x"),
    ] {
        let answer = update(&router, id, json!({"name": "Ghost"})).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: UpdateForm must answer HTTP 404 for an id that names no form, got \
             {} with {:?}",
            answer.status, answer.body
        );
        let read = get(&router, id).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused update must store nothing, but GetForm answered {} with \
             {:?}",
            read.status, read.body
        );
    }

    // A form is addressed by (appId, environmentName, id): the same id in another scope is
    // addressed in the wrong scope — the wrong-scope refusal (400), not the absent one —
    // and the form it does name elsewhere is left alone. Rows differ in which half of the
    // key changes.
    let created = create(&router, baseline()).await;
    let id = created.id();
    for (case, app_id, environment) in [
        ("another environment of the same app", APP_ID, "production"),
        ("another app, the same environment", "d9999zzzz", ENVIRONMENT),
        ("another app and environment", "d9999zzzz", "production"),
    ] {
        let answer = update_in(&router, app_id, environment, &id, json!({"name": "Crossed"})).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: UpdateForm must refuse an id outside the app environment that \
             holds the form as a wrong-scope request, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.body["message"]
                .as_str()
                .unwrap_or_default()
                .contains(WRONG_SCOPE_PHRASE),
            "case {case:?}: the refusal must carry {WRONG_SCOPE_PHRASE:?}, got {:?}",
            answer.body
        );
        let read = get(&router, &id).await;
        assert_eq!(
            read.body, created.body,
            "case {case:?}: a refused cross-scope update must leave the form alone, got {:?} \
             for {:?}",
            read.body, created.body
        );
    }
}

/// The modelled constraints on `UpdateFormData` are refused at the boundary, so no update
/// outside them is ever stored: the form keeps exactly what it held.
#[tokio::test]
async fn an_update_outside_the_modelled_constraints_is_refused_and_stores_nothing() {
    let router = emulator();
    let created = create(&router, baseline()).await;
    let id = created.id();

    // Rows: one per published bound or value set, each one step outside it.
    let cases = vec![
        (
            "a name one character past the @length max",
            json!({"name": "n".repeat(NAME_MAX_LENGTH + 1)}),
        ),
        (
            "a name one character short of the @length min",
            json!({"name": ""}),
        ),
        (
            "a formActionType outside the enum",
            json!({"formActionType": "destroy"}),
        ),
        (
            "a dataSourceType outside the enum",
            json!({"dataType": {"dataSourceType": "Elsewhere", "dataTypeName": "Order"}}),
        ),
        (
            "a labelDecorator outside the enum",
            json!({"labelDecorator": "sometimes"}),
        ),
    ];

    for (case, patch) in cases {
        let answer = update(&router, &id, patch).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 from UpdateForm, got {} with {:?}",
            answer.status, answer.body
        );
        let read = get(&router, &id).await;
        assert_eq!(
            read.body, created.body,
            "case {case:?}: a refused update must store nothing, got {:?} for {:?}",
            read.body, created.body
        );
    }

    // A patch that does satisfy the model still lands on that same form.
    let updated = update(&router, &id, json!({"name": "Survivor"})).await;
    assert_eq!(
        updated.status, 200,
        "an update satisfying the model must still succeed, got {} with {:?}",
        updated.status, updated.body
    );
    assert_eq!(
        at(&updated.body, "/name"),
        json!("Survivor"),
        "the updated form must carry the name the patch supplied, got {:?}",
        at(&updated.body, "/name")
    );
}
