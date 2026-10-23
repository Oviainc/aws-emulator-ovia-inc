// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `DeletePolicyTemplate` and the CASCADE it documents.
//!
//! The rule under test: deleting a policy template removes that template and, in
//! the same call, every policy of the store that was instantiated from it, while
//! every other resource of the store — its other templates, their linked
//! policies and its static policies — is left untouched; a reference (store or
//! template) that names nothing is a 404 `ResourceNotFoundException`.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so a
//! resource is only reported as gone if the delete really reached this service's
//! state.

use serde_json::{json, Value};

use super::harness::{call, error_code, ok, router, store_with_validation_off};

/// One policy template, and how a policy instantiates it.
struct Row {
    case: &'static str,
    /// The template's Cedar text.
    statement: &'static str,
    /// Whether the template's scope uses `?principal` / `?resource`, which is
    /// what decides which bindings an instantiating policy must carry (see
    /// `tests/all/template_linked_policies.rs`, which pins that rule itself).
    binds_principal: bool,
    binds_resource: bool,
    /// How many policies this row links to its template before deleting it.
    linked: usize,
    /// Whether the delete names the template by `name/<policyTemplateName>`
    /// instead of by its id — `DeletePolicyTemplateInput$policyTemplateId`
    /// documents both forms ("You can use the policy template name in place of
    /// the policy template ID. When using a name, prefix it with name/").
    by_name: bool,
    /// Whether the delete names the store by its `policy-store-alias/<name>`
    /// alias instead of by its id, the other reference form
    /// `DeletePolicyTemplateInput$policyStoreId` documents.
    by_alias: bool,
}

/// The template catalog: one row per way a template's scope can use the
/// `?principal` / `?resource` placeholders `CreatePolicyTemplate` documents ("A
/// template can use placeholders for the principal and resource"), crossed with
/// the number of policies linked to it (none, one, several) and with each
/// documented reference form the delete accepts.
const ROWS: &[Row] = &[
    Row {
        case: "both placeholders, one linked policy, deleted by id",
        statement: "permit(principal == ?principal, \
                    action == PhotoApp::Action::\"viewPhoto\", \
                    resource == ?resource);",
        binds_principal: true,
        binds_resource: true,
        linked: 1,
        by_name: false,
        by_alias: false,
    },
    Row {
        case: "principal placeholder, three linked policies, deleted by name",
        statement: "permit(principal == ?principal, \
                    action == PhotoApp::Action::\"viewPhoto\", \
                    resource == PhotoApp::Photo::\"vacation.jpg\");",
        binds_principal: true,
        binds_resource: false,
        linked: 3,
        by_name: true,
        by_alias: false,
    },
    Row {
        case: "resource placeholder, one linked policy, deleted through the store alias",
        statement: "permit(principal == PhotoApp::User::\"erin\", \
                    action == PhotoApp::Action::\"viewPhoto\", \
                    resource == ?resource);",
        binds_principal: false,
        binds_resource: true,
        linked: 1,
        by_name: false,
        by_alias: true,
    },
    Row {
        case: "both placeholders under in, two linked policies",
        statement: "permit(principal in ?principal, action, resource in ?resource);",
        binds_principal: true,
        binds_resource: true,
        linked: 2,
        by_name: true,
        by_alias: false,
    },
    Row {
        case: "a template nothing was instantiated from",
        statement: "forbid(principal == ?principal, action, resource) \
                    when { context.authenticated == false };",
        binds_principal: true,
        binds_resource: false,
        linked: 0,
        by_name: false,
        by_alias: false,
    },
];

/// An `EntityIdentifier` request member for a `(type, id)` pair.
fn identifier(entity_type: &str, entity_id: &str) -> Value {
    json!({ "entityType": entity_type, "entityId": entity_id })
}

/// Create a template in `store_id` and return `(policyTemplateId, name)`.
async fn create_template(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    name: &str,
    statement: &str,
) -> String {
    let created = ok(
        router,
        "CreatePolicyTemplate",
        &json!({ "policyStoreId": store_id, "name": name, "statement": statement }),
    )
    .await;
    created["policyTemplateId"]
        .as_str()
        .expect("policyTemplateId is required on the CreatePolicyTemplate response")
        .to_string()
}

/// Instantiate `template_id` once, binding only the placeholders the template
/// uses, and return the new `policyId`.
async fn link_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    template_id: &str,
    binds_principal: bool,
    binds_resource: bool,
    index: usize,
) -> String {
    let mut linked = json!({ "policyTemplateId": template_id });
    if binds_principal {
        linked["principal"] = identifier("PhotoApp::User", &format!("user-{index}"));
    }
    if binds_resource {
        linked["resource"] = identifier("PhotoApp::Photo", &format!("photo-{index}.jpg"));
    }
    let created = ok(
        router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "templateLinked": linked },
        }),
    )
    .await;
    created["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string()
}

/// The `policyId`s `ListPolicies` reports for `store_id`.
async fn listed_policy_ids(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
) -> Vec<String> {
    let listed = ok(router, "ListPolicies", &json!({ "policyStoreId": store_id })).await;
    listed["policies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["policyId"].as_str().map(str::to_string))
        .collect()
}

/// Deleting a template removes the template and every policy linked to it, and
/// leaves every other resource of the store alone.
#[tokio::test]
async fn deleting_a_template_cascades_onto_the_policies_linked_to_it() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    // One alias for the store, so a row can name the store the other documented
    // way.
    let alias = "policy-store-alias/cascade-store";
    ok(
        &router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": alias, "policyStoreId": store_id }),
    )
    .await;

    // The survivors: a static policy, plus a SECOND template with a policy of
    // its own. Neither may be touched by any row's delete, which is what makes
    // the cascade targeted rather than a sweep of the store.
    let static_policy = ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "static": { "statement":
                "permit(principal == PhotoApp::User::\"erin\", \
                 action == PhotoApp::Action::\"viewPhoto\", \
                 resource == PhotoApp::Photo::\"beach.jpg\");" } },
        }),
    )
    .await["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string();
    let keeper_template = create_template(
        &router,
        &store_id,
        "keeper",
        "permit(principal == ?principal, action, resource);",
    )
    .await;
    let keeper_policy = link_policy(&router, &store_id, &keeper_template, true, false, 900).await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("tpl-{index}");
        let template_id = create_template(&router, &store_id, &name, row.statement).await;

        let mut linked_ids = Vec::new();
        for link in 0..row.linked {
            linked_ids.push(
                link_policy(
                    &router,
                    &store_id,
                    &template_id,
                    row.binds_principal,
                    row.binds_resource,
                    index * 10 + link,
                )
                .await,
            );
        }

        // Everything the row created is reachable before the delete, so an
        // absence asserted afterwards is a change this call made.
        for policy_id in &linked_ids {
            let fetched = ok(
                &router,
                "GetPolicy",
                &json!({ "policyStoreId": store_id, "policyId": policy_id }),
            )
            .await;
            assert_eq!(
                fetched["policyType"].as_str(),
                Some("TEMPLATE_LINKED"),
                "case {case:?}: policy {policy_id} must be linked to the template, got {fetched}"
            );
        }

        let store_reference = if row.by_alias { alias } else { store_id.as_str() };
        let template_reference = if row.by_name {
            format!("name/{name}")
        } else {
            template_id.clone()
        };
        let (status, body) = call(
            &router,
            "DeletePolicyTemplate",
            &json!({
                "policyStoreId": store_reference,
                "policyTemplateId": template_reference,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting {template_reference} must succeed, got {status} {body}"
        );

        // The template itself is gone, by id and by name.
        for (reference_case, reference) in [
            ("by id", template_id.clone()),
            ("by name", format!("name/{name}")),
        ] {
            let (status, body) = call(
                &router,
                "GetPolicyTemplate",
                &json!({ "policyStoreId": store_id, "policyTemplateId": reference }),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?} / {reference_case}: a deleted template must be gone, \
                 got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} / {reference_case}: unexpected error code, got {body}"
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some("POLICY_TEMPLATE"),
                "case {case:?} / {reference_case}: the fault must name the resource type, \
                 got {body}"
            );
        }

        // Every policy created from it is gone with it.
        let listed = listed_policy_ids(&router, &store_id).await;
        for policy_id in &linked_ids {
            let (status, body) = call(
                &router,
                "GetPolicy",
                &json!({ "policyStoreId": store_id, "policyId": policy_id }),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: policy {policy_id} was created from the deleted template and \
                 must be gone, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: unexpected error code for {policy_id}, got {body}"
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some("POLICY"),
                "case {case:?}: the fault must name the resource type, got {body}"
            );
            assert!(
                !listed.contains(policy_id),
                "case {case:?}: ListPolicies still reports {policy_id}, got {listed:?}"
            );
        }

        // The survivors are untouched: the other template, its linked policy and
        // the static policy.
        let keeper = ok(
            &router,
            "GetPolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": keeper_template }),
        )
        .await;
        assert_eq!(
            keeper["policyTemplateId"].as_str(),
            Some(keeper_template.as_str()),
            "case {case:?}: another template must survive the delete, got {keeper}"
        );
        for (survivor_case, policy_id) in [
            ("the other template's linked policy", &keeper_policy),
            ("a static policy", &static_policy),
        ] {
            let fetched = ok(
                &router,
                "GetPolicy",
                &json!({ "policyStoreId": store_id, "policyId": policy_id }),
            )
            .await;
            assert_eq!(
                fetched["policyId"].as_str(),
                Some(policy_id.as_str()),
                "case {case:?}: {survivor_case} must survive the delete, got {fetched}"
            );
            assert!(
                listed.contains(policy_id),
                "case {case:?}: ListPolicies must still report {survivor_case}, got {listed:?}"
            );
        }

        // A repeat delete finds nothing: the template no longer resolves, which
        // is the modelled 404 rather than a silent success.
        let (status, body) = call(
            &router,
            "DeletePolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a repeat delete must be a 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code on the repeat delete, got {body}"
        );
    }
}

/// Once the template is gone, so is the authorization its linked policies
/// granted: the decision that was `ALLOW` through a linked policy becomes `DENY`,
/// determined by nothing.
#[tokio::test]
async fn the_cascade_revokes_the_authorization_the_linked_policies_granted() {
    let router = router();

    // Rows differ along the placeholder form of the template, so the revocation
    // is asserted for each way a link binds its entities.
    for (index, row) in ROWS.iter().filter(|row| row.linked > 0).enumerate() {
        let case = row.case;
        let store_id = store_with_validation_off(&router).await;
        let template_id = create_template(&router, &store_id, "revoked", row.statement).await;
        let policy_id = link_policy(
            &router,
            &store_id,
            &template_id,
            row.binds_principal,
            row.binds_resource,
            index,
        )
        .await;

        // The request the link grants: the bound entity where the template uses
        // a placeholder, the template's own scope entity where it does not.
        let principal = if row.binds_principal {
            format!("user-{index}")
        } else {
            "erin".to_string()
        };
        let resource = if row.binds_resource {
            format!("photo-{index}.jpg")
        } else {
            "vacation.jpg".to_string()
        };
        let request = json!({
            "policyStoreId": store_id,
            "principal": identifier("PhotoApp::User", &principal),
            "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
            "resource": identifier("PhotoApp::Photo", &resource),
        });

        let before = ok(&router, "IsAuthorized", &request).await;
        assert_eq!(
            before["decision"].as_str(),
            Some("ALLOW"),
            "case {case:?}: the linked policy must grant the request before the delete, \
             got {before}"
        );
        let determining: Vec<String> = before["determiningPolicies"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|item| item["policyId"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            determining,
            vec![policy_id.clone()],
            "case {case:?}: the ALLOW must be determined by the linked policy, got {before}"
        );

        ok(
            &router,
            "DeletePolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
        )
        .await;

        let after = ok(&router, "IsAuthorized", &request).await;
        assert_eq!(
            after["decision"].as_str(),
            Some("DENY"),
            "case {case:?}: the grant must be gone with its template, got {after}"
        );
        assert_eq!(
            after["determiningPolicies"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a deny by absence of a policy determines nothing, got {after}"
        );
    }
}

/// A reference that names nothing is a 404 `ResourceNotFoundException`, whichever
/// half of the request it is in, and nothing in the store changes.
#[tokio::test]
async fn an_unknown_store_or_template_is_a_resource_not_found() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let other_store = store_with_validation_off(&router).await;
    let template_id = create_template(&router, &store_id, "kept", ROWS[0].statement).await;
    let policy_id = link_policy(&router, &store_id, &template_id, true, true, 1).await;

    // Rows differ along WHICH reference misses: the store (by id, by alias), the
    // template (by id, by name) and a template that exists but in another store.
    let cases: &[(&str, Value, &str)] = &[
        (
            "an unknown store id",
            json!({
                "policyStoreId": "PSMISSINGMISSINGMISSI",
                "policyTemplateId": template_id.clone(),
            }),
            "POLICY_STORE",
        ),
        (
            "an unknown store alias",
            json!({
                "policyStoreId": "policy-store-alias/not-an-alias",
                "policyTemplateId": template_id.clone(),
            }),
            "POLICY_STORE_ALIAS",
        ),
        (
            "an unknown template id",
            json!({ "policyStoreId": store_id.clone(), "policyTemplateId": "PTMISSINGMISSINGMISSI" }),
            "POLICY_TEMPLATE",
        ),
        (
            "an unknown template name",
            json!({ "policyStoreId": store_id.clone(), "policyTemplateId": "name/no-such-template" }),
            "POLICY_TEMPLATE",
        ),
        (
            "a template that belongs to another store",
            json!({
                "policyStoreId": other_store.clone(),
                "policyTemplateId": template_id.clone(),
            }),
            "POLICY_TEMPLATE",
        ),
    ];

    for (case, request, resource_type) in cases {
        let (status, body) = call(&router, "DeletePolicyTemplate", request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some(*resource_type),
            "case {case:?}: the fault must name the kind of resource the reference named, \
             got {body}"
        );
    }

    // Every refused call left the store exactly as it was.
    let fetched = ok(
        &router,
        "GetPolicyTemplate",
        &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
    )
    .await;
    assert_eq!(
        fetched["policyTemplateId"].as_str(),
        Some(template_id.as_str()),
        "case \"nothing changed\": the template must still be there, got {fetched}"
    );
    let listed = listed_policy_ids(&router, &store_id).await;
    assert!(
        listed.contains(&policy_id),
        "case \"nothing changed\": the linked policy must still be there, got {listed:?}"
    );
}
