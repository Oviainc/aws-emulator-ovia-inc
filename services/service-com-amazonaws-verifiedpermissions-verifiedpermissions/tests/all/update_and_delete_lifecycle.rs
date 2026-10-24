// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the MUTATING half of the identity-source and
//! policy-template lifecycles: `UpdateIdentitySource`, `DeleteIdentitySource`,
//! `UpdatePolicyTemplate` and `DeletePolicyTemplate`, each proved through the
//! reads that make its write observable.
//!
//! The rule under test, one sentence: a mutating operation must reach this
//! service's state, so every later read of the resources it touched reports the
//! POST-mutation state — an update's replacement values, a delete's absence, and
//! for a delete that documents a cascade, the absence of everything the cascade
//! removed.
//!
//! Every assertion goes over the wire through the same router the emulator
//! binary serves, reusing the harness in [`crate::harness`], so a value is only
//! visible to a later request if it really reached state.

use serde_json::{json, Value};

use crate::harness::{call, error_code, ok, router, store_with_validation_off, user_pool_arn};

/// The region every request in this suite is signed for, from the harness, so a
/// Cognito pool ARN and the emulator agree on the region.
const REGION: &str = crate::harness::SIGNING_REGION;

/// The OIDC issuer a row starts from. An issuer is a provider OUTSIDE the
/// emulator and is accepted as given (the model's `^https://.*$`).
const ISSUER: &str = "https://accounts.example.com";

/// The OIDC issuer an update switches to, so a read has to report a value the
/// create never stored.
const OTHER_ISSUER: &str = "https://login.example.org";

/// `{entityType, entityId}` of a scope member, or `None` when the response omits
/// it.
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

/// Create an identity source in `store_id` from `request` members and return its
/// id.
async fn identity_source(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    members: &Value,
) -> String {
    let mut request = members.clone();
    request["policyStoreId"] = json!(store_id);
    let created = ok(router, "CreateIdentitySource", &request).await;
    created["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string()
}

/// Create a template in `store_id` and return its id.
async fn policy_template(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    statement: &str,
) -> String {
    let created = ok(
        router,
        "CreatePolicyTemplate",
        &json!({ "policyStoreId": store_id, "statement": statement }),
    )
    .await;
    created["policyTemplateId"]
        .as_str()
        .expect("policyTemplateId is required on the CreatePolicyTemplate response")
        .to_string()
}

// ---------------------------------------------------------------------------
// UpdateIdentitySource
// ---------------------------------------------------------------------------

/// `UpdateIdentitySource` replaces the stored configuration, so
/// `GetIdentitySource` and `ListIdentitySources` report the NEW provider — every
/// member of it, the ones the replacement drops included.
///
/// The rows differ along what the replacement changes: the arm of the
/// `UpdateConfiguration` union (Cognito, OIDC), the arm of the token selection
/// (access token, identity token), a member that gains a value, one that loses
/// it, and whether `principalEntityType` is carried at all.
#[tokio::test]
async fn an_updated_identity_source_is_read_back_as_the_new_provider() {
    // (case, create members, updateConfiguration + optional principalEntityType,
    //  the `configuration` GetIdentitySource must report, the
    //  `principalEntityType` it must report)
    let pool = format!("{REGION}_1a2b3c4d5");
    let other_pool = format!("{REGION}_9z8y7x6w5");
    let arn = user_pool_arn(REGION, &pool);
    let other_arn = user_pool_arn(REGION, &other_pool);
    let cognito_issuer = format!("https://cognito-idp.{REGION}.amazonaws.com/{other_pool}");

    let cases: Vec<(&str, Value, Value, Value, &str)> = vec![
        (
            "OIDC access token: a new issuer and a new entityIdPrefix",
            json!({
                "principalEntityType": "MyCorp::User",
                "configuration": { "openIdConnectConfiguration": {
                    "issuer": ISSUER,
                    "entityIdPrefix": "ExampleOIDC",
                    "tokenSelection": { "accessTokenOnly": {
                        "principalIdClaim": "sub",
                        "audiences": ["https://api.example.com"],
                    } },
                } },
            }),
            json!({ "updateConfiguration": { "openIdConnectConfiguration": {
                "issuer": OTHER_ISSUER,
                "entityIdPrefix": "UpdatedOIDC",
                "tokenSelection": { "accessTokenOnly": {
                    "principalIdClaim": "sub",
                    "audiences": ["https://api.example.org"],
                } },
            } } }),
            json!({ "openIdConnectConfiguration": {
                "issuer": OTHER_ISSUER,
                "entityIdPrefix": "UpdatedOIDC",
                "tokenSelection": { "accessTokenOnly": {
                    "principalIdClaim": "sub",
                    "audiences": ["https://api.example.org"],
                } },
            } }),
            "MyCorp::User",
        ),
        (
            "OIDC access token: the replacement DROPS the entityIdPrefix",
            json!({
                "principalEntityType": "MyCorp::User",
                "configuration": { "openIdConnectConfiguration": {
                    "issuer": ISSUER,
                    "entityIdPrefix": "ExampleOIDC",
                    "tokenSelection": { "accessTokenOnly": {
                        "principalIdClaim": "sub",
                    } },
                } },
            }),
            json!({ "updateConfiguration": { "openIdConnectConfiguration": {
                "issuer": ISSUER,
                "tokenSelection": { "accessTokenOnly": { "principalIdClaim": "uid" } },
            } } }),
            json!({ "openIdConnectConfiguration": {
                "issuer": ISSUER,
                "tokenSelection": { "accessTokenOnly": { "principalIdClaim": "uid" } },
            } }),
            "MyCorp::User",
        ),
        (
            "OIDC: the token selection switches to identityTokenOnly",
            json!({
                "principalEntityType": "MyCorp::User",
                "configuration": { "openIdConnectConfiguration": {
                    "issuer": ISSUER,
                    "tokenSelection": { "accessTokenOnly": { "principalIdClaim": "sub" } },
                } },
            }),
            json!({
                "principalEntityType": "MyCorp::Person",
                "updateConfiguration": { "openIdConnectConfiguration": {
                    "issuer": ISSUER,
                    "groupConfiguration": {
                        "groupClaim": "groups",
                        "groupEntityType": "MyCorp::Group",
                    },
                    "tokenSelection": { "identityTokenOnly": {
                        "principalIdClaim": "sub",
                        "clientIds": ["client-1", "client-2"],
                    } },
                } },
            }),
            json!({ "openIdConnectConfiguration": {
                "issuer": ISSUER,
                "groupConfiguration": {
                    "groupClaim": "groups",
                    "groupEntityType": "MyCorp::Group",
                },
                "tokenSelection": { "identityTokenOnly": {
                    "principalIdClaim": "sub",
                    "clientIds": ["client-1", "client-2"],
                } },
            } }),
            "MyCorp::Person",
        ),
        (
            "Cognito: a new user pool, client ids and group entity type",
            json!({
                "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": arn } },
            }),
            json!({ "updateConfiguration": { "cognitoUserPoolConfiguration": {
                "userPoolArn": other_arn,
                "clientIds": ["app-1"],
                "groupConfiguration": { "groupEntityType": "MyCorp::Group" },
            } } }),
            json!({ "cognitoUserPoolConfiguration": {
                "userPoolArn": other_arn,
                "clientIds": ["app-1"],
                "issuer": cognito_issuer,
                "groupConfiguration": { "groupEntityType": "MyCorp::Group" },
            } }),
            // No `principalEntityType` on either call, so the create's default
            // for a Cognito source stands.
            "AWS::Cognito",
        ),
        (
            "the provider type itself changes: Cognito replaced by OIDC",
            json!({
                "principalEntityType": "MyCorp::User",
                "configuration": { "cognitoUserPoolConfiguration": {
                    "userPoolArn": arn,
                    "clientIds": ["app-1"],
                } },
            }),
            json!({ "updateConfiguration": { "openIdConnectConfiguration": {
                "issuer": OTHER_ISSUER,
                "entityIdPrefix": "Switched",
                "tokenSelection": { "identityTokenOnly": { "principalIdClaim": "sub" } },
            } } }),
            json!({ "openIdConnectConfiguration": {
                "issuer": OTHER_ISSUER,
                "entityIdPrefix": "Switched",
                "tokenSelection": { "identityTokenOnly": { "principalIdClaim": "sub" } },
            } }),
            "MyCorp::User",
        ),
        (
            "the provider type itself changes: OIDC replaced by Cognito",
            json!({
                "principalEntityType": "MyCorp::User",
                "configuration": { "openIdConnectConfiguration": {
                    "issuer": ISSUER,
                    "tokenSelection": { "accessTokenOnly": { "principalIdClaim": "sub" } },
                } },
            }),
            json!({ "updateConfiguration": { "cognitoUserPoolConfiguration": {
                "userPoolArn": other_arn,
            } } }),
            json!({ "cognitoUserPoolConfiguration": {
                "userPoolArn": other_arn,
                // `CognitoUserPoolConfigurationDetail$clientIds` is a list the
                // read always reports; a configuration naming none reports it
                // empty.
                "clientIds": [],
                "issuer": cognito_issuer,
            } }),
            // `principalEntityType` is absent from the update, so the value the
            // create stored is unchanged — a read requires it.
            "MyCorp::User",
        ),
    ];

    let router = router();

    for (case, create, update, expected_configuration, expected_entity_type) in &cases {
        // Each row gets its own store, so one row's sources cannot be seen by
        // the next row's list.
        let store_id = store_with_validation_off(&router).await;
        let source_id = identity_source(&router, &store_id, create).await;
        let before = ok(
            &router,
            "GetIdentitySource",
            &json!({ "policyStoreId": store_id, "identitySourceId": source_id }),
        )
        .await;

        let mut request = update.clone();
        request["policyStoreId"] = json!(store_id);
        request["identitySourceId"] = json!(source_id);
        let updated = ok(&router, "UpdateIdentitySource", &request).await;
        assert_eq!(
            updated["identitySourceId"].as_str(),
            Some(source_id.as_str()),
            "case {case:?}: the update must answer for the source it updated, got {updated}"
        );
        assert_eq!(
            updated["policyStoreId"].as_str(),
            Some(store_id.as_str()),
            "case {case:?}: the update must name the store, got {updated}"
        );
        assert_eq!(
            updated["createdDate"], before["createdDate"],
            "case {case:?}: an update must not move createdDate, got {updated}"
        );

        let fetched = ok(
            &router,
            "GetIdentitySource",
            &json!({ "policyStoreId": store_id, "identitySourceId": source_id }),
        )
        .await;
        assert_eq!(
            fetched["configuration"], *expected_configuration,
            "case {case:?}: GetIdentitySource must report the replacement configuration, \
             got {fetched}"
        );
        assert_eq!(
            fetched["principalEntityType"].as_str(),
            Some(*expected_entity_type),
            "case {case:?}: unexpected principalEntityType, got {fetched}"
        );
        assert_eq!(
            fetched["lastUpdatedDate"], updated["lastUpdatedDate"],
            "case {case:?}: the read must report the stored lastUpdatedDate, got {fetched}"
        );
        assert_ne!(
            fetched["configuration"], before["configuration"],
            "case {case:?}: the row must change the configuration it reads back, got {fetched}"
        );

        // The list reader reports the same stored source.
        let listed = ok(
            &router,
            "ListIdentitySources",
            &json!({ "policyStoreId": store_id }),
        )
        .await;
        let items = listed["identitySources"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: the store holds exactly the one source, got {listed}"
        );
        assert_eq!(
            items[0]["configuration"], *expected_configuration,
            "case {case:?}: ListIdentitySources must report the replacement configuration, \
             got {listed}"
        );
        assert_eq!(
            items[0]["principalEntityType"].as_str(),
            Some(*expected_entity_type),
            "case {case:?}: ListIdentitySources reported another principalEntityType, \
             got {listed}"
        );
    }
}

/// An `UpdateIdentitySource` naming no live resource is the modelled 404
/// `ResourceNotFoundException`, and a Cognito pool this emulator cannot resolve
/// is the modelled 400 `ValidationException` — either way the stored
/// configuration is untouched.
#[tokio::test]
async fn an_update_identity_source_that_cannot_be_applied_changes_nothing() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let create = json!({
        "principalEntityType": "MyCorp::User",
        "configuration": { "openIdConnectConfiguration": {
            "issuer": ISSUER,
            "entityIdPrefix": "ExampleOIDC",
            "tokenSelection": { "accessTokenOnly": { "principalIdClaim": "sub" } },
        } },
    });
    let source_id = identity_source(&router, &store_id, &create).await;
    let before = ok(
        &router,
        "GetIdentitySource",
        &json!({ "policyStoreId": store_id, "identitySourceId": source_id }),
    )
    .await;

    let oidc = json!({ "openIdConnectConfiguration": {
        "issuer": OTHER_ISSUER,
        "tokenSelection": { "accessTokenOnly": { "principalIdClaim": "sub" } },
    } });
    // Rows differ along WHICH reference or member cannot be resolved: the store,
    // the source, the alias, and each segment of a Cognito pool ARN.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an unknown store id",
            json!({
                "policyStoreId": "PSMISSINGMISSINGMISSI",
                "identitySourceId": source_id,
                "updateConfiguration": oidc,
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an unknown store alias",
            json!({
                "policyStoreId": "policy-store-alias/not-an-alias",
                "identitySourceId": source_id,
                "updateConfiguration": oidc,
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an unknown identity source in a store that exists",
            json!({
                "policyStoreId": store_id,
                "identitySourceId": "ISMISSINGMISSINGMISSI",
                "updateConfiguration": oidc,
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a pool ARN in another account",
            json!({
                "policyStoreId": store_id,
                "identitySourceId": source_id,
                "updateConfiguration": { "cognitoUserPoolConfiguration": {
                    "userPoolArn":
                        format!("arn:aws:cognito-idp:{REGION}:999999999999:userpool/{REGION}_pool"),
                } },
            }),
            400,
            "ValidationException",
        ),
        (
            "a pool ARN in another region",
            json!({
                "policyStoreId": store_id,
                "identitySourceId": source_id,
                "updateConfiguration": { "cognitoUserPoolConfiguration": {
                    "userPoolArn": user_pool_arn("eu-west-1", "eu-west-1_pool"),
                } },
            }),
            400,
            "ValidationException",
        ),
        (
            "an ARN that names no Cognito user pool at all",
            json!({
                "policyStoreId": store_id,
                "identitySourceId": source_id,
                "updateConfiguration": { "cognitoUserPoolConfiguration": {
                    "userPoolArn": "arn:aws:s3:::a-bucket",
                } },
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, expected_status, expected_code) in &cases {
        let (status, body) = call(&router, "UpdateIdentitySource", request).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: unexpected error code, got {body}"
        );

        let fetched = ok(
            &router,
            "GetIdentitySource",
            &json!({ "policyStoreId": store_id, "identitySourceId": source_id }),
        )
        .await;
        assert_eq!(
            fetched["configuration"], before["configuration"],
            "case {case:?}: a refused update must leave the configuration alone, got {fetched}"
        );
        assert_eq!(
            fetched["lastUpdatedDate"], before["lastUpdatedDate"],
            "case {case:?}: a refused update must not move lastUpdatedDate, got {fetched}"
        );
    }
}

// ---------------------------------------------------------------------------
// DeleteIdentitySource
// ---------------------------------------------------------------------------

/// A deleted identity source is gone from every read, and only it is: the other
/// sources in the store still list and still read back.
///
/// Rows differ along how many sources the store holds and which one of them is
/// deleted (the first, a middle one, the last, the only one).
#[tokio::test]
async fn a_deleted_identity_source_is_absent_from_every_read() {
    // (case, how many sources the store holds, the index deleted)
    let cases: &[(&str, usize, usize)] = &[
        ("the only source", 1, 0),
        ("the first of three", 3, 0),
        ("a middle one of three", 3, 1),
        ("the last of three", 3, 2),
        ("one of five", 5, 3),
    ];

    let router = router();

    for (case, count, deleted) in cases {
        let store_id = store_with_validation_off(&router).await;
        let mut ids: Vec<String> = Vec::new();
        for index in 0..*count {
            let members = json!({
                "principalEntityType": format!("MyCorp::User{index}"),
                "configuration": { "openIdConnectConfiguration": {
                    "issuer": format!("{ISSUER}/{index}"),
                    "tokenSelection": { "accessTokenOnly": { "principalIdClaim": "sub" } },
                } },
            });
            ids.push(identity_source(&router, &store_id, &members).await);
        }
        let gone = ids[*deleted].clone();

        let (status, body) = call(
            &router,
            "DeleteIdentitySource",
            &json!({ "policyStoreId": store_id, "identitySourceId": gone }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a delete of a source that is there must succeed, got {status} {body}"
        );

        let (status, body) = call(
            &router,
            "GetIdentitySource",
            &json!({ "policyStoreId": store_id, "identitySourceId": gone }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: GetIdentitySource must answer 404 for a deleted source, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(gone.as_str()),
            "case {case:?}: the 404 must name the missing resource, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("IDENTITY_SOURCE"),
            "case {case:?}: the 404 must name the resource type, got {body}"
        );

        let listed = ok(
            &router,
            "ListIdentitySources",
            &json!({ "policyStoreId": store_id }),
        )
        .await;
        let remaining: Vec<String> = listed["identitySources"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|item| item["identitySourceId"].as_str().map(str::to_string))
            .collect();
        let mut expected: Vec<String> = ids.clone();
        expected.remove(*deleted);
        expected.sort();
        let mut actual = remaining.clone();
        actual.sort();
        assert_eq!(
            actual, expected,
            "case {case:?}: ListIdentitySources must report exactly the sources that \
             remain, got {remaining:?}"
        );

        // Every remaining source still reads back on its own.
        for id in &expected {
            let fetched = ok(
                &router,
                "GetIdentitySource",
                &json!({ "policyStoreId": store_id, "identitySourceId": id }),
            )
            .await;
            assert_eq!(
                fetched["identitySourceId"].as_str(),
                Some(id.as_str()),
                "case {case:?}: a source that was not deleted must still read back, \
                 got {fetched}"
            );
        }

        // The same delete again names a resource that is no longer there.
        let (status, body) = call(
            &router,
            "DeleteIdentitySource",
            &json!({ "policyStoreId": store_id, "identitySourceId": gone }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a second delete names no live source, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {body}"
        );
    }

    // A delete in a store that does not exist is a 404 for the STORE.
    let (status, body) = call(
        &router,
        "DeleteIdentitySource",
        &json!({
            "policyStoreId": "PSMISSINGMISSINGMISSI",
            "identitySourceId": "ISMISSINGMISSINGMISSI",
        }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"unknown store\": expected 404, got {status} {body}"
    );
    assert_eq!(
        body["resourceType"].as_str(),
        Some("POLICY_STORE"),
        "case \"unknown store\": the 404 must name the store, got {body}"
    );
}

// ---------------------------------------------------------------------------
// UpdatePolicyTemplate
// ---------------------------------------------------------------------------

/// One row of the replacement table below: the template as created, the
/// statement that replaces it, and the actions a policy linked to it must report
/// once the replacement is stored.
struct Replacement {
    case: &'static str,
    created_statement: &'static str,
    replacement: &'static str,
    expected_actions: &'static [(&'static str, &'static str)],
}

/// Replacements that change only what `UpdatePolicyTemplateInput$statement`
/// allows to change — "The action referenced by the policy template. Any
/// conditional clauses, such as when or unless clauses" — across the placeholder
/// shapes a template's scope can take and both effects.
const REPLACEMENTS: &[Replacement] = &[
    Replacement {
        case: "the action the template references changes",
        created_statement:
            "permit(principal == ?principal, action == Action::\"view\", resource == ?resource);",
        replacement:
            "permit(principal == ?principal, action == Action::\"edit\", resource == ?resource);",
        expected_actions: &[("Action", "edit")],
    },
    Replacement {
        case: "one action becomes two",
        created_statement:
            "permit(principal == ?principal, action == Action::\"view\", resource == ?resource);",
        replacement: "permit(principal == ?principal, action in [Action::\"view\", \
                      Action::\"edit\"], resource == ?resource);",
        expected_actions: &[("Action", "edit"), ("Action", "view")],
    },
    Replacement {
        case: "the action constraint is dropped altogether",
        created_statement:
            "permit(principal == ?principal, action == Action::\"view\", resource == ?resource);",
        replacement: "permit(principal == ?principal, action, resource == ?resource);",
        expected_actions: &[],
    },
    Replacement {
        case: "a condition clause is added",
        created_statement:
            "permit(principal == ?principal, action == Action::\"view\", resource == ?resource);",
        replacement: "permit(principal == ?principal, action == Action::\"view\", \
                      resource == ?resource) when { context.mfa == true };",
        expected_actions: &[("Action", "view")],
    },
    Replacement {
        case: "only the resource placeholder, and the action changes",
        created_statement: "permit(principal == User::\"erin\", action == Action::\"view\", \
                            resource == ?resource);",
        replacement: "permit(principal == User::\"erin\", action == Action::\"edit\", \
                      resource == ?resource);",
        expected_actions: &[("Action", "edit")],
    },
    Replacement {
        case: "a forbid template under `in`, re-scoped to another action",
        created_statement: "forbid(principal in ?principal, action == Action::\"view\", resource);",
        replacement: "forbid(principal in ?principal, action == Action::\"delete\", resource);",
        expected_actions: &[("Action", "delete")],
    },
];

/// `UpdatePolicyTemplate` replaces the template's statement, so
/// `GetPolicyTemplate` reports the REPLACEMENT — and so
/// does every policy linked to it, in the scope a reader reports for it and in
/// the authorization decisions it takes part in.
///
/// Rows differ along what the replacement changes within the elements the
/// operation allows to change ("The action referenced by the policy template.
/// Any conditional clauses, such as when or unless clauses") and along the
/// placeholder shape of the template's scope.
#[tokio::test]
async fn an_updated_policy_template_is_read_back_and_followed_by_its_linked_policies() {
    let cases: &[Replacement] = REPLACEMENTS;

    let router = router();

    for (
        index,
        Replacement {
            case,
            created_statement,
            replacement,
            expected_actions,
        },
    ) in cases.iter().enumerate()
    {
        let store_id = store_with_validation_off(&router).await;
        let template_id = policy_template(&router, &store_id, created_statement).await;
        let created = ok(
            &router,
            "GetPolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
        )
        .await;

        // A policy instantiated from the template, binding exactly the
        // placeholders the created statement uses — which the replacement keeps,
        // because the principal and the resource a template references are the
        // elements it may not change.
        let mut linked = json!({ "policyTemplateId": template_id });
        if created_statement.contains("?principal") {
            linked["principal"] = json!({ "entityType": "User", "entityId": "alice" });
        }
        if created_statement.contains("?resource") {
            linked["resource"] = json!({ "entityType": "Photo", "entityId": "vacation" });
        }
        let policy = ok(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "templateLinked": linked },
            }),
        )
        .await;
        let policy_id = policy["policyId"].as_str().unwrap_or_default().to_string();

        let updated = ok(
            &router,
            "UpdatePolicyTemplate",
            &json!({
                "policyStoreId": store_id,
                "policyTemplateId": template_id,
                "name": format!("renamed-{index}"),
                "description": "the replacement",
                "statement": replacement,
            }),
        )
        .await;
        assert_eq!(
            updated["policyTemplateId"].as_str(),
            Some(template_id.as_str()),
            "case {case:?}: the update must answer for the template it updated, got {updated}"
        );
        assert_eq!(
            updated["createdDate"], created["createdDate"],
            "case {case:?}: an update must not move createdDate, got {updated}"
        );

        let fetched = ok(
            &router,
            "GetPolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
        )
        .await;
        assert_eq!(
            fetched["statement"].as_str(),
            Some(*replacement),
            "case {case:?}: GetPolicyTemplate must report the replacement statement, \
             got {fetched}"
        );
        assert_eq!(
            fetched["name"].as_str(),
            Some(format!("renamed-{index}").as_str()),
            "case {case:?}: the update must store the new name, got {fetched}"
        );
        assert_eq!(
            fetched["description"].as_str(),
            Some("the replacement"),
            "case {case:?}: the update must store the new description, got {fetched}"
        );
        assert_eq!(
            fetched["lastUpdatedDate"], updated["lastUpdatedDate"],
            "case {case:?}: the read must report the stored lastUpdatedDate, got {fetched}"
        );

        // The new name resolves to the same template, so the rename reached
        // state too.
        let by_name = ok(
            &router,
            "GetPolicyTemplate",
            &json!({
                "policyStoreId": store_id,
                "policyTemplateId": format!("name/renamed-{index}"),
            }),
        )
        .await;
        assert_eq!(
            by_name["policyTemplateId"].as_str(),
            Some(template_id.as_str()),
            "case {case:?}: name/renamed-{index} must resolve to the template, got {by_name}"
        );

        // The linked policy follows the template: its reported scope is the
        // scope of the REPLACEMENT linked to its bound entities.
        let linked_policy = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        let expected: Vec<(String, String)> = {
            let mut pairs: Vec<(String, String)> = expected_actions
                .iter()
                .map(|(t, i)| ((*t).to_string(), (*i).to_string()))
                .collect();
            pairs.sort();
            pairs
        };
        assert_eq!(
            actions(&linked_policy["actions"]),
            expected,
            "case {case:?}: a linked policy must report the replacement's actions, \
             got {linked_policy}"
        );
        assert_eq!(
            entity(&linked_policy["principal"]),
            entity(&policy["principal"]),
            "case {case:?}: the principal a linked policy reports cannot change, \
             got {linked_policy}"
        );
        assert_eq!(
            entity(&linked_policy["resource"]),
            entity(&policy["resource"]),
            "case {case:?}: the resource a linked policy reports cannot change, \
             got {linked_policy}"
        );
    }
}

/// The authorization decision a template-linked policy takes part in follows the
/// template's CURRENT text: the action the template referenced before the update
/// no longer matches, and the one it references now does.
#[tokio::test]
async fn an_updated_policy_template_changes_the_decisions_of_its_linked_policies() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let template_id = policy_template(
        &router,
        &store_id,
        "permit(principal == ?principal, action == Action::\"view\", resource == ?resource);",
    )
    .await;
    ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "templateLinked": {
                "policyTemplateId": template_id,
                "principal": { "entityType": "User", "entityId": "alice" },
                "resource": { "entityType": "Photo", "entityId": "vacation" },
            } },
        }),
    )
    .await;

    let request_for = |action: &str| {
        json!({
            "policyStoreId": store_id,
            "principal": { "entityType": "User", "entityId": "alice" },
            "action": { "actionType": "Action", "actionId": action },
            "resource": { "entityType": "Photo", "entityId": "vacation" },
        })
    };

    // (case, the template's statement, the decision for `view`, for `edit`)
    let stages: &[(&str, &str, &str, &str)] = &[
        (
            "as created, the template permits view",
            "permit(principal == ?principal, action == Action::\"view\", resource == ?resource);",
            "ALLOW",
            "DENY",
        ),
        (
            "re-scoped to edit, the same link permits edit instead",
            "permit(principal == ?principal, action == Action::\"edit\", resource == ?resource);",
            "DENY",
            "ALLOW",
        ),
        (
            "widened to both actions, the link permits each",
            "permit(principal == ?principal, action in [Action::\"view\", Action::\"edit\"], \
             resource == ?resource);",
            "ALLOW",
            "ALLOW",
        ),
    ];

    for (case, statement, view, edit) in stages {
        // The first row is the statement the template was created with, so the
        // update is a no-op replacement there and a real one afterwards.
        ok(
            &router,
            "UpdatePolicyTemplate",
            &json!({
                "policyStoreId": store_id,
                "policyTemplateId": template_id,
                "statement": statement,
            }),
        )
        .await;

        for (action, expected) in [("view", view), ("edit", edit)] {
            let answer = ok(&router, "IsAuthorized", &request_for(action)).await;
            assert_eq!(
                answer["decision"].as_str(),
                Some(*expected),
                "case {case:?}: expected {expected} for action {action:?}, got {answer}"
            );
        }
    }
}

/// A replacement that would change an element the operation documents as
/// immutable — "The effect (permit or forbid) of the policy template. The
/// principal referenced by the policy template. The resource referenced by the
/// policy template." — is refused with the modelled 400 `ValidationException`,
/// and nothing is stored. Text the Cedar template parser refuses, and a
/// reference naming no live resource, are refused too.
#[tokio::test]
async fn a_policy_template_update_that_is_refused_changes_nothing() {
    let created_statement =
        "permit(principal == ?principal, action == Action::\"view\", resource == ?resource);";

    // (case, the replacement statement, the HTTP status, the error code)
    let cases: &[(&str, &str, u16, &str)] = &[
        (
            // Accepted: the action is an element the operation allows to change.
            "the action changes",
            "permit(principal == ?principal, action == Action::\"edit\", resource == ?resource);",
            200,
            "",
        ),
        (
            "the effect changes from permit to forbid",
            "forbid(principal == ?principal, action == Action::\"view\", resource == ?resource);",
            400,
            "ValidationException",
        ),
        (
            "the ?principal placeholder is replaced by a concrete principal",
            "permit(principal == User::\"erin\", action == Action::\"view\", \
             resource == ?resource);",
            400,
            "ValidationException",
        ),
        (
            "the ?resource placeholder is replaced by a concrete resource",
            "permit(principal == ?principal, action == Action::\"view\", \
             resource == Photo::\"p\");",
            400,
            "ValidationException",
        ),
        (
            "the principal constraint is dropped",
            "permit(principal, action == Action::\"view\", resource == ?resource);",
            400,
            "ValidationException",
        ),
        (
            "the resource constraint is dropped",
            "permit(principal == ?principal, action == Action::\"view\", resource);",
            400,
            "ValidationException",
        ),
        (
            "the replacement is not valid Cedar",
            "permit(principal == ?principal, action, resource",
            400,
            "ValidationException",
        ),
        (
            "the replacement is a static policy, not a template",
            "permit(principal, action == Action::\"view\", resource);",
            400,
            "ValidationException",
        ),
    ];

    let router = router();

    for (case, replacement, expected_status, expected_code) in cases {
        let store_id = store_with_validation_off(&router).await;
        let template_id = policy_template(&router, &store_id, created_statement).await;
        let before = ok(
            &router,
            "GetPolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
        )
        .await;

        let (status, body) = call(
            &router,
            "UpdatePolicyTemplate",
            &json!({
                "policyStoreId": store_id,
                "policyTemplateId": template_id,
                "statement": replacement,
            }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );

        let fetched = ok(
            &router,
            "GetPolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
        )
        .await;
        if *expected_status == 200 {
            assert_eq!(
                fetched["statement"].as_str(),
                Some(*replacement),
                "case {case:?}: an accepted replacement must be what is stored, got {fetched}"
            );
            continue;
        }
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: unexpected error code, got {body}"
        );
        let fields = body["fieldList"].as_array().cloned().unwrap_or_default();
        assert!(
            !fields.is_empty() && fields.iter().all(|f| f["path"].as_str() == Some("/statement")),
            "case {case:?}: every refusal must name /statement in fieldList, got {body}"
        );
        assert_eq!(
            fetched["statement"].as_str(),
            Some(created_statement),
            "case {case:?}: a refused update must leave the statement alone, got {fetched}"
        );
        assert_eq!(
            fetched["lastUpdatedDate"], before["lastUpdatedDate"],
            "case {case:?}: a refused update must not move lastUpdatedDate, got {fetched}"
        );
    }

    // A reference naming no live resource is the modelled 404, whichever member
    // carries it.
    let store_id = store_with_validation_off(&router).await;
    let template_id = policy_template(&router, &store_id, created_statement).await;
    let missing: &[(&str, Value)] = &[
        (
            "an unknown store id",
            json!({
                "policyStoreId": "PSMISSINGMISSINGMISSI",
                "policyTemplateId": template_id,
                "statement": created_statement,
            }),
        ),
        (
            "an unknown template id",
            json!({
                "policyStoreId": store_id,
                "policyTemplateId": "PTMISSINGMISSINGMISSI",
                "statement": created_statement,
            }),
        ),
        (
            "an unknown template name",
            json!({
                "policyStoreId": store_id,
                "policyTemplateId": "name/no-such-template",
                "statement": created_statement,
            }),
        ),
    ];
    for (case, request) in missing {
        let (status, body) = call(&router, "UpdatePolicyTemplate", request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {body}"
        );
    }
}

/// A template name is unique within its store, so an update taking a name
/// another template already holds is the modelled 409 `ConflictException` —
/// while an update repeating the template's OWN name is accepted, and an empty
/// name removes it ("To remove a name, set it to an empty string").
#[tokio::test]
async fn a_policy_template_update_follows_the_documented_name_rules() {
    let statement =
        "permit(principal == ?principal, action == Action::\"view\", resource == ?resource);";
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    let first = ok(
        &router,
        "CreatePolicyTemplate",
        &json!({ "policyStoreId": store_id, "name": "taken", "statement": statement }),
    )
    .await;
    let first_id = first["policyTemplateId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let second_id = policy_template(&router, &store_id, statement).await;

    // (case, the name the update asks for, the status, the name a read reports)
    let cases: &[(&str, Option<&str>, u16, Option<&str>)] = &[
        ("a name no other template holds", Some("free"), 200, Some("free")),
        ("the template's own name again", Some("free"), 200, Some("free")),
        ("a name another template holds", Some("taken"), 409, Some("free")),
        ("an empty name removes it", Some(""), 200, None),
        ("no name member leaves it as it is", None, 200, None),
    ];

    for (case, name, expected_status, expected_name) in cases {
        let mut request = json!({
            "policyStoreId": store_id,
            "policyTemplateId": second_id,
            "statement": statement,
        });
        if let Some(name) = name {
            request["name"] = json!(name);
        }
        let (status, body) = call(&router, "UpdatePolicyTemplate", &request).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if *expected_status == 409 {
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {case:?}: unexpected error code, got {body}"
            );
        }

        let fetched = ok(
            &router,
            "GetPolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": second_id }),
        )
        .await;
        assert_eq!(
            fetched["name"].as_str(),
            *expected_name,
            "case {case:?}: unexpected stored name, got {fetched}"
        );
    }

    // The template that held the contested name still holds it.
    let held = ok(
        &router,
        "GetPolicyTemplate",
        &json!({ "policyStoreId": store_id, "policyTemplateId": "name/taken" }),
    )
    .await;
    assert_eq!(
        held["policyTemplateId"].as_str(),
        Some(first_id.as_str()),
        "case \"the contested name\": it must still resolve to the first template, got {held}"
    );
}

/// `description` is not `name`: the model documents an "unchanged unless
/// included" rule for `name` only, so the description a read reports FOLLOWS the
/// request — the value it carries, or none at all when it carries none, exactly
/// as `UpdatePolicyStore` treats the member of the same name.
///
/// Rows differ along what the request carries for `description`: a first value,
/// a replacement value, the same value again, the empty string the member's
/// `length 0..150` admits, and no member at all.
#[tokio::test]
async fn a_policy_template_update_stores_the_description_the_request_carries() {
    let statement =
        "permit(principal == ?principal, action == Action::\"view\", resource == ?resource);";
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let template_id = ok(
        &router,
        "CreatePolicyTemplate",
        &json!({
            "policyStoreId": store_id,
            "statement": statement,
            "description": "as created",
        }),
    )
    .await["policyTemplateId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, the description the update carries, the description a read reports)
    let cases: &[(&str, Option<&str>, Option<&str>)] = &[
        ("a first replacement value", Some("first"), Some("first")),
        ("another value replaces it", Some("second"), Some("second")),
        ("the same value again", Some("second"), Some("second")),
        ("the empty string the model admits", Some(""), Some("")),
        ("no description member at all", None, None),
    ];

    for (case, description, expected) in cases {
        let mut request = json!({
            "policyStoreId": store_id,
            "policyTemplateId": template_id,
            "statement": statement,
        });
        if let Some(description) = description {
            request["description"] = json!(description);
        }
        ok(&router, "UpdatePolicyTemplate", &request).await;

        let fetched = ok(
            &router,
            "GetPolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
        )
        .await;
        assert_eq!(
            fetched["description"].as_str(),
            *expected,
            "case {case:?}: unexpected stored description, got {fetched}"
        );
    }
}

// ---------------------------------------------------------------------------
// DeletePolicyTemplate
// ---------------------------------------------------------------------------

/// Deleting a policy template deletes the policies created from it: "This
/// operation also deletes any policies that were created from the specified
/// policy template. Those policies are immediately removed from all future API
/// responses." Everything else in the store survives.
///
/// Rows differ along how many policies are linked to the deleted template, and
/// each row's store also holds a static policy and a second template with its
/// own link, so the cascade is shown to select exactly the links of the template
/// that was deleted.
#[tokio::test]
async fn deleting_a_policy_template_deletes_the_policies_linked_to_it() {
    let template_statement =
        "permit(principal == ?principal, action == Action::\"view\", resource == ?resource);";
    let other_template_statement =
        "permit(principal == ?principal, action == Action::\"list\", resource == Album::\"all\");";
    let static_statement =
        "permit(principal == User::\"zoe\", action == Action::\"view\", resource == Photo::\"z\");";

    // (case, how many policies link to the template that is deleted)
    let cases: &[(&str, usize)] = &[
        ("no linked policy", 0),
        ("one linked policy", 1),
        ("three linked policies", 3),
    ];

    let router = router();

    for (case, linked_count) in cases {
        let store_id = store_with_validation_off(&router).await;
        let template_id = policy_template(&router, &store_id, template_statement).await;
        let other_template_id =
            policy_template(&router, &store_id, other_template_statement).await;

        // The policies that follow the template under test.
        let mut cascaded: Vec<String> = Vec::new();
        for index in 0..*linked_count {
            let created = ok(
                &router,
                "CreatePolicy",
                &json!({
                    "policyStoreId": store_id,
                    "definition": { "templateLinked": {
                        "policyTemplateId": template_id,
                        "principal": { "entityType": "User", "entityId": format!("u{index}") },
                        "resource": { "entityType": "Photo", "entityId": format!("p{index}") },
                    } },
                }),
            )
            .await;
            cascaded.push(created["policyId"].as_str().unwrap_or_default().to_string());
        }

        // A policy that follows the OTHER template, and a static one: neither
        // may be touched.
        let survivor = ok(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "templateLinked": {
                    "policyTemplateId": other_template_id,
                    "principal": { "entityType": "User", "entityId": "keep" },
                } },
            }),
        )
        .await;
        let survivor_id = survivor["policyId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let static_policy = ok(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": static_statement } },
            }),
        )
        .await;
        let static_id = static_policy["policyId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, body) = call(
            &router,
            "DeletePolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting a template that is there must succeed, got {status} {body}"
        );

        // The template itself is gone.
        let (status, body) = call(
            &router,
            "GetPolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: GetPolicyTemplate must answer 404 for a deleted template, \
             got {status} {body}"
        );

        // Every policy created from it is gone, with the identity of what is
        // missing in the members that model it.
        for policy_id in &cascaded {
            let (status, body) = call(
                &router,
                "GetPolicy",
                &json!({ "policyStoreId": store_id, "policyId": policy_id }),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: GetPolicy must answer 404 for a cascaded policy, \
                 got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: unexpected error code, got {body}"
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(policy_id.as_str()),
                "case {case:?}: the 404 must name the policy that is gone, got {body}"
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some("POLICY"),
                "case {case:?}: the 404 must name resourceType POLICY, got {body}"
            );
        }

        // Exactly the survivors list.
        let listed = ok(&router, "ListPolicies", &json!({ "policyStoreId": store_id })).await;
        let mut remaining: Vec<String> = listed["policies"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|item| item["policyId"].as_str().map(str::to_string))
            .collect();
        remaining.sort();
        let mut expected = vec![survivor_id.clone(), static_id.clone()];
        expected.sort();
        assert_eq!(
            remaining, expected,
            "case {case:?}: only the policies that do not follow the deleted template may \
             remain, got {listed}"
        );

        // The surviving link still reads back through its own template.
        let fetched = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": survivor_id }),
        )
        .await;
        assert_eq!(
            fetched["definition"]["templateLinked"]["policyTemplateId"].as_str(),
            Some(other_template_id.as_str()),
            "case {case:?}: the surviving link must still name its template, got {fetched}"
        );

        // A second delete names a template that is no longer there.
        let (status, body) = call(
            &router,
            "DeletePolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a second delete names no live template, got {status} {body}"
        );
    }

    // A delete naming no live resource is the modelled 404, whichever member
    // carries the reference.
    let store_id = store_with_validation_off(&router).await;
    let missing: &[(&str, Value, &str)] = &[
        (
            "an unknown store id",
            json!({
                "policyStoreId": "PSMISSINGMISSINGMISSI",
                "policyTemplateId": "PTMISSINGMISSINGMISSI",
            }),
            "POLICY_STORE",
        ),
        (
            "an unknown template id",
            json!({ "policyStoreId": store_id, "policyTemplateId": "PTMISSINGMISSINGMISSI" }),
            "POLICY_TEMPLATE",
        ),
        (
            "an unknown template name",
            json!({ "policyStoreId": store_id, "policyTemplateId": "name/no-such-template" }),
            "POLICY_TEMPLATE",
        ),
    ];
    for (case, request, resource_type) in missing {
        let (status, body) = call(&router, "DeletePolicyTemplate", request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some(*resource_type),
            "case {case:?}: the 404 must name the resource type, got {body}"
        );
    }
}

/// A grant made through a template vanishes with the template: after the delete,
/// the authorization the linked policy allowed is denied, because the policy it
/// came from is no longer in the store.
#[tokio::test]
async fn deleting_a_policy_template_revokes_the_decisions_its_links_allowed() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let template_id = policy_template(
        &router,
        &store_id,
        "permit(principal == ?principal, action == Action::\"view\", resource == ?resource);",
    )
    .await;
    ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "templateLinked": {
                "policyTemplateId": template_id,
                "principal": { "entityType": "User", "entityId": "alice" },
                "resource": { "entityType": "Photo", "entityId": "vacation" },
            } },
        }),
    )
    .await;
    let request = json!({
        "policyStoreId": store_id,
        "principal": { "entityType": "User", "entityId": "alice" },
        "action": { "actionType": "Action", "actionId": "view" },
        "resource": { "entityType": "Photo", "entityId": "vacation" },
    });

    let before = ok(&router, "IsAuthorized", &request).await;
    assert_eq!(
        before["decision"].as_str(),
        Some("ALLOW"),
        "case \"before the delete\": the link must allow the request, got {before}"
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
        "case \"after the delete\": the grant must be gone, got {after}"
    );
    assert_eq!(
        after["errors"].as_array().map(Vec::len),
        Some(0),
        "case \"after the delete\": a cascaded policy must be GONE, not a dangling link \
         reported as an evaluation error, got {after}"
    );
}
