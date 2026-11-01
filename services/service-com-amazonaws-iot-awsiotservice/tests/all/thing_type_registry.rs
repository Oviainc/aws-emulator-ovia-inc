// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AWS IoT thing-type registry: `CreateThingType`,
//! the `DescribeThingType` reader that proves what it persisted,
//! `DeprecateThingType`, and the effect a deprecated type has on `CreateThing`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rules under test:
//!
//! * `CreateThingType` REGISTERS a type under its name and answers with the identity
//!   it persisted (a freshly minted `thingTypeId` and an
//!   `arn:aws:iot:<region>:<account>:thingtype/<thingTypeName>` scoped to the region
//!   the call was signed for); a later read of that name returns that record
//!   member-for-member; a repeat create of the same name and configuration is
//!   idempotent; the same name with a different configuration is refused
//!   `ResourceAlreadyExistsException` (409); and a name the registry does not hold
//!   reads `ResourceNotFoundException` (404).
//! * `DeprecateThingType` records the deprecation, which `DescribeThingType` reports
//!   as `thingTypeMetadata.deprecated` with a `deprecationDate`, and `undoDeprecate`
//!   takes it back; both directions are idempotent.
//! * `ThingTypeMetadata$deprecated` is "If true, no new things could be associated
//!   with this type", so `CreateThing` naming a deprecated type is refused
//!   `InvalidRequestException` (400) while a type that is not deprecated is accepted.
//!
//! Each table spans the class the rules generalise over — the form and length of a
//! thing type name, which optional members the request carries, how many searchable
//! and propagating attributes it carries and of what form, which member of a
//! configuration differs on a conflicting re-create, and the region the call is
//! signed for — rather than one recorded request.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    MAX_THING_TYPE_PROPERTY_ENTRIES, MAX_THING_TYPE_TAGS,
};
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

    /// The `thingTypeProperties` the read reports, as the comparable form the table
    /// states its expectation in. An absent member is `None`, which is what a type
    /// created without it carries.
    fn properties(&self) -> Properties {
        let properties = self.body.get("thingTypeProperties");
        Properties {
            description: properties
                .and_then(|properties| properties.get("thingTypeDescription"))
                .and_then(Value::as_str)
                .map(str::to_string),
            searchable_attributes: properties
                .and_then(|properties| properties.get("searchableAttributes"))
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .map(|entry| entry.as_str().unwrap_or_default().to_string())
                        .collect()
                }),
            propagating_attributes: properties
                .and_then(|properties| properties.get("mqtt5Configuration"))
                .and_then(|configuration| configuration.get("propagatingAttributes"))
                .and_then(Value::as_array)
                .map(|entries| entries.iter().map(Propagating::from_json).collect()),
        }
    }

    /// `thingTypeMetadata.deprecated`. Absent is `false`: the member is modelled with
    /// `default false`, so a type nobody deprecated may report either form.
    fn deprecated(&self) -> bool {
        self.body
            .get("thingTypeMetadata")
            .and_then(|metadata| metadata.get("deprecated"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// `thingTypeMetadata.deprecationDate`, as the epoch seconds the restJson1
    /// timestamp carries.
    fn deprecation_date(&self) -> Option<f64> {
        self.body
            .get("thingTypeMetadata")?
            .get("deprecationDate")?
            .as_f64()
    }

    /// `thingTypeMetadata.creationDate`, as the epoch seconds the restJson1 timestamp
    /// carries.
    fn creation_date(&self) -> Option<f64> {
        self.body
            .get("thingTypeMetadata")?
            .get("creationDate")?
            .as_f64()
    }
}

/// One `propagatingAttributes` entry, in a comparable form.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Propagating {
    user_property_key: Option<String>,
    thing_attribute: Option<String>,
    connection_attribute: Option<String>,
}

impl Propagating {
    fn from_json(entry: &Value) -> Self {
        let member = |name: &str| {
            entry
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        Self {
            user_property_key: member("userPropertyKey"),
            thing_attribute: member("thingAttribute"),
            connection_attribute: member("connectionAttribute"),
        }
    }

    fn to_json(&self) -> Value {
        let mut entry = Map::new();
        if let Some(value) = &self.user_property_key {
            entry.insert("userPropertyKey".to_string(), json!(value));
        }
        if let Some(value) = &self.thing_attribute {
            entry.insert("thingAttribute".to_string(), json!(value));
        }
        if let Some(value) = &self.connection_attribute {
            entry.insert("connectionAttribute".to_string(), json!(value));
        }
        Value::Object(entry)
    }
}

/// A thing type's properties as a case states them and as a read must report them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Properties {
    description: Option<String>,
    searchable_attributes: Option<Vec<String>>,
    propagating_attributes: Option<Vec<Propagating>>,
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

    async fn create_type(&self, thing_type_name: &str, request: &Value) -> Reply {
        self.create_type_in(thing_type_name, request, REGION).await
    }

    async fn create_type_in(&self, thing_type_name: &str, request: &Value, region: &str) -> Reply {
        self.call(
            "POST",
            &format!("/thing-types/{thing_type_name}"),
            Some(request),
            region,
        )
        .await
    }

    async fn describe_type(&self, thing_type_name: &str) -> Reply {
        self.call(
            "GET",
            &format!("/thing-types/{thing_type_name}"),
            None,
            REGION,
        )
        .await
    }

    async fn deprecate_type(&self, thing_type_name: &str, undo: Option<bool>) -> Reply {
        let body = match undo {
            Some(undo) => json!({ "undoDeprecate": undo }),
            None => json!({}),
        };
        self.call(
            "POST",
            &format!("/thing-types/{thing_type_name}/deprecate"),
            Some(&body),
            REGION,
        )
        .await
    }

    async fn create_thing(&self, thing_name: &str, thing_type_name: &str) -> Reply {
        self.call(
            "POST",
            &format!("/things/{thing_name}"),
            Some(&json!({ "thingTypeName": thing_type_name })),
            REGION,
        )
        .await
    }
}

/// The ARN a thing type created in `region` must carry.
fn expected_arn(region: &str, thing_type_name: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:thingtype/{thing_type_name}")
}

/// A `thingTypeProperties` request body carrying exactly the members `properties`
/// states, omitting each one it leaves out.
fn body_of(properties: &Properties) -> Value {
    let mut members = Map::new();
    if let Some(description) = &properties.description {
        members.insert("thingTypeDescription".to_string(), json!(description));
    }
    if let Some(searchable) = &properties.searchable_attributes {
        members.insert("searchableAttributes".to_string(), json!(searchable));
    }
    if let Some(propagating) = &properties.propagating_attributes {
        members.insert(
            "mqtt5Configuration".to_string(),
            json!({
                "propagatingAttributes": propagating
                    .iter()
                    .map(Propagating::to_json)
                    .collect::<Vec<Value>>(),
            }),
        );
    }
    json!({ "thingTypeProperties": Value::Object(members) })
}

/// One create request: a label, the type name, the body, and the properties a read
/// must report afterwards.
struct Case {
    label: &'static str,
    thing_type_name: String,
    body: Value,
    properties: Properties,
}

/// A propagating attribute carrying all three members.
fn propagating_all() -> Propagating {
    Propagating {
        user_property_key: Some("fleet:region".to_string()),
        thing_attribute: Some("site_id".to_string()),
        connection_attribute: Some("iot:ClientId".to_string()),
    }
}

/// A propagating attribute carrying only the connection attribute — every member of
/// `PropagatingAttribute` is optional
/// (`smithy_query(query="shape",shape="PropagatingAttribute")`).
fn propagating_connection_only() -> Propagating {
    Propagating {
        user_property_key: None,
        thing_attribute: None,
        connection_attribute: Some("iot:Connection.Thing.ThingName".to_string()),
    }
}

/// The table every case list below is built from. It spans what the rule generalises
/// over:
///
/// * the form and length of `thingTypeName` — modelled
///   `length 1..128, pattern ^[a-zA-Z0-9:_-]+$`
///   (`smithy_query(query="operation",operation="CreateThingType")`), so the rows
///   include the shortest legal name, the longest, and one using every character
///   class the pattern allows;
/// * which optional members the request carries — no `thingTypeProperties` at all,
///   empty properties, a description only, searchable attributes only, an
///   `mqtt5Configuration` only, and all three together;
/// * how many searchable attributes it carries — none, an empty list, one, three;
/// * the form of a searchable attribute — `AttributeName` is modelled
///   `length 0..128, pattern ^[a-zA-Z0-9_.,@/:#-]+$`
///   (`smithy_query(query="shape",shape="SearchableAttributes")`), so the rows
///   include names using the punctuation that pattern allows;
/// * how many propagating attributes it carries and which of their three members are
///   set;
/// * the form and length of the description — `ThingTypeDescription` is modelled
///   `length 0..2028, pattern ^[\p{Graph}\x20]*$`
///   (`smithy_query(query="shape",shape="ThingTypeProperties")`), so the rows include
///   the empty description the `*` admits, one carrying spaces and punctuation, and
///   one at the published 2028-character maximum.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no thingTypeProperties at all",
            thing_type_name: "sensor".to_string(),
            body: json!({}),
            properties: Properties::default(),
        },
        Case {
            label: "empty thingTypeProperties",
            thing_type_name: "a".to_string(),
            body: body_of(&Properties::default()),
            properties: Properties::default(),
        },
        Case {
            label: "an empty searchableAttributes list",
            thing_type_name: "empty-searchable".to_string(),
            body: body_of(&Properties {
                searchable_attributes: Some(Vec::new()),
                ..Properties::default()
            }),
            // An empty list leaves the type with no searchable attributes, which is
            // the same type as one created without the member.
            properties: Properties::default(),
        },
        Case {
            label: "a description only",
            thing_type_name: "described_type".to_string(),
            body: body_of(&Properties {
                description: Some("Floor 3 sensors, west wing (building #7).".to_string()),
                ..Properties::default()
            }),
            properties: Properties {
                description: Some("Floor 3 sensors, west wing (building #7).".to_string()),
                ..Properties::default()
            },
        },
        Case {
            label: "the empty description the pattern admits",
            thing_type_name: "empty-description".to_string(),
            body: body_of(&Properties {
                description: Some(String::new()),
                ..Properties::default()
            }),
            properties: Properties {
                description: Some(String::new()),
                ..Properties::default()
            },
        },
        Case {
            label: "a description at the modelled 2028-character maximum",
            thing_type_name: "longest-description".to_string(),
            body: body_of(&Properties {
                description: Some("d".repeat(2028)),
                ..Properties::default()
            }),
            properties: Properties {
                description: Some("d".repeat(2028)),
                ..Properties::default()
            },
        },
        Case {
            label: "one searchable attribute",
            thing_type_name: "one-searchable".to_string(),
            body: body_of(&Properties {
                searchable_attributes: Some(vec!["serial".to_string()]),
                ..Properties::default()
            }),
            properties: Properties {
                searchable_attributes: Some(vec!["serial".to_string()]),
                ..Properties::default()
            },
        },
        Case {
            label: "three searchable attributes using the punctuation the pattern allows",
            thing_type_name: "THREE:searchable_2-x".to_string(),
            body: body_of(&Properties {
                searchable_attributes: Some(vec![
                    "serial.number".to_string(),
                    "owner@site".to_string(),
                    "line/1,2#3-4:5_6".to_string(),
                ]),
                ..Properties::default()
            }),
            properties: Properties {
                searchable_attributes: Some(vec![
                    "serial.number".to_string(),
                    "owner@site".to_string(),
                    "line/1,2#3-4:5_6".to_string(),
                ]),
                ..Properties::default()
            },
        },
        Case {
            label: "an mqtt5Configuration carrying one fully populated attribute",
            thing_type_name: "mqtt5-full".to_string(),
            body: body_of(&Properties {
                propagating_attributes: Some(vec![propagating_all()]),
                ..Properties::default()
            }),
            properties: Properties {
                propagating_attributes: Some(vec![propagating_all()]),
                ..Properties::default()
            },
        },
        Case {
            label: "every property member at once, two propagating attributes",
            thing_type_name: "g".repeat(128),
            body: body_of(&Properties {
                description: Some("Gateways: 2.4 GHz & 5 GHz".to_string()),
                searchable_attributes: Some(vec!["serial".to_string(), "firmware".to_string()]),
                propagating_attributes: Some(vec![
                    propagating_all(),
                    propagating_connection_only(),
                ]),
            }),
            properties: Properties {
                description: Some("Gateways: 2.4 GHz & 5 GHz".to_string()),
                searchable_attributes: Some(vec!["serial".to_string(), "firmware".to_string()]),
                propagating_attributes: Some(vec![
                    propagating_all(),
                    propagating_connection_only(),
                ]),
            },
        },
    ]
}

#[tokio::test]
async fn a_created_thing_type_reads_back_member_for_member() {
    let emulator = Emulator::new();
    let mut minted_ids = BTreeSet::new();

    for case in cases() {
        let label = case.label;
        let created = emulator
            .create_type(&case.thing_type_name, &case.body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateThingType must succeed, got {created:?}"
        );
        assert_eq!(
            created.text("thingTypeName").as_deref(),
            Some(case.thing_type_name.as_str()),
            "case {label:?}: CreateThingType must report the name it registered, got {created:?}"
        );
        assert_eq!(
            created.text("thingTypeArn"),
            Some(expected_arn(REGION, &case.thing_type_name)),
            "case {label:?}: CreateThingType must report the minted ARN, got {created:?}"
        );
        let type_id = created
            .text("thingTypeId")
            .unwrap_or_else(|| panic!("case {label:?}: CreateThingType reported no thingTypeId"));
        assert!(
            !type_id.is_empty(),
            "case {label:?}: expected a non-empty thingTypeId, got {type_id:?}"
        );
        assert!(
            minted_ids.insert(type_id.clone()),
            "case {label:?}: thingTypeId {type_id:?} was already minted for another thing type"
        );

        let described = emulator.describe_type(&case.thing_type_name).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: DescribeThingType must find the created type, got {described:?}"
        );
        assert_eq!(
            described.text("thingTypeName").as_deref(),
            Some(case.thing_type_name.as_str()),
            "case {label:?}: DescribeThingType must report the persisted name, got {described:?}"
        );
        assert_eq!(
            described.text("thingTypeId").as_deref(),
            Some(type_id.as_str()),
            "case {label:?}: DescribeThingType must report the persisted id, got {described:?}"
        );
        assert_eq!(
            described.text("thingTypeArn"),
            Some(expected_arn(REGION, &case.thing_type_name)),
            "case {label:?}: DescribeThingType must report the persisted ARN, got {described:?}"
        );
        assert_eq!(
            described.properties(),
            case.properties,
            "case {label:?}: DescribeThingType must report exactly the persisted properties, got {described:?}"
        );
        assert!(
            !described.deprecated(),
            "case {label:?}: a freshly created type is not deprecated, got {described:?}"
        );
        assert_eq!(
            described.deprecation_date(),
            None,
            "case {label:?}: a type nobody deprecated has no deprecationDate, got {described:?}"
        );
        assert!(
            described.creation_date().is_some(),
            "case {label:?}: the metadata must carry a creationDate, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_thing_type_arn_is_scoped_to_the_region_the_call_was_signed_for() {
    // The region an ARN carries comes from the caller's SigV4 credential scope, so
    // the same create signed for another region mints another ARN.
    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        let emulator = Emulator::new();
        for case in cases() {
            let label = case.label;
            let created = emulator
                .create_type_in(&case.thing_type_name, &case.body, region)
                .await;
            assert_eq!(
                created.status, 200,
                "case {label:?} in {region}: CreateThingType must succeed, got {created:?}"
            );
            assert_eq!(
                created.text("thingTypeArn"),
                Some(expected_arn(region, &case.thing_type_name)),
                "case {label:?} in {region}: expected a {region}-scoped ARN, got {created:?}"
            );
        }
    }
}

#[tokio::test]
async fn reading_a_thing_type_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();

    for case in cases() {
        let label = case.label;
        let created = emulator
            .create_type(&case.thing_type_name, &case.body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateThingType must succeed, got {created:?}"
        );

        let first = emulator.describe_type(&case.thing_type_name).await;
        let second = emulator.describe_type(&case.thing_type_name).await;
        assert_eq!(
            first.creation_date(),
            second.creation_date(),
            "case {label:?}: creationDate must be stable across reads, got {second:?}"
        );
        assert_eq!(
            first.properties(),
            second.properties(),
            "case {label:?}: a read must not change the reported properties, got {second:?}"
        );
        assert!(
            !second.deprecated(),
            "case {label:?}: a read must not deprecate the type, got {second:?}"
        );
    }
}

#[tokio::test]
async fn recreating_a_thing_type_with_the_same_configuration_is_idempotent() {
    let emulator = Emulator::new();

    for case in cases() {
        let label = case.label;
        let first = emulator
            .create_type(&case.thing_type_name, &case.body)
            .await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: the first CreateThingType must succeed, got {first:?}"
        );
        let repeat = emulator
            .create_type(&case.thing_type_name, &case.body)
            .await;
        assert_eq!(
            repeat.status, 200,
            "case {label:?}: repeating a create with the same configuration succeeds, got {repeat:?}"
        );
        assert_eq!(
            repeat.text("thingTypeId"),
            first.text("thingTypeId"),
            "case {label:?}: the repeat create must not mint a new thingTypeId, got {repeat:?}"
        );
        assert_eq!(
            repeat.text("thingTypeArn"),
            first.text("thingTypeArn"),
            "case {label:?}: the repeat create must report the stored ARN, got {repeat:?}"
        );

        let described = emulator.describe_type(&case.thing_type_name).await;
        assert_eq!(
            described.properties(),
            case.properties,
            "case {label:?}: the repeat create must not change the configuration, got {described:?}"
        );
    }
}

/// The ways a second create of the same name can differ from the first. Each row
/// changes ONE part of the configuration, so what is asserted is that the whole
/// configuration decides the conflict and not one member of it.
fn conflicting_pairs() -> Vec<(&'static str, Properties, Properties)> {
    vec![
        (
            "a description appears",
            Properties::default(),
            Properties {
                description: Some("Rooftop gateways".to_string()),
                ..Properties::default()
            },
        ),
        (
            "the description differs",
            Properties {
                description: Some("Rooftop gateways".to_string()),
                ..Properties::default()
            },
            Properties {
                description: Some("Basement gateways".to_string()),
                ..Properties::default()
            },
        ),
        (
            "a searchable attribute is added",
            Properties {
                searchable_attributes: Some(vec!["serial".to_string()]),
                ..Properties::default()
            },
            Properties {
                searchable_attributes: Some(vec!["serial".to_string(), "firmware".to_string()]),
                ..Properties::default()
            },
        ),
        (
            "the searchable attributes are reordered",
            Properties {
                searchable_attributes: Some(vec!["serial".to_string(), "firmware".to_string()]),
                ..Properties::default()
            },
            Properties {
                searchable_attributes: Some(vec!["firmware".to_string(), "serial".to_string()]),
                ..Properties::default()
            },
        ),
        (
            "a propagating attribute differs",
            Properties {
                propagating_attributes: Some(vec![propagating_all()]),
                ..Properties::default()
            },
            Properties {
                propagating_attributes: Some(vec![propagating_connection_only()]),
                ..Properties::default()
            },
        ),
        (
            "an mqtt5Configuration appears beside an unchanged description",
            Properties {
                description: Some("Rooftop gateways".to_string()),
                ..Properties::default()
            },
            Properties {
                description: Some("Rooftop gateways".to_string()),
                propagating_attributes: Some(vec![propagating_all()]),
                ..Properties::default()
            },
        ),
    ]
}

#[tokio::test]
async fn recreating_a_thing_type_with_a_different_configuration_is_refused() {
    let emulator = Emulator::new();

    for (index, (label, first_properties, second_properties)) in
        conflicting_pairs().into_iter().enumerate()
    {
        let thing_type_name = format!("conflict-{index}");
        let first = emulator
            .create_type(&thing_type_name, &body_of(&first_properties))
            .await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: the first CreateThingType must succeed, got {first:?}"
        );

        let second = emulator
            .create_type(&thing_type_name, &body_of(&second_properties))
            .await;
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
            Some(thing_type_name.as_str()),
            "case {label:?}: the refusal must name the thing type as its resourceId, got {second:?}"
        );
        assert_eq!(
            second.text("resourceArn"),
            Some(expected_arn(REGION, &thing_type_name)),
            "case {label:?}: the refusal must name the stored ARN, got {second:?}"
        );

        // The refused create changed nothing: the first configuration is still what
        // the registry holds.
        let described = emulator.describe_type(&thing_type_name).await;
        assert_eq!(
            described.properties(),
            first_properties,
            "case {label:?}: the refused create must not overwrite the configuration, got {described:?}"
        );
    }
}

/// A thing type whose property lists and tags are at the ceiling the registry
/// bounds one record by — the largest record a create may persist.
fn request_at_the_ceiling() -> Value {
    let searchable: Vec<String> = (0..MAX_THING_TYPE_PROPERTY_ENTRIES)
        .map(|index| format!("attribute_{index}"))
        .collect();
    let propagating: Vec<Value> = (0..MAX_THING_TYPE_PROPERTY_ENTRIES)
        .map(|index| json!({ "thingAttribute": format!("attribute_{index}") }))
        .collect();
    let tags: Vec<Value> = (0..MAX_THING_TYPE_TAGS)
        .map(|index| json!({ "Key": format!("key-{index}"), "Value": format!("value-{index}") }))
        .collect();
    json!({
        "thingTypeProperties": {
            "searchableAttributes": searchable,
            "mqtt5Configuration": { "propagatingAttributes": propagating },
        },
        "tags": tags,
    })
}

#[tokio::test]
async fn one_thing_type_record_is_bounded_at_its_published_ceiling() {
    let emulator = Emulator::new();

    // AT the ceiling the create succeeds and the record is persisted whole.
    let created = emulator
        .create_type("at-the-ceiling", &request_at_the_ceiling())
        .await;
    assert_eq!(
        created.status, 200,
        "a request at the ceiling must succeed, got {created:?}"
    );
    let described = emulator.describe_type("at-the-ceiling").await;
    assert_eq!(
        described
            .properties()
            .searchable_attributes
            .map(|entries| entries.len()),
        Some(MAX_THING_TYPE_PROPERTY_ENTRIES),
        "the read must report every searchable attribute the create carried, got {described:?}"
    );
    assert_eq!(
        described
            .properties()
            .propagating_attributes
            .map(|entries| entries.len()),
        Some(MAX_THING_TYPE_PROPERTY_ENTRIES),
        "the read must report every propagating attribute the create carried, got {described:?}"
    );

    // ONE past it, each caller-sized part of the record on its own, is refused 400
    // and persists nothing.
    let one_too_many_searchable: Vec<String> = (0..=MAX_THING_TYPE_PROPERTY_ENTRIES)
        .map(|index| format!("attribute_{index}"))
        .collect();
    let one_too_many_propagating: Vec<Value> = (0..=MAX_THING_TYPE_PROPERTY_ENTRIES)
        .map(|index| json!({ "thingAttribute": format!("attribute_{index}") }))
        .collect();
    let one_too_many_tags: Vec<Value> = (0..=MAX_THING_TYPE_TAGS)
        .map(|index| json!({ "Key": format!("key-{index}") }))
        .collect();
    let oversized = [
        (
            "one searchableAttributes entry too many",
            json!({ "thingTypeProperties": { "searchableAttributes": one_too_many_searchable } }),
        ),
        (
            "one propagatingAttributes entry too many",
            json!({
                "thingTypeProperties": {
                    "mqtt5Configuration": { "propagatingAttributes": one_too_many_propagating },
                },
            }),
        ),
        ("one tag too many", json!({ "tags": one_too_many_tags })),
    ];
    for (index, (label, body)) in oversized.into_iter().enumerate() {
        let thing_type_name = format!("oversized-{index}");
        let refused = emulator.create_type(&thing_type_name, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: a request past the ceiling must be refused 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("InvalidRequestException"),
            "case {label:?}: expected InvalidRequestException, got {refused:?}"
        );
        let described = emulator.describe_type(&thing_type_name).await;
        assert_eq!(
            described.status, 404,
            "case {label:?}: the refused create must register nothing, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_thing_type_the_registry_does_not_hold_is_refused_by_every_reader() {
    let emulator = Emulator::new();
    // One type exists, so what is asserted is a MISSING name and not an empty store.
    let created = emulator.create_type("sensor", &json!({})).await;
    assert_eq!(
        created.status, 200,
        "the fixture type must be created, got {created:?}"
    );

    for name in ["absent", "sensor-2", "SENSOR", "a", &"z".repeat(128)] {
        let described = emulator.describe_type(name).await;
        assert_eq!(
            described.status, 404,
            "case {name:?}: DescribeThingType of a name no type was created under must be refused 404, got {described:?}"
        );
        assert_eq!(
            described.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {name:?}: expected ResourceNotFoundException, got {described:?}"
        );

        let deprecated = emulator.deprecate_type(name, None).await;
        assert_eq!(
            deprecated.status, 404,
            "case {name:?}: DeprecateThingType of a name no type was created under must be refused 404, got {deprecated:?}"
        );
        assert_eq!(
            deprecated.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {name:?}: expected ResourceNotFoundException, got {deprecated:?}"
        );
    }
}

#[tokio::test]
async fn deprecating_a_thing_type_is_observable_and_reversible() {
    let emulator = Emulator::new();

    for case in cases() {
        let label = case.label;
        let created = emulator
            .create_type(&case.thing_type_name, &case.body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateThingType must succeed, got {created:?}"
        );

        // `undoDeprecate` is modelled with `default false`, so a body that omits it
        // deprecates, exactly as one that sends `false` does.
        for omitted in [None, Some(false)] {
            let deprecated = emulator
                .deprecate_type(&case.thing_type_name, omitted)
                .await;
            assert_eq!(
                deprecated.status, 200,
                "case {label:?}: DeprecateThingType({omitted:?}) must succeed, got {deprecated:?}"
            );
        }

        let described = emulator.describe_type(&case.thing_type_name).await;
        assert!(
            described.deprecated(),
            "case {label:?}: a deprecated type reports deprecated, got {described:?}"
        );
        let deprecation_date = described.deprecation_date().unwrap_or_else(|| {
            panic!("case {label:?}: a deprecated type must carry a deprecationDate, got {described:?}")
        });
        // Deprecating twice does not re-date the deprecation: the type was
        // deprecated by the first call.
        let repeat = emulator.describe_type(&case.thing_type_name).await;
        assert_eq!(
            repeat.deprecation_date(),
            Some(deprecation_date),
            "case {label:?}: the deprecationDate must be stable, got {repeat:?}"
        );
        // Deprecation changes nothing else about the type.
        assert_eq!(
            described.properties(),
            case.properties,
            "case {label:?}: deprecation must not change the properties, got {described:?}"
        );
        assert_eq!(
            described.text("thingTypeId"),
            created.text("thingTypeId"),
            "case {label:?}: deprecation must not re-mint the id, got {described:?}"
        );

        // `undoDeprecate` takes it back, and is itself idempotent.
        for _ in 0..2 {
            let undone = emulator
                .deprecate_type(&case.thing_type_name, Some(true))
                .await;
            assert_eq!(
                undone.status, 200,
                "case {label:?}: DeprecateThingType(undoDeprecate) must succeed, got {undone:?}"
            );
        }
        let after_undo = emulator.describe_type(&case.thing_type_name).await;
        assert!(
            !after_undo.deprecated(),
            "case {label:?}: an undeprecated type does not report deprecated, got {after_undo:?}"
        );
        assert_eq!(
            after_undo.deprecation_date(),
            None,
            "case {label:?}: an undeprecated type carries no deprecationDate, got {after_undo:?}"
        );
    }
}

#[tokio::test]
async fn a_thing_may_only_name_a_thing_type_that_exists_and_is_not_deprecated() {
    let emulator = Emulator::new();

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let created = emulator
            .create_type(&case.thing_type_name, &case.body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateThingType must succeed, got {created:?}"
        );

        // A type that exists and is not deprecated may be named by a new thing.
        let thing = emulator
            .create_thing(&format!("thing-{index}-live"), &case.thing_type_name)
            .await;
        assert_eq!(
            thing.status, 200,
            "case {label:?}: CreateThing naming a live thing type must succeed, got {thing:?}"
        );

        let deprecated = emulator.deprecate_type(&case.thing_type_name, None).await;
        assert_eq!(
            deprecated.status, 200,
            "case {label:?}: DeprecateThingType must succeed, got {deprecated:?}"
        );

        // Deprecated, the same type may no longer be associated with a new thing.
        let refused = emulator
            .create_thing(&format!("thing-{index}-dead"), &case.thing_type_name)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: CreateThing naming a deprecated thing type must be refused 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("InvalidRequestException"),
            "case {label:?}: expected InvalidRequestException, got {refused:?}"
        );

        // Undeprecated, it may be again.
        let undone = emulator
            .deprecate_type(&case.thing_type_name, Some(true))
            .await;
        assert_eq!(
            undone.status, 200,
            "case {label:?}: DeprecateThingType(undoDeprecate) must succeed, got {undone:?}"
        );
        let again = emulator
            .create_thing(&format!("thing-{index}-again"), &case.thing_type_name)
            .await;
        assert_eq!(
            again.status, 200,
            "case {label:?}: CreateThing naming an undeprecated thing type must succeed, got {again:?}"
        );
    }

    // A thing type nobody created is still a 404 from CreateThing, which is a
    // different refusal from the deprecated one above.
    let missing = emulator.create_thing("thing-absent-type", "absent").await;
    assert_eq!(
        missing.status, 404,
        "CreateThing naming no existing thing type must be refused 404, got {missing:?}"
    );
    assert_eq!(
        missing.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {missing:?}"
    );
}
