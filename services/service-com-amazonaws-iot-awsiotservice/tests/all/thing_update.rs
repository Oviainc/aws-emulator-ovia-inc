// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdateThing`: what a change to a registered thing
//! record does, and what the `DescribeThing` reader then reports.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rules under test:
//!
//! * an update applies the instructions the request carries to the stored record and
//!   leaves everything the request says nothing about alone, so a later read reports
//!   the merged or replaced attributes and the associated (or removed) thing type;
//! * `AttributePayload$merge` decides whether the provided attribute list is "merged
//!   with the attributes stored in the registry, instead of overwriting them", and in
//!   either case "To remove an attribute, call `UpdateThing` with an empty attribute
//!   value";
//! * one successful update is one write of the record, so its `version` — "The current
//!   version of the thing record in the registry" — advances by exactly one, and a
//!   refused update advances nothing;
//! * `expectedVersion` is optimistic concurrency: a version that does not match the
//!   record is refused `VersionConflictException` (409), and the version an update left
//!   is the one `DeleteThing` then accepts;
//! * a `thingName` the registry does not hold is refused `ResourceNotFoundException`
//!   (404), and a `thingTypeName` is resolved exactly as a create resolves it — a type
//!   the account does not hold is a 404 and one it has DEPRECATED is an
//!   `InvalidRequestException` (400).
//!
//! The tables span what those rules generalise over: whether the payload is merged, how
//! many attributes it carries and of what form (including the empty value that removes
//! one), whether the payload or its `attributes` map is present at all, which thing-type
//! instruction the request carries, and how the supplied `expectedVersion` relates to
//! the stored one.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and region: the scope is how the router picks the claiming service. The signature
/// itself is never checked here — the harness turns request authentication off, exactly
/// as an unauthenticated emulator runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `version` member, or `None` when it is absent or not a number.
    fn version(&self) -> Option<i64> {
        self.body.get("version").and_then(Value::as_i64)
    }

    /// The `attributes` member as an ordered map, so two maps compare independently of
    /// JSON member order. `None` when the member is absent altogether.
    fn attributes(&self) -> Option<BTreeMap<String, String>> {
        self.body.get("attributes").and_then(Value::as_object).map(
            |members: &Map<String, Value>| {
                members
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.clone(),
                            value.as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            },
        )
    }

    /// The error message the refusal carries, or the empty string when it carries none.
    fn message(&self) -> String {
        self.text("message").unwrap_or_default()
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_thing(&self, thing_name: &str, request: &Value) -> Reply {
        self.call("POST", &format!("/things/{thing_name}"), Some(request))
            .await
    }

    async fn update_thing(&self, thing_name: &str, request: &Value) -> Reply {
        self.call("PATCH", &format!("/things/{thing_name}"), Some(request))
            .await
    }

    async fn describe_thing(&self, thing_name: &str) -> Reply {
        self.call("GET", &format!("/things/{thing_name}"), None).await
    }

    async fn delete_thing(&self, thing_name: &str, expected_version: i64) -> Reply {
        self.call(
            "DELETE",
            &format!("/things/{thing_name}?expectedVersion={expected_version}"),
            None,
        )
        .await
    }

    async fn create_type(&self, thing_type_name: &str) -> Reply {
        self.call(
            "POST",
            &format!("/thing-types/{thing_type_name}"),
            Some(&json!({})),
        )
        .await
    }

    async fn deprecate_type(&self, thing_type_name: &str) -> Reply {
        self.call(
            "POST",
            &format!("/thing-types/{thing_type_name}/deprecate"),
            Some(&json!({})),
        )
        .await
    }
}

fn attributes_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// An `attributePayload` carrying `pairs` as its `attributes` map, and `merge`.
fn payload_of(pairs: &[(&str, &str)], merge: bool) -> Value {
    let attributes: Map<String, Value> = pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), json!(value)))
        .collect();
    json!({ "attributes": Value::Object(attributes), "merge": merge })
}

/// One attribute-update case: the attributes the thing is created with, the
/// `attributePayload` the update carries (absent when `None`), and the attributes the
/// record must hold afterwards.
struct AttributeCase {
    label: &'static str,
    created: Vec<(&'static str, &'static str)>,
    payload: Option<Value>,
    expected: BTreeMap<String, String>,
}

/// The table of attribute updates. Its rows differ along what the merge rule
/// generalises over:
///
/// * whether the provided list is MERGED into the stored attributes or OVERWRITES them
///   (`AttributePayload$merge`, modelled `default false`);
/// * how many attributes the list carries — none, one, several — and how many the record
///   already held;
/// * whether an entry's value is EMPTY, which removes the attribute ("To remove an
///   attribute, call `UpdateThing` with an empty attribute value");
/// * whether the request provides an attribute list at all: no `attributePayload`, or a
///   payload carrying no `attributes` map, says nothing about the thing's attributes;
/// * the form of a name and a value — `Attributes` is modelled key
///   `length 0..128, pattern ^[a-zA-Z0-9_.,@/:#-]+$` and value
///   `length 0..800, pattern ^[a-zA-Z0-9_.,@/:#=\[\]-]*$`
///   (`smithy_query(query="shape",shape="Attributes")`), so one row uses the punctuation
///   both patterns allow and one a value at the modelled maximum length.
fn attribute_cases() -> Vec<AttributeCase> {
    let longest_value: &'static str = Box::leak("v".repeat(800).into_boxed_str());
    vec![
        AttributeCase {
            label: "merge adds an attribute and keeps the stored ones",
            created: vec![("color", "red"), ("floor", "3")],
            payload: Some(payload_of(&[("size", "large")], true)),
            expected: attributes_of(&[("color", "red"), ("floor", "3"), ("size", "large")]),
        },
        AttributeCase {
            label: "merge overwrites one stored attribute",
            created: vec![("color", "red"), ("floor", "3")],
            payload: Some(payload_of(&[("color", "blue")], true)),
            expected: attributes_of(&[("color", "blue"), ("floor", "3")]),
        },
        AttributeCase {
            label: "merge with an empty value removes that attribute only",
            created: vec![("color", "red"), ("floor", "3")],
            payload: Some(payload_of(&[("color", "")], true)),
            expected: attributes_of(&[("floor", "3")]),
        },
        AttributeCase {
            label: "merge of several attributes at once",
            created: vec![("color", "red")],
            payload: Some(payload_of(
                &[("color", "green"), ("size", "small"), ("floor", "2")],
                true,
            )),
            expected: attributes_of(&[("color", "green"), ("floor", "2"), ("size", "small")]),
        },
        AttributeCase {
            label: "merge of an empty list leaves the record alone",
            created: vec![("color", "red")],
            payload: Some(payload_of(&[], true)),
            expected: attributes_of(&[("color", "red")]),
        },
        AttributeCase {
            label: "no merge: the provided list overwrites the stored attributes",
            created: vec![("color", "red"), ("floor", "3")],
            payload: Some(payload_of(&[("size", "large")], false)),
            expected: attributes_of(&[("size", "large")]),
        },
        AttributeCase {
            label: "no merge, with merge omitted from the payload altogether",
            created: vec![("color", "red"), ("floor", "3")],
            payload: Some(json!({ "attributes": { "size": "large" } })),
            expected: attributes_of(&[("size", "large")]),
        },
        AttributeCase {
            label: "no merge with an empty list leaves the thing with no attributes",
            created: vec![("color", "red"), ("floor", "3")],
            payload: Some(payload_of(&[], false)),
            expected: BTreeMap::new(),
        },
        AttributeCase {
            label: "a payload carrying no attributes map says nothing about them",
            created: vec![("color", "red")],
            payload: Some(json!({ "merge": true })),
            expected: attributes_of(&[("color", "red")]),
        },
        AttributeCase {
            label: "no attributePayload at all says nothing about them",
            created: vec![("color", "red")],
            payload: None,
            expected: attributes_of(&[("color", "red")]),
        },
        AttributeCase {
            label: "a name and value using the whole modelled alphabet",
            created: vec![("color", "red")],
            payload: Some(payload_of(
                &[("build.no,v@1/2:3#4-5", "a=b[c]-d.e,f@g/h:i#j")],
                true,
            )),
            expected: attributes_of(&[
                ("build.no,v@1/2:3#4-5", "a=b[c]-d.e,f@g/h:i#j"),
                ("color", "red"),
            ]),
        },
        AttributeCase {
            label: "a value at the modelled maximum length",
            created: vec![],
            payload: Some(payload_of(&[("blob", longest_value)], true)),
            expected: attributes_of(&[("blob", longest_value)]),
        },
    ]
}

#[tokio::test]
async fn an_update_applies_its_attribute_payload_and_a_read_reports_it() {
    let emulator = Emulator::new();

    for (index, case) in attribute_cases().into_iter().enumerate() {
        let label = case.label;
        let thing_name = format!("attr-thing-{index}");
        let created = emulator
            .create_thing(
                &thing_name,
                &json!({ "attributePayload": payload_of(&case.created, false) }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateThing must succeed, got {created:?}"
        );

        let updated = emulator
            .update_thing(
                &thing_name,
                &match &case.payload {
                    Some(payload) => json!({ "attributePayload": payload }),
                    None => json!({}),
                },
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: UpdateThing must succeed, got {updated:?}"
        );

        let described = emulator.describe_thing(&thing_name).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: DescribeThing must succeed, got {described:?}"
        );
        assert_eq!(
            described.attributes(),
            Some(case.expected.clone()),
            "case {label:?}: expected the attributes {:?}, got {described:?}",
            case.expected
        );
        // The update is one write of the record, so the version the reader reports is
        // one past the version a create leaves.
        assert_eq!(
            described.version(),
            Some(2),
            "case {label:?}: one update must advance the record to version 2, got {described:?}"
        );
    }
}

/// One thing-type instruction, and the thing type the record must carry afterwards.
struct TypeCase {
    label: &'static str,
    /// The thing type the thing is created with, when it is created with one.
    created_with: Option<&'static str>,
    /// The members of the update request that concern the thing type.
    request: Value,
    /// The `thingTypeName` the record must report afterwards.
    expected: Option<&'static str>,
}

/// The table of thing-type instructions. Its rows differ along which instruction the
/// request carries (`thingTypeName`, `removeThingType`, neither) and what the record
/// already held, which is what the thing-type rule generalises over.
fn type_cases() -> Vec<TypeCase> {
    vec![
        TypeCase {
            label: "a thing with no type is associated with one",
            created_with: None,
            request: json!({ "thingTypeName": "sensor" }),
            expected: Some("sensor"),
        },
        TypeCase {
            label: "a thing's type is re-pointed at another type",
            created_with: Some("sensor"),
            request: json!({ "thingTypeName": "gateway" }),
            expected: Some("gateway"),
        },
        TypeCase {
            label: "removeThingType clears the association",
            created_with: Some("sensor"),
            request: json!({ "removeThingType": true }),
            expected: None,
        },
        TypeCase {
            label: "removeThingType on a thing that has no type leaves it with none",
            created_with: None,
            request: json!({ "removeThingType": true }),
            expected: None,
        },
        TypeCase {
            label: "removeThingType false says nothing about the association",
            created_with: Some("sensor"),
            request: json!({ "removeThingType": false }),
            expected: Some("sensor"),
        },
        TypeCase {
            label: "an update carrying no thing-type member leaves the association",
            created_with: Some("sensor"),
            request: json!({ "attributePayload": { "attributes": { "k": "v" }, "merge": true } }),
            expected: Some("sensor"),
        },
    ]
}

#[tokio::test]
async fn an_update_sets_and_removes_the_thing_type_association() {
    let emulator = Emulator::new();
    for thing_type_name in ["sensor", "gateway"] {
        let created = emulator.create_type(thing_type_name).await;
        assert_eq!(
            created.status, 200,
            "CreateThingType({thing_type_name:?}) must succeed, got {created:?}"
        );
    }

    for (index, case) in type_cases().into_iter().enumerate() {
        let label = case.label;
        let thing_name = format!("type-thing-{index}");
        let body = match case.created_with {
            Some(thing_type_name) => json!({ "thingTypeName": thing_type_name }),
            None => json!({}),
        };
        let created = emulator.create_thing(&thing_name, &body).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateThing must succeed, got {created:?}"
        );

        let updated = emulator.update_thing(&thing_name, &case.request).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: UpdateThing must succeed, got {updated:?}"
        );

        let described = emulator.describe_thing(&thing_name).await;
        assert_eq!(
            described.text("thingTypeName").as_deref(),
            case.expected,
            "case {label:?}: expected thingTypeName {:?}, got {described:?}",
            case.expected
        );
    }
}

#[tokio::test]
async fn a_thing_type_an_update_names_is_resolved_as_a_create_resolves_it() {
    let emulator = Emulator::new();
    let created_type = emulator.create_type("retired").await;
    assert_eq!(
        created_type.status, 200,
        "CreateThingType must succeed, got {created_type:?}"
    );
    let created = emulator.create_thing("resolving-thing", &json!({})).await;
    assert_eq!(
        created.status, 200,
        "CreateThing must succeed, got {created:?}"
    );

    // A type the account holds no record of: the same 404 a create answers.
    let unknown = emulator
        .update_thing("resolving-thing", &json!({ "thingTypeName": "nosuchtype" }))
        .await;
    assert_eq!(
        unknown.status, 404,
        "an update naming a thing type the account does not hold must be refused 404, got {unknown:?}"
    );
    assert_eq!(
        unknown.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {unknown:?}"
    );
    assert!(
        unknown.message().contains("nosuchtype"),
        "the refusal must name the thing type the request asked for, got {unknown:?}"
    );

    // A DEPRECATED type exists, so it is no 404: it is a type nothing may be associated
    // with ("Deprecates a thing type. You can not associate new things with deprecated
    // thing type"), which is a request the service cannot perform.
    let deprecated = emulator.deprecate_type("retired").await;
    assert_eq!(
        deprecated.status, 200,
        "DeprecateThingType must succeed, got {deprecated:?}"
    );
    let refused = emulator
        .update_thing("resolving-thing", &json!({ "thingTypeName": "retired" }))
        .await;
    assert_eq!(
        refused.status, 400,
        "an update naming a deprecated thing type must be refused 400, got {refused:?}"
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("InvalidRequestException"),
        "expected InvalidRequestException, got {refused:?}"
    );

    // Both setting and removing the association in one request states two contradictory
    // things about one member of the record.
    let contradictory = emulator
        .update_thing(
            "resolving-thing",
            &json!({ "thingTypeName": "retired", "removeThingType": true }),
        )
        .await;
    assert_eq!(
        contradictory.status, 400,
        "an update that both sets and removes the thing type must be refused 400, got {contradictory:?}"
    );
    assert_eq!(
        contradictory.error_code.as_deref(),
        Some("InvalidRequestException"),
        "expected InvalidRequestException, got {contradictory:?}"
    );

    // Every refusal above left the record untouched: no type, and the version a create
    // left.
    let described = emulator.describe_thing("resolving-thing").await;
    assert_eq!(
        described.text("thingTypeName"),
        None,
        "a refused update must not associate a thing type, got {described:?}"
    );
    assert_eq!(
        described.version(),
        Some(1),
        "a refused update must not advance the record's version, got {described:?}"
    );
}

#[tokio::test]
async fn every_successful_update_advances_the_record_version_by_one() {
    let emulator = Emulator::new();
    let created = emulator.create_thing("versioned-thing", &json!({})).await;
    assert_eq!(
        created.status, 200,
        "CreateThing must succeed, got {created:?}"
    );
    let described = emulator.describe_thing("versioned-thing").await;
    assert_eq!(
        described.version(),
        Some(1),
        "a freshly created thing is at version 1, got {described:?}"
    );

    // Each update is one write, whatever it changes — a new attribute, the same
    // attribute again, or nothing at all — so the version it leaves is one past the
    // previous read, never two and never the same.
    let updates = [
        json!({ "attributePayload": { "attributes": { "a": "1" }, "merge": true } }),
        json!({ "attributePayload": { "attributes": { "a": "1" }, "merge": true } }),
        json!({}),
        json!({ "removeThingType": false }),
        json!({ "attributePayload": { "attributes": {} } }),
    ];
    for (index, request) in updates.iter().enumerate() {
        let expected = i64::try_from(index).expect("a small index fits an i64") + 2;
        let updated = emulator.update_thing("versioned-thing", request).await;
        assert_eq!(
            updated.status, 200,
            "update {index}: UpdateThing must succeed, got {updated:?}"
        );
        let described = emulator.describe_thing("versioned-thing").await;
        assert_eq!(
            described.version(),
            Some(expected),
            "update {index}: expected version {expected}, got {described:?}"
        );
    }
}

/// One `expectedVersion` case: the value supplied relative to the stored version, and
/// whether the update must be taken.
struct VersionCase {
    label: &'static str,
    /// The `expectedVersion` member, absent when `None`.
    expected_version: Option<i64>,
    /// Whether the record must take the update.
    accepted: bool,
}

/// The table of `expectedVersion` cases: the member is absent, matches the stored
/// version, or does not — one version below it, one above it, and the zero and negative
/// values `OptionalVersion` (a `long`) admits.
fn version_cases() -> Vec<VersionCase> {
    vec![
        VersionCase {
            label: "no expectedVersion at all",
            expected_version: None,
            accepted: true,
        },
        VersionCase {
            label: "an expectedVersion matching the stored version",
            expected_version: Some(1),
            accepted: true,
        },
        VersionCase {
            label: "one version below the stored version",
            expected_version: Some(0),
            accepted: false,
        },
        VersionCase {
            label: "one version above the stored version",
            expected_version: Some(2),
            accepted: false,
        },
        VersionCase {
            label: "a far higher version",
            expected_version: Some(4_294_967_296),
            accepted: false,
        },
        VersionCase {
            label: "a negative version",
            expected_version: Some(-1),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn expected_version_is_optimistic_concurrency_on_the_record() {
    let emulator = Emulator::new();

    for (index, case) in version_cases().into_iter().enumerate() {
        let label = case.label;
        let thing_name = format!("concurrent-thing-{index}");
        let created = emulator
            .create_thing(
                &thing_name,
                &json!({ "attributePayload": { "attributes": { "color": "red" } } }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateThing must succeed, got {created:?}"
        );

        let mut request = json!({ "attributePayload": { "attributes": { "color": "blue" }, "merge": true } });
        if let Some(expected_version) = case.expected_version {
            request
                .as_object_mut()
                .expect("the request body is an object")
                .insert("expectedVersion".to_string(), json!(expected_version));
        }
        let updated = emulator.update_thing(&thing_name, &request).await;
        let described = emulator.describe_thing(&thing_name).await;

        if case.accepted {
            assert_eq!(
                updated.status, 200,
                "case {label:?}: UpdateThing must succeed, got {updated:?}"
            );
            assert_eq!(
                described.attributes(),
                Some(attributes_of(&[("color", "blue")])),
                "case {label:?}: the update must be applied, got {described:?}"
            );
            assert_eq!(
                described.version(),
                Some(2),
                "case {label:?}: an applied update advances the version, got {described:?}"
            );
        } else {
            assert_eq!(
                updated.status, 409,
                "case {label:?}: UpdateThing must be refused 409, got {updated:?}"
            );
            assert_eq!(
                updated.error_code.as_deref(),
                Some("VersionConflictException"),
                "case {label:?}: expected VersionConflictException, got {updated:?}"
            );
            assert_eq!(
                described.attributes(),
                Some(attributes_of(&[("color", "red")])),
                "case {label:?}: a refused update must change nothing, got {described:?}"
            );
            assert_eq!(
                described.version(),
                Some(1),
                "case {label:?}: a refused update must not advance the version, got {described:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_version_an_update_left_is_the_one_a_delete_then_expects() {
    let emulator = Emulator::new();
    let created = emulator.create_thing("deleted-thing", &json!({})).await;
    assert_eq!(
        created.status, 200,
        "CreateThing must succeed, got {created:?}"
    );
    let updated = emulator
        .update_thing(
            "deleted-thing",
            &json!({ "attributePayload": { "attributes": { "color": "blue" } } }),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "UpdateThing must succeed, got {updated:?}"
    );

    // The version the caller held BEFORE the update is now stale, so the delete that
    // names it conflicts.
    let stale = emulator.delete_thing("deleted-thing", 1).await;
    assert_eq!(
        stale.status, 409,
        "DeleteThing at the pre-update version must be refused 409, got {stale:?}"
    );
    assert_eq!(
        stale.error_code.as_deref(),
        Some("VersionConflictException"),
        "expected VersionConflictException, got {stale:?}"
    );

    let current = emulator.delete_thing("deleted-thing", 2).await;
    assert_eq!(
        current.status, 200,
        "DeleteThing at the version the update left must succeed, got {current:?}"
    );
    let described = emulator.describe_thing("deleted-thing").await;
    assert_eq!(
        described.status, 404,
        "the deleted thing must be gone, got {described:?}"
    );
}

#[tokio::test]
async fn an_update_of_a_thing_the_registry_does_not_hold_is_a_404() {
    let emulator = Emulator::new();
    // Every form of request refuses the same way: the name addresses no record at all,
    // so there is nothing to apply the instructions to.
    let requests = [
        json!({}),
        json!({ "attributePayload": { "attributes": { "color": "blue" }, "merge": true } }),
        json!({ "removeThingType": true }),
        json!({ "expectedVersion": 1 }),
    ];
    for (index, request) in requests.iter().enumerate() {
        let refused = emulator.update_thing("ghost-thing", request).await;
        assert_eq!(
            refused.status, 404,
            "request {index}: UpdateThing of an unregistered name must be refused 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "request {index}: expected ResourceNotFoundException, got {refused:?}"
        );
        assert!(
            refused.message().contains("ghost-thing"),
            "request {index}: the refusal must name the thing the request asked for, got {refused:?}"
        );
    }
}
