// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for policy templates: `CreatePolicyTemplate` and the
//! `GetPolicyTemplate` read that makes its write observable.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so a
//! member only round-trips if it really reached this service's state.

use serde_json::json;

use super::harness::{call, error_code, ok, router, store_with_validation_off};

/// Cedar TEMPLATE statements, one per way a template's scope can use the
/// `?principal` / `?resource` placeholders the operation documents ("A template
/// can use placeholders for the principal and resource"). Every row is Cedar the
/// real template parser accepts.
const TEMPLATE_STATEMENTS: &[(&str, &str)] = &[
    (
        "principal placeholder",
        "permit(principal == ?principal, action == Action::\"view\", resource == Photo::\"p\");",
    ),
    (
        "resource placeholder",
        "permit(principal == User::\"erin\", action == Action::\"view\", resource == ?resource);",
    ),
    (
        "both placeholders",
        "permit(principal in ?principal, action, resource in ?resource);",
    ),
    (
        "principal placeholder with in",
        "permit(principal in ?principal, action == Action::\"view\", resource);",
    ),
    (
        "forbid with a condition",
        "forbid(principal == ?principal, action, resource) when { context.authenticated == false };",
    ),
];

/// A template's every response member is read back out of state, across the
/// whole range of optional members a create may or may not set and every way a
/// template's scope can use its placeholders.
#[tokio::test]
async fn policy_template_round_trips_every_member_it_was_created_with() {
    // Rows differ along the placeholder form of the statement (the catalog above)
    // crossed with which optional members the request carries.
    let optional: &[(&str, Option<&str>, Option<&str>)] = &[
        ("bare", None, None),
        ("named", Some("tpl"), None),
        ("described", None, Some("a described template")),
        ("named and described", Some("tpl"), Some("both members")),
    ];

    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let mut seen_ids: Vec<String> = Vec::new();

    for (statement_case, statement) in TEMPLATE_STATEMENTS {
        for (index, (optional_case, name, description)) in optional.iter().enumerate() {
            let case = format!("{statement_case} / {optional_case}");
            // A template name is unique within its store, so each row that asks
            // for a name gets its own.
            let name = name.map(|name| format!("{name}-{statement_case}-{index}").replace(' ', "-"));
            let mut request = json!({ "policyStoreId": store_id, "statement": statement });
            if let Some(name) = name.as_deref() {
                request["name"] = json!(name);
            }
            if let Some(description) = description {
                request["description"] = json!(description);
            }

            let created = ok(&router, "CreatePolicyTemplate", &request).await;
            let template_id = created["policyTemplateId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                template_id.starts_with("PT") && template_id.len() > 2,
                "case {case:?}: expected a minted PT-prefixed policyTemplateId, got {created}"
            );
            assert!(
                template_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-/_".contains(c)),
                "case {case:?}: policyTemplateId must match ^[a-zA-Z0-9-/_]*$, got {template_id:?}"
            );
            assert!(
                !seen_ids.contains(&template_id),
                "case {case:?}: policyTemplateId {template_id:?} was already minted"
            );
            seen_ids.push(template_id.clone());
            assert_eq!(
                created["policyStoreId"].as_str(),
                Some(store_id.as_str()),
                "case {case:?}: the response must name the store it was created in, got {created}"
            );
            assert_eq!(
                created["createdDate"], created["lastUpdatedDate"],
                "case {case:?}: a template that was never updated reports one timestamp twice, \
                 got {created}"
            );

            // The round trip: read the template back through GetPolicyTemplate.
            let fetched = ok(
                &router,
                "GetPolicyTemplate",
                &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
            )
            .await;
            assert_eq!(
                fetched["statement"].as_str(),
                Some(*statement),
                "case {case:?}: the statement must come back byte-identical, got {fetched}"
            );
            assert_eq!(
                fetched["name"].as_str(),
                name.as_deref(),
                "case {case:?}: unexpected name, got {fetched}"
            );
            assert_eq!(
                fetched["description"].as_str(),
                *description,
                "case {case:?}: unexpected description, got {fetched}"
            );
            assert_eq!(
                fetched["policyTemplateId"].as_str(),
                Some(template_id.as_str()),
                "case {case:?}: unexpected policyTemplateId, got {fetched}"
            );
            assert_eq!(
                fetched["createdDate"], created["createdDate"],
                "case {case:?}: the read must report the stored createdDate, got {fetched}"
            );
            assert_eq!(
                fetched["lastUpdatedDate"], created["lastUpdatedDate"],
                "case {case:?}: the read must report the stored lastUpdatedDate, got {fetched}"
            );

            // A named template is reachable by `name/<name>` as well as by its
            // id: "You can use the policy template name in place of the policy
            // template ID. When using a name, prefix it with name/."
            if let Some(name) = name.as_deref() {
                let by_name = ok(
                    &router,
                    "GetPolicyTemplate",
                    &json!({
                        "policyStoreId": store_id,
                        "policyTemplateId": format!("name/{name}"),
                    }),
                )
                .await;
                assert_eq!(
                    by_name["policyTemplateId"].as_str(),
                    Some(template_id.as_str()),
                    "case {case:?}: name/{name} must resolve to the same template, got {by_name}"
                );
            }
        }
    }
}

/// A statement the Cedar TEMPLATE parser refuses is refused here as a 400
/// `ValidationException`, and nothing is stored.
#[tokio::test]
async fn an_unparseable_template_statement_is_refused_and_nothing_is_stored() {
    // Rows differ along HOW the Cedar text is broken.
    let cases: &[(&str, &str)] = &[
        ("not cedar at all", "this is not a policy"),
        (
            "missing semicolon",
            "permit(principal == ?principal, action, resource)",
        ),
        (
            "unknown effect",
            "allow(principal == ?principal, action, resource);",
        ),
        (
            "unbalanced parenthesis",
            "permit(principal == ?principal, action, resource;",
        ),
        (
            "placeholder where cedar allows none",
            "permit(principal, action == ?action, resource);",
        ),
        (
            // A template is Cedar's own notion: text with no placeholder at all
            // is a STATIC policy, which the template parser refuses ("expected a
            // template, got a static policy").
            "a static policy, not a template",
            "permit(principal, action == Action::\"view\", resource);",
        ),
        (
            "condition that is not an expression",
            "permit(principal == ?principal, action, resource) when { };",
        ),
    ];

    let router = router();
    let store_id = store_with_validation_off(&router).await;

    for (case, statement) in cases {
        let (status, body) = call(
            &router,
            "CreatePolicyTemplate",
            &json!({ "policyStoreId": store_id, "statement": statement }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unparseable template statement must be refused with 400, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: unexpected error code, got {body}"
        );
    }

    // Nothing above reached state: a fresh store holds no template, so every id
    // the refused calls might have minted is absent.
    let (status, body) = call(
        &router,
        "GetPolicyTemplate",
        &json!({ "policyStoreId": store_id, "policyTemplateId": "PT000000000000000000000" }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"nothing stored\": expected 404 for a template no call created, got {status} {body}"
    );
}

/// Both template operations answer 404 for a resource that does not exist: an
/// unknown store, and an unknown template inside a store that does exist.
#[tokio::test]
async fn an_unknown_store_or_template_is_a_resource_not_found() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let statement = TEMPLATE_STATEMENTS[0].1;

    let cases: &[(&str, &str, serde_json::Value)] = &[
        (
            "CreatePolicyTemplate in an unknown store",
            "CreatePolicyTemplate",
            json!({ "policyStoreId": "PSMISSINGMISSINGMISSI", "statement": statement }),
        ),
        (
            "CreatePolicyTemplate behind an unknown alias",
            "CreatePolicyTemplate",
            json!({
                "policyStoreId": "policy-store-alias/not-an-alias",
                "statement": statement,
            }),
        ),
        (
            "GetPolicyTemplate in an unknown store",
            "GetPolicyTemplate",
            json!({
                "policyStoreId": "PSMISSINGMISSINGMISSI",
                "policyTemplateId": "PTMISSINGMISSINGMISSI",
            }),
        ),
        (
            "GetPolicyTemplate for an unknown template id",
            "GetPolicyTemplate",
            json!({ "policyStoreId": store_id, "policyTemplateId": "PTMISSINGMISSINGMISSI" }),
        ),
        (
            "GetPolicyTemplate for an unknown template name",
            "GetPolicyTemplate",
            json!({ "policyStoreId": store_id, "policyTemplateId": "name/no-such-template" }),
        ),
    ];

    for (case, operation, request) in cases {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {body}"
        );
    }
}

/// A template name is unique within its policy store, whatever the statements
/// are: the second create with a name already in use is a 409
/// `ConflictException`, and the first template is untouched.
#[tokio::test]
async fn a_duplicate_template_name_conflicts() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    // Rows differ along whether the clashing create carries the same statement
    // as the one that holds the name: the name is what conflicts, not the text.
    let cases: &[(&str, &str)] = &[
        ("same statement", TEMPLATE_STATEMENTS[0].1),
        ("different statement", TEMPLATE_STATEMENTS[2].1),
    ];

    for (index, (case, statement)) in cases.iter().enumerate() {
        let name = format!("unique-{index}");
        let first = ok(
            &router,
            "CreatePolicyTemplate",
            &json!({
                "policyStoreId": store_id,
                "name": name,
                "statement": TEMPLATE_STATEMENTS[0].1,
            }),
        )
        .await;

        let (status, body) = call(
            &router,
            "CreatePolicyTemplate",
            &json!({ "policyStoreId": store_id, "name": name, "statement": statement }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a duplicate template name must conflict, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: unexpected error code, got {body}"
        );

        // The name still resolves to the template that holds it.
        let fetched = ok(
            &router,
            "GetPolicyTemplate",
            &json!({
                "policyStoreId": store_id,
                "policyTemplateId": format!("name/{name}"),
            }),
        )
        .await;
        assert_eq!(
            fetched["policyTemplateId"], first["policyTemplateId"],
            "case {case:?}: the refused create must not have replaced the first template, \
             got {fetched}"
        );
        assert_eq!(
            fetched["statement"].as_str(),
            Some(TEMPLATE_STATEMENTS[0].1),
            "case {case:?}: the stored statement must be the first one, got {fetched}"
        );
    }

    // A request that is wrong in TWO ways is refused for the statement first: a
    // malformed request is a 400 whatever the store already holds.
    let name = "unique-0";
    let (status, body) = call(
        &router,
        "CreatePolicyTemplate",
        &json!({
            "policyStoreId": store_id,
            "name": name,
            "statement": "permit(principal == ?principal, action, resource)",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"duplicate name and broken statement\": expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"duplicate name and broken statement\": unexpected error code, got {body}"
    );
}

/// `clientToken` is an `@idempotencyToken`: a replay answers with the template
/// the first call created, and the same token with different parameters is a 409
/// `ConflictException`. The token is scoped to ONE operation.
#[tokio::test]
async fn a_client_token_replay_does_not_create_a_second_template() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    // Rows differ along WHICH parameter the retry changes.
    let changed: &[(&str, &str, serde_json::Value)] = &[
        ("statement", "statement", json!(TEMPLATE_STATEMENTS[2].1)),
        ("description", "description", json!("changed")),
        ("name", "name", json!("changed-name")),
        (
            // The ledger's fingerprint quotes every member, so a value carrying
            // its separators is still told apart from the first call's.
            "description carrying the fingerprint separators",
            "description",
            json!("first;statement=;name="),
        ),
    ];

    for (index, (case, member, value)) in changed.iter().enumerate() {
        let token = format!("token-{index}");
        let request = json!({
            "policyStoreId": store_id,
            "statement": TEMPLATE_STATEMENTS[0].1,
            "description": "first",
            "name": format!("replayed-{index}"),
            "clientToken": token,
        });

        let first = ok(&router, "CreatePolicyTemplate", &request).await;
        let replay = ok(&router, "CreatePolicyTemplate", &request).await;
        assert_eq!(
            replay["policyTemplateId"], first["policyTemplateId"],
            "case {case:?}: a replay must not create a second template, got {replay}"
        );
        assert_eq!(
            replay["createdDate"], first["createdDate"],
            "case {case:?}: a replay must report the first template's createdDate, got {replay}"
        );

        let mut retried = request.clone();
        retried[*member] = value.clone();
        let (status, body) = call(&router, "CreatePolicyTemplate", &retried).await;
        assert_eq!(
            status, 409,
            "case {case:?}: the same token with a different {member} must conflict, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: unexpected error code, got {body}"
        );

        // An idempotency token is scoped to one operation, so the same value
        // presented to CreatePolicy is a fresh token rather than a conflict.
        let policy = ok(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "clientToken": token,
                "definition": { "static": { "statement":
                    "permit(principal == User::\"erin\", action == Action::\"view\", \
                     resource == Photo::\"p\");" } },
            }),
        )
        .await;
        assert!(
            !policy["policyId"].as_str().unwrap_or_default().is_empty(),
            "case {case:?}: a token is scoped to one operation, got {policy}"
        );
    }
}

/// A STRICT store holds no schema until `PutSchema` stores one, and
/// `ValidationSettings` documents both halves of the rule: such a store "rejects
/// all static policies and policy templates because there is no schema to
/// validate against", and once a schema is there "Cedar performs validation on
/// all submitted new or updated static policies and policy templates".
#[tokio::test]
async fn a_strict_store_refuses_a_policy_template_it_cannot_validate() {
    let router = router();
    let created = ok(
        &router,
        "CreatePolicyStore",
        &json!({ "validationSettings": { "mode": "STRICT" } }),
    )
    .await;
    let store_id = created["policyStoreId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // With no schema in the store, every template row is refused.
    for (case, statement) in TEMPLATE_STATEMENTS {
        let (status, body) = call(
            &router,
            "CreatePolicyTemplate",
            &json!({ "policyStoreId": store_id, "statement": statement }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a STRICT store with no schema must refuse a template, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: unexpected error code, got {body}"
        );
    }

    // The schema's shape is the Cedar schema JSON `SchemaDefinition$cedarJson`
    // documents: an object keyed by namespace, each holding `entityTypes` and
    // `actions`, an action's `appliesTo` naming its principal and resource types.
    let schema = serde_json::to_string(&json!({
        "PhotoApp": {
            "entityTypes": {
                "User": { "shape": { "type": "Record", "attributes": {} } },
                "Photo": { "shape": { "type": "Record", "attributes": {} } },
            },
            "actions": {
                "viewPhoto": {
                    "appliesTo": {
                        "principalTypes": ["User"],
                        "resourceTypes": ["Photo"],
                    },
                },
            },
        },
    }))
    .expect("the schema document serializes");
    ok(
        &router,
        "PutSchema",
        &json!({
            "policyStoreId": store_id,
            "definition": { "cedarJson": schema },
        }),
    )
    .await;

    // Rows differ along whether the template's scope agrees with that schema:
    // the first three do, the last two name a type or an action it never
    // declares, so Cedar's STRICT validation refuses them.
    let cases: &[(&str, &str, u16)] = &[
        (
            "principal placeholder over declared types",
            "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
             resource is PhotoApp::Photo);",
            200,
        ),
        (
            "resource placeholder over declared types",
            "permit(principal is PhotoApp::User, action == PhotoApp::Action::\"viewPhoto\", \
             resource == ?resource);",
            200,
        ),
        (
            "both placeholders over declared types",
            "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
             resource == ?resource);",
            200,
        ),
        (
            "an entity type the schema never declares",
            "permit(principal == ?principal, action == PhotoApp::Action::\"viewPhoto\", \
             resource is PhotoApp::Album);",
            400,
        ),
        (
            "an action the schema never declares",
            "permit(principal == ?principal, action == PhotoApp::Action::\"deletePhoto\", \
             resource == ?resource);",
            400,
        ),
    ];

    for (index, (case, statement, expected)) in cases.iter().enumerate() {
        let (status, body) = call(
            &router,
            "CreatePolicyTemplate",
            &json!({
                "policyStoreId": store_id,
                "name": format!("strict-{index}"),
                "statement": statement,
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
                "case {case:?}: a template that fails validation must be a \
                 ValidationException, got {body}"
            );
        } else {
            // What validated is what is stored, read back through the reader.
            let template_id = body["policyTemplateId"].as_str().unwrap_or_default();
            let fetched = ok(
                &router,
                "GetPolicyTemplate",
                &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
            )
            .await;
            assert_eq!(
                fetched["statement"].as_str(),
                Some(*statement),
                "case {case:?}: the validated statement must be what was stored, got {fetched}"
            );
        }
    }
}

/// A policy template is stored in the policy store the request names — by id or
/// by alias — and is invisible in every other store.
#[tokio::test]
async fn a_template_belongs_to_exactly_one_store() {
    let router = router();
    let first = store_with_validation_off(&router).await;
    let second = store_with_validation_off(&router).await;

    let created = ok(
        &router,
        "CreatePolicyTemplate",
        &json!({
            "policyStoreId": first,
            "name": "shared-name",
            "statement": TEMPLATE_STATEMENTS[0].1,
        }),
    )
    .await;
    let template_id = created["policyTemplateId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, body) = call(
        &router,
        "GetPolicyTemplate",
        &json!({ "policyStoreId": second, "policyTemplateId": template_id }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"other store\": a template is not visible in another store, got {status} {body}"
    );

    // The same name is free in the other store, because uniqueness is per store.
    let other = ok(
        &router,
        "CreatePolicyTemplate",
        &json!({
            "policyStoreId": second,
            "name": "shared-name",
            "statement": TEMPLATE_STATEMENTS[1].1,
        }),
    )
    .await;
    assert_eq!(
        other["policyStoreId"].as_str(),
        Some(second.as_str()),
        "case \"other store\": the response must name the store it was created in, got {other}"
    );

    // Each store reports its OWN template: the two are separate resources, each
    // reachable only through the store that holds it.
    for (case, store_id, statement) in [
        ("first store", &first, TEMPLATE_STATEMENTS[0].1),
        ("second store", &second, TEMPLATE_STATEMENTS[1].1),
    ] {
        let fetched = ok(
            &router,
            "GetPolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": "name/shared-name" }),
        )
        .await;
        assert_eq!(
            fetched["statement"].as_str(),
            Some(statement),
            "case {case:?}: each store must report its own template, got {fetched}"
        );
        assert_eq!(
            fetched["policyStoreId"].as_str(),
            Some(store_id.as_str()),
            "case {case:?}: the read must name the store it came from, got {fetched}"
        );
    }
}
