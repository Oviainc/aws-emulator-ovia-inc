// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AWS IoT thing-group registry: `CreateThingGroup`
//! and the `DescribeThingGroup` reader that proves what it persisted.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: `CreateThingGroup` REGISTERS a group under its name and
//! answers with the identity it persisted (a freshly minted `thingGroupId` matching
//! the modelled `^[a-zA-Z0-9\-]+$` and an
//! `arn:aws:iot:<region>:<account>:thinggroup/<thingGroupName>` scoped to the region
//! the call was signed for); a later read of that name returns that record
//! member-for-member, including the hierarchy chain from the root down to its
//! parent; a repeat create of the same name and configuration is idempotent; the
//! same name with a different configuration is refused
//! `ResourceAlreadyExistsException` (409); a `parentGroupName` that resolves to no
//! group is refused with the modelled `InvalidRequestException` (400); and a name
//! the registry does not hold reads `ResourceNotFoundException` (404).
//!
//! Each table spans the class the rule generalises over — the form and length of a
//! group name, which optional members the request carries, how many attributes it
//! carries and of what form, how deep in the hierarchy the group sits, and the
//! region it is signed for — rather than one recorded request.

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

    /// `thingGroupProperties.thingGroupDescription`, when the read reports one.
    fn description(&self) -> Option<String> {
        self.body
            .get("thingGroupProperties")?
            .get("thingGroupDescription")?
            .as_str()
            .map(str::to_string)
    }

    /// `thingGroupProperties.attributePayload.attributes` as an ordered map, so two
    /// maps compare independently of JSON member order. An absent payload is the
    /// empty map: a group that carries no attributes.
    fn attributes(&self) -> BTreeMap<String, String> {
        let Some(members) = self
            .body
            .get("thingGroupProperties")
            .and_then(|properties| properties.get("attributePayload"))
            .and_then(|payload| payload.get("attributes"))
            .and_then(Value::as_object)
        else {
            return BTreeMap::new();
        };
        members
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    value.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    /// `thingGroupMetadata.parentGroupName`, when the read reports one.
    fn parent_group_name(&self) -> Option<String> {
        self.body
            .get("thingGroupMetadata")?
            .get("parentGroupName")?
            .as_str()
            .map(str::to_string)
    }

    /// `thingGroupMetadata.rootToParentThingGroups` as the `(groupName, groupArn)`
    /// pairs it carries, in the order the read reported them. `None` when the member
    /// is absent altogether.
    fn root_to_parent(&self) -> Option<Vec<(String, String)>> {
        let entries = self
            .body
            .get("thingGroupMetadata")?
            .get("rootToParentThingGroups")?
            .as_array()?;
        Some(
            entries
                .iter()
                .map(|entry| {
                    (
                        entry
                            .get("groupName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        entry
                            .get("groupArn")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect(),
        )
    }

    /// `thingGroupMetadata.creationDate`, as the epoch seconds the restJson1
    /// timestamp carries.
    fn creation_date(&self) -> Option<f64> {
        self.body
            .get("thingGroupMetadata")?
            .get("creationDate")?
            .as_f64()
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

    async fn create_group(&self, thing_group_name: &str, request: &Value) -> Reply {
        self.create_group_in(thing_group_name, request, REGION).await
    }

    async fn create_group_in(
        &self,
        thing_group_name: &str,
        request: &Value,
        region: &str,
    ) -> Reply {
        self.call(
            "POST",
            &format!("/thing-groups/{thing_group_name}"),
            Some(request),
            region,
        )
        .await
    }

    async fn describe_group(&self, thing_group_name: &str) -> Reply {
        self.call(
            "GET",
            &format!("/thing-groups/{thing_group_name}"),
            None,
            REGION,
        )
        .await
    }
}

/// The ARN a thing group created in `region` must carry.
fn expected_arn(region: &str, thing_group_name: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:thinggroup/{thing_group_name}")
}

/// Whether `value` satisfies `ThingGroupId`'s modelled constraints —
/// `length 1..128, pattern ^[a-zA-Z0-9\-]+$`
/// (`smithy_query(query="operation",operation="CreateThingGroup")`).
fn is_modelled_thing_group_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c == '-' || c.is_ascii_alphanumeric())
}

fn attributes_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// A `thingGroupProperties` body carrying `pairs` as its attribute payload and
/// `description` as its description, omitting each member the case leaves out.
fn properties_of(description: Option<&str>, attributes: Option<&[(&str, &str)]>) -> Value {
    let mut properties = Map::new();
    if let Some(description) = description {
        properties.insert("thingGroupDescription".to_string(), json!(description));
    }
    if let Some(pairs) = attributes {
        let attributes: Map<String, Value> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), json!(value)))
            .collect();
        properties.insert(
            "attributePayload".to_string(),
            json!({ "attributes": Value::Object(attributes) }),
        );
    }
    json!({ "thingGroupProperties": Value::Object(properties) })
}

/// One create request: a label, the group name, the body, and the description and
/// attributes the registry must hold afterwards.
struct Case {
    label: &'static str,
    thing_group_name: String,
    body: Value,
    description: Option<String>,
    attributes: BTreeMap<String, String>,
}

/// The table every case list below is built from. It spans what the rule
/// generalises over:
///
/// * the form and length of `thingGroupName` — modelled
///   `length 1..128, pattern ^[a-zA-Z0-9:_-]+$`
///   (`smithy_query(query="shape",shape="ThingGroupName")`), so the rows include the
///   shortest legal name, the longest, and one using every character class the
///   pattern allows;
/// * which optional members the request carries — no `thingGroupProperties` at all,
///   empty properties, a description only, attributes only, both, plus `tags`;
/// * how many attributes the request carries — none, an empty map, one pair, three
///   pairs;
/// * the form of an attribute — `Attributes` is modelled key
///   `length 0..128, pattern ^[a-zA-Z0-9_.,@/:#-]+$` and value
///   `length 0..800, pattern ^[a-zA-Z0-9_.,@/:#=\[\]-]*$`
///   (`smithy_query(query="shape",shape="Attributes")`), so the rows include a key
///   using the punctuation that pattern allows, a value using `=` and `[]`, and the
///   empty value the value pattern's `*` admits;
/// * the form and length of the description — `ThingGroupDescription` is modelled
///   `length 0..2028, pattern ^[\p{Graph}\x20]*$`
///   (`smithy_query(query="shape",shape="ThingGroupDescription")`), so the rows
///   include the empty description the `*` admits, one carrying spaces and
///   punctuation, and one at the published 2028-character maximum.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no thingGroupProperties at all",
            thing_group_name: "demo-group".to_string(),
            body: json!({}),
            description: None,
            attributes: BTreeMap::new(),
        },
        Case {
            label: "empty thingGroupProperties",
            thing_group_name: "empty-properties-group".to_string(),
            body: properties_of(None, None),
            description: None,
            attributes: BTreeMap::new(),
        },
        Case {
            label: "an empty attributes map",
            thing_group_name: "empty-payload-group".to_string(),
            body: properties_of(None, Some(&[])),
            description: None,
            attributes: BTreeMap::new(),
        },
        Case {
            label: "a description only",
            thing_group_name: "described-group".to_string(),
            body: properties_of(Some("Floor 3 sensors, west wing (building #7)."), None),
            description: Some("Floor 3 sensors, west wing (building #7).".to_string()),
            attributes: BTreeMap::new(),
        },
        Case {
            label: "the empty description the pattern's * admits",
            thing_group_name: "blank-description-group".to_string(),
            body: properties_of(Some(""), None),
            description: Some(String::new()),
            attributes: BTreeMap::new(),
        },
        Case {
            label: "a description at the published 2028-character maximum",
            thing_group_name: "max-description-group".to_string(),
            body: properties_of(Some(&"d".repeat(2028)), None),
            description: Some("d".repeat(2028)),
            attributes: BTreeMap::new(),
        },
        Case {
            label: "one attribute",
            thing_group_name: "Fleet:01_group-A".to_string(),
            body: properties_of(None, Some(&[("color", "red")])),
            description: None,
            attributes: attributes_of(&[("color", "red")]),
        },
        Case {
            label: "three attributes beside a description",
            thing_group_name: "mixed_group".to_string(),
            body: properties_of(
                Some("three attributes"),
                Some(&[("color", "red"), ("size", "large"), ("floor", "3")]),
            ),
            description: Some("three attributes".to_string()),
            attributes: attributes_of(&[("color", "red"), ("size", "large"), ("floor", "3")]),
        },
        Case {
            label: "attribute key and value using the whole modelled alphabet",
            thing_group_name: "punct_group".to_string(),
            body: properties_of(None, Some(&[("build.no,v@1/2:3#4-5", "a=b[c]-d.e,f@g/h:i#j")])),
            description: None,
            attributes: attributes_of(&[("build.no,v@1/2:3#4-5", "a=b[c]-d.e,f@g/h:i#j")]),
        },
        Case {
            label: "an empty attribute value, which the value pattern admits",
            thing_group_name: "blank-value-group".to_string(),
            body: properties_of(None, Some(&[("color", "")])),
            description: None,
            attributes: attributes_of(&[("color", "")]),
        },
        Case {
            label: "tags beside properties",
            thing_group_name: "tagged-group".to_string(),
            body: json!({
                "thingGroupProperties": { "thingGroupDescription": "tagged" },
                "tags": [{ "Key": "env", "Value": "prod" }, { "Key": "team" }]
            }),
            description: Some("tagged".to_string()),
            attributes: BTreeMap::new(),
        },
        Case {
            label: "the shortest legal group name",
            thing_group_name: "a".to_string(),
            body: properties_of(None, Some(&[("k", "v")])),
            description: None,
            attributes: attributes_of(&[("k", "v")]),
        },
        Case {
            label: "the longest legal group name",
            thing_group_name: "n".repeat(128),
            body: json!({}),
            description: None,
            attributes: BTreeMap::new(),
        },
    ]
}

#[tokio::test]
async fn a_created_thing_group_round_trips_through_describe_thing_group() {
    let emulator = Emulator::new();

    let mut minted_ids = BTreeSet::new();
    for case in cases() {
        let label = case.label;
        let created = emulator
            .create_group(&case.thing_group_name, &case.body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateThingGroup must succeed, got {created:?}"
        );
        assert_eq!(
            created.text("thingGroupName").as_deref(),
            Some(case.thing_group_name.as_str()),
            "case {label:?}: CreateThingGroup must report the created name, got {created:?}"
        );
        assert_eq!(
            created.text("thingGroupArn"),
            Some(expected_arn(REGION, &case.thing_group_name)),
            "case {label:?}: CreateThingGroup must report the group's ARN, got {created:?}"
        );
        let group_id = created
            .text("thingGroupId")
            .unwrap_or_else(|| panic!("case {label:?}: CreateThingGroup reported no thingGroupId"));
        assert!(
            is_modelled_thing_group_id(&group_id),
            "case {label:?}: expected a modelled thingGroupId, got {group_id:?}"
        );
        assert!(
            minted_ids.insert(group_id.clone()),
            "case {label:?}: thingGroupId {group_id:?} was already minted for another group"
        );

        let described = emulator.describe_group(&case.thing_group_name).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: DescribeThingGroup must find the created group, got {described:?}"
        );
        assert_eq!(
            described.text("thingGroupId").as_deref(),
            Some(group_id.as_str()),
            "case {label:?}: DescribeThingGroup must report the persisted id, got {described:?}"
        );
        assert_eq!(
            described.text("thingGroupArn"),
            Some(expected_arn(REGION, &case.thing_group_name)),
            "case {label:?}: DescribeThingGroup must report the persisted ARN, got {described:?}"
        );
        assert_eq!(
            described.text("thingGroupName").as_deref(),
            Some(case.thing_group_name.as_str()),
            "case {label:?}: DescribeThingGroup must report the persisted name, got {described:?}"
        );
        assert_eq!(
            described.description(),
            case.description,
            "case {label:?}: DescribeThingGroup must report exactly the persisted description, got {described:?}"
        );
        assert_eq!(
            described.attributes(),
            case.attributes,
            "case {label:?}: DescribeThingGroup must report exactly the persisted attributes, got {described:?}"
        );
        assert_eq!(
            described.body.get("version").and_then(Value::as_i64),
            Some(1),
            "case {label:?}: a freshly created group is at version 1, got {described:?}"
        );
        assert!(
            described.creation_date().is_some(),
            "case {label:?}: the metadata must carry a creationDate, got {described:?}"
        );
        assert_eq!(
            described.parent_group_name(),
            None,
            "case {label:?}: no parentGroupName was requested, got {described:?}"
        );
        assert_eq!(
            described.root_to_parent(),
            None,
            "case {label:?}: a root group has no rootToParentThingGroups, got {described:?}"
        );
        // `status`, `indexName`, `queryString` and `queryVersion` are the DYNAMIC
        // thing-group members; a group created by `CreateThingGroup` is static.
        for dynamic_only in ["status", "indexName", "queryString", "queryVersion"] {
            assert!(
                described.body.get(dynamic_only).is_none(),
                "case {label:?}: a static group reports no {dynamic_only}, got {described:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_creation_date_is_stable_across_reads() {
    let emulator = Emulator::new();

    // A read must not change what a later caller sees: the creationDate recorded on
    // the create is what every read reports, however many times it is read.
    for case in cases() {
        let label = case.label;
        let created = emulator
            .create_group(&case.thing_group_name, &case.body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateThingGroup must succeed, got {created:?}"
        );

        let first = emulator.describe_group(&case.thing_group_name).await;
        let second = emulator.describe_group(&case.thing_group_name).await;
        assert_eq!(
            first.creation_date(),
            second.creation_date(),
            "case {label:?}: creationDate must be stable across reads, got {second:?}"
        );
        assert_eq!(
            first.body.get("version").and_then(Value::as_i64),
            second.body.get("version").and_then(Value::as_i64),
            "case {label:?}: a read must not bump the version, got {second:?}"
        );
    }
}

#[tokio::test]
async fn recreating_a_thing_group_with_the_same_configuration_is_idempotent() {
    let emulator = Emulator::new();

    for case in cases() {
        let label = case.label;
        let first = emulator
            .create_group(&case.thing_group_name, &case.body)
            .await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: the first CreateThingGroup must succeed, got {first:?}"
        );
        let repeat = emulator
            .create_group(&case.thing_group_name, &case.body)
            .await;
        assert_eq!(
            repeat.status, 200,
            "case {label:?}: repeating a create with the same configuration succeeds, got {repeat:?}"
        );
        assert_eq!(
            repeat.text("thingGroupId"),
            first.text("thingGroupId"),
            "case {label:?}: the repeat create must not mint a new thingGroupId, got {repeat:?}"
        );
        assert_eq!(
            repeat.text("thingGroupArn"),
            first.text("thingGroupArn"),
            "case {label:?}: the repeat create must report the stored ARN, got {repeat:?}"
        );

        let described = emulator.describe_group(&case.thing_group_name).await;
        assert_eq!(
            described.body.get("version").and_then(Value::as_i64),
            Some(1),
            "case {label:?}: an idempotent repeat writes nothing, so the version stands, got {described:?}"
        );
        assert_eq!(
            described.attributes(),
            case.attributes,
            "case {label:?}: an idempotent repeat leaves the attributes alone, got {described:?}"
        );
        assert_eq!(
            described.description(),
            case.description,
            "case {label:?}: an idempotent repeat leaves the description alone, got {described:?}"
        );
    }
}

#[tokio::test]
async fn recreating_a_thing_group_with_a_different_configuration_is_refused() {
    let emulator = Emulator::new();

    // The hierarchy root every "parent changed" row re-points to.
    for fixture in ["root-a", "root-b"] {
        let created = emulator.create_group(fixture, &json!({})).await;
        assert_eq!(
            created.status, 200,
            "case {fixture:?}: the fixture root group must be created, got {created:?}"
        );
    }

    // Each row is the SAME name created twice with configurations that differ along
    // one axis: a description added, changed or dropped, an attribute added, its
    // value changed, its key renamed, every attribute dropped, the parent changed.
    let rows = [
        (
            "a description added",
            json!({}),
            properties_of(Some("now described"), None),
        ),
        (
            "a description changed",
            properties_of(Some("first"), None),
            properties_of(Some("second"), None),
        ),
        (
            "a description dropped",
            properties_of(Some("first"), None),
            json!({}),
        ),
        (
            "an attribute added",
            properties_of(None, Some(&[("color", "red")])),
            properties_of(None, Some(&[("color", "red"), ("size", "large")])),
        ),
        (
            "an attribute value changed",
            properties_of(None, Some(&[("color", "red")])),
            properties_of(None, Some(&[("color", "blue")])),
        ),
        (
            "an attribute key renamed",
            properties_of(None, Some(&[("color", "red")])),
            properties_of(None, Some(&[("colour", "red")])),
        ),
        (
            "every attribute dropped",
            properties_of(None, Some(&[("color", "red")])),
            properties_of(None, Some(&[])),
        ),
        (
            "the parent changed",
            json!({ "parentGroupName": "root-a" }),
            json!({ "parentGroupName": "root-b" }),
        ),
        (
            "a parent added to a root group",
            json!({}),
            json!({ "parentGroupName": "root-a" }),
        ),
    ];

    for (index, (label, first_body, second_body)) in rows.iter().enumerate() {
        let thing_group_name = format!("conflict-{index}");
        let first = emulator.create_group(&thing_group_name, first_body).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: the first CreateThingGroup must succeed, got {first:?}"
        );
        let first_id = first.text("thingGroupId");

        let second = emulator.create_group(&thing_group_name, second_body).await;
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
            Some(thing_group_name.as_str()),
            "case {label:?}: the refusal names the resource that exists, got {second:?}"
        );
        assert_eq!(
            second.text("resourceArn"),
            Some(expected_arn(REGION, &thing_group_name)),
            "case {label:?}: the refusal carries the existing group's ARN, got {second:?}"
        );

        // The refused create wrote NOTHING: the registry still holds the first one.
        let described = emulator.describe_group(&thing_group_name).await;
        assert_eq!(
            described.text("thingGroupId"),
            first_id,
            "case {label:?}: a refused create must leave the stored group alone, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_hierarchy_reports_the_chain_from_the_root_down_to_the_parent() {
    let emulator = Emulator::new();

    // One chain, built a level at a time, so each row is a group one level deeper
    // than the last: the rule generalises over DEPTH, not over one parent/child pair.
    let chain = ["level0", "level1", "level2", "level3", "level4"];
    for (depth, name) in chain.iter().enumerate() {
        let body = match depth.checked_sub(1).map(|parent| chain[parent]) {
            Some(parent) => json!({
                "parentGroupName": parent,
                "thingGroupProperties": { "thingGroupDescription": format!("depth {depth}") }
            }),
            None => json!({ "thingGroupProperties": { "thingGroupDescription": "depth 0" } }),
        };
        let created = emulator.create_group(name, &body).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: CreateThingGroup must succeed at depth {depth}, got {created:?}"
        );
    }

    for (depth, name) in chain.iter().enumerate() {
        let described = emulator.describe_group(name).await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: DescribeThingGroup must find the group, got {described:?}"
        );
        let expected_parent = depth.checked_sub(1).map(|parent| chain[parent].to_string());
        assert_eq!(
            described.parent_group_name(),
            expected_parent,
            "case {name:?}: expected parentGroupName {expected_parent:?}, got {described:?}"
        );

        // rootToParentThingGroups runs from the hierarchy root down to the parent,
        // so a group at depth N carries exactly N entries — and none at the root.
        let expected_chain: Option<Vec<(String, String)>> = if depth == 0 {
            None
        } else {
            Some(
                chain[..depth]
                    .iter()
                    .map(|ancestor| {
                        (
                            (*ancestor).to_string(),
                            expected_arn(REGION, ancestor),
                        )
                    })
                    .collect(),
            )
        };
        assert_eq!(
            described.root_to_parent(),
            expected_chain,
            "case {name:?}: expected the root-to-parent chain {expected_chain:?}, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_parent_group_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();

    let existing = emulator.create_group("existing-parent", &json!({})).await;
    assert_eq!(
        existing.status, 200,
        "the fixture parent group must be created, got {existing:?}"
    );

    // A `parentGroupName` is a reference to another group, so every name that
    // resolves to no group is unresolvable — whatever its form — and so is a group
    // naming ITSELF as its parent.
    let rows = [
        ("an unknown parent", "child-0", json!({ "parentGroupName": "no-such-group" })),
        (
            "a parent differing from an existing one only by case",
            "child-1",
            json!({ "parentGroupName": "Existing-Parent" }),
        ),
        (
            "a parent differing from an existing one by a suffix",
            "child-2",
            json!({ "parentGroupName": "existing-parent-2" }),
        ),
        (
            "the shortest legal name, which nothing created",
            "child-3",
            json!({ "parentGroupName": "a" }),
        ),
        (
            "the longest legal name, which nothing created",
            "child-4",
            json!({ "parentGroupName": "n".repeat(128) }),
        ),
        (
            "the group naming itself as its parent",
            "child-5",
            json!({ "parentGroupName": "child-5" }),
        ),
        (
            "an unknown parent beside properties and tags",
            "child-6",
            json!({
                "parentGroupName": "Other:Group_1-x",
                "thingGroupProperties": { "attributePayload": { "attributes": { "color": "red" } } },
                "tags": [{ "Key": "env", "Value": "prod" }]
            }),
        ),
    ];

    for (label, thing_group_name, body) in &rows {
        let created = emulator.create_group(thing_group_name, body).await;
        assert_eq!(
            created.status, 400,
            "case {label:?}: an unresolvable parentGroupName must be refused 400, got {created:?}"
        );
        assert_eq!(
            created.error_code.as_deref(),
            Some("InvalidRequestException"),
            "case {label:?}: expected InvalidRequestException, got {created:?}"
        );

        // A refused create registers nothing.
        let described = emulator.describe_group(thing_group_name).await;
        assert_eq!(
            described.status, 404,
            "case {label:?}: the refused create must have created nothing, got {described:?}"
        );
    }
}

#[tokio::test]
async fn describing_a_thing_group_the_registry_does_not_hold_is_refused() {
    let emulator = Emulator::new();

    let created = emulator
        .create_group("created-group", &properties_of(Some("fixture"), None))
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture group must be created, got {created:?}"
    );

    // Names that differ from the created one by case, by a suffix, by a prefix, and
    // names of a form nothing created.
    for label in [
        "Created-Group",
        "created-group-2",
        "x-created-group",
        "a",
        &"n".repeat(128),
    ] {
        let described = emulator.describe_group(label).await;
        assert_eq!(
            described.status, 404,
            "case {label:?}: a name no group was created under must be refused 404, got {described:?}"
        );
        assert_eq!(
            described.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_thing_group_arn_is_scoped_to_the_region_the_create_was_signed_for() {
    let emulator = Emulator::new();

    // The only region a request carries is its SigV4 credential scope, so that is
    // the region the minted ARN is scoped to — and the stored ARN is what a later
    // read reports, whatever region that read is signed for.
    for (index, region) in ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"]
        .into_iter()
        .enumerate()
    {
        let thing_group_name = format!("regional-{index}");
        let created = emulator
            .create_group_in(&thing_group_name, &json!({}), region)
            .await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: CreateThingGroup must succeed, got {created:?}"
        );
        assert_eq!(
            created.text("thingGroupArn"),
            Some(expected_arn(region, &thing_group_name)),
            "case {region:?}: the ARN must carry the signed-for region, got {created:?}"
        );

        let described = emulator.describe_group(&thing_group_name).await;
        assert_eq!(
            described.text("thingGroupArn"),
            Some(expected_arn(region, &thing_group_name)),
            "case {region:?}: a read reports the stored ARN, got {described:?}"
        );
    }
}

#[tokio::test]
async fn every_created_thing_group_is_reachable_independently() {
    let emulator = Emulator::new();

    // One registry, many groups: each read must answer from its OWN record. The
    // count is more than the table above so the map, not a single slot, is exercised.
    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for index in 0..12u32 {
        let thing_group_name = format!("group-{index}");
        let created = emulator
            .create_group(
                &thing_group_name,
                &properties_of(None, Some(&[("index", &index.to_string())])),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {thing_group_name:?}: CreateThingGroup must succeed, got {created:?}"
        );
        let group_id = created.text("thingGroupId").unwrap_or_else(|| {
            panic!("case {thing_group_name:?}: CreateThingGroup reported no thingGroupId")
        });
        expected.insert(thing_group_name, group_id);
    }

    let distinct: BTreeSet<&String> = expected.values().collect();
    assert_eq!(
        distinct.len(),
        expected.len(),
        "every group must carry its own thingGroupId, got {expected:?}"
    );

    for (thing_group_name, group_id) in &expected {
        let described = emulator.describe_group(thing_group_name).await;
        assert_eq!(
            described.text("thingGroupId").as_deref(),
            Some(group_id.as_str()),
            "case {thing_group_name:?}: each read answers from its own record, got {described:?}"
        );
        assert_eq!(
            described.attributes(),
            attributes_of(&[("index", thing_group_name.trim_start_matches("group-"))]),
            "case {thing_group_name:?}: each read answers with its own attributes, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_thing_and_a_thing_group_of_the_same_name_are_separate_resources() {
    let emulator = Emulator::new();

    // The two registries are keyed independently, so one name can exist in both and
    // neither read sees the other's record.
    let shared_name = "shared-name";
    let thing = emulator
        .call(
            "POST",
            &format!("/things/{shared_name}"),
            Some(&json!({ "attributePayload": { "attributes": { "kind": "thing" } } })),
            REGION,
        )
        .await;
    assert_eq!(
        thing.status, 200,
        "case {shared_name:?}: CreateThing must succeed, got {thing:?}"
    );
    let group = emulator
        .create_group(shared_name, &properties_of(Some("group"), None))
        .await;
    assert_eq!(
        group.status, 200,
        "case {shared_name:?}: CreateThingGroup must succeed under a taken thing name, got {group:?}"
    );
    assert_eq!(
        group.text("thingGroupArn"),
        Some(expected_arn(REGION, shared_name)),
        "case {shared_name:?}: a thing group's ARN names thinggroup/, got {group:?}"
    );
    assert_ne!(
        group.text("thingGroupId"),
        thing.text("thingId"),
        "case {shared_name:?}: the two resources must carry different ids, got {group:?}"
    );

    let described_group = emulator.describe_group(shared_name).await;
    assert_eq!(
        described_group.description(),
        Some("group".to_string()),
        "case {shared_name:?}: the group read reports the group's record, got {described_group:?}"
    );
    assert_eq!(
        described_group.attributes(),
        BTreeMap::new(),
        "case {shared_name:?}: the group read must not see the thing's attributes, got {described_group:?}"
    );
}
