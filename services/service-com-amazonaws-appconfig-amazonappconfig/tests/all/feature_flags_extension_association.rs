// SPDX-License-Identifier: CC0-1.0

//! The extension association AppConfig itself makes, and what a deployment reports as
//! applied.
//!
//! The rules under test:
//!
//! * a configuration profile whose `Type` names AppConfig's feature-flag helper is handled
//!   by that AWS-authored extension, so creating such a profile is what associates the
//!   extension to it — the association binds the extension's published version to the
//!   profile's own ARN with no parameter values — while a profile of any other kind gets
//!   none;
//! * a deployment reports every association bound to the application, environment or
//!   configuration profile it runs as one `AppliedExtensions` entry, each naming the
//!   extension's id, the association's id and version, the association's parameter values,
//!   and the `ExtensionIdentifier` that association was created with as its
//!   `ProvidedIdentifier` — and every later read of the deployment reports the same list.
//!
//! The tables span what those rules generalise over: WHICH `Type` the profile declares
//! (the feature-flag type, the freeform type, and no `Type` at all), HOW MANY feature-flag
//! profiles one application holds, and — for the provided identifier — which of the three
//! forms `ExtensionIdentifier` accepts the association was created with (a name, an id and
//! the ARN of one version), over both halves of the extension catalogue. Every assertion
//! names its case and prints the actual value.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::ACTION_POINT_VALUES;
use service_com_amazonaws_appconfig_amazonappconfig::state::{
    AWS_AUTHORED_EXTENSION_VERSION, FEATURE_FLAGS_EXTENSION_ID, FEATURE_FLAGS_PROFILE_TYPE,
    FREEFORM_PROFILE_TYPE,
};

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile_from, json_call, router, send, Answer,
};
use crate::extension_associations::{associate, get_association};
use crate::extensions::{actions, create_extension, get_extension, minted_id};
use crate::resource_tags::{arn, pct};

/// The feature-flag document a profile of [`FEATURE_FLAGS_PROFILE_TYPE`] accepts: the
/// helper checks what is written to such a profile, so a version that is to be deployed
/// has to be one it admits.
fn flags_document() -> Vec<u8> {
    json!({
        "flags": { "applied-flag": { "name": "applied-flag" } },
        "values": { "applied-flag": { "enabled": true } },
        "version": "1",
    })
    .to_string()
    .into_bytes()
}

/// `GET /extensionassociations?resource_identifier=<arn>`: the associations bound to ONE
/// resource, which is how a caller asks what AppConfig associated to it.
async fn associations_on(router: &MultiServiceRouter, resource_arn: &str) -> Vec<Value> {
    let answer = json_call(
        router,
        "GET",
        &format!("/extensionassociations?resource_identifier={}", pct(resource_arn)),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "ListExtensionAssociations({resource_arn}) expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Write one version of `content` into a profile's hosted store.
async fn host_version(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    profile: &str,
    content: Vec<u8>,
) {
    let hosted = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        content,
        &[],
    )
    .await;
    assert_eq!(
        hosted.status,
        201,
        "case {case:?}: the version to deploy must be hosted first, got {} / {}",
        hosted.status,
        String::from_utf8_lossy(&hosted.body)
    );
}

/// `POST /applications/{a}/environments/{e}/deployments`, deploying version 1.
async fn start_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": "AppConfig.AllAtOnce",
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await
}

/// `GET /applications/{a}/environments/{e}/deployments/{n}`.
async fn get_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i64,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}/deployments/{number}"),
        &Value::Null,
    )
    .await
}

/// The `AppliedExtensions` of a deployment document, keyed by the association id each
/// entry names, so two reads are compared as records rather than by list position.
fn applied_by_association(case: &str, body: &Value) -> BTreeMap<String, Value> {
    body.get("AppliedExtensions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|applied| {
            let id = applied
                .get("ExtensionAssociationId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an applied extension names its association, got {applied}")
                })
                .to_string();
            (id, applied)
        })
        .collect()
}

/// A 2xx answer's body, with the status named in the failure.
fn body_of(case: &str, expected: u16, answer: &Answer) -> Value {
    assert_eq!(
        answer.status,
        expected,
        "case {case:?}: expected {expected}, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// Creating a configuration profile associates AppConfig's feature-flag helper to it when,
/// and only when, the profile's `Type` names that helper.
///
/// Rows differ along the `Type` the create declares — the feature-flag type, the freeform
/// type and no `Type` at all — and the feature-flag row is created twice in one
/// application, so each profile gets an association of its own rather than sharing one.
#[tokio::test]
async fn a_feature_flag_profile_is_associated_with_the_feature_flag_helper_and_others_are_not() {
    let router = router();
    let application = create_application(&router, "implicit-association-app").await;
    // The helper's own published version ARN, read back through `GetExtension` rather than
    // spelled here, so what the association binds is the ARN the service mints.
    let helper = body_of(
        FEATURE_FLAGS_EXTENSION_ID,
        200,
        &get_extension(&router, FEATURE_FLAGS_EXTENSION_ID, None).await,
    );
    let helper_arn = helper
        .get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("GetExtension must report the helper's Arn, got {helper}"))
        .to_string();

    // (case, declared Type, whether the helper handles such a profile)
    let rows: Vec<(&str, Option<&str>, bool)> = vec![
        ("the feature-flag type", Some(FEATURE_FLAGS_PROFILE_TYPE), true),
        (
            "a second feature-flag profile",
            Some(FEATURE_FLAGS_PROFILE_TYPE),
            true,
        ),
        ("the freeform type", Some(FREEFORM_PROFILE_TYPE), false),
        ("no Type at all", None, false),
    ];

    for (index, (case, declared, associated)) in rows.iter().enumerate() {
        let name = format!("implicit-{index}");
        let mut body = json!({ "Name": name, "LocationUri": "hosted" });
        if let Some(declared) = declared {
            body["Type"] = Value::from(*declared);
        }
        let created = create_profile_from(&router, &application, case, &body).await;
        let profile = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: the create answered no Id"));
        let profile_arn = arn(&format!(
            "application/{application}/configurationprofile/{profile}"
        ));

        let listed = associations_on(&router, &profile_arn).await;
        if !associated {
            assert!(
                listed.is_empty(),
                "case {case:?}: a profile the helper does not handle is associated with \
                 nothing, got {listed:?}"
            );
            continue;
        }
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected exactly the helper's association, got {listed:?}"
        );
        let id = listed[0]
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: a listed association carries an Id"));

        // The listing is the stored record, which `GetExtensionAssociation` reports in
        // full: the association binds the helper's published version to this profile and
        // supplies no parameter value, the helper declaring no parameter.
        let read = body_of(case, 200, &get_association(&router, id).await);
        assert_eq!(
            read.get("ExtensionArn").and_then(Value::as_str),
            Some(helper_arn.as_str()),
            "case {case:?}: expected the helper's version ARN, got {read}"
        );
        assert_eq!(
            read.get("ResourceArn").and_then(Value::as_str),
            Some(profile_arn.as_str()),
            "case {case:?}: expected the profile's own ARN, got {read}"
        );
        assert_eq!(
            read.get("ExtensionVersionNumber").and_then(Value::as_i64),
            Some(i64::from(AWS_AUTHORED_EXTENSION_VERSION)),
            "case {case:?}: expected the helper's published version, got {read}"
        );
        assert_eq!(
            read.get("Parameters"),
            Some(&json!({})),
            "case {case:?}: expected no parameter values, got {read}"
        );
    }

    // Each feature-flag profile got its OWN association: the account now holds one per
    // such profile and no more.
    let every = associations_on(&router, &arn(&format!("application/{application}"))).await;
    assert!(
        every.is_empty(),
        "the helper is associated to the PROFILE, not to its application, got {every:?}"
    );
}

/// A deployment of a feature-flag profile reports the helper's association among its
/// applied extensions, and every read of that deployment reports the same entry.
#[tokio::test]
async fn a_deployment_of_a_feature_flag_profile_reports_the_helper_as_applied() {
    let router = router();
    let case = "a feature-flag profile deployed";
    let application = create_application(&router, "applied-helper-app").await;
    let environment = create_environment(&router, &application, "applied-helper-env").await;
    let created = create_profile_from(
        &router,
        &application,
        case,
        &json!({
            "Name": "applied-helper-profile",
            "LocationUri": "hosted",
            "Type": FEATURE_FLAGS_PROFILE_TYPE,
        }),
    )
    .await;
    let profile = created
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: the create answered no Id"));
    host_version(&router, case, &application, &profile, flags_document()).await;

    let association = associations_on(
        &router,
        &arn(&format!(
            "application/{application}/configurationprofile/{profile}"
        )),
    )
    .await;
    let association_id = association
        .first()
        .and_then(|item| item.get("Id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the helper must be associated to the profile, got {association:?}")
        })
        .to_string();

    let started = body_of(
        case,
        201,
        &start_deployment(&router, &application, &environment, &profile).await,
    );
    let applied = applied_by_association(case, &started);
    assert_eq!(
        applied.keys().cloned().collect::<Vec<String>>(),
        vec![association_id.clone()],
        "case {case:?}: expected exactly the helper's association as applied, got {started}"
    );
    let entry = &applied[&association_id];
    assert_eq!(
        entry.get("ExtensionId").and_then(Value::as_str),
        Some(FEATURE_FLAGS_EXTENSION_ID),
        "case {case:?}: an applied extension names the extension it ran, got {entry}"
    );
    assert_eq!(
        entry.get("ProvidedIdentifier").and_then(Value::as_str),
        Some(FEATURE_FLAGS_EXTENSION_ID),
        "case {case:?}: expected the identifier the association was made with, got {entry}"
    );
    assert_eq!(
        entry.get("VersionNumber").and_then(Value::as_i64),
        Some(i64::from(AWS_AUTHORED_EXTENSION_VERSION)),
        "case {case:?}: expected the helper's published version, got {entry}"
    );
    assert_eq!(
        entry.get("Parameters"),
        Some(&json!({})),
        "case {case:?}: expected an empty parameter map, got {entry}"
    );

    // The deployment RECORD carries the list, so the read path reports what the start
    // reported — twice over, so reading it changes nothing.
    let number = started
        .get("DeploymentNumber")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: the deployment must carry its number"));
    for read in ["the first read", "the second read"] {
        let body = body_of(
            read,
            200,
            &get_deployment(&router, &application, &environment, number).await,
        );
        assert_eq!(
            applied_by_association(read, &body),
            applied,
            "case {case:?} {read}: expected the applied list the start reported, got {body}"
        );
    }

    // A deployment of a profile the helper does not handle reports no applied extension at
    // all, so the entry above is the association's and not something every deployment
    // carries.
    let freeform = create_profile_from(
        &router,
        &application,
        "a freeform profile deployed",
        &json!({
            "Name": "applied-freeform-profile",
            "LocationUri": "hosted",
            "Type": FREEFORM_PROFILE_TYPE,
        }),
    )
    .await;
    let freeform = freeform
        .string("Id")
        .unwrap_or_else(|| panic!("the create answered no Id"));
    host_version(
        &router,
        "a freeform profile deployed",
        &application,
        &freeform,
        br#"{"enabled":true}"#.to_vec(),
    )
    .await;
    let other_environment = create_environment(&router, &application, "applied-freeform-env").await;
    let started = body_of(
        "a freeform profile deployed",
        201,
        &start_deployment(&router, &application, &other_environment, &freeform).await,
    );
    assert!(
        applied_by_association("a freeform profile deployed", &started).is_empty(),
        "a profile the helper does not handle applies no extension, got {started}"
    );
}

/// An applied extension reports the `ExtensionIdentifier` its association was created
/// with, in the form the caller used.
///
/// Rows differ along which identifier form the association named the extension by — a
/// name, a system-generated id and the ARN of one version — and over both halves of the
/// catalogue: an extension this account created and one AppConfig authored.
#[tokio::test]
async fn an_applied_extension_reports_the_identifier_its_association_was_created_with() {
    let router = router();
    let application = create_application(&router, "provided-identifier-app").await;
    let environment = create_environment(&router, &application, "provided-identifier-env").await;
    let created = create_profile_from(
        &router,
        &application,
        "provided-identifier-profile",
        &json!({ "Name": "provided-identifier-profile", "LocationUri": "hosted" }),
    )
    .await;
    let profile = created
        .string("Id")
        .unwrap_or_else(|| panic!("the create answered no Id"));
    host_version(
        &router,
        "provided-identifier-profile",
        &application,
        &profile,
        br#"{"enabled":true}"#.to_vec(),
    )
    .await;

    let name = "provided-identifier-extension";
    let answer = create_extension(
        &router,
        &json!({ "Name": name, "Actions": actions(&[ACTION_POINT_VALUES[0]]) }),
        None,
    )
    .await;
    let extension_id = minted_id(name, &body_of(name, 201, &answer));
    let version_arn = body_of(name, 200, &get_extension(&router, &extension_id, Some(1)).await)
        .get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("GetExtension must report an Arn"))
        .to_string();
    // The authored extension addressed by its published name, which is NOT its id: the
    // identifier a caller provided is reported as provided, not normalised to the id.
    let authored = body_of(
        FEATURE_FLAGS_EXTENSION_ID,
        200,
        &get_extension(&router, FEATURE_FLAGS_EXTENSION_ID, None).await,
    );
    let authored_name = authored
        .get("Name")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("GetExtension must report a Name, got {authored}"))
        .to_string();

    // (case, the identifier the association is created with, the extension id it resolves)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a created extension by name",
            name.to_string(),
            extension_id.clone(),
        ),
        (
            "a created extension by its minted id",
            extension_id.clone(),
            extension_id.clone(),
        ),
        (
            "a created extension by the ARN of its version",
            version_arn.clone(),
            extension_id.clone(),
        ),
        (
            "an authored extension by its published id",
            FEATURE_FLAGS_EXTENSION_ID.to_string(),
            FEATURE_FLAGS_EXTENSION_ID.to_string(),
        ),
        (
            "an authored extension by its published name",
            authored_name.clone(),
            FEATURE_FLAGS_EXTENSION_ID.to_string(),
        ),
    ];

    let mut expected: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (case, identifier, extension) in &rows {
        let answer = associate(
            &router,
            &json!({
                "ExtensionIdentifier": identifier,
                "ResourceIdentifier": arn(&format!(
                    "application/{application}/configurationprofile/{profile}"
                )),
            }),
        )
        .await;
        let association = body_of(case, 201, &answer);
        let id = association
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the create answered no Id"))
            .to_string();
        expected.insert(id, (identifier.clone(), extension.clone()));
    }

    let started = body_of(
        "every association applied",
        201,
        &start_deployment(&router, &application, &environment, &profile).await,
    );
    let applied = applied_by_association("every association applied", &started);
    assert_eq!(
        applied.keys().cloned().collect::<Vec<String>>(),
        expected.keys().cloned().collect::<Vec<String>>(),
        "expected every association in force to be applied, got {started}"
    );
    for (id, (identifier, extension)) in &expected {
        let entry = &applied[id];
        assert_eq!(
            entry.get("ProvidedIdentifier").and_then(Value::as_str),
            Some(identifier.as_str()),
            "case {identifier:?}: expected the identifier the association was made with, got \
             {entry}"
        );
        assert_eq!(
            entry.get("ExtensionId").and_then(Value::as_str),
            Some(extension.as_str()),
            "case {identifier:?}: expected the extension the identifier resolves, got {entry}"
        );
    }
}
