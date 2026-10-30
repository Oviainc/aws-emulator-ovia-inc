// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the entity KINDS an authorization request that names its
//! own action may carry in `entities`.
//!
//! The rule: "You can include only principal and resource entities in this
//! parameter; you can't include actions. You must specify actions in the schema"
//! (`smithy_query(query="doc", shape="IsAuthorizedInput", member="entities")`,
//! worded identically on `BatchIsAuthorizedInput$entities`). So a slice carrying
//! an entity whose type names an ACTION is refused, whichever arm of
//! `EntitiesDefinition` carried it and whatever the entity describes, while an
//! entity of any other type is carried into the evaluation — the actions one
//! evaluation knows, and the groups they belong to, come from the policy store and
//! never from the request.
//!
//! An action's entity type is the form the model gives it: `ActionType`'s pattern
//! is `^Action$|^.+::Action$`
//! (`smithy_query(query="shape", shape="ActionIdentifier")`), so `Action` and any
//! namespace-qualified `…::Action` name actions, and a type that merely ends in
//! those letters (`MyAction`, `ActionGroup`) does not.
//!
//! Every assertion goes over the wire through the same router the emulator binary
//! serves, reusing the shared harness in [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{call, error_code, fault_message, ok, router, store_with_validation_off};

/// The permit the reported request tried to satisfy with caller-supplied data: it
/// matches only when the request's action belongs to an action group, which is
/// something the store's schema declares and not something an `entities` slice
/// may describe.
const PERMIT_ACTION_IN_GROUP: &str =
    "permit(principal == User::\"alice\", action in Action::\"readers\", \
     resource == Photo::\"p\");";

/// A permit that reads an ATTRIBUTE of the principal, so a slice that IS carried
/// into the evaluation can be told apart from one that is not: only an evaluation
/// that saw the slice can read the attribute.
const PERMIT_ADMIN_ATTRIBUTE: &str = "permit(principal, action, resource) \
     when { principal has level && principal.level == \"admin\" };";

/// The `message` every `ValidationException` this service raises carries
/// (`INVALID_INPUT_MESSAGE` in `src/ops/mod.rs`): the specifics live in
/// `fieldList`.
const INVALID_INPUT: &str = "Invalid input";

/// What a row of the table expects of the slice it sends.
enum Expect {
    /// The call is refused with a 400 `ValidationException` naming `/entities`,
    /// whose reason names this entity type — the offending one, read off the
    /// request rather than fixed text.
    Refused(&'static str),
    /// The slice is carried into the evaluation, which answers this decision.
    Decision(&'static str),
}

/// One `EntityItem` for the modelled `entityList` arm: an identifier, plus
/// whatever `described` names of `attributes`, `parents` and `tags`.
fn listed(entity_type: &str, entity_id: &str, described: Value) -> Value {
    let mut item = json!({
        "identifier": { "entityType": entity_type, "entityId": entity_id },
    });
    for (member, value) in described
        .as_object()
        .expect("the described members are an object")
    {
        item[member.clone()] = value.clone();
    }
    item
}

/// The `entities` member carrying `items` in the modelled `entityList` arm.
fn entity_list(items: Vec<Value>) -> Value {
    json!({ "entityList": items })
}

/// The `entities` member carrying `entities` in the Cedar-JSON arm: a Cedar entity
/// store as written, which is where a client sends an entity Cedar's own way.
fn cedar_json(entities: Vec<Value>) -> Value {
    json!({ "cedarJson": Value::Array(entities).to_string() })
}

/// One entity in Cedar's own entity JSON, with its uid written plainly.
fn cedar_entity(entity_type: &str, entity_id: &str, parents: Vec<Value>) -> Value {
    json!({
        "uid": { "type": entity_type, "id": entity_id },
        "attrs": {},
        "parents": parents,
    })
}

/// A Cedar entity reference, as a `parents` entry of a Cedar-JSON entity.
fn cedar_parent(entity_type: &str, entity_id: &str) -> Value {
    json!({ "type": entity_type, "id": entity_id })
}

/// The principal entity the attribute permit reads, so a row whose slice is
/// carried can prove it was.
fn admin_principal() -> Value {
    listed(
        "User",
        "alice",
        json!({ "attributes": { "level": { "string": "admin" } } }),
    )
}

/// A store holding both permits above, with validation OFF so the store's schema
/// plays no part: the rule under test is the member's own input contract, not
/// schema conformance.
async fn store(router: &smithy_server_runtime::MultiServiceRouter) -> String {
    let store_id = store_with_validation_off(router).await;
    for statement in [PERMIT_ACTION_IN_GROUP, PERMIT_ADMIN_ATTRIBUTE] {
        ok(
            router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": statement } },
            }),
        )
        .await;
    }
    store_id
}

/// Assert what a row expects of an `IsAuthorized` call carrying `entities`.
async fn assert_is_authorized(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    store_id: &str,
    entities: &Value,
    expect: &Expect,
) {
    let mut body = json!({
        "policyStoreId": store_id,
        "principal": { "entityType": "User", "entityId": "alice" },
        "action": { "actionType": "Action", "actionId": "view" },
        "resource": { "entityType": "Photo", "entityId": "p" },
    });
    // A row whose slice is `Null` sends NO `entities` member at all: an absent
    // optional member is a different request from one carrying an empty value.
    if !entities.is_null() {
        body["entities"] = entities.clone();
    }
    let (status, answered) = call(router, "IsAuthorized", &body).await;
    match expect {
        Expect::Refused(entity_type) => {
            assert_refused(case, "IsAuthorized", entity_type, status, &answered)
        }
        Expect::Decision(decision) => {
            assert_eq!(
                status, 200,
                "case {case:?}: IsAuthorized must evaluate a slice of principal and resource \
                 entities, got {status} with {answered}"
            );
            assert_eq!(
                answered["decision"].as_str(),
                Some(*decision),
                "case {case:?}: expected IsAuthorized to answer {decision}, got {answered}"
            );
        }
    }
}

/// Assert what the same row expects of `BatchIsAuthorized`, whose `entities`
/// member documents the same rule and is shared by every request in the batch.
async fn assert_batch_is_authorized(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    store_id: &str,
    entities: &Value,
    expect: &Expect,
) {
    let mut body = json!({
        "policyStoreId": store_id,
        "requests": [
            {
                "principal": { "entityType": "User", "entityId": "alice" },
                "action": { "actionType": "Action", "actionId": "view" },
                "resource": { "entityType": "Photo", "entityId": "p" },
            },
            {
                "principal": { "entityType": "User", "entityId": "alice" },
                "action": { "actionType": "Action", "actionId": "edit" },
                "resource": { "entityType": "Photo", "entityId": "p" },
            },
        ],
    });
    if !entities.is_null() {
        body["entities"] = entities.clone();
    }
    let (status, answered) = call(router, "BatchIsAuthorized", &body).await;
    match expect {
        Expect::Refused(entity_type) => {
            assert_refused(case, "BatchIsAuthorized", entity_type, status, &answered)
        }
        Expect::Decision(decision) => {
            assert_eq!(
                status, 200,
                "case {case:?}: BatchIsAuthorized must evaluate a slice of principal and \
                 resource entities, got {status} with {answered}"
            );
            let decisions: Vec<&str> = answered["results"]
                .as_array()
                .map(|results| {
                    results
                        .iter()
                        .map(|result| result["decision"].as_str().unwrap_or_default())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                decisions,
                vec![*decision, *decision],
                "case {case:?}: expected every request of the batch to answer {decision}, got \
                 {answered}"
            );
        }
    }
}

/// The one refusal shape both operations answer a forbidden entity kind with: a
/// 400 `ValidationException` whose `fieldList` names `/entities` — the member at
/// fault — and nothing else.
fn assert_refused(
    case: &str,
    operation: &str,
    entity_type: &str,
    status: u16,
    answered: &Value,
) {
    assert_eq!(
        (status, error_code(answered).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected {operation} to answer a 400 ValidationException, got {status} \
         with {answered}"
    );
    assert_eq!(
        fault_message(answered),
        INVALID_INPUT,
        "case {case:?}: unexpected {operation} refusal message, got {answered}"
    );
    let named: Vec<String> = answered["fieldList"]
        .as_array()
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field["path"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        named,
        vec!["/entities".to_string()],
        "case {case:?}: expected {operation} to name /entities in fieldList, got {answered}"
    );
    let reason = answered["fieldList"][0]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        reason.contains(entity_type),
        "case {case:?}: expected the {operation} refusal to name the offending entity type \
         {entity_type:?}, got {reason:?}"
    );
}

/// An `entities` slice carrying an action entity is refused, and one carrying only
/// principal and resource entities is evaluated.
///
/// The rows differ along what the rule generalises over: whether the slice carries
/// an action entity at all, how that entity's type is spelled (bare, namespaced,
/// nested namespace, and the near-misses that are NOT action types), what the
/// entity describes (nothing, parent entities, attributes, tags), where in the
/// slice it sits, and which arm of `EntitiesDefinition` the request sent it in.
/// Every row is sent to both operations whose `entities` member documents the rule.
#[tokio::test]
async fn an_entities_slice_carries_no_action_entity() {
    let router = router();
    let store_id = store(&router).await;

    let rows: Vec<(&str, Value, Expect)> = vec![
        // The reported request: an action entity putting the request's own action
        // into an action group the caller invented. The filed repro spells that
        // group `ActionGroup::"readers"`, which Cedar refuses in a policy's action
        // scope ("expected an entity uid with type `Action`"), so the group a
        // policy can really match on is the row above it; both spellings of the
        // parent are sent, because what is refused is the action entity itself.
        (
            "an action entity whose parent is the action group a policy matches on",
            entity_list(vec![listed(
                "Action",
                "view",
                json!({ "parents": [{ "entityType": "Action", "entityId": "readers" }] }),
            )]),
            Expect::Refused("Action"),
        ),
        (
            "an action entity whose parent is the group spelling the filed report used",
            entity_list(vec![listed(
                "Action",
                "view",
                json!({ "parents": [{ "entityType": "ActionGroup", "entityId": "readers" }] }),
            )]),
            Expect::Refused("Action"),
        ),
        (
            "an action entity carrying nothing but its identifier",
            entity_list(vec![listed("Action", "view", json!({}))]),
            Expect::Refused("Action"),
        ),
        (
            "a namespaced action entity",
            entity_list(vec![listed("PhotoApp::Action", "viewPhoto", json!({}))]),
            Expect::Refused("PhotoApp::Action"),
        ),
        (
            "an action entity whose namespace itself is qualified",
            entity_list(vec![listed("Org::PhotoApp::Action", "viewPhoto", json!({}))]),
            Expect::Refused("Org::PhotoApp::Action"),
        ),
        (
            "an action entity carrying attributes",
            entity_list(vec![listed(
                "Action",
                "view",
                json!({ "attributes": { "risk": { "long": 1 } } }),
            )]),
            Expect::Refused("Action"),
        ),
        (
            "an action entity carrying tags",
            entity_list(vec![listed(
                "Action",
                "view",
                json!({ "tags": { "class": { "string": "public" } } }),
            )]),
            Expect::Refused("Action"),
        ),
        (
            "an action entity behind the principal and resource entities",
            entity_list(vec![
                admin_principal(),
                listed("Photo", "p", json!({})),
                listed("Action", "view", json!({})),
            ]),
            Expect::Refused("Action"),
        ),
        (
            "an action entity ahead of the principal entity",
            entity_list(vec![listed("Action", "view", json!({})), admin_principal()]),
            Expect::Refused("Action"),
        ),
        (
            "an action entity in the Cedar-JSON arm",
            cedar_json(vec![cedar_entity(
                "Action",
                "view",
                vec![cedar_parent("Action", "readers")],
            )]),
            Expect::Refused("Action"),
        ),
        (
            "a namespaced action entity beside a resource entity in the Cedar-JSON arm",
            cedar_json(vec![
                cedar_entity("Photo", "p", vec![]),
                cedar_entity("PhotoApp::Action", "viewPhoto", vec![]),
            ]),
            Expect::Refused("PhotoApp::Action"),
        ),
        // The slice the reported request would have needed in order to be allowed
        // is gone, so the permit on the action group is unreachable: the principal
        // and the resource it names alone decide, and they do not satisfy it.
        (
            "the principal and the resource of the reported request, with no action entity",
            entity_list(vec![
                listed("User", "alice", json!({})),
                listed("Photo", "p", json!({})),
            ]),
            Expect::Decision("DENY"),
        ),
        (
            "a principal entity carrying the attribute a policy reads",
            entity_list(vec![admin_principal()]),
            Expect::Decision("ALLOW"),
        ),
        (
            "a resource entity beside that principal entity",
            entity_list(vec![admin_principal(), listed("Photo", "p", json!({}))]),
            Expect::Decision("ALLOW"),
        ),
        (
            "an entity of a type that merely ends in the letters of an action type",
            entity_list(vec![
                admin_principal(),
                listed("MyAction", "x", json!({})),
            ]),
            Expect::Decision("ALLOW"),
        ),
        (
            "an entity of a type that merely begins with the letters of an action type",
            entity_list(vec![
                admin_principal(),
                listed("ActionGroup", "readers", json!({})),
            ]),
            Expect::Decision("ALLOW"),
        ),
        (
            "an entity of a plural type whose name contains an action type",
            entity_list(vec![admin_principal(), listed("Actions", "all", json!({}))]),
            Expect::Decision("ALLOW"),
        ),
        (
            "a principal entity in the Cedar-JSON arm",
            cedar_json(vec![json!({
                "uid": { "type": "User", "id": "alice" },
                "attrs": { "level": "admin" },
                "parents": [],
            })]),
            Expect::Decision("ALLOW"),
        ),
        (
            "an absent slice",
            Value::Null,
            Expect::Decision("DENY"),
        ),
    ];

    for (case, entities, expect) in &rows {
        assert_is_authorized(&router, case, &store_id, entities, expect).await;
        assert_batch_is_authorized(&router, case, &store_id, entities, expect).await;
    }
}
