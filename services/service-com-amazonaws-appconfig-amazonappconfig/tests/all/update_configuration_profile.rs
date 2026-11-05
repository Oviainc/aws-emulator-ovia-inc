// SPDX-License-Identifier: CC0-1.0

//! `UpdateConfigurationProfile` as a PATCH: the members a request carries are written to
//! the stored profile, every member it omits keeps the value the profile already holds,
//! and `LocationUri` — which the request cannot carry — cannot change at all. A body that
//! names the profile's `Type` is refused outright, which
//! `tests/all/configuration_profile_immutable_type.rs` owns.
//!
//! The suite drives real HTTP requests through the router and reads every update back
//! through `GetConfigurationProfile`, so an update that answered a value it never
//! persisted fails here.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::appconfig_workflow::{
    create_application, create_profile_from, is_modelled_id, json_call, router,
    with_required_retrieval_role, Answer,
};

/// A retrieval role of the shape the model's `RoleArn` pattern demands
/// (`^((arn):(aws|…):(iam)::\d{12}:role[/].*)$`).
const BASELINE_ROLE: &str = "arn:aws:iam::000000000000:role/baseline-retrieval";
/// A second one, so a row can change the role rather than only set it.
const OTHER_ROLE: &str = "arn:aws-us-gov:iam::000000000000:role/other-retrieval";

/// One validator of each `ValidatorType` the model's `Validator` shape lists
/// (`JSON_SCHEMA` | `LAMBDA`).
fn json_schema_validator(schema: &str) -> Value {
    json!({ "Type": "JSON_SCHEMA", "Content": schema })
}

/// See [`json_schema_validator`].
fn lambda_validator(function: &str) -> Value {
    json!({ "Type": "LAMBDA", "Content": function })
}

/// The profile every row starts from: every member `UpdateConfigurationProfile` can
/// write is already set, so a row can change one, clear one, or leave one alone.
fn baseline_body(name: &str) -> Value {
    json!({
        "Name": name,
        "LocationUri": "s3://amzn-s3-demo-bucket/my-app/config.json",
        "Type": "AWS.Freeform",
        "Description": "the profile as created",
        "RetrievalRoleArn": BASELINE_ROLE,
        "KmsKeyIdentifier": "alias/baseline-key",
        "Validators": [json_schema_validator("{\"type\":\"object\"}")],
    })
}

/// `PATCH` a profile.
async fn update_profile(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    patch: &Value,
) -> Answer {
    json_call(
        router,
        "PATCH",
        &format!("/applications/{application}/configurationprofiles/{profile}"),
        patch,
    )
    .await
}

/// `GET` a profile.
async fn get_profile(router: &MultiServiceRouter, application: &str, profile: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/applications/{application}/configurationprofiles/{profile}"),
        &Value::Null,
    )
    .await
}

/// The record a profile created from `baseline` must report after `patch` is applied:
/// the patched members take the request's value, the rest keep the created one, and an
/// empty `KmsKeyIdentifier` clears the key the profile had.
fn expected_after(created: &Value, patch: &Value) -> Value {
    let mut expected = created.clone();
    for member in ["Name", "Description", "RetrievalRoleArn"] {
        if let Some(value) = patch.get(member) {
            expected[member] = value.clone();
        }
    }
    if let Some(validators) = patch.get("Validators") {
        // A supplied list REPLACES the stored one; an empty list leaves the profile with
        // no validators, and the member is then not reported at all.
        if validators.as_array().is_some_and(Vec::is_empty) {
            expected
                .as_object_mut()
                .expect("a profile is a JSON object")
                .remove("Validators");
        } else {
            expected["Validators"] = validators.clone();
        }
    }
    if let Some(identifier) = patch.get("KmsKeyIdentifier") {
        if identifier.as_str() == Some("") {
            expected
                .as_object_mut()
                .expect("a profile is a JSON object")
                .remove("KmsKeyIdentifier");
        } else {
            expected["KmsKeyIdentifier"] = identifier.clone();
        }
    }
    expected
}

/// Every patch writes exactly the members it carries, leaves the rest of the profile
/// alone, and persists: the update's own answer and a later `GetConfigurationProfile`
/// report the same record.
#[tokio::test]
async fn an_update_writes_the_members_it_carries_and_leaves_the_others_alone() {
    let router = router();
    let application = create_application(&router, "updates").await;

    // Rows differ along WHICH members the patch carries (none, one, several, all), the
    // kind of value each carries (a longer and a shorter string, a different ARN, a list
    // of one and of two, the empty list, the empty string), and the length of `Name` and
    // `Description` at the published bounds of `LongName` (1..128) and `Description`
    // (0..1024).
    let long_name = "n".repeat(128);
    let long_description = "d".repeat(1024);
    let cases: Vec<(&str, Value)> = vec![
        ("no members at all", json!({})),
        ("name only", json!({ "Name": "a-renamed-profile" })),
        ("name at the 1-character bound", json!({ "Name": "x" })),
        ("name at the 128-character bound", json!({ "Name": long_name })),
        (
            "description only",
            json!({ "Description": "updated description" }),
        ),
        (
            "description at the 0-character bound",
            json!({ "Description": "" }),
        ),
        (
            "description at the 1024-character bound",
            json!({ "Description": long_description }),
        ),
        (
            "retrieval role replaced",
            json!({ "RetrievalRoleArn": OTHER_ROLE }),
        ),
        (
            "one validator replaces the created one",
            json!({ "Validators": [lambda_validator("arn:aws:lambda:us-east-1:000000000000:function:validate")] }),
        ),
        (
            "two validators, the modelled maximum",
            json!({ "Validators": [
                json_schema_validator("{\"type\":\"object\",\"required\":[\"enabled\"]}"),
                lambda_validator("arn:aws:lambda:us-east-1:000000000000:function:validate"),
            ] }),
        ),
        ("validators cleared by an empty list", json!({ "Validators": [] })),
        (
            "kms key identifier replaced",
            json!({ "KmsKeyIdentifier": "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555" }),
        ),
        (
            "kms key identifier cleared by the empty string",
            json!({ "KmsKeyIdentifier": "" }),
        ),
        (
            "every writable member at once",
            json!({
                "Name": "fully-updated",
                "Description": "every member at once",
                "RetrievalRoleArn": OTHER_ROLE,
                "KmsKeyIdentifier": "alias/other-key",
                "Validators": [json_schema_validator("{\"type\":\"array\"}")],
            }),
        ),
    ];

    for (case, patch) in &cases {
        // A fresh profile per row, so each row is judged against the same baseline.
        let baseline = baseline_body(&format!("baseline-{}", case.replace(' ', "-")));
        let created = create_profile_from(&router, &application, case, &baseline).await;
        let created_json = created.json();
        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {created_json}"));

        let updated = update_profile(&router, &application, &id, patch).await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: UpdateConfigurationProfile expected 200, got {} / {}",
            updated.status,
            String::from_utf8_lossy(&updated.body)
        );
        let expected = expected_after(&created_json, patch);
        assert_eq!(
            updated.json(),
            expected,
            "case {case:?}: expected {expected}, got {}",
            updated.json()
        );
        assert!(
            is_modelled_id(&id),
            "case {case:?}: expected an Id matching ^[a-z0-9]{{4,7}}$, got {id:?}"
        );

        // The write is only real if a later read sees it.
        let fetched = get_profile(&router, &application, &id).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetConfigurationProfile expected 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_eq!(
            fetched.json(),
            expected,
            "case {case:?}: GetConfigurationProfile must report what the update persisted, got {}",
            fetched.json()
        );

        // Applying the same patch again changes nothing further: the update is a write of
        // the values it carries, not an accumulation.
        let again = update_profile(&router, &application, &id, patch).await;
        assert_eq!(
            again.json(),
            expected,
            "case {case:?}: a repeated identical update must be idempotent, got {}",
            again.json()
        );
    }
}

/// `LocationUri` is not a member of this request, so no update can change it: whatever
/// the profile was created with is what it keeps reporting, and the members the request
/// does carry are still applied.
///
/// A body that names the profile's `Type` is a different rule and lives in
/// `tests/all/configuration_profile_immutable_type.rs`: the kind a profile is cannot
/// change, and a request asking it to is refused outright. The patch below therefore
/// names the kind the profile already is, which asks for no change.
#[tokio::test]
async fn an_update_cannot_change_where_a_configuration_lives() {
    let router = router();
    let application = create_application(&router, "immutable-members").await;

    // Rows span the `Type` values the model's `Type` documentation lists
    // ("AWS.AppConfig.FeatureFlags", "AWS.Freeform"), a profile with no `Type` at all
    // (created freeform, so that is the value it keeps), and the location kinds
    // `CreateConfigurationProfile`'s `LocationUri` doc names.
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("hosted feature flags", "hosted", Some("AWS.AppConfig.FeatureFlags")),
        ("hosted freeform", "hosted", Some("AWS.Freeform")),
        ("ssm parameter, no type", "ssm-parameter://my-parameter", None),
        (
            "s3 object",
            "s3://amzn-s3-demo-bucket/my-app/us-east-1/my-config.json",
            Some("AWS.Freeform"),
        ),
        (
            "secrets manager secret",
            "secretsmanager://my-secret",
            Some("AWS.Freeform"),
        ),
    ];

    for (case, location_uri, profile_type) in &cases {
        let mut body = json!({ "Name": format!("profile-{case}"), "LocationUri": location_uri });
        if let Some(profile_type) = profile_type {
            body["Type"] = Value::from(*profile_type);
        }
        let body = with_required_retrieval_role(body);
        let created = create_profile_from(&router, &application, case, &body).await;
        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {}", created.json()));

        // The patch names the location anyway, and restates the kind the profile already
        // is. `LocationUri` is not a member of the request shape, so the update applies
        // the members it does carry and leaves the location as it was — it never adopts a
        // value the operation cannot accept.
        let patch = json!({
            "Name": "renamed-but-not-relocated",
            "LocationUri": "hosted",
            "Type": profile_type.unwrap_or("AWS.Freeform"),
        });
        let updated = update_profile(&router, &application, &id, &patch).await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: UpdateConfigurationProfile expected 200, got {} / {}",
            updated.status,
            String::from_utf8_lossy(&updated.body)
        );
        assert_eq!(
            updated.string("LocationUri").as_deref(),
            Some(*location_uri),
            "case {case:?}: LocationUri must stay as created, got {:?}",
            updated.string("LocationUri")
        );
        assert_eq!(
            updated.string("Type").as_deref(),
            Some(profile_type.unwrap_or("AWS.Freeform")),
            "case {case:?}: Type must stay as created, got {:?}",
            updated.string("Type")
        );
        assert_eq!(
            updated.string("Name").as_deref(),
            Some("renamed-but-not-relocated"),
            "case {case:?}: the writable member must still be applied, got {:?}",
            updated.string("Name")
        );

        let fetched = get_profile(&router, &application, &id).await;
        assert_eq!(
            fetched.json(),
            updated.json(),
            "case {case:?}: GetConfigurationProfile must report what the update persisted, got {}",
            fetched.json()
        );
    }
}

/// An update is addressed under its application, so it answers 404
/// `ResourceNotFoundException` when either the application or the profile does not
/// exist — and persists nothing.
#[tokio::test]
async fn updating_a_profile_that_does_not_exist_is_not_found() {
    let router = router();
    let application = create_application(&router, "not-found").await;
    let profile = create_profile_from(
        &router,
        &application,
        "existing",
        &baseline_body("existing"),
    )
    .await;
    let profile_id = profile
        .string("Id")
        .expect("the created profile must carry an Id");

    // Rows differ along WHICH half of the address is unresolvable and along the shape of
    // the unresolvable id, including a near miss of a real one.
    let near_miss_application = format!("{application}x");
    let near_miss_profile = format!("{profile_id}x");
    let cases: Vec<(&str, String, String)> = vec![
        (
            "unknown application, known profile",
            "nosuchapp".to_string(),
            profile_id.clone(),
        ),
        (
            "near-miss application id",
            near_miss_application,
            profile_id.clone(),
        ),
        (
            "known application, unknown profile",
            application.clone(),
            "zzzzzzz".to_string(),
        ),
        (
            "known application, near-miss profile id",
            application.clone(),
            near_miss_profile,
        ),
        (
            "known application, profile id of the application itself",
            application.clone(),
            application.clone(),
        ),
        (
            "both unknown",
            "0000".to_string(),
            "a".to_string(),
        ),
    ];

    for (case, application_id, configuration_profile_id) in &cases {
        let answer = update_profile(
            &router,
            application_id,
            configuration_profile_id,
            &json!({ "Description": "should not be written" }),
        )
        .await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
    }

    // Nothing the refused calls carried reached the profile that does exist.
    let fetched = get_profile(&router, &application, &profile_id).await;
    assert_eq!(
        fetched.string("Description").as_deref(),
        Some("the profile as created"),
        "a refused update must persist nothing, got {:?}",
        fetched.string("Description")
    );
}
