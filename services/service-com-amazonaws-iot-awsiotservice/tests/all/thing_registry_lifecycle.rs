// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AWS IoT thing registry: `CreateThing` and the
//! `DescribeThing` reader that proves what it persisted.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: `CreateThing` REGISTERS a thing under its name and answers
//! with the identity it persisted (a freshly minted v4-UUID `thingId` and an
//! `arn:aws:iot:<region>:<account>:thing/<thingName>`), a later read of that name
//! returns that record member-for-member, a repeat create of the same name and
//! configuration is idempotent, the same name with a different configuration is
//! refused `ResourceAlreadyExistsException` (409), and a name — or a referenced
//! thing type or billing group — the registry does not hold is refused
//! `ResourceNotFoundException` (404).
//!
//! Each table spans the class the rule generalises over — the form and length of a
//! thing name, how many attributes a request carries and of what form, which
//! optional members it carries, and the region it is signed for — rather than one
//! recorded request.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing
/// name and `region`: the scope is how the router picks the claiming service, and
/// how the service picks the region its ARNs are scoped to. The signature itself is
/// never checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{region}/iot/aws4_request, \
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

    /// The `attributes` member as an ordered map, so two maps compare independently
    /// of JSON member order. `None` when the member is absent altogether.
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

    async fn call(&self, method: &str, path: &str, body: Option<&Value>, region: &str) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
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
        self.create_thing_in(thing_name, request, REGION).await
    }

    async fn create_thing_in(&self, thing_name: &str, request: &Value, region: &str) -> Reply {
        self.call("POST", &format!("/things/{thing_name}"), Some(request), region)
            .await
    }

    async fn describe_thing(&self, thing_name: &str) -> Reply {
        self.call("GET", &format!("/things/{thing_name}"), None, REGION)
            .await
    }

    async fn delete_thing(&self, thing_name: &str, query: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/things/{thing_name}{query}"),
            None,
            REGION,
        )
        .await
    }
}

/// The ARN a thing registered in `region` must carry.
fn expected_arn(region: &str, thing_name: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:thing/{thing_name}")
}

/// Whether `value` is a canonical lowercase version-4 UUID (8-4-4-4-12 hex, version
/// nibble `4`, variant nibble in `[89ab]`) — the form AWS IoT's `thingId` takes.
fn is_uuid_v4(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    if groups.len() != 5 {
        return false;
    }
    let lengths = [8, 4, 4, 4, 12];
    if groups
        .iter()
        .zip(lengths)
        .any(|(group, want)| group.len() != want)
    {
        return false;
    }
    if !value
        .chars()
        .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c))
    {
        return false;
    }
    groups[2].starts_with('4') && matches!(&groups[3][0..1], "8" | "9" | "a" | "b")
}

/// One create request: a label, the thing name, the body, and the attributes the
/// registry must hold afterwards.
struct Case {
    label: &'static str,
    thing_name: String,
    body: Value,
    attributes: BTreeMap<String, String>,
}

fn attributes_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

fn payload_of(pairs: &[(&str, &str)]) -> Value {
    let attributes: Map<String, Value> = pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), json!(value)))
        .collect();
    json!({ "attributePayload": { "attributes": Value::Object(attributes) } })
}

/// The table every case list below is built from. It spans what the rule
/// generalises over:
///
/// * the form and length of `thingName` — `ThingName` is modelled
///   `length 1..128, pattern ^[a-zA-Z0-9:_-]+$`
///   (`smithy_query(query="operation",operation="CreateThing")`), so the rows
///   include the shortest legal name, the longest, and one using every character
///   class the pattern allows;
/// * how many attributes the request carries — none at all (no `attributePayload`),
///   an empty map, one pair, three pairs;
/// * the form of an attribute — `Attributes` is modelled key
///   `length 0..128, pattern ^[a-zA-Z0-9_.,@/:#-]+$` and value
///   `length 0..800, pattern ^[a-zA-Z0-9_.,@/:#=\[\]-]*$`
///   (`smithy_query(query="shape",shape="Attributes")`), so the rows include a key
///   using the punctuation that pattern allows, a value using `=` and `[]`, and the
///   empty value the value pattern's `*` admits.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no attributePayload at all",
            thing_name: "demo-thing".to_string(),
            body: json!({}),
            attributes: BTreeMap::new(),
        },
        Case {
            label: "an empty attributes map",
            thing_name: "empty-payload-thing".to_string(),
            body: payload_of(&[]),
            attributes: BTreeMap::new(),
        },
        Case {
            label: "one attribute",
            thing_name: "sensor1".to_string(),
            body: payload_of(&[("color", "red")]),
            attributes: attributes_of(&[("color", "red")]),
        },
        Case {
            label: "three attributes",
            thing_name: "Fleet:01_sensor-A".to_string(),
            body: payload_of(&[("color", "red"), ("size", "large"), ("floor", "3")]),
            attributes: attributes_of(&[("color", "red"), ("size", "large"), ("floor", "3")]),
        },
        Case {
            label: "attribute key and value using the whole modelled alphabet",
            thing_name: "punct_thing".to_string(),
            body: payload_of(&[("build.no,v@1/2:3#4-5", "a=b[c]-d.e,f@g/h:i#j")]),
            attributes: attributes_of(&[("build.no,v@1/2:3#4-5", "a=b[c]-d.e,f@g/h:i#j")]),
        },
        Case {
            label: "an empty attribute value, which the value pattern admits",
            thing_name: "blank-value-thing".to_string(),
            body: payload_of(&[("color", "")]),
            attributes: attributes_of(&[("color", "")]),
        },
        Case {
            label: "the shortest legal thing name",
            thing_name: "a".to_string(),
            body: payload_of(&[("k", "v")]),
            attributes: attributes_of(&[("k", "v")]),
        },
        Case {
            label: "the longest legal thing name",
            thing_name: "n".repeat(128),
            body: json!({}),
            attributes: BTreeMap::new(),
        },
    ]
}

#[tokio::test]
async fn a_created_thing_round_trips_through_describe_thing() {
    let emulator = Emulator::new();

    let mut minted_ids = BTreeSet::new();
    for case in cases() {
        let label = case.label;
        let created = emulator.create_thing(&case.thing_name, &case.body).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateThing must succeed, got {created:?}"
        );
        assert_eq!(
            created.text("thingName").as_deref(),
            Some(case.thing_name.as_str()),
            "case {label:?}: CreateThing must report the registered name, got {created:?}"
        );
        assert_eq!(
            created.text("thingArn"),
            Some(expected_arn(REGION, &case.thing_name)),
            "case {label:?}: CreateThing must report the thing's ARN, got {created:?}"
        );
        let thing_id = created
            .text("thingId")
            .unwrap_or_else(|| panic!("case {label:?}: CreateThing reported no thingId"));
        assert!(
            is_uuid_v4(&thing_id),
            "case {label:?}: expected a v4 UUID thingId, got {thing_id:?}"
        );
        assert!(
            minted_ids.insert(thing_id.clone()),
            "case {label:?}: thingId {thing_id:?} was already minted for another thing"
        );

        let described = emulator.describe_thing(&case.thing_name).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: DescribeThing must find the created thing, got {described:?}"
        );
        assert_eq!(
            described.text("thingId").as_deref(),
            Some(thing_id.as_str()),
            "case {label:?}: DescribeThing must report the persisted thingId, got {described:?}"
        );
        assert_eq!(
            described.text("thingArn"),
            Some(expected_arn(REGION, &case.thing_name)),
            "case {label:?}: DescribeThing must report the persisted ARN, got {described:?}"
        );
        assert_eq!(
            described.text("thingName").as_deref(),
            Some(case.thing_name.as_str()),
            "case {label:?}: DescribeThing must report the persisted name, got {described:?}"
        );
        assert_eq!(
            described.text("defaultClientId").as_deref(),
            Some(case.thing_name.as_str()),
            "case {label:?}: defaultClientId is the thing name, got {described:?}"
        );
        assert_eq!(
            described.attributes(),
            Some(case.attributes.clone()),
            "case {label:?}: DescribeThing must report exactly the persisted attributes, got {described:?}"
        );
        assert_eq!(
            described.body.get("version").and_then(Value::as_i64),
            Some(1),
            "case {label:?}: a freshly registered thing is at version 1, got {described:?}"
        );
        assert!(
            described.body.get("thingTypeName").is_none(),
            "case {label:?}: no thing type was requested, got {described:?}"
        );
        assert!(
            described.body.get("billingGroupName").is_none(),
            "case {label:?}: no billing group was requested, got {described:?}"
        );
    }
}

#[tokio::test]
async fn recreating_a_thing_with_the_same_configuration_is_idempotent() {
    let emulator = Emulator::new();

    for case in cases() {
        let label = case.label;
        let first = emulator.create_thing(&case.thing_name, &case.body).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: the first CreateThing must succeed, got {first:?}"
        );
        let repeat = emulator.create_thing(&case.thing_name, &case.body).await;
        assert_eq!(
            repeat.status, 200,
            "case {label:?}: repeating a create with the same configuration succeeds, got {repeat:?}"
        );
        assert_eq!(
            repeat.text("thingId"),
            first.text("thingId"),
            "case {label:?}: the repeat create must not mint a new thingId, got {repeat:?}"
        );
        assert_eq!(
            repeat.text("thingArn"),
            first.text("thingArn"),
            "case {label:?}: the repeat create must report the stored ARN, got {repeat:?}"
        );

        let described = emulator.describe_thing(&case.thing_name).await;
        assert_eq!(
            described.body.get("version").and_then(Value::as_i64),
            Some(1),
            "case {label:?}: an idempotent repeat writes nothing, so the version stands, got {described:?}"
        );
        assert_eq!(
            described.attributes(),
            Some(case.attributes.clone()),
            "case {label:?}: an idempotent repeat leaves the attributes alone, got {described:?}"
        );
    }
}

#[tokio::test]
async fn recreating_a_thing_with_a_different_configuration_is_refused() {
    let emulator = Emulator::new();

    // Each row is the SAME name created twice with configurations that differ along
    // one axis: a pair added, a value changed, a key renamed, every pair dropped.
    let rows = [
        ("an attribute added", payload_of(&[("color", "red")]), payload_of(&[("color", "red"), ("size", "large")])),
        ("an attribute value changed", payload_of(&[("color", "red")]), payload_of(&[("color", "blue")])),
        ("an attribute key renamed", payload_of(&[("color", "red")]), payload_of(&[("colour", "red")])),
        ("every attribute dropped", payload_of(&[("color", "red")]), payload_of(&[])),
        ("attributes added to a thing created without any", json!({}), payload_of(&[("color", "red")])),
    ];

    for (index, (label, first_body, second_body)) in rows.iter().enumerate() {
        let thing_name = format!("conflict-{index}");
        let first = emulator.create_thing(&thing_name, first_body).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: the first CreateThing must succeed, got {first:?}"
        );
        let first_id = first.text("thingId");

        let second = emulator.create_thing(&thing_name, second_body).await;
        assert_eq!(
            second.status, 409,
            "case {label:?}: a differing configuration must be refused 409, got {second:?}"
        );
        assert_eq!(
            second.error_code.as_deref(),
            Some("ResourceAlreadyExistsException"),
            "case {label:?}: expected ResourceAlreadyExistsException, got {second:?}"
        );
        assert_eq!(
            second.text("resourceId").as_deref(),
            Some(thing_name.as_str()),
            "case {label:?}: the refusal names the resource that exists, got {second:?}"
        );
        assert_eq!(
            second.text("resourceArn"),
            Some(expected_arn(REGION, &thing_name)),
            "case {label:?}: the refusal carries the existing thing's ARN, got {second:?}"
        );

        // The refused create wrote NOTHING: the registry still holds the first one.
        let described = emulator.describe_thing(&thing_name).await;
        assert_eq!(
            described.text("thingId"),
            first_id,
            "case {label:?}: a refused create must leave the stored thing alone, got {described:?}"
        );
        assert_eq!(
            described.attributes(),
            emulator
                .describe_thing(&thing_name)
                .await
                .attributes(),
            "case {label:?}: reading the thing twice must report the same attributes"
        );
    }
}

#[tokio::test]
async fn a_create_referencing_a_resource_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();

    // Neither a thing type nor a billing group can exist in a registry nothing has
    // created one in, so every reference to one is a reference to a resource that
    // does not exist — the modelled ResourceNotFoundException (404).
    let rows = [
        (
            "an unknown thingTypeName",
            json!({ "thingTypeName": "sensor-type" }),
        ),
        (
            "an unknown billingGroupName",
            json!({ "billingGroupName": "group-one" }),
        ),
        (
            "an unknown thingTypeName beside attributes",
            json!({
                "thingTypeName": "Other:Type_1-x",
                "attributePayload": { "attributes": { "color": "red" } }
            }),
        ),
        (
            "an unknown billingGroupName beside attributes",
            json!({
                "billingGroupName": "Other:Group_1-x",
                "attributePayload": { "attributes": { "color": "red" } }
            }),
        ),
    ];

    for (index, (label, body)) in rows.iter().enumerate() {
        let thing_name = format!("dangling-{index}");
        let created = emulator.create_thing(&thing_name, body).await;
        assert_eq!(
            created.status, 404,
            "case {label:?}: a reference to a missing resource must be refused 404, got {created:?}"
        );
        assert_eq!(
            created.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {created:?}"
        );

        // A refused create registers nothing.
        let described = emulator.describe_thing(&thing_name).await;
        assert_eq!(
            described.status, 404,
            "case {label:?}: the refused create must have registered nothing, got {described:?}"
        );
    }
}

#[tokio::test]
async fn describing_a_thing_the_registry_does_not_hold_is_refused() {
    let emulator = Emulator::new();

    let registered = emulator
        .create_thing("registered-thing", &payload_of(&[("color", "red")]))
        .await;
    assert_eq!(
        registered.status, 200,
        "the fixture thing must register, got {registered:?}"
    );

    // Names that differ from the registered one by case, by a suffix, by a prefix,
    // and names of a form nothing created.
    for label in [
        "Registered-Thing",
        "registered-thing-2",
        "x-registered-thing",
        "a",
        &"n".repeat(128),
    ] {
        let described = emulator.describe_thing(label).await;
        assert_eq!(
            described.status, 404,
            "case {label:?}: an unregistered name must be refused 404, got {described:?}"
        );
        assert_eq!(
            described.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {described:?}"
        );
        // The refusal's message is built from the request: it names the ACCOUNT and
        // the thing name that was asked for, so it is never one fixed string.
        let expected = format!("Thing {ACCOUNT_ID}:{label} not found.");
        assert_eq!(
            described.text("message").as_deref(),
            Some(expected.as_str()),
            "case {label:?}: expected the message {expected:?}, got {described:?}"
        );
    }
}

/// RULE: `DeleteThing` removes the thing from the registry — a later read of that
/// name is a 404 — and is IDEMPOTENT: "Returns successfully with no error if the
/// deletion is successful or you specify a thing that doesn't exist". An
/// `expectedVersion` that does not match the stored record is refused 409
/// `VersionConflictException` and removes nothing.
#[tokio::test]
async fn deleting_a_thing_removes_it_and_is_idempotent() {
    let emulator = Emulator::new();

    // (case, the name to delete, whether it is registered first, the query string the
    // delete carries, the expected status)
    //
    // The rows differ in whether the thing exists, in the form of its name, and in
    // whether the delete states an expected version — the matching one, and one that
    // does not match.
    let rows: [(&str, &str, bool, &str, u16); 6] = [
        ("a registered thing", "delete-plain", true, "", 200),
        (
            "a registered thing, at its stored version",
            "delete-versioned",
            true,
            "?expectedVersion=1",
            200,
        ),
        (
            "a registered thing, at a version it is not at",
            "delete-stale",
            true,
            "?expectedVersion=7",
            409,
        ),
        (
            "a name nothing registered",
            "delete-absent",
            false,
            "",
            200,
        ),
        (
            "a name nothing registered, with an expected version",
            "delete-absent-versioned",
            false,
            "?expectedVersion=1",
            200,
        ),
        (
            "a name carrying the colon its pattern allows",
            "Delete:Thing_9-x",
            true,
            "",
            200,
        ),
    ];

    for (case, thing_name, registered, query, expected) in rows {
        if registered {
            let created = emulator
                .create_thing(thing_name, &payload_of(&[("color", "red")]))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the fixture thing must register, got {created:?}"
            );
        }

        let deleted = emulator.delete_thing(thing_name, query).await;
        assert_eq!(
            deleted.status, expected,
            "case {case:?}: expected {expected}, got {deleted:?}"
        );

        if expected == 409 {
            assert_eq!(
                deleted.error_code.as_deref(),
                Some("VersionConflictException"),
                "case {case:?}: expected VersionConflictException, got {deleted:?}"
            );
            // The refused delete removed nothing: the thing is still readable.
            let described = emulator.describe_thing(thing_name).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: a refused delete must leave the thing registered, got {described:?}"
            );
            continue;
        }

        // A successful delete is observable: the name is gone, and deleting it again
        // still succeeds.
        let described = emulator.describe_thing(thing_name).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: the deleted name must be a 404, got {described:?}"
        );
        let again = emulator.delete_thing(thing_name, "").await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: a repeated delete is still a success, got {again:?}"
        );
        // And the name is free again: a create re-registers it.
        let recreated = emulator.create_thing(thing_name, &json!({})).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the deleted name must be reusable, got {recreated:?}"
        );
    }
}

#[tokio::test]
async fn a_thing_arn_is_scoped_to_the_region_the_create_was_signed_for() {
    let emulator = Emulator::new();

    // The only region a request carries is its SigV4 credential scope, so that is
    // the region the minted ARN is scoped to — and the stored ARN is what a later
    // read reports, whatever region that read is signed for.
    for (index, region) in ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"]
        .into_iter()
        .enumerate()
    {
        let thing_name = format!("regional-{index}");
        let created = emulator
            .create_thing_in(&thing_name, &json!({}), region)
            .await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: CreateThing must succeed, got {created:?}"
        );
        assert_eq!(
            created.text("thingArn"),
            Some(expected_arn(region, &thing_name)),
            "case {region:?}: the ARN must carry the signed-for region, got {created:?}"
        );

        let described = emulator.describe_thing(&thing_name).await;
        assert_eq!(
            described.text("thingArn"),
            Some(expected_arn(region, &thing_name)),
            "case {region:?}: a read reports the stored ARN, got {described:?}"
        );
    }
}

#[tokio::test]
async fn every_registered_thing_is_reachable_independently() {
    let emulator = Emulator::new();

    // One registry, many things: each read must answer from its OWN record. The
    // count is more than the table above so the map, not a single slot, is exercised.
    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for index in 0..12u32 {
        let thing_name = format!("thing-{index}");
        let created = emulator
            .create_thing(&thing_name, &payload_of(&[("index", &index.to_string())]))
            .await;
        assert_eq!(
            created.status, 200,
            "case {thing_name:?}: CreateThing must succeed, got {created:?}"
        );
        let thing_id = created
            .text("thingId")
            .unwrap_or_else(|| panic!("case {thing_name:?}: CreateThing reported no thingId"));
        expected.insert(thing_name, thing_id);
    }

    let distinct: BTreeSet<&String> = expected.values().collect();
    assert_eq!(
        distinct.len(),
        expected.len(),
        "every thing must carry its own thingId, got {expected:?}"
    );

    for (thing_name, thing_id) in &expected {
        let described = emulator.describe_thing(thing_name).await;
        assert_eq!(
            described.text("thingId").as_deref(),
            Some(thing_id.as_str()),
            "case {thing_name:?}: each read answers from its own record, got {described:?}"
        );
        assert_eq!(
            described.attributes(),
            Some(attributes_of(&[(
                "index",
                thing_name.trim_start_matches("thing-")
            )])),
            "case {thing_name:?}: each read answers with its own attributes, got {described:?}"
        );
    }
}
