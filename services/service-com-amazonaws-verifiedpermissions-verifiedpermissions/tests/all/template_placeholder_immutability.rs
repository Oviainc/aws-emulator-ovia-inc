// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for ONE rule of `UpdatePolicyTemplate`: a scope position
//! HOLDING a `?principal` / `?resource` placeholder and one NAMING an entity are
//! different references, whatever entity is named, so a replacement that turns
//! one into the other changes the principal (or the resource) the template
//! references and is refused.
//!
//! The sentence the rule comes from is `UpdatePolicyTemplateInput$statement`'s
//! own — "You can't change the following elements: The effect (permit or forbid)
//! of the policy template. The `principal` referenced by the policy template.
//! The `resource` referenced by the policy template."
//! (`smithy_query(query="shape", shape="UpdatePolicyTemplateInput")`, member
//! `statement`).
//!
//! The table's rows therefore differ along the DIRECTION of the change
//! (placeholder to entity and back), the POSITION it is in (principal,
//! resource, both at once), the FORM the position takes (`==`, `in`, `is T in`)
//! and the ENTITY the concrete side names — including one in a namespace this
//! emulator once used internally to stand in for a placeholder, which is an
//! ordinary entity reference a caller may write and which must therefore not
//! compare equal to a placeholder either.
//!
//! What a surviving placeholder is FOR is the policies linked to the template,
//! so every row also checks the consequence: the store's template-linked
//! policies stay instantiable, which an authorization call reports by raising no
//! evaluation error at all.
//!
//! Every assertion goes over the wire through the shared [`crate::harness`], so
//! a refusal only counts if nothing reached state behind it.

use serde_json::{json, Value};

use crate::harness::{call, error_code, ok, router, store_with_validation_off};

/// One row of the table: the text the template is created with, the
/// replacement, and which immutable scope elements that replacement CHANGES.
struct Row {
    case: &'static str,
    stored: &'static str,
    replacement: &'static str,
    /// The immutable elements the replacement changes, each named by the word
    /// the refusal's `fieldList` message uses for it. EMPTY for a replacement
    /// that changes only what the operation allows to change, which is applied.
    changed: &'static [&'static str],
}

/// Cedar accepts a template only when its scope uses at least one placeholder,
/// so every row below — stored text and replacement alike — keeps one position
/// open while the other is the one under test.
const ROWS: &[Row] = &[
    Row {
        case: "a placeholder principal becomes the entity this emulator once stood in for it with",
        stored: "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "permit(principal == EmulatorInternal::TemplateSlot::\"unbound\", \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        case: "that same entity principal becomes a placeholder",
        stored: "permit(principal == EmulatorInternal::TemplateSlot::\"unbound\", \
                 action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        replacement: "permit(principal == ?principal, \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        case: "a placeholder principal becomes an ordinary entity",
        stored: "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "permit(principal == PhotoApp::User::\"alice\", \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        case: "an ordinary entity principal becomes a placeholder",
        stored: "permit(principal == StoreApp::Admin::\"root\", \
                 action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        replacement: "permit(principal == ?principal, \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        case: "a placeholder resource becomes the entity this emulator once stood in for it with",
        stored: "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "permit(principal == ?principal, \
                      action == PhotoApp::Action::\"viewPhoto\", \
                      resource == EmulatorInternal::TemplateSlot::\"unbound\");",
        changed: &["resource"],
    },
    Row {
        case: "an ordinary entity resource becomes a placeholder",
        stored: "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == PhotoApp::Photo::\"vacation.jpg\");",
        replacement: "permit(principal == ?principal, \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["resource"],
    },
    Row {
        case: "a placeholder principal under `in` becomes an entity",
        stored: "permit(principal in ?principal, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "permit(principal in EmulatorInternal::TemplateSlot::\"unbound\", \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        case: "a placeholder principal under `is ... in` becomes an entity, the type kept",
        stored: "permit(principal is PhotoApp::User in ?principal, \
                 action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        replacement: "permit(principal is PhotoApp::User in PhotoApp::Group::\"eng\", \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        case: "an entity principal under `is ... in` becomes a placeholder, the type kept",
        stored: "permit(principal is PhotoApp::User in PhotoApp::Group::\"eng\", \
                 action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        replacement: "permit(principal is PhotoApp::User in ?principal, \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["principal"],
    },
    Row {
        // Two immutable elements at once: one `fieldList` entry per element.
        case: "a placeholder principal becomes an entity while the resource's type changes",
        stored: "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
                 resource is PhotoApp::Photo in ?resource);",
        replacement: "permit(principal == PhotoApp::User::\"alice\", \
                      action == PhotoApp::Action::\"viewPhoto\", \
                      resource is PhotoApp::Album in ?resource);",
        changed: &["principal", "resource"],
    },
    Row {
        case: "the effect changes while both placeholders are kept",
        stored: "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "forbid(principal == ?principal, \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        changed: &["effect"],
    },
    Row {
        case: "the action changes while both placeholders are kept",
        stored: "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "permit(principal == ?principal, \
                      action == PhotoApp::Action::\"sharePhoto\", resource == ?resource);",
        changed: &[],
    },
    Row {
        case: "a condition is added while a concretely named principal is kept",
        stored: "permit(principal == EmulatorInternal::TemplateSlot::\"unbound\", \
                 action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);",
        replacement: "permit(principal == EmulatorInternal::TemplateSlot::\"unbound\", \
                      action == PhotoApp::Action::\"viewPhoto\", resource == ?resource) \
                      when { true };",
        changed: &[],
    },
    Row {
        case: "only the whitespace around a placeholder principal changes",
        stored: "permit(principal in ?principal, action == PhotoApp::Action::\"viewPhoto\", \
                 resource == ?resource);",
        replacement: "permit(\n  principal in ?principal,\n  \
                      action == PhotoApp::Action::\"viewPhoto\",\n  resource == ?resource\n);",
        changed: &[],
    },
];

/// `{entityType, entityId}` of a scope member, or `None` when the response omits
/// it.
fn entity(value: &Value) -> Option<(String, String)> {
    let object = value.as_object()?;
    Some((
        object.get("entityType")?.as_str()?.to_string(),
        object.get("entityId")?.as_str()?.to_string(),
    ))
}

/// The `errorDescription` of every evaluation error an authorization answer
/// reports.
fn error_descriptions(body: &Value) -> Vec<String> {
    body["errors"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item["errorDescription"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `policyId` of every policy an authorization answer says determined it.
fn determining(body: &Value) -> Vec<String> {
    body["determiningPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["policyId"].as_str().unwrap_or_default().to_string())
        .collect()
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
        "case {case:?}: expected one fieldList entry per changed element {changed:?}, got {body}"
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

/// A `templateLinked` definition binding exactly the placeholders `statement`
/// uses — the only binding `CreatePolicy` accepts for it.
fn link_for(template_id: &str, statement: &str) -> Value {
    let mut link = json!({ "policyTemplateId": template_id });
    if statement.contains("?principal") {
        link["principal"] = json!({ "entityType": "PhotoApp::User", "entityId": "alice" });
    }
    if statement.contains("?resource") {
        link["resource"] = json!({ "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" });
    }
    link
}

/// One authorization request, used only to ask whether the store's policies are
/// all still instantiable: a link whose bindings no longer match its template's
/// slots is reported as an evaluation error on every request.
fn authorization_request(store_id: &str) -> Value {
    json!({
        "policyStoreId": store_id,
        "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
        "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
        "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
    })
}

/// A replacement may not turn a scope position holding a placeholder into one
/// naming an entity, or the reverse, in either position and in any form the
/// position takes — while a change the operation allows is applied, and the
/// store's linked policies stay instantiable either way.
#[tokio::test]
async fn a_placeholder_scope_position_and_a_named_entity_are_different_references() {
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

        let linked = ok(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "templateLinked": link_for(&template_id, row.stored) },
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

        // The scope the linked policy reports is the template's, which no
        // permitted replacement changes.
        let after = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
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

        // Whichever way the update went, the policy linked to this template can
        // still be instantiated from it, so evaluating the store raises nothing.
        let answer = ok(&router, "IsAuthorized", &authorization_request(&store_id)).await;
        assert_eq!(
            error_descriptions(&answer),
            Vec::<String>::new(),
            "case {case:?}: the store's linked policy must still be instantiable, got {answer}"
        );
    }
}

/// The scenario the rule protects, end to end: a template whose principal names
/// an entity concretely keeps that principal, so the policy linked to it keeps
/// taking part in every decision and keeps being creatable on the bindings the
/// template's slots call for.
#[tokio::test]
async fn a_template_naming_a_principal_concretely_keeps_its_linked_decision() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let stored = "permit(principal == EmulatorInternal::TemplateSlot::\"unbound\", \
                  action == PhotoApp::Action::\"viewPhoto\", resource == ?resource);";
    let created = ok(
        &router,
        "CreatePolicyTemplate",
        &json!({ "policyStoreId": store_id, "statement": stored }),
    )
    .await;
    let template_id = created["policyTemplateId"]
        .as_str()
        .expect("policyTemplateId is required on the CreatePolicyTemplate response")
        .to_string();
    let linked = ok(
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
    let policy_id = linked["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string();

    let request = json!({
        "policyStoreId": store_id,
        "principal": { "entityType": "EmulatorInternal::TemplateSlot", "entityId": "unbound" },
        "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
        "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
    });

    // (case, when the answer is asked for)
    for case in ["before the refused update", "after the refused update"] {
        let answer = ok(&router, "IsAuthorized", &request).await;
        assert_eq!(
            answer["decision"].as_str(),
            Some("ALLOW"),
            "case {case:?}: the linked policy permits this request, got {answer}"
        );
        assert_eq!(
            determining(&answer),
            vec![policy_id.clone()],
            "case {case:?}: the linked policy must determine the decision, got {answer}"
        );
        assert_eq!(
            error_descriptions(&answer),
            Vec::<String>::new(),
            "case {case:?}: nothing was raised while evaluating, got {answer}"
        );

        if case.starts_with("before") {
            let (status, body) = call(
                &router,
                "UpdatePolicyTemplate",
                &json!({
                    "policyStoreId": store_id,
                    "policyTemplateId": template_id,
                    "statement": "permit(principal == ?principal, \
                                  action == PhotoApp::Action::\"viewPhoto\", \
                                  resource == ?resource);",
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case \"opening the template's principal\": expected HTTP 400, got {status} {body}"
            );
            assert_refusal(
                "opening the template's principal",
                &body,
                "/template/statement",
                &["principal"],
            );
        }
    }

    // The slots the template uses are the ones a new policy must bind, which the
    // refused replacement did not change either.
    let (status, body) = call(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "templateLinked": {
                "policyTemplateId": template_id,
                "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
                "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
            } },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"binding a placeholder the template does not use\": expected HTTP 400, \
         got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"binding a placeholder the template does not use\": unexpected error code, \
         got {body}"
    );
}
