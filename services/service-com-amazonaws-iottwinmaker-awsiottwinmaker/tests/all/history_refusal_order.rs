// SPDX-License-Identifier: CC0-1.0

//! The ORDER `GetPropertyValueHistory` decides a refusal in, end to end over
//! HTTP.
//!
//! RULE under test: the SCOPE a history read names — the workspace, then the
//! entity and the component, or the component type, inside it — is resolved
//! before the read's own page and shaping members are judged, so a read naming
//! a scope this service does not hold is refused for THAT scope, with the
//! status and the message the scope's own refusal carries, whatever page or
//! shaping members the request also carries.
//!
//! The rows of the first table span what the rule generalises over: every scope
//! level a history read resolves (a workspace, an entity, a component of an
//! existing entity, a composite component addressed by a path, a component
//! type), crossed with every page or shaping member that is itself unservable —
//! a `maxResults` below the minimum its own member documents, a `startTime`
//! that is not an ISO 8601 DateTime, a window whose start follows its end, an
//! `interpolation` missing the interval its points are spaced by, a `nextToken`
//! this read never minted, and all of them at once.
//!
//! The second table pins the other half of the same rule: once the scope
//! resolves, those very members ARE what the read is refused for, and a request
//! carrying none of them is served — so the first table is about the order of
//! two real refusals and not about members that are quietly accepted.

use serde_json::{json, Value};

use crate::harness::{data_writer_functions, Emulator, Resp};

/// The workspace every resolvable case below reads from.
const WORKSPACE: &str = "historyorder";
/// The component type whose `Temp` property is a time series property.
const SENSOR: &str = "com.example.sensor";
/// A component type declaring a COMPOSITE component type of [`SENSOR`], so an
/// entity component of this type carries a composite component addressed by the
/// path `Rig/inner`.
const RIG: &str = "com.example.rig";
/// The entity carrying a top-level component of [`SENSOR`].
const ENTITY: &str = "unit-1";
/// The name of that top-level component.
const COMPONENT: &str = "Sensor";
/// The entity carrying a component of [`RIG`], and so a composite component.
const RIG_ENTITY: &str = "rig-1";
/// The path the composite component of [`RIG_ENTITY`] sits at.
const COMPOSITE_PATH: &str = "Rig/inner";
/// The time series property every read below selects.
const PROPERTY: &str = "Temp";

/// Instants one minute apart, as the ISO 8601 `Time` a write carries.
const T0: &str = "2026-02-01T00:00:00Z";
/// See [`T0`].
const T1: &str = "2026-02-01T00:01:00Z";

/// A `nextToken` no read of this service ever minted — hex, the encoding this
/// service's tokens travel in, over bytes it never encodes.
const UNMINTED_TOKEN: &str = "6e6f742d612d746f6b656e";

/// A `startTime` of the modelled length (`Time` is `@length 20..35`, so the
/// boundary validator admits it) that spells no instant at all.
const UNREADABLE_TIME: &str = "this-is-not-a-time!!";

/// The workspace, component types, entities and written values the resolvable
/// cases read.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            SENSOR,
            json!({
                "propertyDefinitions": {
                    PROPERTY: {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                },
                // The write connector a value is set THROUGH: without it no
                // write to this component type's properties is accepted.
                "functions": data_writer_functions(),
            }),
        ),
        (
            RIG,
            json!({"compositeComponentTypes": {"inner": {"componentTypeId": SENSOR}}}),
        ),
    ];
    for (component_type_id, body) in component_types {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "CreateComponentType {component_type_id:?}: expected 200, got {created:?}"
        );
    }

    let entities: Vec<Value> = vec![
        json!({
            "entityId": ENTITY,
            "entityName": ENTITY,
            "components": {COMPONENT: {"componentTypeId": SENSOR}},
        }),
        json!({
            "entityId": RIG_ENTITY,
            "entityName": RIG_ENTITY,
            "components": {"Rig": {"componentTypeId": RIG}},
            "compositeComponents": {COMPOSITE_PATH: {}},
        }),
    ];
    for body in entities {
        let created = emulator.create_entity(WORKSPACE, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {body:?}: expected 200, got {created:?}"
        );
    }

    // Two values, so a served read below reports a real history rather than an
    // empty one.
    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([
                {
                    "entityPropertyReference": {
                        "entityId": ENTITY,
                        "componentName": COMPONENT,
                        "propertyName": PROPERTY,
                    },
                    "propertyValues": [
                        {"time": T0, "value": {"doubleValue": 10.0}},
                        {"time": T1, "value": {"doubleValue": 11.0}},
                    ],
                },
            ]),
        )
        .await;
    assert_eq!(
        written.status, 200,
        "BatchPutPropertyValues: expected 200, got {written:?}"
    );
    assert_eq!(
        written.member("errorEntries"),
        &json!([]),
        "BatchPutPropertyValues: expected no error entries, got {written:?}"
    );
}

/// One scope a history read addresses that this service cannot resolve: the
/// members that address it, and the refusal the scope itself carries.
#[derive(Debug)]
struct Scope {
    /// What this row is about, quoted by every assertion it makes.
    case: &'static str,
    workspace_id: &'static str,
    /// The addressing members, carrying no page or shaping member at all: those
    /// are what the rows of [`shaping_members`] add.
    address: Value,
    status: u16,
    error: &'static str,
    /// What the refusal has to NAME: the thing that could not be resolved.
    names: &'static str,
}

/// One page or shaping member that is itself unservable: what it is called and
/// what it adds to a request.
#[derive(Debug)]
struct ShapingMembers {
    case: &'static str,
    /// The members to merge into a request's body.
    members: Value,
    /// Text any refusal of these members may carry — the member it is about.
    named: &'static [&'static str],
}

/// The member names a refusal of the SCOPE may never be about.
const SHAPING_MEMBER_NAMES: [&str; 6] = [
    "maxResults",
    "orderByTime",
    "startTime",
    "endTime",
    "interpolation",
    "nextToken",
];

/// The page and shaping members the first table crosses every scope with.
///
/// Each is a value this read cannot serve: `maxResults` publishes "Valid Range:
/// Minimum value of 1."
/// (`smithy_query(query="doc",shape="GetPropertyValueHistoryRequest",member="maxResults")`),
/// `startTime` is "the ISO8601 DateTime of the earliest property value to
/// return", a window whose start follows its end holds no instant,
/// `interpolation` needs the interval its points are spaced by, and a
/// `nextToken` is opaque and bound to the read that minted it.
///
/// `orderByTime` is NOT among them: it targets the modelled `OrderByTime` enum,
/// so a value outside that enum is refused at the request boundary before any
/// handler of this service runs, which is no handler's ordering to decide.
fn shaping_members() -> Vec<ShapingMembers> {
    vec![
        ShapingMembers {
            case: "noShapingMember",
            members: json!({}),
            named: &[],
        },
        ShapingMembers {
            case: "pageSizeBelowTheDocumentedMinimum",
            members: json!({"maxResults": 0}),
            named: &["maxResults"],
        },
        ShapingMembers {
            case: "startTimeThatIsNoInstant",
            members: json!({"startTime": UNREADABLE_TIME}),
            named: &["startTime"],
        },
        ShapingMembers {
            case: "windowWhoseStartFollowsItsEnd",
            members: json!({"startTime": T1, "endTime": T0}),
            named: &["window"],
        },
        ShapingMembers {
            case: "interpolationWithoutAnInterval",
            members: json!({"interpolation": {"interpolationType": "LINEAR"}}),
            named: &["interpolation"],
        },
        ShapingMembers {
            case: "unmintedNextToken",
            members: json!({"nextToken": UNMINTED_TOKEN}),
            named: &["nextToken"],
        },
        ShapingMembers {
            case: "everyUnservableMemberAtOnce",
            members: json!({
                "maxResults": 0,
                "startTime": UNREADABLE_TIME,
                "interpolation": {"interpolationType": "LINEAR"},
                "nextToken": UNMINTED_TOKEN,
            }),
            named: &["maxResults", "startTime", "interpolation", "nextToken"],
        },
    ]
}

/// `address` with `members` merged into it: the request one row sends.
fn request(address: &Value, members: &Value) -> Value {
    let mut body = address.clone();
    for (name, value) in members
        .as_object()
        .expect("a row's shaping members are an object")
    {
        body[name.as_str()] = value.clone();
    }
    body
}

/// The `message` a refusal carries, or the empty string when it carries none.
fn message(response: &Resp) -> String {
    response
        .member("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: the scope a history read names is resolved before its own page and
/// shaping members, so a read of a scope this service does not hold is refused
/// for the scope — the status a caller branches on and a message naming what
/// could not be resolved — whatever those members carry.
#[tokio::test]
async fn an_unresolvable_scope_is_refused_whatever_shaping_members_the_read_carries() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let scopes = vec![
        Scope {
            case: "a workspace this service does not hold",
            workspace_id: "no-such-workspace",
            address: json!({
                "entityId": ENTITY,
                "componentName": COMPONENT,
                "selectedProperties": [PROPERTY],
            }),
            status: 404,
            error: "ResourceNotFoundException",
            names: "no-such-workspace",
        },
        Scope {
            case: "a workspace this service does not hold, asked across a component type",
            workspace_id: "no-such-workspace",
            address: json!({
                "componentTypeId": SENSOR,
                "selectedProperties": [PROPERTY],
            }),
            status: 404,
            error: "ResourceNotFoundException",
            names: "no-such-workspace",
        },
        Scope {
            case: "an entity the workspace does not hold",
            workspace_id: WORKSPACE,
            address: json!({
                "entityId": "no-such-entity",
                "componentName": COMPONENT,
                "selectedProperties": [PROPERTY],
            }),
            status: 404,
            error: "ResourceNotFoundException",
            names: "no-such-entity",
        },
        Scope {
            case: "a component type the workspace does not hold",
            workspace_id: WORKSPACE,
            address: json!({
                "componentTypeId": "com.example.absent",
                "selectedProperties": [PROPERTY],
            }),
            status: 404,
            error: "ResourceNotFoundException",
            names: "com.example.absent",
        },
        Scope {
            case: "a component an existing entity does not have",
            workspace_id: WORKSPACE,
            address: json!({
                "entityId": ENTITY,
                "componentName": "NoSuchComponent",
                "selectedProperties": [PROPERTY],
            }),
            status: 400,
            error: "ValidationException",
            names: "NoSuchComponent",
        },
        Scope {
            case: "a composite component no path of the entity addresses",
            workspace_id: WORKSPACE,
            address: json!({
                "entityId": RIG_ENTITY,
                "componentPath": "Rig/missing",
                "selectedProperties": [PROPERTY],
            }),
            status: 400,
            error: "ValidationException",
            names: "Rig/missing",
        },
    ];

    for scope in &scopes {
        for shaping in shaping_members() {
            let case = format!("{} + {}", scope.case, shaping.case);
            let refused = emulator
                .get_property_value_history(
                    scope.workspace_id,
                    request(&scope.address, &shaping.members),
                )
                .await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (scope.status, Some(scope.error)),
                "case {case:?}: the refusal is the scope's, got {refused:?}"
            );
            let reported = message(&refused);
            assert!(
                reported.contains(scope.names),
                "case {case:?}: the refusal must name {:?}, got {reported:?}",
                scope.names
            );
            for member in SHAPING_MEMBER_NAMES {
                assert!(
                    !reported.contains(member),
                    "case {case:?}: the refusal is the scope's, so it must not be about \
                     {member:?}, got {reported:?}"
                );
            }
        }
    }
}

/// RULE: once the scope resolves, the read's own page and shaping members are
/// what decide the answer — an unservable one is refused with HTTP 400
/// `ValidationException` naming what about it cannot be served, and a request
/// carrying none of them is served.
///
/// The rows cross every way a history read addresses a scope — one top-level
/// component, one composite component by its path, and every instance of a
/// component type — with the same unservable members the table above carries,
/// so those members really are refusals and not values this read accepts.
#[tokio::test]
async fn a_read_whose_scope_resolves_is_refused_for_its_own_unservable_member() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let shapes: Vec<(&str, Value)> = vec![
        (
            "the history of one top-level component's property",
            json!({
                "entityId": ENTITY,
                "componentName": COMPONENT,
                "selectedProperties": [PROPERTY],
            }),
        ),
        (
            "the history of a composite component's property",
            json!({
                "entityId": RIG_ENTITY,
                "componentPath": COMPOSITE_PATH,
                "selectedProperties": [PROPERTY],
            }),
        ),
        (
            "the history across every instance of a component type",
            json!({
                "componentTypeId": SENSOR,
                "selectedProperties": [PROPERTY],
            }),
        ),
    ];

    for (shape, address) in &shapes {
        // The request carrying no unservable member at all: the scope resolves,
        // so it is SERVED — resolving the scope first refuses nothing a read can
        // answer.
        let case = format!("{shape} with servable members");
        let served = emulator
            .get_property_value_history(WORKSPACE, address.clone())
            .await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: expected the read to be served, got {served:?}"
        );

        for shaping in shaping_members() {
            if shaping.named.is_empty() {
                // The row that adds nothing is the served request above.
                continue;
            }
            let case = format!("{shape} + {}", shaping.case);
            let refused = emulator
                .get_property_value_history(WORKSPACE, request(address, &shaping.members))
                .await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: an unservable member must be refused, got {refused:?}"
            );
            let reported = message(&refused);
            assert!(
                shaping.named.iter().any(|named| reported.contains(named)),
                "case {case:?}: the refusal must be about one of {:?}, got {reported:?}",
                shaping.named
            );
        }
    }
}
