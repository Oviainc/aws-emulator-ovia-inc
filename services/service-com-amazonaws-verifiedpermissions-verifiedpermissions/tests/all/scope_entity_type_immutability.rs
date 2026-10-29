// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for ONE rule, over both operations that state it:
//! `UpdatePolicy` ("You can't change these elements of a static policy: ... The
//! principal referenced by a static policy. The resource referenced by a static
//! policy.") and `UpdatePolicyTemplate` ("You can't change the following
//! elements: ... The principal referenced by the policy template. The resource
//! referenced by the policy template.").
//!
//! What a scope position REFERENCES is the whole constraint Cedar reports for it
//! — the entity type an `is T` constraint restricts the position to as much as
//! the entity an `==` / `in` constraint names — so a replacement is accepted
//! only when both positions reference exactly what the stored text referenced,
//! and refused otherwise, whichever form the position takes.
//!
//! One table drives both operations: every row is written as TEMPLATE text, and
//! the static-policy driver binds its `?principal` / `?resource` placeholders to
//! concrete entities, so the same change is exercised on a static policy and on
//! a policy template.
//!
//! Every assertion goes over the wire through the shared [`crate::harness`], so
//! a refusal only counts if nothing reached state behind it.

use serde_json::{json, Value};

use crate::harness::{call, error_code, ok, router, store_with_validation_off};

/// The entity a static row binds `?principal` to.
const BOUND_PRINCIPAL: &str = "PhotoApp::User::\"alice\"";

/// The entity a static row binds `?resource` to.
const BOUND_RESOURCE: &str = "PhotoApp::Photo::\"vacation.jpg\"";

/// One row of the table: the text the resource is created with, the replacement,
/// and which scope elements that replacement CHANGES.
struct Row {
    case: &'static str,
    stored: &'static str,
    replacement: &'static str,
    /// The immutable elements the replacement changes, each named by the word
    /// the refusal's `fieldList` message uses for it. EMPTY for a replacement
    /// that changes only what the operation allows to change, which is accepted.
    changed: &'static [&'static str],
}

/// The table. Rows differ along the scope POSITION the change is in (principal,
/// resource, both), the FORM the position takes (`is T`, `is T in E`, `== E`,
/// `in E`, unconstrained), WHAT the replacement does to it (a different type, a
/// type whose name extends the stored one, the same leaf name in another
/// namespace, a different `in` entity, a dropped or added constraint), and
/// whether the change is one the operation allows (the action, a condition
/// clause, reformatting).
const ROWS: &[Row] = &[
    Row {
        case: "the principal's entity type changes under `is`",
        stored: "permit(principal is PhotoApp::User, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "permit(principal is PhotoApp::Admin, \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        // The replacement's type NAME extends the stored one, so a comparison
        // that only asked whether one contains the other would accept it.
        case: "the principal's entity type changes to one whose name extends it",
        stored: "permit(principal is PhotoApp::User, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "permit(principal is PhotoApp::UserGroup, \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        // Only the NAMESPACE differs: the leaf name is the stored one.
        case: "the principal's entity type keeps its leaf name in another namespace",
        stored: "permit(principal is PhotoApp::User, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "permit(principal is StoreApp::User, \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        case: "the resource's entity type changes under `is`",
        stored: "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
                 resource is PhotoApp::Photo);",
        replacement: "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
                      resource is PhotoApp::Album);",
        changed: &["resource"],
    },
    Row {
        case: "the principal's entity type changes under `is ... in`",
        stored: "permit(principal is PhotoApp::User in PhotoApp::Group::\"eng\", \
                 action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        replacement: "permit(principal is PhotoApp::Admin in PhotoApp::Group::\"eng\", \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        case: "the resource's entity type changes under `is ... in`",
        stored: "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
                 resource is PhotoApp::Photo in PhotoApp::Album::\"trip\");",
        replacement: "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
                      resource is PhotoApp::Document in PhotoApp::Album::\"trip\");",
        changed: &["resource"],
    },
    Row {
        case: "the `in` entity of an `is ... in` principal changes, the type kept",
        stored: "permit(principal is PhotoApp::User in PhotoApp::Group::\"eng\", \
                 action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        replacement: "permit(principal is PhotoApp::User in PhotoApp::Group::\"ops\", \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        case: "an `is` principal becomes unconstrained",
        stored: "permit(principal is PhotoApp::User, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "permit(principal, action == PhotoApp::Action::\"viewPhoto\", \
                      resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        case: "an `is` principal becomes an entity of that same type",
        stored: "permit(principal is PhotoApp::User, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "permit(principal == PhotoApp::User::\"alice\", \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        case: "an `is` principal gains an `in` entity, the type kept",
        stored: "permit(principal is PhotoApp::User, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "permit(principal is PhotoApp::User in PhotoApp::Group::\"eng\", \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        // Two immutable elements at once: one `fieldList` entry per element.
        case: "both positions' entity types change at once",
        stored: "permit(principal is PhotoApp::User in ?principal, \
                 action == PhotoApp::Action::\"viewPhoto\", \
                 resource is PhotoApp::Photo in ?resource);",
        replacement: "permit(principal is PhotoApp::Admin in ?principal, \
                      action == PhotoApp::Action::\"viewPhoto\", \
                      resource is PhotoApp::Album in ?resource);",
        changed: &["principal", "resource"],
    },
    Row {
        case: "the effect changes while an `is` principal is kept",
        stored: "permit(principal is PhotoApp::User, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "forbid(principal is PhotoApp::User, \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["effect"],
    },
    Row {
        case: "the action changes while an `is` principal is kept",
        stored: "permit(principal is PhotoApp::User, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "permit(principal is PhotoApp::User, \
                      action == PhotoApp::Action::\"sharePhoto\", resource == ?resource);",
        changed: &[],
    },
    Row {
        case: "a condition clause is added while an `is ... in` principal is kept",
        stored: "permit(principal is PhotoApp::User in PhotoApp::Group::\"eng\", \
                 action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        replacement: "permit(principal is PhotoApp::User in PhotoApp::Group::\"eng\", \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource) \
                      when { context.mfa == true };",
        changed: &[],
    },
    Row {
        case: "an `is` resource is kept while the actions widen",
        stored: "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
                 resource is PhotoApp::Photo);",
        replacement: "permit(principal == ?principal, \
                      action in [PhotoApp::Action::\"viewPhoto\", \
                      PhotoApp::Action::\"sharePhoto\"], resource is PhotoApp::Photo);",
        changed: &[],
    },
    Row {
        case: "only the whitespace of an `is` scope changes",
        stored: "permit(principal is PhotoApp::User, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "permit(\n  principal is PhotoApp::User,\n  \
                      action == PhotoApp::Action::\"viewPhoto\",\n  resource == ?resource\n);",
        changed: &[],
    },
];

/// A row's text as a STATIC policy: the placeholders a template uses bound to
/// concrete entities, so the row's change is exercised on text a static policy
/// can hold.
fn as_static(statement: &str) -> String {
    statement
        .replace("?principal", BOUND_PRINCIPAL)
        .replace("?resource", BOUND_RESOURCE)
}

/// `{entityType, entityId}` of a scope member, or `None` when the response omits
/// it.
fn entity(value: &Value) -> Option<(String, String)> {
    let object = value.as_object()?;
    Some((
        object.get("entityType")?.as_str()?.to_string(),
        object.get("entityId")?.as_str()?.to_string(),
    ))
}

/// Assert that `body` is the modelled `ValidationException` whose `fieldList`
/// names `path` once per element in `changed`, with one entry naming each.
fn assert_refusal(case: &str, body: &Value, path: &str, changed: &[&str]) {
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: unexpected error code, got {body}"
    );
    let fields = body["fieldList"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        fields.len(),
        changed.len(),
        "case {case:?}: expected one fieldList entry per changed element {changed:?}, \
         got {body}"
    );
    assert!(
        fields.iter().all(|field| field["path"].as_str() == Some(path)),
        "case {case:?}: every entry must name {path} in fieldList, got {body}"
    );
    for element in changed {
        assert_eq!(
            fields
                .iter()
                .filter(|field| field["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(element))
                .count(),
            1,
            "case {case:?}: expected exactly one entry about the {element}, got {body}"
        );
    }
}

/// `UpdatePolicy` refuses a replacement that changes what the policy's scope
/// references — the entity type an `is T` position restricts to as much as the
/// entity a position names — and applies one that changes only what the
/// operation allows.
#[tokio::test]
async fn an_update_policy_replacement_is_held_to_the_whole_scope_reference() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stored = as_static(row.stored);
        let replacement = as_static(row.replacement);

        let created = ok(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "name": format!("scope-{index}"),
                "definition": { "static": { "statement": stored } },
            }),
        )
        .await;
        let policy_id = created["policyId"]
            .as_str()
            .expect("policyId is required on the CreatePolicy response")
            .to_string();

        let (status, body) = call(
            &router,
            "UpdatePolicy",
            &json!({
                "policyStoreId": store_id,
                "policyId": policy_id,
                "definition": { "static": { "statement": replacement } },
            }),
        )
        .await;

        let fetched = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        let statement = fetched["definition"]["static"]["statement"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if row.changed.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: a change the operation allows must be applied, got {status} {body}"
            );
            assert_eq!(
                statement, replacement,
                "case {case:?}: GetPolicy must report the replacement, got {fetched}"
            );
            assert_eq!(
                entity(&fetched["principal"]),
                entity(&created["principal"]),
                "case {case:?}: the principal the policy reports cannot change, got {fetched}"
            );
            assert_eq!(
                entity(&fetched["resource"]),
                entity(&created["resource"]),
                "case {case:?}: the resource the policy reports cannot change, got {fetched}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a changed {changed:?}, got {status} {body}",
            changed = row.changed
        );
        assert_refusal(case, &body, "/definition/static/statement", row.changed);
        assert_eq!(
            statement, stored,
            "case {case:?}: a refused update must leave the statement alone, got {fetched}"
        );
        assert_eq!(
            fetched["effect"], created["effect"],
            "case {case:?}: a refused update must leave the effect alone, got {fetched}"
        );
        assert_eq!(
            fetched["lastUpdatedDate"], created["lastUpdatedDate"],
            "case {case:?}: a refused update must not move lastUpdatedDate, got {fetched}"
        );
    }
}

/// `UpdatePolicyTemplate` is held to the same rule, and a refused replacement
/// leaves every policy linked to the template following the text it was created
/// with.
#[tokio::test]
async fn an_update_policy_template_replacement_is_held_to_the_whole_scope_reference() {
    let router = router();

    for row in ROWS {
        let case = row.case;
        // Each row gets its own store, so a template id and a link are this
        // row's alone.
        let store_id = store_with_validation_off(&router).await;
        let created = ok(
            &router,
            "CreatePolicyTemplate",
            &json!({ "policyStoreId": store_id, "statement": row.stored }),
        )
        .await;
        let template_id = created["policyTemplateId"]
            .as_str()
            .expect("policyTemplateId is required on the CreatePolicyTemplate response")
            .to_string();

        // A policy instantiated from the template, binding exactly the
        // placeholders the stored text uses.
        let mut link = json!({ "policyTemplateId": template_id });
        if row.stored.contains("?principal") {
            link["principal"] = json!({ "entityType": "PhotoApp::User", "entityId": "alice" });
        }
        if row.stored.contains("?resource") {
            link["resource"] = json!({ "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" });
        }
        let linked = ok(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "templateLinked": link },
            }),
        )
        .await;
        let policy_id = linked["policyId"]
            .as_str()
            .expect("policyId is required on the CreatePolicy response")
            .to_string();

        let (status, body) = call(
            &router,
            "UpdatePolicyTemplate",
            &json!({
                "policyStoreId": store_id,
                "policyTemplateId": template_id,
                "statement": row.replacement,
            }),
        )
        .await;

        let fetched = ok(
            &router,
            "GetPolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
        )
        .await;
        let after = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;

        if row.changed.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: a change the operation allows must be applied, got {status} {body}"
            );
            assert_eq!(
                fetched["statement"].as_str(),
                Some(row.replacement),
                "case {case:?}: GetPolicyTemplate must report the replacement, got {fetched}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a changed {changed:?}, got {status} {body}",
                changed = row.changed
            );
            assert_refusal(case, &body, "/template/statement", row.changed);
            assert_eq!(
                fetched["statement"].as_str(),
                Some(row.stored),
                "case {case:?}: a refused update must leave the statement alone, got {fetched}"
            );
            assert_eq!(
                fetched["lastUpdatedDate"], created["lastUpdatedDate"],
                "case {case:?}: a refused update must not move lastUpdatedDate, got {fetched}"
            );
        }

        // Whichever way the update went, the principal and the resource a linked
        // policy reports are the template's own, which a replacement cannot
        // change.
        assert_eq!(
            entity(&after["principal"]),
            entity(&linked["principal"]),
            "case {case:?}: the principal a linked policy reports cannot change, got {after}"
        );
        assert_eq!(
            entity(&after["resource"]),
            entity(&linked["resource"]),
            "case {case:?}: the resource a linked policy reports cannot change, got {after}"
        );
    }
}

/// The decision a template-linked policy takes part in does not follow an entity
/// type the template never accepted: the replacement that would have re-typed the
/// template's principal is refused, so every authorization answer is the one the
/// stored template gives.
#[tokio::test]
async fn a_refused_template_replacement_leaves_every_linked_decision_alone() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let created = ok(
        &router,
        "CreatePolicyTemplate",
        &json!({
            "policyStoreId": store_id,
            "statement": "permit(principal is PhotoApp::User, \
                          action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        }),
    )
    .await;
    let template_id = created["policyTemplateId"]
        .as_str()
        .expect("policyTemplateId is required on the CreatePolicyTemplate response")
        .to_string();
    ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "templateLinked": {
                "policyTemplateId": template_id,
                "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
            } },
        }),
    )
    .await;

    // (case, the principal's entity type, the decision the stored template gives)
    let decisions: &[(&str, &str, &str)] = &[
        ("the type the template references", "PhotoApp::User", "ALLOW"),
        ("the type the replacement would have introduced", "PhotoApp::Admin", "DENY"),
        ("a type neither text references", "PhotoApp::Guest", "DENY"),
    ];
    let answer_for = |entity_type: &str| {
        json!({
            "policyStoreId": store_id,
            "principal": { "entityType": entity_type, "entityId": "alice" },
            "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
            "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
        })
    };

    for (case, entity_type, expected) in decisions {
        let answer = ok(&router, "IsAuthorized", &answer_for(entity_type)).await;
        assert_eq!(
            answer["decision"].as_str(),
            Some(*expected),
            "case {case:?}: before the update, expected {expected}, got {answer}"
        );
    }

    let (status, body) = call(
        &router,
        "UpdatePolicyTemplate",
        &json!({
            "policyStoreId": store_id,
            "policyTemplateId": template_id,
            "statement": "permit(principal is PhotoApp::Admin, \
                          action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"re-typing the template's principal\": expected HTTP 400, got {status} {body}"
    );

    for (case, entity_type, expected) in decisions {
        let answer = ok(&router, "IsAuthorized", &answer_for(entity_type)).await;
        assert_eq!(
            answer["decision"].as_str(),
            Some(*expected),
            "case {case:?}: after the refused update, expected {expected}, got {answer}"
        );
    }
}
