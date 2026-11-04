// SPDX-License-Identifier: CC0-1.0

//! The Amplify UI Builder form create/read path, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test: a form's members are whatever `CreateForm` stored
//! under the (appId, environmentName) the create addressed, and `GetForm` answers from
//! that one record.
//!
//! Each suite in this crate carries its own request harness (see
//! `component_lifecycle.rs` and `component_wire_fidelity.rs`), so this one does too.

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

/// The longest `FormName` the model admits (`@length(min: 1, max: 255)` on
/// `CreateFormData$name`, from `smithy_query(query="shape", shape="FormName")`).
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

/// `Form$id` targets the model's `Uuid` shape. Checked here against the textual form of a
/// version 4 UUID rather than through the crate's own minting code, so an id that is not a
/// UUID cannot make this test pass.
fn is_uuid_v4(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let ok = match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
        };
        if !ok {
            return false;
        }
    }
    // Version nibble, then the variant nibble the RFC pins to one of 8, 9, a, b.
    bytes[14] == b'4' && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
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
    /// The form id the answer carries, or the empty string.
    ///
    /// `CreateForm` and `GetForm` both bind their form to `@httpPayload`, so the body IS
    /// the `Form`.
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

async fn create_in_with_token(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    data: Value,
    token: &str,
) -> Answer {
    let uri = format!("{}?clientToken={}", forms_uri(app_id, environment), token);
    call(router, "POST", &uri, Some(data)).await
}

async fn create_with_token(router: &MultiServiceRouter, data: Value, token: &str) -> Answer {
    create_in_with_token(router, APP_ID, ENVIRONMENT, data, token).await
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

/// The create payloads the round-trip cases drive, differing along what the rule
/// generalises over: which values, how many map entries, which union member a style config
/// or a field position carries, how deeply a field input nests, whether an optional member
/// is present at all, and the published bound of `name`.
fn create_cases() -> Vec<(&'static str, Value)> {
    let mut cases = vec![
        ("only the required members", minimal_create("Blank")),
        (
            "two fields, a validation list and a fixed position",
            json!({
                "name": "PostCreateForm",
                "formActionType": "create",
                "dataType": {"dataSourceType": "DataStore", "dataTypeName": "Post"},
                "fields": {
                    "title": {
                        "label": "Title",
                        "position": {"fixed": "first"},
                        "inputType": {"type": "TextField", "required": true},
                        "validations": [
                            {"type": "Required", "validationMessage": "Title is required"},
                            {"type": "StartWith", "strValues": ["a", "b"]},
                        ],
                    },
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
                },
                "style": {"horizontalGap": {"value": "16px"}},
                "sectionalElements": {},
                "schemaVersion": "1.0",
            }),
        ),
        (
            "both style union members, three sectional elements and a full CTA",
            json!({
                "name": "StyledForm",
                "formActionType": "update",
                "dataType": {"dataSourceType": "Custom", "dataTypeName": "Order"},
                "fields": {},
                "style": {
                    "horizontalGap": {"tokenReference": "space.small"},
                    "verticalGap": {"value": "8px"},
                    "outerPadding": {"tokenReference": "space.large"},
                },
                "sectionalElements": {
                    "heading": {"type": "Heading", "level": 1, "text": "Order"},
                    "divider": {"type": "Divider", "orientation": "horizontal", "excluded": false},
                    "spacer": {"type": "Text", "text": "note", "position": {"rightOf": "heading"}},
                },
                "schemaVersion": "2.1",
                "cta": {
                    "position": "top_and_bottom",
                    "clear": {"excluded": true},
                    "cancel": {"children": "Back", "position": {"fixed": "first"}},
                    "submit": {"excluded": false, "children": "Save"},
                },
            }),
        ),
        (
            "tags, a nested value-mapping field and a file uploader",
            json!({
                "name": "UploadForm",
                "formActionType": "create",
                "dataType": {"dataSourceType": "Custom", "dataTypeName": "Asset"},
                "fields": {
                    "kind": {
                        "inputType": {
                            "type": "SelectField",
                            "valueMappings": {
                                "values": [
                                    {"value": {"value": "image"}, "displayValue": {"value": "Image"}},
                                    {
                                        "value": {
                                            "bindingProperties": {"property": "asset", "field": "kind"},
                                        },
                                    },
                                    {"value": {"concat": [{"value": "a"}, {"value": "b"}]}},
                                ],
                                "bindingProperties": {
                                    "asset": {"type": "Data", "bindingProperties": {"model": "Asset"}},
                                },
                            },
                        },
                    },
                    "file": {
                        "inputType": {
                            "type": "StorageField",
                            "fileUploaderConfig": {
                                "accessLevel": "protected",
                                "acceptedFileTypes": ["image/png", "image/jpeg"],
                                "showThumbnails": true,
                                "isResumable": false,
                                "maxFileCount": 3,
                                "maxSize": 1024,
                            },
                        },
                    },
                },
                "style": {},
                "sectionalElements": {},
                "schemaVersion": "1.0",
                "tags": {"env": "test", "team": "studio"},
            }),
        ),
        (
            "the longest name the model admits",
            minimal_create(&"n".repeat(NAME_MAX_LENGTH)),
        ),
        ("the shortest name the model admits", minimal_create("f")),
    ];

    // One row per published value of each enumerated member, so the catalog is covered
    // rather than only the value the recorded request happened to carry.
    // A form name is unique among the live forms of one app environment, so each row
    // of a catalog walk names its form after the value it carries.
    for action in FORM_ACTION_TYPES {
        let mut data = minimal_create(&format!("ActionTyped-{action}"));
        data["formActionType"] = json!(action);
        cases.push(("every formActionType the model admits", data));
    }
    for source in FORM_DATA_SOURCE_TYPES {
        let mut data = minimal_create(&format!("SourceTyped-{source}"));
        data["dataType"] = json!({"dataSourceType": source, "dataTypeName": "Post"});
        cases.push(("every dataSourceType the model admits", data));
    }
    for decorator in LABEL_DECORATORS {
        let mut data = minimal_create(&format!("Decorated-{decorator}"));
        data["labelDecorator"] = json!(decorator);
        cases.push(("every labelDecorator the model admits", data));
    }

    cases
}

/// `CreateForm` persists what it was given and `GetForm` reports that record: every member
/// the create carried comes back with the value it was stored with, the id is a fresh
/// UUID, the scope members echo the path the create addressed, and the two bodies are
/// equal.
#[tokio::test]
async fn a_created_form_is_read_back_from_what_was_stored() {
    let router = emulator();
    let mut minted: Vec<String> = Vec::new();

    for (case, data) in create_cases() {
        let created = create(&router, data.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateForm, got {} with {:?}",
            created.status, created.body
        );

        let id = created.id();
        assert!(
            is_uuid_v4(&id),
            "case {case:?}: expected a version 4 UUID id, got {id:?}"
        );
        assert!(
            !minted.contains(&id),
            "case {case:?}: CreateForm reissued the id {id:?}"
        );
        minted.push(id.clone());

        assert_eq!(
            at(&created.body, "/appId"),
            json!(APP_ID),
            "case {case:?}: the created form must carry the appId it was created under, got {:?}",
            at(&created.body, "/appId")
        );
        assert_eq!(
            at(&created.body, "/environmentName"),
            json!(ENVIRONMENT),
            "case {case:?}: the created form must carry the environment it was created \
             under, got {:?}",
            at(&created.body, "/environmentName")
        );

        let read = get(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetForm, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created.body,
            "case {case:?}: GetForm must report exactly what CreateForm persisted, got {:?} \
             for {:?}",
            read.body, created.body
        );

        for (member, expected) in data.as_object().expect("each case is a JSON object") {
            let observed = at(&read.body, &format!("/{member}"));
            assert_eq!(
                &observed, expected,
                "case {case:?}: member {member:?} was created as {expected:?} but GetForm \
                 reports {observed:?}"
            );
        }

        // An optional member the create did not carry is reported as an absence, not as an
        // invented value: `CreateFormData`'s `cta`, `tags` and `labelDecorator` are each
        // optional on `Form` too.
        for member in ["cta", "tags", "labelDecorator"] {
            if !data.as_object().is_some_and(|data| data.contains_key(member)) {
                assert_eq!(
                    at(&read.body, &format!("/{member}")),
                    Value::Null,
                    "case {case:?}: member {member:?} was not created, so GetForm must not \
                     report it, got {:?}",
                    at(&read.body, &format!("/{member}"))
                );
            }
        }
    }

    assert!(
        minted.len() >= create_cases().len(),
        "every case must have minted a form, got {:?}",
        minted.len()
    );
}

/// `clientToken` is an `@idempotencyToken`: the same token always names the form it first
/// created, distinct tokens name distinct forms, and no token at all mints a new form every
/// time.
#[tokio::test]
async fn a_repeated_create_with_one_client_token_returns_one_form() {
    // Rows differ in token shape and in how often the create is retried.
    let cases: Vec<(&str, String, usize)> = vec![
        ("a plain token, retried once", "token-a".to_string(), 2),
        ("the same token, retried four times", "token-b".to_string(), 4),
        ("a one-character token", "!".to_string(), 3),
        ("a 64-character token", "t".repeat(64), 2),
        ("a token of punctuation only", "~!-_.%7E".to_string(), 2),
    ];

    let router = emulator();
    let mut distinct = Vec::new();
    for (row, (case, token, attempts)) in cases.iter().enumerate() {
        let mut ids = Vec::new();
        // Each row mints its own form, and a form name is unique among the live forms
        // of one app environment, so the rows carry distinct names; the retries within
        // a row replay the token rather than taking the name a second time.
        let name = format!("Idempotent-{row}");
        for attempt in 0..*attempts {
            let answer = create_with_token(&router, minimal_create(&name), token).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: attempt {attempt} expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            ids.push(answer.id());
        }
        assert!(
            ids.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case:?}: one client token must name one form, got {ids:?}"
        );
        distinct.push(ids[0].clone());
    }

    // Two creates with no token at all are two forms.
    let first = create(&router, minimal_create("Untokened-first")).await.id();
    let second = create(&router, minimal_create("Untokened-second"))
        .await
        .id();
    assert_ne!(
        first, second,
        "a create carrying no clientToken must mint a new form every time, got {first:?} twice"
    );
    distinct.push(first);
    distinct.push(second);

    let mut unique = distinct.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        distinct.len(),
        "distinct client tokens must name distinct forms, got {distinct:?}"
    );

    // A token is scoped to the app environment it was used in: reusing one elsewhere
    // creates that environment's own form rather than reporting one the request does not
    // address. Rows differ in which half of the key changes.
    let shared_token = "token-a";
    for (case, app_id, environment) in [
        ("another environment of the same app", APP_ID, "production"),
        ("another app, the same environment", "d9999zzzz", ENVIRONMENT),
        ("another app and environment", "d9999zzzz", "production"),
    ] {
        let answer = create_in_with_token(
            &router,
            app_id,
            environment,
            minimal_create("Scoped"),
            shared_token,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            !distinct.contains(&answer.id()),
            "case {case:?}: a client token is scoped to one app environment, but reusing \
             {shared_token:?} reported the form {:?} created elsewhere",
            answer.id()
        );
        assert_eq!(
            at(&answer.body, "/appId"),
            json!(app_id),
            "case {case:?}: the form must carry the appId the create addressed, got {:?}",
            at(&answer.body, "/appId")
        );
        assert_eq!(
            at(&answer.body, "/environmentName"),
            json!(environment),
            "case {case:?}: the form must carry the environment the create addressed, got {:?}",
            at(&answer.body, "/environmentName")
        );
    }
}

/// A form belongs to the (appId, environmentName) it was created under: `GetForm` reports
/// it there and refuses an id it holds no form for in the scope the request addressed.
#[tokio::test]
async fn get_form_answers_only_within_the_app_environment_that_holds_the_form() {
    let router = emulator();

    // Rows differ in which half of the key the form was created under.
    let scopes = [
        ("the default app environment", APP_ID, ENVIRONMENT),
        ("another environment of the same app", APP_ID, "production"),
        ("another app, the same environment", "d9999zzzz", ENVIRONMENT),
    ];

    let mut created = Vec::new();
    for (case, app_id, environment) in scopes {
        let answer = create_in(&router, app_id, environment, minimal_create("Scoped")).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from CreateForm, got {} with {:?}",
            answer.status, answer.body
        );
        created.push((case, app_id, environment, answer.id()));
    }

    for (case, app_id, environment, id) in &created {
        let read = get_in(&router, app_id, environment, id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetForm must report the form in the scope it was created in, \
             got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.id(),
            *id,
            "case {case:?}: GetForm must report the form {id:?}, got {:?}",
            read.id()
        );

        // The same id addressed in any OTHER scope names no form there.
        for (other_case, other_app, other_environment, _) in &created {
            if other_app == app_id && other_environment == environment {
                continue;
            }
            let crossed = get_in(&router, other_app, other_environment, id).await;
            assert!(
                (400..500).contains(&crossed.status),
                "case {case:?} read from {other_case:?}: a form of another app environment \
                 must be refused as a client error, got {} with {:?}",
                crossed.status,
                crossed.body
            );
            assert_ne!(
                crossed.id(),
                *id,
                "case {case:?} read from {other_case:?}: GetForm must not report the form \
                 {id:?} outside its own app environment"
            );
        }
    }

    // An id nothing created is not found, whatever its spelling.
    for (case, id) in [
        ("a well-formed uuid nothing minted", "9fb4bb01-2f14-47c2-aef7-40ff219ed1c1"),
        ("an id that is not a uuid", "not-a-uuid"),
        ("the id of no form, one character", "x"),
    ] {
        let answer = get(&router, id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: GetForm must answer HTTP 404 for an id that names no form, got \
             {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// The modelled constraints on `CreateFormData` are refused at the boundary, so nothing
/// outside them is ever stored: a refused create leaves no form behind.
#[tokio::test]
async fn a_create_outside_the_modelled_constraints_is_refused_and_stores_nothing() {
    let router = emulator();

    let too_long = "n".repeat(NAME_MAX_LENGTH + 1);
    let mut nameless = minimal_create("Nameless");
    nameless
        .as_object_mut()
        .expect("the payload is an object")
        .remove("name");
    let mut bad_action = minimal_create("BadAction");
    bad_action["formActionType"] = json!("destroy");
    let mut bad_source = minimal_create("BadSource");
    bad_source["dataType"] = json!({"dataSourceType": "Elsewhere", "dataTypeName": "Post"});
    let mut bad_decorator = minimal_create("BadDecorator");
    bad_decorator["labelDecorator"] = json!("sometimes");

    // Rows: one per published bound or value set, each one step outside it.
    let cases = vec![
        ("a name one character past the @length max", {
            minimal_create(&too_long)
        }),
        ("a name one character short of the @length min", {
            minimal_create("")
        }),
        ("no name at all, which the model requires", nameless),
        ("a formActionType outside the enum", bad_action),
        ("a dataSourceType outside the enum", bad_source),
        ("a labelDecorator outside the enum", bad_decorator),
    ];

    for (case, data) in cases {
        let answer = create(&router, data).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 from CreateForm, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // Nothing above was stored: a create that did satisfy the model is the only form any
    // read can find, and the refused ones left no id behind.
    let stored = create(&router, minimal_create("Survivor")).await;
    assert_eq!(
        stored.status, 200,
        "a create satisfying the model must still succeed, got {} with {:?}",
        stored.status, stored.body
    );
    let read = get(&router, &stored.id()).await;
    assert_eq!(
        read.body,
        stored.body,
        "GetForm must report the one form the refused creates did not displace, got {:?}",
        read.body
    );
    assert_eq!(
        at(&read.body, "/name"),
        json!("Survivor"),
        "the stored form must carry the name its create supplied, got {:?}",
        at(&read.body, "/name")
    );
}

/// A field or sectional-element map is stored key by key, whatever its keys are: the rule
/// generalises over the map's size and over the spelling of its keys.
#[tokio::test]
async fn field_and_sectional_element_maps_round_trip_whatever_their_keys() {
    let router = emulator();

    // Rows differ in key spelling and in how many entries the map holds.
    let keys = [
        ("a plain key", "title"),
        ("a key with punctuation", "address.line_1"),
        ("a single-character key", "t"),
        ("a key of digits", "42"),
        ("a non-ascii key", "título"),
    ];

    for (row, (case, key)) in keys.into_iter().enumerate() {
        let mut data = minimal_create(&format!("Keyed-{row}"));
        data["fields"] = one_entry(key, json!({"label": key, "inputType": {"type": "TextField"}}));
        data["sectionalElements"] = one_entry(key, json!({"type": "Heading", "text": key}));

        let created = create(&router, data.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateForm, got {} with {:?}",
            created.status, created.body
        );
        let read = get(&router, &created.id()).await;
        assert_eq!(
            at(&read.body, "/fields"),
            data["fields"],
            "case {case:?}: the fields map must round-trip under the key {key:?}, got {:?}",
            at(&read.body, "/fields")
        );
        assert_eq!(
            at(&read.body, "/sectionalElements"),
            data["sectionalElements"],
            "case {case:?}: the sectionalElements map must round-trip under the key {key:?}, \
             got {:?}",
            at(&read.body, "/sectionalElements")
        );
    }

    // And a map that grows: every entry is kept, not just the last.
    for size in [0usize, 1, 2, 7] {
        let mut fields = Map::new();
        let mut elements = Map::new();
        for index in 0..size {
            fields.insert(
                format!("field{index}"),
                json!({"label": format!("Field {index}")}),
            );
            elements.insert(format!("element{index}"), json!({"type": "Divider"}));
        }
        let mut data = minimal_create(&format!("Sized-{size}"));
        data["fields"] = Value::Object(fields);
        data["sectionalElements"] = Value::Object(elements);

        let created = create(&router, data.clone()).await;
        let read = get(&router, &created.id()).await;
        assert_eq!(
            at(&read.body, "/fields"),
            data["fields"],
            "case {size} fields: every entry must be stored, got {:?}",
            at(&read.body, "/fields")
        );
        assert_eq!(
            at(&read.body, "/sectionalElements"),
            data["sectionalElements"],
            "case {size} sectional elements: every entry must be stored, got {:?}",
            at(&read.body, "/sectionalElements")
        );
    }
}
