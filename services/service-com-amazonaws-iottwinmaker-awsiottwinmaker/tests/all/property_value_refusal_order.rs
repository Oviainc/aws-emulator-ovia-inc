// SPDX-License-Identifier: CC0-1.0

//! The ORDER `GetPropertyValue` decides a refusal in, end to end over HTTP.
//!
//! RULE under test: the SCOPE a read names — the workspace, then the entity and
//! component, or the component type and property group, inside it — is resolved
//! before the read's own page members are judged, so a read naming a scope this
//! service does not hold is refused for THAT scope, with the status and the
//! message the scope's own refusal carries, whatever page members the request
//! also carries.
//!
//! The rows of the first table span what the rule generalises over: every scope
//! level a read resolves (a workspace, an entity, a component of an existing
//! entity, a component type, a property group), in both answer shapes (the
//! latest-value map and the tabular query), crossed with every page member that
//! is itself unservable — a `maxResults` below the minimum its own member
//! documents, a `nextToken` this read never minted, and both at once.
//!
//! The second table pins the other half of the same rule: once the scope
//! resolves, those very members ARE what the read is refused for, so the first
//! table is about the order of two real refusals and not about members that are
//! quietly accepted.

use serde_json::{json, Value};

use crate::harness::{Emulator, Resp};

/// The workspace every resolvable case below reads from.
const WORKSPACE: &str = "refusalorder";
/// The component type the seeded component instantiates.
const COMPONENT_TYPE: &str = "com.example.spec";
/// The entity the seeded component lives on.
const ENTITY: &str = "unit-1";
/// The seeded component's name.
const COMPONENT: &str = "Spec";
/// A property [`COMPONENT_TYPE`] defines, and the one every read below selects.
const PROPERTY: &str = "Tag";
/// A property group [`COMPONENT_TYPE`] declares, holding [`PROPERTY`].
const GROUP: &str = "Readings";

/// A `nextToken` no read of this service ever minted — hex, the encoding this
/// service's tokens travel in, over bytes it never encodes.
const UNMINTED_TOKEN: &str = "6e6f742d612d746f6b656e";

/// The workspace, component type and entity the resolvable cases read.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let created = emulator
        .create_component_type(
            WORKSPACE,
            COMPONENT_TYPE,
            json!({
                "propertyDefinitions": {
                    PROPERTY: {
                        "dataType": {"type": "STRING"},
                        "defaultValue": {"stringValue": "unset-tag"},
                    },
                },
                "propertyGroups": {
                    GROUP: {"groupType": "TABULAR", "propertyNames": [PROPERTY]},
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateComponentType: expected 200, got {created:?}"
    );

    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": ENTITY,
                "entityName": ENTITY,
                "components": {COMPONENT: {"componentTypeId": COMPONENT_TYPE}},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateEntity: expected 200, got {created:?}"
    );
}

/// One scope a read addresses that this service cannot resolve: the request
/// members that address it, and the refusal the scope itself carries.
#[derive(Debug)]
struct Scope {
    /// What this row is about, quoted by every assertion it makes.
    case: &'static str,
    workspace_id: &'static str,
    /// The addressing members, carrying no page member at all: those are what
    /// the rows below add.
    address: Value,
    status: u16,
    error: &'static str,
    /// What the refusal has to NAME: the thing that could not be resolved.
    names: &'static str,
}

/// One page member that is itself unservable: what it is called and what it adds
/// to a request.
#[derive(Debug)]
struct PageMembers {
    case: &'static str,
    /// The members to merge into a request's body.
    members: Value,
    /// The member names any refusal of these members may name.
    named: &'static [&'static str],
}

/// The page members below the first table crosses every scope with.
///
/// `maxResults` publishes "Valid Range: Minimum value of 1."
/// (`smithy_query(query="doc",shape="GetPropertyValueRequest",member="maxResults")`),
/// so a zero is a member this read cannot serve; a `nextToken` is opaque and
/// bound to the read that minted it, so one no read minted is another.
fn page_members() -> Vec<PageMembers> {
    vec![
        PageMembers {
            case: "noPageMember",
            members: json!({}),
            named: &[],
        },
        PageMembers {
            case: "pageSizeBelowTheDocumentedMinimum",
            members: json!({"maxResults": 0}),
            named: &["maxResults"],
        },
        PageMembers {
            case: "unmintedNextToken",
            members: json!({"nextToken": UNMINTED_TOKEN}),
            named: &["nextToken"],
        },
        PageMembers {
            case: "bothUnservableAtOnce",
            members: json!({"maxResults": 0, "nextToken": UNMINTED_TOKEN}),
            named: &["maxResults", "nextToken"],
        },
    ]
}

/// `address` with `members` merged into it: the request one row sends.
fn request(address: &Value, members: &Value) -> Value {
    let mut body = address.clone();
    for (name, value) in members
        .as_object()
        .expect("a row's page members are an object")
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

/// RULE: the scope a read names is resolved before its own page members, so a
/// read of a scope this service does not hold is refused for the scope — the
/// status a caller branches on and a message naming what could not be resolved —
/// whatever page members the request carries.
#[tokio::test]
async fn an_unresolvable_scope_is_refused_whatever_page_members_the_read_carries() {
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
            case: "a workspace this service does not hold, asked for as a table",
            workspace_id: "no-such-workspace",
            address: json!({
                "componentTypeId": COMPONENT_TYPE,
                "selectedProperties": [PROPERTY],
                "tabularConditions": {},
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
                "tabularConditions": {},
            }),
            status: 404,
            error: "ResourceNotFoundException",
            names: "com.example.absent",
        },
        Scope {
            case: "a property group the addressed component does not have",
            workspace_id: WORKSPACE,
            address: json!({
                "entityId": ENTITY,
                "componentName": COMPONENT,
                "propertyGroupName": "NoSuchGroup",
                "selectedProperties": [PROPERTY],
            }),
            status: 404,
            error: "ResourceNotFoundException",
            names: "NoSuchGroup",
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
                "entityId": ENTITY,
                "componentPath": "Spec/missing",
                "selectedProperties": [PROPERTY],
            }),
            status: 400,
            error: "ValidationException",
            names: "Spec/missing",
        },
    ];

    for scope in &scopes {
        for page in page_members() {
            let case = format!("{} + {}", scope.case, page.case);
            let refused = emulator
                .get_property_value(scope.workspace_id, request(&scope.address, &page.members))
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
            for member in ["maxResults", "nextToken"] {
                assert!(
                    !reported.contains(member),
                    "case {case:?}: the refusal is the scope's, so it must not be about \
                     {member:?}, got {reported:?}"
                );
            }
        }
    }
}

/// RULE: once the scope resolves, the read's own page members are what decide
/// the answer — an unservable one is refused with HTTP 400 `ValidationException`
/// naming the member, and a servable request is served.
///
/// The rows cross both answer shapes — the latest-value map and the tabular
/// query — with the same unservable members the table above carries, so those
/// members really are refusals and not values this read accepts.
#[tokio::test]
async fn a_read_whose_scope_resolves_is_refused_for_its_own_unservable_page_member() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let shapes: Vec<(&str, Value)> = vec![
        (
            "the latest values of one component",
            json!({
                "entityId": ENTITY,
                "componentName": COMPONENT,
                "selectedProperties": [PROPERTY],
            }),
        ),
        (
            "a table over the instances of a component type",
            json!({
                "componentTypeId": COMPONENT_TYPE,
                "selectedProperties": [PROPERTY],
                "tabularConditions": {},
            }),
        ),
        (
            "the latest values inside a property group",
            json!({
                "entityId": ENTITY,
                "componentName": COMPONENT,
                "propertyGroupName": GROUP,
                "selectedProperties": [PROPERTY],
            }),
        ),
    ];

    for (shape, address) in &shapes {
        // The request carrying no unservable member at all: the scope resolves,
        // so it is SERVED — the reorder above refuses nothing a read can answer.
        let case = format!("{shape} with servable members");
        let served = emulator
            .get_property_value(WORKSPACE, address.clone())
            .await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: expected the read to be served, got {served:?}"
        );

        for page in page_members() {
            if page.named.is_empty() {
                // The row that adds nothing is the served request above.
                continue;
            }
            let case = format!("{shape} + {}", page.case);
            let refused = emulator
                .get_property_value(WORKSPACE, request(address, &page.members))
                .await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: an unservable page member must be refused, got {refused:?}"
            );
            let reported = message(&refused);
            assert!(
                page.named.iter().any(|member| reported.contains(member)),
                "case {case:?}: the refusal must name one of {:?}, got {reported:?}",
                page.named
            );
        }
    }
}
