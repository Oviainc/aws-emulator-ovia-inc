// SPDX-License-Identifier: CC0-1.0

//! A configuration profile's `Type` is a property of the stored profile.
//!
//! RULE: creating a configuration profile stores a `Type` — the one the request
//! declared, or the freeform type when it declared none — and every document that
//! describes that profile afterwards (the create response, `GetConfigurationProfile`,
//! `UpdateConfigurationProfile`, a `ListConfigurationProfiles` summary, and the `Type`
//! filter that listing accepts) reports that same value.
//!
//! The rows below span the two `Type` values the model's own `Type` documentation lists
//! and the absence of the member, crossed with the location kinds
//! `CreateConfigurationProfile`'s `LocationUri` doc names and with the other optional
//! members a profile can carry, because none of them has any bearing on the profile's
//! kind.

use serde_json::{json, Value};
use service_com_amazonaws_appconfig_amazonappconfig::state::{
    CONFIGURATION_PROFILE_TYPES, FEATURE_FLAGS_PROFILE_TYPE, FREEFORM_PROFILE_TYPE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::appconfig_workflow::{
    create_application, create_profile_from, json_call, router, with_required_retrieval_role,
    Answer,
};

/// One row of the table: what the create request declares, and the `Type` the profile
/// must report ever after.
struct Row {
    case: String,
    /// The `Type` member of the create request, or `None` when it carries none.
    declared: Option<&'static str>,
    /// The rest of the create request, minus `Name` and `Type`.
    body: Value,
}

/// The `Type` a profile created from `row` reports: the declared one, or the freeform
/// type when the request declared none.
fn expected_type(row: &Row) -> &'static str {
    row.declared.unwrap_or(FREEFORM_PROFILE_TYPE)
}

/// The create requests this suite spans. The `LocationUri` values are the location kinds
/// the model's `LocationUri` documentation lists ("specify `hosted`", `ssm-parameter://`,
/// `codepipeline://`, `secretsmanager://`, `s3://`); `Validators`, `Description` and
/// `KmsKeyIdentifier` are the profile's other optional members.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    let locations: [(&'static str, Value); 5] = [
        ("hosted", json!({ "LocationUri": "hosted" })),
        (
            "ssm parameter",
            json!({ "LocationUri": "ssm-parameter://my-parameter" }),
        ),
        (
            "codepipeline",
            json!({ "LocationUri": "codepipeline://my-pipeline" }),
        ),
        (
            "secretsmanager",
            json!({ "LocationUri": "secretsmanager://my-secret" }),
        ),
        (
            "s3 object",
            json!({ "LocationUri": "s3://amzn-s3-demo-bucket/my-app/my-config.json" }),
        ),
    ];
    // `None` is the row the defaulting rule is about; the two named values are what the
    // model's `Type` doc lists.
    let declarations: [Option<&'static str>; 3] = [
        None,
        Some(FREEFORM_PROFILE_TYPE),
        Some(FEATURE_FLAGS_PROFILE_TYPE),
    ];
    for declared in declarations {
        for (location_case, location) in &locations {
            rows.push(Row {
                case: format!("{location_case}, Type={}", declared.unwrap_or("<absent>")),
                declared,
                body: location.clone(),
            });
        }
    }
    // The profile's other optional members, so the table also varies along what else the
    // request carries while the kind stays the one the rule gives.
    rows.push(Row {
        case: "hosted with a validator and no Type".to_string(),
        declared: None,
        body: json!({
            "LocationUri": "hosted",
            "Validators": [{ "Type": "JSON_SCHEMA", "Content": "{\"type\":\"object\"}" }],
        }),
    });
    rows.push(Row {
        case: "hosted with a description, a KMS key and no Type".to_string(),
        declared: None,
        body: json!({
            "LocationUri": "hosted",
            "Description": "a profile that never declared a kind",
            "KmsKeyIdentifier": "alias/appconfig",
        }),
    });
    rows.push(Row {
        case: "hosted feature flags with a description".to_string(),
        declared: Some(FEATURE_FLAGS_PROFILE_TYPE),
        body: json!({
            "LocationUri": "hosted",
            "Description": "a profile that declared the feature-flag kind",
        }),
    });
    rows
}

/// The create request for `row`, with the `RetrievalRoleArn` its location needs.
fn create_body(row: &Row, name: &str) -> Value {
    let mut body = row.body.clone();
    body["Name"] = Value::from(name);
    if let Some(declared) = row.declared {
        body["Type"] = Value::from(declared);
    }
    with_required_retrieval_role(body)
}

/// `GET` one profile.
async fn get_profile(router: &MultiServiceRouter, application: &str, id: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/applications/{application}/configurationprofiles/{id}"),
        &Value::Null,
    )
    .await
}

/// The summaries `ListConfigurationProfiles` reports, optionally filtered by `Type`.
async fn list_summaries(
    router: &MultiServiceRouter,
    application: &str,
    filter: Option<&str>,
    case: &str,
) -> Vec<Value> {
    let uri = match filter {
        Some(wanted) => {
            format!("/applications/{application}/configurationprofiles?type={wanted}")
        }
        None => format!("/applications/{application}/configurationprofiles"),
    };
    let answer = json_call(router, "GET", &uri, &Value::Null).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListConfigurationProfiles expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the listing carried no Items: {}", answer.json()))
}

/// Every document describing a created profile carries the `Type` the create request
/// declared, or the freeform type when it declared none.
#[tokio::test]
async fn a_created_profile_reports_its_type_on_every_document() {
    let router = router();
    let application = create_application(&router, "profile-type").await;

    for (index, row) in rows().iter().enumerate() {
        let case = row.case.as_str();
        let expected = expected_type(row);
        let body = create_body(row, &format!("profile-{index}"));
        let created = create_profile_from(&router, &application, case, &body).await;
        assert_eq!(
            created.string("Type").as_deref(),
            Some(expected),
            "case {case:?}: CreateConfigurationProfile must report Type {expected}, got {:?}",
            created.string("Type")
        );
        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {}", created.json()));

        // The same record, read back: the kind is persisted, not computed per response.
        let fetched = get_profile(&router, &application, &id).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetConfigurationProfile expected 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_eq!(
            fetched.string("Type").as_deref(),
            Some(expected),
            "case {case:?}: GetConfigurationProfile must report Type {expected}, got {:?}",
            fetched.string("Type")
        );

        // An update carries no `Type` member at all, so the profile's kind survives one.
        let updated = json_call(
            &router,
            "PATCH",
            &format!("/applications/{application}/configurationprofiles/{id}"),
            &json!({ "Description": "updated" }),
        )
        .await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: UpdateConfigurationProfile expected 200, got {} / {}",
            updated.status,
            String::from_utf8_lossy(&updated.body)
        );
        assert_eq!(
            updated.string("Type").as_deref(),
            Some(expected),
            "case {case:?}: UpdateConfigurationProfile must report Type {expected}, got {:?}",
            updated.string("Type")
        );

        // The summary of that same record, and the filter that selects by kind.
        let summaries = list_summaries(&router, &application, None, case).await;
        let summary = summaries
            .iter()
            .find(|summary| summary.get("Id").and_then(Value::as_str) == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {id} is missing from {summaries:?}"));
        assert_eq!(
            summary.get("Type").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: the summary must report Type {expected}, got {summary}"
        );

        for wanted in CONFIGURATION_PROFILE_TYPES {
            let filtered = list_summaries(&router, &application, Some(wanted), case).await;
            let selected = filtered
                .iter()
                .any(|summary| summary.get("Id").and_then(Value::as_str) == Some(id.as_str()));
            assert_eq!(
                selected,
                *wanted == expected,
                "case {case:?}: Type={wanted} must select {id} only when the profile is of that \
                 kind (it is {expected}), got {filtered:?}"
            );
        }
    }
}

/// Declaring the freeform type and declaring nothing create the same kind of profile:
/// the two records differ in no member but their identity and name.
#[tokio::test]
async fn omitting_type_is_the_same_as_declaring_the_freeform_type() {
    let router = router();
    let application = create_application(&router, "freeform-default").await;

    // Rows differ along the location kind, because the default is a property of the
    // profile's kind and not of where its configuration lives.
    let locations = [
        "hosted",
        "ssm-parameter://my-parameter",
        "codepipeline://my-pipeline",
        "s3://amzn-s3-demo-bucket/my-app/my-config.json",
    ];
    for location_uri in locations {
        let case = format!("LocationUri={location_uri}");
        let declared = create_profile_from(
            &router,
            &application,
            &case,
            &with_required_retrieval_role(json!({
                "Name": format!("declared-{location_uri}"),
                "LocationUri": location_uri,
                "Type": FREEFORM_PROFILE_TYPE,
            })),
        )
        .await;
        let omitted = create_profile_from(
            &router,
            &application,
            &case,
            &with_required_retrieval_role(json!({
                "Name": format!("omitted-{location_uri}"),
                "LocationUri": location_uri,
            })),
        )
        .await;

        let mut left = declared.json();
        let mut right = omitted.json();
        // Identity and name are what the two calls were asked to differ in; everything
        // else, `Type` included, must match.
        for document in [&mut left, &mut right] {
            if let Some(object) = document.as_object_mut() {
                object.remove("Id");
                object.remove("Name");
            }
        }
        assert_eq!(
            left, right,
            "case {case:?}: a profile that declared no Type must match one that declared \
             {FREEFORM_PROFILE_TYPE}, got {left} vs {right}"
        );
        assert_eq!(
            omitted.string("Type").as_deref(),
            Some(FREEFORM_PROFILE_TYPE),
            "case {case:?}: expected Type {FREEFORM_PROFILE_TYPE}, got {:?}",
            omitted.string("Type")
        );
    }
}

/// The kind a profile defaults to is the freeform one, not the feature-flag one: the
/// content rule that only a feature-flags profile carries does not apply to it.
#[tokio::test]
async fn the_default_kind_carries_no_feature_flag_content_rule() {
    let router = router();
    let application = create_application(&router, "default-kind").await;

    let untyped = create_profile_from(
        &router,
        &application,
        "untyped",
        &json!({ "Name": "untyped", "LocationUri": "hosted" }),
    )
    .await;
    let untyped_id = untyped
        .string("Id")
        .expect("a created profile carries an Id");
    let flags = create_profile_from(
        &router,
        &application,
        "feature flags",
        &json!({
            "Name": "flags",
            "LocationUri": "hosted",
            "Type": FEATURE_FLAGS_PROFILE_TYPE,
        }),
    )
    .await;
    let flags_id = flags.string("Id").expect("a created profile carries an Id");

    // Documents that are configuration data but not feature-flag documents: a
    // feature-flags profile refuses each one, any other kind stores it.
    let documents = [
        ("object", json!({ "greeting": "hello" })),
        ("empty object", json!({})),
        ("array", json!([1, 2, 3])),
    ];
    for (case, document) in documents {
        let content = serde_json::to_vec(&document).expect("serialize the document");
        for (profile_case, profile_id, expected_status) in [
            ("default kind", &untyped_id, 201u16),
            ("feature flags", &flags_id, 400u16),
        ] {
            let answer = super::appconfig_workflow::send(
                &router,
                "POST",
                &format!(
                    "/applications/{application}/configurationprofiles/{profile_id}/hostedconfigurationversions"
                ),
                "application/json",
                content.clone(),
                &[],
            )
            .await;
            assert_eq!(
                answer.status,
                expected_status,
                "case {case:?} on a {profile_case} profile: expected {expected_status}, got {} / \
                 {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }
    }
}
