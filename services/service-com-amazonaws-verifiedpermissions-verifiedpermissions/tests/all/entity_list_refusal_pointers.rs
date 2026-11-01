// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for WHERE a refusal about one entity of an authorization
//! request's `entities.entityList` says the fault is.
//!
//! The rule under test: a `ValidationException$fieldList` entry about a member
//! that lives inside one element of a list the request carries addresses that
//! element by its own 0-based position (RFC 6901), so the pointer names a member
//! the request body really has and a client can resolve it against the body it
//! sent. `entityList` is a list on the wire, so a refusal about an item's
//! `attributes`, `parents` or `tags` carries that item's position between the list
//! and the member.
//!
//! Every assertion goes over the wire through the same router the emulator binary
//! serves, reusing the harness in [`crate::harness`], and every reported pointer
//! is resolved against the request body that was sent — which is what "the path to
//! the specific element" means.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, error_code, field_list};
use crate::is_authorized_with_token::{app_client, identity_source, sign_in, SignedIn};

/// `MAX_COLLECTION_LEN` in `src/state/cedar.rs`: the most elements one collection
/// inside an authorization request's payload may carry.
const MAX_COLLECTION_LEN: usize = 1_000;

/// `MAX_VALUE_DEPTH` in `src/state/cedar.rs`: how deep one attribute or tag value
/// may nest.
const MAX_VALUE_DEPTH: usize = 32;

/// The entity type every item in this suite carries. It is a RESOURCE type, not a
/// principal one, so the SAME entity list is readable by the calls that take their
/// principal from a token — those carry no caller-supplied principal entities.
const ENTITY_TYPE: &str = "App::Photo";

/// One well-formed entity item identified by `id`, carrying nothing but its
/// identifier.
fn item(id: &str) -> Value {
    json!({ "identifier": { "entityType": ENTITY_TYPE, "entityId": id } })
}

/// `count` well-formed items, the one at position `at` additionally carrying
/// `member`.
fn items(count: usize, at: usize, member: &str, value: Value) -> Vec<Value> {
    let mut list: Vec<Value> = (0..count).map(|n| item(&format!("p{n}"))).collect();
    list[at][member] = value;
    list
}

/// A map of `len` attribute (or tag) values, each a string.
fn map_of(len: usize) -> Value {
    let mut map = serde_json::Map::new();
    for n in 0..len {
        map.insert(format!("a{n}"), json!({ "string": "x" }));
    }
    Value::Object(map)
}

/// A parent list of `len` DISTINCT parent identifiers — `ParentList` is
/// `@uniqueItems` (`smithy_query(query="shape", shape="ParentList")`), so repeats
/// would be refused at the boundary instead.
fn parents_of(len: usize) -> Value {
    Value::Array(
        (0..len)
            .map(|n| json!({ "entityType": "App::Album", "entityId": format!("album{n}") }))
            .collect(),
    )
}

/// An attribute (or tag) value nesting `levels` Cedar records deep.
fn nested_value(levels: usize) -> Value {
    let mut value = json!({ "string": "x" });
    for _ in 0..levels {
        value = json!({ "record": { "k": value } });
    }
    value
}

/// The reason this service states for a collection past the length bound.
fn too_many(len: usize) -> String {
    format!("it carries {len} elements, more than the {MAX_COLLECTION_LEN} allowed")
}

/// The reason this service states for a value past the depth bound.
fn too_deep() -> String {
    format!("the value nests more than {MAX_VALUE_DEPTH} levels deep")
}

/// One row: the entity list sent, the pointer the refusal must carry and the
/// reason it must state.
struct Case {
    /// What the row is about, named in every assertion.
    name: &'static str,
    /// The `entityList` the request carries, in the order it is sent.
    items: Vec<Value>,
    /// The JSON pointer the refusal must name.
    path: String,
    /// The reason text the refusal must state.
    reason: String,
}

/// Everything the four calls under test need: a store, and a signed-in identity
/// whose tokens the store's identity source accepts.
struct Fixture {
    router: MultiServiceRouter,
    store: String,
    member: SignedIn,
}

/// Seed [`Fixture`].
async fn fixture() -> Fixture {
    let router = harness::router();
    let pool = harness::user_pool(&router, "entity-list-pointer-pool").await;
    let client = app_client(&router, &pool, "entity-list-pointer-client").await;
    let member = sign_in(&router, &pool, &client, "alice", &[], &json!([])).await;
    let store = harness::store_with_validation_off(&router).await;
    identity_source(&router, &store, &pool, &[&client], None).await;
    Fixture {
        router,
        store,
        member,
    }
}

/// The four calls that read an `entities` member, each as `(operation, body)` for
/// the given entity list.
fn bodies(fixture: &Fixture, entity_list: &[Value]) -> Vec<(&'static str, Value)> {
    let store = fixture.store.as_str();
    let entities = json!({ "entityList": entity_list });
    let action = json!({ "actionType": "App::Action", "actionId": "view" });
    let resource = json!({ "entityType": ENTITY_TYPE, "entityId": "p0" });
    let principal = json!({ "entityType": "App::User", "entityId": "alice" });
    let token = fixture.member.access_token.as_str();
    vec![
        (
            "IsAuthorized",
            json!({
                "policyStoreId": store,
                "principal": principal,
                "action": action,
                "resource": resource,
                "entities": entities,
            }),
        ),
        (
            "BatchIsAuthorized",
            json!({
                "policyStoreId": store,
                "requests": [{
                    "principal": principal,
                    "action": action,
                    "resource": resource,
                }],
                "entities": entities,
            }),
        ),
        (
            "IsAuthorizedWithToken",
            json!({
                "policyStoreId": store,
                "accessToken": token,
                "action": action,
                "resource": resource,
                "entities": entities,
            }),
        ),
        (
            "BatchIsAuthorizedWithToken",
            json!({
                "policyStoreId": store,
                "accessToken": token,
                "requests": [{ "action": action, "resource": resource }],
                "entities": entities,
            }),
        ),
    ]
}

/// RULE: a refusal about a member of ONE `entityList` item addresses that item by
/// its own 0-based position in the list — `/entities/entityList/<position>/<member>`
/// — so the pointer resolves against the request body the client sent.
///
/// The rows span which member of the item was refused (`attributes`, `parents`,
/// `tags`), which bound it broke (a collection's length, a value's nesting depth),
/// how long the list is, and which position the offending item sits at (the only
/// item, the second of two, the middle of four, the last of five, and one that
/// follows a repeated identifier — where the item's position in the REQUEST is
/// not its position in the entity store the list collapses onto). Every row is
/// sent to each of the four operations that read an `entities` member.
#[tokio::test]
async fn a_refusal_about_one_entity_names_that_entitys_position_in_the_list() {
    let fixture = fixture().await;

    let cases: Vec<Case> = vec![
        Case {
            name: "an over-long attribute map on the only item",
            items: items(
                1,
                0,
                "attributes",
                map_of(MAX_COLLECTION_LEN + 1),
            ),
            path: "/entities/entityList/0/attributes".to_string(),
            reason: too_many(MAX_COLLECTION_LEN + 1),
        },
        Case {
            name: "an over-long attribute map on the second of two items",
            items: items(2, 1, "attributes", map_of(MAX_COLLECTION_LEN + 1)),
            path: "/entities/entityList/1/attributes".to_string(),
            reason: too_many(MAX_COLLECTION_LEN + 1),
        },
        Case {
            name: "an over-long attribute map on the last of five items",
            items: items(5, 4, "attributes", map_of(MAX_COLLECTION_LEN + 2)),
            path: "/entities/entityList/4/attributes".to_string(),
            reason: too_many(MAX_COLLECTION_LEN + 2),
        },
        Case {
            name: "an attribute value nested past the depth bound, on the third of four items",
            items: items(
                4,
                2,
                "attributes",
                json!({ "deep": nested_value(MAX_VALUE_DEPTH + 2) }),
            ),
            path: "/entities/entityList/2/attributes".to_string(),
            reason: too_deep(),
        },
        Case {
            name: "an over-long parent list on the second of three items",
            items: items(3, 1, "parents", parents_of(MAX_COLLECTION_LEN + 1)),
            path: "/entities/entityList/1/parents".to_string(),
            reason: too_many(MAX_COLLECTION_LEN + 1),
        },
        Case {
            name: "an over-long tag map on the first of three items",
            items: items(3, 0, "tags", map_of(MAX_COLLECTION_LEN + 1)),
            path: "/entities/entityList/0/tags".to_string(),
            reason: too_many(MAX_COLLECTION_LEN + 1),
        },
        Case {
            name: "a tag value nested past the depth bound, on the second of two items",
            items: items(
                2,
                1,
                "tags",
                json!({ "deep": nested_value(MAX_VALUE_DEPTH + 2) }),
            ),
            path: "/entities/entityList/1/tags".to_string(),
            reason: too_deep(),
        },
        Case {
            name: "an over-long attribute map on an item following a repeated identifier",
            // The first two items name ONE identifier, so the entity store this
            // list collapses onto holds the offending item at position 1 — the
            // pointer still names where the REQUEST carries it.
            items: vec![
                item("same"),
                item("same"),
                {
                    let mut offending = item("other");
                    offending["attributes"] = map_of(MAX_COLLECTION_LEN + 1);
                    offending
                },
            ],
            path: "/entities/entityList/2/attributes".to_string(),
            reason: too_many(MAX_COLLECTION_LEN + 1),
        },
    ];

    for case in &cases {
        for (operation, body) in bodies(&fixture, &case.items) {
            let (status, answered) = harness::call(&fixture.router, operation, &body).await;
            let case_name = case.name;
            assert_eq!(
                status, 400,
                "case {case_name:?} on {operation}: expected HTTP 400, got {status} {answered}"
            );
            assert_eq!(
                error_code(&answered),
                "ValidationException",
                "case {case_name:?} on {operation}: expected the ValidationException code, \
                 got {answered}"
            );
            let fields = field_list(&answered);
            assert_eq!(
                fields.len(),
                1,
                "case {case_name:?} on {operation}: expected one fieldList entry, got {fields:?}"
            );
            let (reported_path, reported_reason) = &fields[0];
            assert_eq!(
                reported_path, &case.path,
                "case {case_name:?} on {operation}: expected the path {:?}, got {reported_path:?}",
                case.path
            );
            assert!(
                reported_reason.contains(&case.reason),
                "case {case_name:?} on {operation}: expected the reason to state {:?}, \
                 got {reported_reason:?}",
                case.reason
            );
            // The pointer is "the path to the specific element": resolved against
            // the body that was sent, it must land on a member that body has.
            assert!(
                body.pointer(reported_path).is_some(),
                "case {case_name:?} on {operation}: the reported path {reported_path:?} does \
                 not resolve against the request body that was sent"
            );
        }
    }
}

/// RULE (the same one, on the accepting side): an entity list whose items are all
/// inside the bounds is evaluated rather than refused, at every length and with
/// each of the three describable members populated — so the rows above are
/// refusals of the member they name and not of the list's shape.
#[tokio::test]
async fn an_entity_list_inside_the_bounds_is_evaluated_by_every_caller() {
    let fixture = fixture().await;

    let described = |id: &str| {
        let mut described = item(id);
        described["attributes"] = map_of(MAX_COLLECTION_LEN);
        described["parents"] = parents_of(2);
        described["tags"] = json!({ "deep": nested_value(MAX_VALUE_DEPTH - 2) });
        described
    };

    let accepted: Vec<(&str, Vec<Value>)> = vec![
        ("an empty list", vec![]),
        ("one item at the bounds", vec![described("p0")]),
        (
            "five items, the last at the bounds",
            vec![
                item("p0"),
                item("p1"),
                item("p2"),
                item("p3"),
                described("p4"),
            ],
        ),
    ];

    for (case, entity_list) in &accepted {
        for (operation, body) in bodies(&fixture, entity_list) {
            let (status, answered) = harness::call(&fixture.router, operation, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?} on {operation}: a list inside the bounds must be evaluated, \
                 got {status} {answered}"
            );
        }
    }
}
