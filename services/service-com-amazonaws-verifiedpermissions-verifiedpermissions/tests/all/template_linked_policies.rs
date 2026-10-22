// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for TEMPLATE-LINKED policies: `CreatePolicy`'s
//! `templateLinked` definition, the readers that report such a policy
//! (`GetPolicy`, `ListPolicies`, `BatchGetPolicy`) and the authorization
//! decisions it takes part in.
//!
//! The rule under test: a `templateLinked` definition instantiates the policy
//! template it names, binding the template's `?principal` / `?resource`
//! placeholders to the entities the request supplies — one binding per
//! placeholder the template USES, and none for a placeholder it does not — and
//! the policy that results is a real policy of the store, reported with
//! `policyType` `TEMPLATE_LINKED` and evaluated through its template's text.
//!
//! Every assertion goes over the wire through the same router the emulator binary
//! serves, reusing the harness in [`crate::harness`], so a value is only visible
//! to a later request if it really reached state.

use serde_json::{json, Value};

use crate::harness::{call, error_code, ok, router, store_with_validation_off};
use crate::schema::photo_app_schema;

/// The entity a row binds `?principal` to.
const PRINCIPAL: (&str, &str) = ("PhotoApp::User", "alice");

/// The entity a row binds `?resource` to.
const RESOURCE: (&str, &str) = ("PhotoApp::Photo", "vacation.jpg");

/// `{entityType, entityId}` of a scope member, or `None` when the response omits
/// it — which is what the model says happens for a scope that names no entity.
fn entity(value: &Value) -> Option<(String, String)> {
    let object = value.as_object()?;
    Some((
        object.get("entityType")?.as_str()?.to_string(),
        object.get("entityId")?.as_str()?.to_string(),
    ))
}

/// `{actionType, actionId}` pairs of an `actions` list, sorted, so a comparison
/// does not depend on the order the parser walked the scope in.
fn actions(value: &Value) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            Some((
                item.get("actionType")?.as_str()?.to_string(),
                item.get("actionId")?.as_str()?.to_string(),
            ))
        })
        .collect();
    out.sort();
    out
}

/// An `EntityIdentifier` request member for a `(type, id)` pair.
fn identifier(pair: (&str, &str)) -> Value {
    json!({ "entityType": pair.0, "entityId": pair.1 })
}

/// A pair as the response reports it, for comparison with [`entity`].
fn owned(pair: (&str, &str)) -> Option<(String, String)> {
    Some((pair.0.to_string(), pair.1.to_string()))
}

/// One row of the template table.
struct Template {
    case: &'static str,
    /// The template's Cedar text, as `CreatePolicyTemplate` stores it.
    statement: &'static str,
    /// Whether the template's scope uses `?principal` / `?resource`, which is
    /// what decides which bindings the instantiating policy must carry.
    binds_principal: bool,
    binds_resource: bool,
    /// The scope the instantiated policy must report: a bound placeholder
    /// resolves to the entity the request supplied, an unbound slot keeps
    /// whatever the TEMPLATE's own scope says, and a scope that names no entity
    /// is reported as absent.
    principal: Option<(&'static str, &'static str)>,
    resource: Option<(&'static str, &'static str)>,
    actions: &'static [(&'static str, &'static str)],
    effect: &'static str,
}

/// Cedar policy templates, one per way a template's scope can use its
/// placeholders — `==` and `in`, principal only, resource only, both, neither
/// slot in the resource position — crossed with both effects and with a
/// condition clause. Every row is text the real Cedar template parser accepts.
const TEMPLATES: &[Template] = &[
    Template {
        case: "principal placeholder, the resource fixed by the template",
        statement: "permit(principal == ?principal, \
                    action == PhotoApp::Action::\"viewPhoto\", \
                    resource == PhotoApp::Photo::\"vacation.jpg\");",
        binds_principal: true,
        binds_resource: false,
        principal: Some(PRINCIPAL),
        resource: Some(RESOURCE),
        actions: &[("PhotoApp::Action", "viewPhoto")],
        effect: "Permit",
    },
    Template {
        case: "resource placeholder, the principal fixed by the template",
        statement: "permit(principal == PhotoApp::User::\"erin\", \
                    action == PhotoApp::Action::\"viewPhoto\", \
                    resource == ?resource);",
        binds_principal: false,
        binds_resource: true,
        principal: Some(("PhotoApp::User", "erin")),
        resource: Some(RESOURCE),
        actions: &[("PhotoApp::Action", "viewPhoto")],
        effect: "Permit",
    },
    Template {
        case: "both placeholders under in, with an unconstrained action",
        statement: "permit(principal in ?principal, action, resource in ?resource);",
        binds_principal: true,
        binds_resource: true,
        principal: Some(PRINCIPAL),
        resource: Some(RESOURCE),
        actions: &[],
        effect: "Permit",
    },
    Template {
        case: "a forbid with a condition and no resource in its scope",
        statement: "forbid(principal == ?principal, action, resource) \
                    when { context.authenticated == false };",
        binds_principal: true,
        binds_resource: false,
        principal: Some(PRINCIPAL),
        resource: None,
        actions: &[],
        effect: "Forbid",
    },
    Template {
        case: "principal placeholder under in, two actions, no resource",
        statement: "permit(principal in ?principal, \
                    action in [PhotoApp::Action::\"viewPhoto\", \
                               PhotoApp::Action::\"sharePhoto\"], \
                    resource);",
        binds_principal: true,
        binds_resource: false,
        principal: Some(PRINCIPAL),
        resource: None,
        actions: &[
            ("PhotoApp::Action", "sharePhoto"),
            ("PhotoApp::Action", "viewPhoto"),
        ],
        effect: "Permit",
    },
];

/// The `templateLinked` definition for `row`, binding exactly the placeholders
/// the template uses.
fn definition(template_id: &str, row: &Template) -> Value {
    let mut linked = json!({ "policyTemplateId": template_id });
    if row.binds_principal {
        linked["principal"] = identifier(PRINCIPAL);
    }
    if row.binds_resource {
        linked["resource"] = identifier(RESOURCE);
    }
    json!({ "templateLinked": linked })
}

/// Create the template of `row` in `store_id` and return its id.
async fn template_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    row: &Template,
) -> String {
    let created = ok(
        router,
        "CreatePolicyTemplate",
        &json!({ "policyStoreId": store_id, "statement": row.statement }),
    )
    .await;
    created["policyTemplateId"]
        .as_str()
        .expect("policyTemplateId is required on the CreatePolicyTemplate response")
        .to_string()
}

// ---------------------------------------------------------------------------
// The instantiation
// ---------------------------------------------------------------------------

/// A `templateLinked` create returns a real policy — `policyType`
/// `TEMPLATE_LINKED`, the scope the link resolves to — and every member of it
/// comes back out of state through `GetPolicy`, `BatchGetPolicy` and
/// `ListPolicies`, whose `policyTemplateId` filter selects exactly the policies
/// that follow that template.
#[tokio::test]
async fn a_template_linked_policy_is_created_and_read_back_out_of_state() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    let mut created_ids: Vec<String> = Vec::new();
    for row in TEMPLATES {
        let case = row.case;
        let template_id = template_of(&router, &store_id, row).await;
        let created = ok(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": definition(&template_id, row),
            }),
        )
        .await;

        let policy_id = created["policyId"].as_str().unwrap_or_default().to_string();
        assert!(
            !policy_id.is_empty(),
            "case {case:?}: policyId is required on the response, got {created}"
        );
        assert!(
            !created_ids.contains(&policy_id),
            "case {case:?}: policyId {policy_id:?} was already minted"
        );
        created_ids.push(policy_id.clone());
        assert_eq!(
            created["policyType"].as_str(),
            Some("TEMPLATE_LINKED"),
            "case {case:?}: expected policyType TEMPLATE_LINKED, got {created}"
        );
        assert_eq!(
            created["policyStoreId"].as_str(),
            Some(store_id.as_str()),
            "case {case:?}: the response must name the store it was created in, got {created}"
        );
        assert_eq!(
            entity(&created["principal"]),
            row.principal.and_then(owned),
            "case {case:?}: unexpected principal in {created}"
        );
        assert_eq!(
            entity(&created["resource"]),
            row.resource.and_then(owned),
            "case {case:?}: unexpected resource in {created}"
        );
        assert_eq!(
            actions(&created["actions"]),
            row.actions
                .iter()
                .map(|(t, i)| (t.to_string(), i.to_string()))
                .collect::<Vec<_>>(),
            "case {case:?}: unexpected actions in {created}"
        );
        assert_eq!(
            created["effect"].as_str(),
            Some(row.effect),
            "case {case:?}: expected effect {:?}, got {created}",
            row.effect
        );
        assert_eq!(
            created["createdDate"], created["lastUpdatedDate"],
            "case {case:?}: a policy that was never updated reports one timestamp twice, \
             got {created}"
        );

        // The round trip: `GetPolicy` reports the template the policy follows
        // and the entities it bound, and carries no statement of its own.
        let fetched = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        assert_eq!(
            fetched["policyType"].as_str(),
            Some("TEMPLATE_LINKED"),
            "case {case:?}: GetPolicy must report policyType TEMPLATE_LINKED, got {fetched}"
        );
        assert_eq!(
            fetched["definition"]["templateLinked"]["policyTemplateId"].as_str(),
            Some(template_id.as_str()),
            "case {case:?}: the detail must name the template the policy follows, got {fetched}"
        );
        assert!(
            fetched["definition"]["static"].is_null(),
            "case {case:?}: a template-linked policy carries no static definition, got {fetched}"
        );
        assert_eq!(
            entity(&fetched["definition"]["templateLinked"]["principal"]),
            row.binds_principal.then(|| owned(PRINCIPAL)).flatten(),
            "case {case:?}: the detail must report the bound principal, got {fetched}"
        );
        assert_eq!(
            entity(&fetched["definition"]["templateLinked"]["resource"]),
            row.binds_resource.then(|| owned(RESOURCE)).flatten(),
            "case {case:?}: the detail must report the bound resource, got {fetched}"
        );
        assert_eq!(
            entity(&fetched["principal"]),
            entity(&created["principal"]),
            "case {case:?}: GetPolicy reported a different principal: {fetched}"
        );
        assert_eq!(
            actions(&fetched["actions"]),
            actions(&created["actions"]),
            "case {case:?}: GetPolicy reported different actions: {fetched}"
        );

        // The same policy through the batch reader.
        let batch = ok(
            &router,
            "BatchGetPolicy",
            &json!({ "requests": [{ "policyStoreId": store_id, "policyId": policy_id }] }),
        )
        .await;
        assert_eq!(
            batch["results"][0]["definition"]["templateLinked"]["policyTemplateId"].as_str(),
            Some(template_id.as_str()),
            "case {case:?}: BatchGetPolicy must report the same template, got {batch}"
        );

        // `ListPolicies`' `policyTemplateId` filter selects the policies that
        // follow that template, and nothing else.
        let filtered = ok(
            &router,
            "ListPolicies",
            &json!({
                "policyStoreId": store_id,
                "filter": { "policyTemplateId": template_id },
            }),
        )
        .await;
        let items = filtered["policies"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: exactly the one policy linked to {template_id} must match, \
             got {filtered}"
        );
        assert_eq!(
            items[0]["policyId"].as_str(),
            Some(policy_id.as_str()),
            "case {case:?}: the filter selected another policy, got {filtered}"
        );
        assert_eq!(
            items[0]["definition"]["templateLinked"]["policyTemplateId"].as_str(),
            Some(template_id.as_str()),
            "case {case:?}: a listed template-linked policy names its template, got {filtered}"
        );
    }

    // Every created policy lists, and the type filter selects all of them.
    let listed = ok(
        &router,
        "ListPolicies",
        &json!({
            "policyStoreId": store_id,
            "filter": { "policyType": "TEMPLATE_LINKED" },
            "maxResults": 200,
        }),
    )
    .await;
    assert_eq!(
        listed["policies"].as_array().map(Vec::len),
        Some(created_ids.len()),
        "expected {} template-linked policies, got {listed}",
        created_ids.len()
    );
}

// ---------------------------------------------------------------------------
// The decision
// ---------------------------------------------------------------------------

/// A template-linked policy takes part in authorization through its TEMPLATE's
/// text: a request matching the bound placeholders is allowed and names the
/// policy — not the template — as determining, and one that does not match is
/// denied.
///
/// The rows differ along which placeholder the template uses and the request
/// triple, so the decision depends on the binding rather than on the template
/// alone.
#[tokio::test]
async fn a_template_linked_policy_decides_authorization_through_its_template() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let row = &TEMPLATES[0];
    let template_id = template_of(&router, &store_id, row).await;
    let created = ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": definition(&template_id, row),
        }),
    )
    .await;
    let policy_id = created["policyId"].as_str().unwrap_or_default().to_string();

    // (case, principal id, action id, resource id, decision)
    let cases: &[(&str, &str, &str, &str, &str)] = &[
        (
            "the bound principal, the template's action and resource",
            "alice",
            "viewPhoto",
            "vacation.jpg",
            "ALLOW",
        ),
        (
            "another principal, so the bound placeholder does not match",
            "bob",
            "viewPhoto",
            "vacation.jpg",
            "DENY",
        ),
        (
            "another action, which the template's scope does not name",
            "alice",
            "sharePhoto",
            "vacation.jpg",
            "DENY",
        ),
        (
            "another resource, which the template's scope does not name",
            "alice",
            "viewPhoto",
            "beach.jpg",
            "DENY",
        ),
    ];

    for (case, principal, action, resource, decision) in cases {
        let answer = ok(
            &router,
            "IsAuthorized",
            &json!({
                "policyStoreId": store_id,
                "principal": identifier(("PhotoApp::User", principal)),
                "action": { "actionType": "PhotoApp::Action", "actionId": action },
                "resource": identifier(("PhotoApp::Photo", resource)),
            }),
        )
        .await;
        assert_eq!(
            answer["decision"].as_str(),
            Some(*decision),
            "case {case:?}: expected {decision}, got {answer}"
        );
        assert_eq!(
            answer["errors"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a template-linked policy must evaluate cleanly, got {answer}"
        );
        let determining: Vec<String> = answer["determiningPolicies"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|item| item["policyId"].as_str().map(str::to_string))
            .collect();
        if *decision == "ALLOW" {
            assert_eq!(
                determining,
                vec![policy_id.clone()],
                "case {case:?}: the decision must name the POLICY, not its template, \
                 got {answer}"
            );
        } else {
            assert!(
                determining.is_empty(),
                "case {case:?}: a deny by absence of a matching policy determines nothing, \
                 got {answer}"
            );
        }
    }

    // The batch path evaluates the same linked policy once for the whole batch.
    let batch = ok(
        &router,
        "BatchIsAuthorized",
        &json!({
            "policyStoreId": store_id,
            "requests": [
                {
                    "principal": identifier(("PhotoApp::User", "alice")),
                    "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
                    "resource": identifier(RESOURCE),
                },
                {
                    "principal": identifier(("PhotoApp::User", "alice")),
                    "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
                    "resource": identifier(("PhotoApp::Photo", "beach.jpg")),
                },
            ],
        }),
    )
    .await;
    assert_eq!(
        batch["results"][0]["decision"].as_str(),
        Some("ALLOW"),
        "case \"batch, bound request\": expected ALLOW, got {batch}"
    );
    assert_eq!(
        batch["results"][1]["decision"].as_str(),
        Some("DENY"),
        "case \"batch, other resource\": expected DENY, got {batch}"
    );
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// A placeholder binding that disagrees with the template is refused with the
/// modelled `ValidationException`, naming the binding at fault, and nothing is
/// stored: a placeholder the template uses must be bound, and one it does not
/// use must not be.
#[tokio::test]
async fn a_placeholder_binding_that_disagrees_with_the_template_is_refused() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    // One template per placeholder shape, so a row can leave out a binding the
    // template needs and supply one it has no slot for.
    let principal_only = template_of(&router, &store_id, &TEMPLATES[0]).await;
    let resource_only = template_of(&router, &store_id, &TEMPLATES[1]).await;
    let both = template_of(&router, &store_id, &TEMPLATES[2]).await;

    // (case, the templateLinked definition, the member the refusal must name)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "no binding for the ?principal the template uses",
            json!({ "templateLinked": { "policyTemplateId": principal_only } }),
            "/definition/templateLinked/principal",
        ),
        (
            "no binding for the ?resource the template uses",
            json!({ "templateLinked": { "policyTemplateId": resource_only } }),
            "/definition/templateLinked/resource",
        ),
        (
            "a resource bound where the template has no ?resource",
            json!({
                "templateLinked": {
                    "policyTemplateId": principal_only,
                    "principal": identifier(PRINCIPAL),
                    "resource": identifier(RESOURCE),
                },
            }),
            "/definition/templateLinked/resource",
        ),
        (
            "a principal bound where the template has no ?principal",
            json!({
                "templateLinked": {
                    "policyTemplateId": resource_only,
                    "principal": identifier(PRINCIPAL),
                    "resource": identifier(RESOURCE),
                },
            }),
            "/definition/templateLinked/principal",
        ),
        (
            "only one of the two placeholders the template uses",
            json!({
                "templateLinked": {
                    "policyTemplateId": both,
                    "principal": identifier(PRINCIPAL),
                },
            }),
            "/definition/templateLinked/resource",
        ),
        (
            "a principal whose type is not a Cedar entity type",
            json!({
                "templateLinked": {
                    "policyTemplateId": principal_only,
                    "principal": identifier(("1 not a type", "alice")),
                },
            }),
            "/definition/templateLinked/principal",
        ),
    ];

    for (case, definition, path) in &cases {
        let (status, body) = call(
            &router,
            "CreatePolicy",
            &json!({ "policyStoreId": store_id, "definition": definition }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        let fields = body["fieldList"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            fields.len(),
            1,
            "case {case:?}: expected exactly one fieldList entry, got {body}"
        );
        assert_eq!(
            fields[0]["path"].as_str(),
            Some(*path),
            "case {case:?}: expected fieldList path {path:?}, got {body}"
        );
    }

    // Nothing above reached state.
    let listed = ok(&router, "ListPolicies", &json!({ "policyStoreId": store_id })).await;
    assert_eq!(
        listed["policies"].as_array().map(Vec::len),
        Some(0),
        "a refused templateLinked create must not store a policy, got {listed}"
    );
}

/// A `templateLinked` definition naming a template the store does not hold is a
/// 404 for that TEMPLATE, whatever form the reference takes, and a template in
/// another store is not reachable either.
#[tokio::test]
async fn a_template_linked_create_naming_no_template_is_a_resource_not_found() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let other_store = store_with_validation_off(&router).await;
    let elsewhere = ok(
        &router,
        "CreatePolicyTemplate",
        &json!({
            "policyStoreId": other_store,
            "name": "elsewhere",
            "statement": TEMPLATES[0].statement,
        }),
    )
    .await;
    let elsewhere_id = elsewhere["policyTemplateId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, the policyTemplateId reference, the resourceId the error reports)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "an id no template in this store has",
            "PTMISSINGMISSINGMISSI".to_string(),
            "PTMISSINGMISSINGMISSI".to_string(),
        ),
        (
            "a name no template in this store has",
            "name/no-such-template".to_string(),
            "no-such-template".to_string(),
        ),
        (
            "a template that lives in another store",
            elsewhere_id.clone(),
            elsewhere_id.clone(),
        ),
        (
            "a template named in another store",
            "name/elsewhere".to_string(),
            "elsewhere".to_string(),
        ),
    ];

    for (case, reference, resource_id) in &cases {
        let (status, body) = call(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": {
                    "templateLinked": {
                        "policyTemplateId": reference,
                        "principal": identifier(PRINCIPAL),
                    },
                },
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("POLICY_TEMPLATE"),
            "case {case:?}: the missing resource is a policy template, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(resource_id.as_str()),
            "case {case:?}: expected resourceId {resource_id:?}, got {body}"
        );
    }
}

/// A template-linked policy is validated against the store's schema exactly as a
/// static one is: a link whose template the schema sanctions is stored, and one
/// whose template it does not is refused at `/definition`.
#[tokio::test]
async fn a_strict_store_validates_the_policy_a_link_resolves_to() {
    let router = router();
    // The store starts with validation OFF so both templates can be stored —
    // a STRICT store refuses a template its schema does not sanction when the
    // template is created — and is then switched to STRICT, which is what makes
    // the LINK the thing being validated.
    let store_id = store_with_validation_off(&router).await;
    ok(
        &router,
        "PutSchema",
        &json!({
            "policyStoreId": store_id,
            "definition": { "cedarJson": photo_app_schema() },
        }),
    )
    .await;

    // The schema declares `viewPhoto` over principal type `User` and resource
    // type `Photo`, and declares no `deletePhoto` at all.
    let mut template_ids: Vec<String> = Vec::new();
    for statement in [
        "permit(principal == ?principal, \
         action == PhotoApp::Action::\"viewPhoto\", \
         resource == PhotoApp::Photo::\"vacation.jpg\");",
        "permit(principal == ?principal, \
         action == PhotoApp::Action::\"deletePhoto\", \
         resource == PhotoApp::Photo::\"vacation.jpg\");",
    ] {
        let created = ok(
            &router,
            "CreatePolicyTemplate",
            &json!({ "policyStoreId": store_id, "statement": statement }),
        )
        .await;
        template_ids.push(
            created["policyTemplateId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    ok(
        &router,
        "UpdatePolicyStore",
        &json!({
            "policyStoreId": store_id,
            "validationSettings": { "mode": "STRICT" },
        }),
    )
    .await;

    // Rows differ along whether the schema sanctions the policy the link
    // resolves to.
    let cases: &[(&str, usize, u16)] = &[
        ("a template the schema sanctions", 0, 200),
        ("a template naming an action the schema never declares", 1, 400),
    ];

    for (case, index, expected) in cases {
        let template_id = template_ids[*index].clone();
        let (status, body) = call(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": {
                    "templateLinked": {
                        "policyTemplateId": template_id,
                        "principal": identifier(PRINCIPAL),
                    },
                },
            }),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body}"
            );
            let fields = body["fieldList"].as_array().cloned().unwrap_or_default();
            assert!(
                !fields.is_empty(),
                "case {case:?}: a schema refusal reports its reasons, got {body}"
            );
            for field in &fields {
                assert_eq!(
                    field["path"].as_str(),
                    Some("/definition"),
                    "case {case:?}: a schema-validation entry names the validated member, \
                     got {body}"
                );
            }
        } else {
            let policy_id = body["policyId"].as_str().unwrap_or_default();
            let fetched = ok(
                &router,
                "GetPolicy",
                &json!({ "policyStoreId": store_id, "policyId": policy_id }),
            )
            .await;
            assert_eq!(
                fetched["definition"]["templateLinked"]["policyTemplateId"].as_str(),
                Some(template_id.as_str()),
                "case {case:?}: the validated link must be what was stored, got {fetched}"
            );
        }
    }

    // Exactly the one link that validated is in the store.
    let listed = ok(&router, "ListPolicies", &json!({ "policyStoreId": store_id })).await;
    assert_eq!(
        listed["policies"].as_array().map(Vec::len),
        Some(1),
        "only the link that validated may be stored, got {listed}"
    );
}

/// `clientToken` is an `@idempotencyToken` on this arm too: a replay answers
/// with the policy the first call created, and the same token with a different
/// binding is a 409 `ConflictException`.
#[tokio::test]
async fn a_client_token_replay_does_not_create_a_second_linked_policy() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let template_id = template_of(&router, &store_id, &TEMPLATES[2]).await;

    // Rows differ along WHICH member of the definition the retry changes.
    let changed: &[(&str, Value)] = &[
        (
            "the bound principal",
            json!({
                "policyTemplateId": template_id,
                "principal": identifier(("PhotoApp::User", "bob")),
                "resource": identifier(RESOURCE),
            }),
        ),
        (
            "the bound resource",
            json!({
                "policyTemplateId": template_id,
                "principal": identifier(PRINCIPAL),
                "resource": identifier(("PhotoApp::Photo", "beach.jpg")),
            }),
        ),
    ];

    for (index, (case, retried)) in changed.iter().enumerate() {
        let token = format!("linked-token-{index}");
        let request = json!({
            "policyStoreId": store_id,
            "clientToken": token,
            "definition": {
                "templateLinked": {
                    "policyTemplateId": template_id,
                    "principal": identifier(PRINCIPAL),
                    "resource": identifier(RESOURCE),
                },
            },
        });

        let first = ok(&router, "CreatePolicy", &request).await;
        let replay = ok(&router, "CreatePolicy", &request).await;
        assert_eq!(
            replay["policyId"], first["policyId"],
            "case {case:?}: a replay must not create a second policy, got {replay}"
        );
        assert_eq!(
            replay["createdDate"], first["createdDate"],
            "case {case:?}: a replay must report the first policy's createdDate, got {replay}"
        );

        let (status, body) = call(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "clientToken": token,
                "definition": { "templateLinked": retried },
            }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: the same token with a different binding must conflict, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: unexpected error code, got {body}"
        );
    }
}
