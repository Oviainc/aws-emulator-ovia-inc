// SPDX-License-Identifier: CC0-1.0

//! The group entity type of an Amazon Cognito user pool identity source.
//!
//! The rule under test, one sentence: the Cedar entity type a Cognito user pool
//! identity source maps its groups to is decided ONCE, by the write — the type
//! the request's `groupConfiguration` named, or the service's own default where
//! the request named none — and persisted, so every read of the source reports
//! that type and an authorization call against the source maps the token's
//! groups to the very same type.
//!
//! The default itself is the one `CognitoGroupConfigurationDetail$groupEntityType`
//! documents ("Defaults to `AWS::CognitoGroup`"), published by this crate as
//! [`DEFAULT_COGNITO_GROUP_ENTITY_TYPE`] so the suite follows the constant
//! rather than a copy of its value.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so
//! a reported type is only there if the write really reached state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    self, call, error_code, field_list, ok, store_with_validation_off, user_pool_arn,
    SIGNING_REGION,
};
use super::is_authorized_with_token::{
    app_client, group_permit, identity_source, policy, sign_in, DOCUMENT, PERMITTED_GROUP,
};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::state::DEFAULT_COGNITO_GROUP_ENTITY_TYPE;

/// The longest and shortest `groupEntityType` the model admits: `GroupEntityType`
/// is `length 1..200` with pattern
/// `^([_a-zA-Z][_a-zA-Z0-9]*::)*[_a-zA-Z][_a-zA-Z0-9]*$`
/// (`smithy_query(query="shape", shape="CognitoGroupConfiguration")`), so one
/// leading letter followed by letters is a value of either length.
fn entity_type_of_length(length: usize) -> String {
    format!("G{}", "r".repeat(length - 1))
}

/// The `cognitoUserPoolConfiguration` a write carries for `pool`, naming
/// `group_entity_type` when one is given.
fn provider(pool: &str, group_entity_type: Option<&str>) -> Value {
    let mut cognito = json!({ "userPoolArn": user_pool_arn(SIGNING_REGION, pool) });
    if let Some(entity_type) = group_entity_type {
        cognito["groupConfiguration"] = json!({ "groupEntityType": entity_type });
    }
    json!({ "cognitoUserPoolConfiguration": cognito })
}

/// Create an identity source carrying `provider` and return its id.
async fn created_with(router: &MultiServiceRouter, store_id: &str, provider: &Value) -> String {
    ok(
        router,
        "CreateIdentitySource",
        &json!({ "policyStoreId": store_id, "configuration": provider }),
    )
    .await["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string()
}

/// Create an identity source on `seed` and UPDATE it to `provider`, then return
/// its id — so one provider description is written by both writers.
async fn updated_to(
    router: &MultiServiceRouter,
    store_id: &str,
    seed: &Value,
    provider: &Value,
) -> String {
    let id = created_with(router, store_id, seed).await;
    ok(
        router,
        "UpdateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "identitySourceId": id,
            "updateConfiguration": provider,
        }),
    )
    .await;
    id
}

/// `GetIdentitySource`'s view of one source.
async fn read(router: &MultiServiceRouter, store_id: &str, id: &str) -> Value {
    ok(
        router,
        "GetIdentitySource",
        &json!({ "policyStoreId": store_id, "identitySourceId": id }),
    )
    .await
}

/// `ListIdentitySources`'s view of one source, found by its id.
async fn listed(router: &MultiServiceRouter, store_id: &str, id: &str) -> Value {
    ok(
        router,
        "ListIdentitySources",
        &json!({ "policyStoreId": store_id }),
    )
    .await["identitySources"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|item| item["identitySourceId"] == json!(id))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `groupConfiguration` a read reports inside `cognitoUserPoolConfiguration`.
fn reported_group_configuration(source: &Value) -> &Value {
    &source["configuration"]["cognitoUserPoolConfiguration"]["groupConfiguration"]
}

/// RULE: every read of a Cognito user pool identity source reports the group
/// entity type the write resolved — the one the request named, or the service's
/// documented default where it named none — whichever writer carried it and
/// whichever read reports it.
///
/// The rows differ along the `groupEntityType` the write carries: absent, the
/// default named outright, a namespaced type, a type with several namespace
/// segments, and the shortest and longest value the modelled member admits.
#[tokio::test]
async fn every_read_reports_the_group_entity_type_the_write_resolved() {
    let longest = entity_type_of_length(200);
    // (case, the groupEntityType the write carries, the type every read reports)
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        (
            "no group configuration at all",
            None,
            DEFAULT_COGNITO_GROUP_ENTITY_TYPE,
        ),
        (
            "the service default named outright",
            Some(DEFAULT_COGNITO_GROUP_ENTITY_TYPE),
            DEFAULT_COGNITO_GROUP_ENTITY_TYPE,
        ),
        (
            "a namespaced type of the caller's own schema",
            Some("MyCorp::Group"),
            "MyCorp::Group",
        ),
        (
            "a type with several namespace segments",
            Some("MyCorp::Directory::UserGroup"),
            "MyCorp::Directory::UserGroup",
        ),
        (
            // `GroupEntityType` is `length 1..200`: one character is the
            // shortest value the member admits.
            "the shortest type the member admits",
            Some("G"),
            "G",
        ),
        (
            "the longest type the member admits",
            Some(longest.as_str()),
            longest.as_str(),
        ),
    ];

    let router = harness::router();
    let pool = harness::user_pool(&router, "group-type-pool").await;
    // The provider an update's source is seeded with before the row replaces it:
    // the same pool with no group configuration, so the row's own write is the
    // only thing that decides the type it ends up reporting.
    let seed = provider(&pool, None);

    for (case, requested, expected) in rows {
        // One store per row, so a row's list holds only that row's sources.
        let store_id = store_with_validation_off(&router).await;
        let written = provider(&pool, requested);
        let created = created_with(&router, &store_id, &written).await;
        let updated = updated_to(&router, &store_id, &seed, &written).await;

        let expected_group_configuration = json!({ "groupEntityType": expected });
        let written_by = [
            ("CreateIdentitySource", &created),
            ("UpdateIdentitySource", &updated),
        ];
        for (writer, id) in written_by {
            let got = read(&router, &store_id, id).await;
            assert_eq!(
                reported_group_configuration(&got),
                &expected_group_configuration,
                "case {case:?}: expected GetIdentitySource to report {expected_group_configuration} \
                 for a source written by {writer}, got {got}"
            );
            let item = listed(&router, &store_id, id).await;
            assert_eq!(
                reported_group_configuration(&item),
                &expected_group_configuration,
                "case {case:?}: expected ListIdentitySources to report \
                 {expected_group_configuration} for a source written by {writer}, got {item}"
            );
        }
    }
}

/// RULE: a write that names no `groupConfiguration` and a write that names the
/// service's default leave the SAME source, because the default is resolved
/// before anything is stored — so the two sources are member-for-member alike
/// but for their own identity and timestamps.
#[tokio::test]
async fn naming_the_default_and_naming_nothing_store_one_source() {
    let router = harness::router();
    let pool = harness::user_pool(&router, "default-equivalence-pool").await;
    let store_id = store_with_validation_off(&router).await;

    let absent = created_with(&router, &store_id, &provider(&pool, None)).await;
    let named = created_with(
        &router,
        &store_id,
        &provider(&pool, Some(DEFAULT_COGNITO_GROUP_ENTITY_TYPE)),
    )
    .await;

    let from_absent = read(&router, &store_id, &absent).await;
    let from_named = read(&router, &store_id, &named).await;
    assert_eq!(
        from_absent["configuration"], from_named["configuration"],
        "expected a write naming no groupConfiguration to store the configuration a write \
         naming {DEFAULT_COGNITO_GROUP_ENTITY_TYPE} stores, got {:?} and {:?}",
        from_absent["configuration"], from_named["configuration"]
    );
}

/// One `IsAuthorizedWithToken` request against `store`, carrying `identityToken`
/// and whatever `entities` the row supplies.
fn authorization_request(store: &str, token: &str, entities: Option<Value>) -> Value {
    let mut body = json!({
        "policyStoreId": store,
        "identityToken": token,
        "action": { "actionType": "App::Action", "actionId": "read" },
        "resource": { "entityType": "App::Doc", "entityId": DOCUMENT },
    });
    if let Some(entities) = entities {
        body["entities"] = entities;
    }
    body
}

/// RULE: the group entity type a read reports is the one the authorization path
/// uses, for the same stored source — a token's group claim becomes a parent
/// entity of that type, and an `entities` slice supplying an entity of that type
/// is refused as a type the identity source generates. The reads and the
/// decision cannot disagree, because both take the type out of state.
///
/// The rows differ along what the write named: nothing (so the service's default
/// stands) and a type of the caller's own namespace.
#[tokio::test]
async fn the_reported_group_entity_type_is_the_one_authorization_maps_groups_to() {
    let (router, _engine) = harness::router_with_engine();
    let pool = harness::user_pool(&router, "group-authz-pool").await;
    let client = app_client(&router, &pool, "group-authz-client").await;
    let member = sign_in(
        &router,
        &pool,
        &client,
        "member",
        &[PERMITTED_GROUP],
        &json!([]),
    )
    .await;

    // (case, the groupEntityType the write carries)
    let rows: Vec<(&str, Option<&str>)> = vec![
        ("no group configuration at all", None),
        (
            "a namespaced type of the caller's own schema",
            Some("MyCorp::Group"),
        ),
    ];

    for (case, requested) in rows {
        let store_id = store_with_validation_off(&router).await;
        let source_id = identity_source(&router, &store_id, &pool, &[&client], requested).await;

        // What the READ says this source maps its groups to. Everything below is
        // driven by this value, so the assertions compare the two paths rather
        // than each path against a literal.
        let got = read(&router, &store_id, &source_id).await;
        let reported = reported_group_configuration(&got)["groupEntityType"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a read to report a group entity type, got {got}")
            })
            .to_string();

        // The decision: a policy over the REPORTED type matches the group the
        // token claims, so that is the type the groups were mapped to.
        let permit = policy(&router, &store_id, group_permit(&reported, &pool)).await;
        let decided = ok(
            &router,
            "IsAuthorizedWithToken",
            &authorization_request(&store_id, &member.id_token, None),
        )
        .await;
        assert_eq!(
            decided["decision"].as_str(),
            Some("ALLOW"),
            "case {case:?}: expected a policy over the reported type {reported:?} to match the \
             token's group, got {decided}"
        );
        assert_eq!(
            decided["determiningPolicies"][0]["policyId"].as_str(),
            Some(permit.as_str()),
            "case {case:?}: expected the group policy {permit} to be the determining one, got \
             {decided}"
        );

        // The refusal: an entity of the REPORTED type is one this call builds
        // itself, so supplying one is refused — naming a type the read reports.
        let (status, refused) = call(
            &router,
            "IsAuthorizedWithToken",
            &authorization_request(
                &store_id,
                &member.id_token,
                Some(json!({ "entityList": [
                    { "identifier": { "entityType": reported, "entityId": PERMITTED_GROUP } },
                ] })),
            ),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException for a supplied entity of the \
             reported type {reported:?}, got {status} with {refused}"
        );
        // The modelled `ValidationException` carries the reason in its
        // `fieldList`, against the member that carried the entity.
        let named = field_list(&refused);
        assert!(
            named
                .iter()
                .any(|(path, reason)| path == "/entities" && reason.contains(&reported)),
            "case {case:?}: expected the refusal to name /entities and the reported type \
             {reported:?}, got {named:?}"
        );

        // An entity of a type NEITHER the source generates nor the store's
        // schema declares is still accepted, so the refusal above is about the
        // group type and not about `entities` as such.
        let accepted = ok(
            &router,
            "IsAuthorizedWithToken",
            &authorization_request(
                &store_id,
                &member.id_token,
                Some(json!({ "entityList": [
                    { "identifier": { "entityType": "App::Doc", "entityId": DOCUMENT } },
                ] })),
            ),
        )
        .await;
        assert_eq!(
            accepted["decision"].as_str(),
            Some("ALLOW"),
            "case {case:?}: expected a resource entity to be accepted beside the token, got \
             {accepted}"
        );
    }
}
