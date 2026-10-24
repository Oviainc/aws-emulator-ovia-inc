// SPDX-License-Identifier: CC0-1.0

//! `DeleteConfigurationProfile`.
//!
//! RULE: a configuration profile owns the hosted configuration versions written into it
//! and AppConfig does not cascade that ownership away — while ANY hosted version remains
//! under the profile the delete is refused with the modelled `BadRequestException` (HTTP
//! 400) and changes nothing (the profile stays describable and listable, every version
//! stays readable), and once no version remains the same call answers the modelled 204
//! and the profile is gone, so every later call naming it misses with
//! `ResourceNotFoundException` (HTTP 404).
//!
//! Every case drives real HTTP requests through the router the emulator's front door
//! uses, so what is asserted is what an AppConfig client sees on the wire.
//!
//! The table spans what the rule generalises over rather than one recorded profile: HOW
//! MANY hosted versions the profile holds (none, one, two, five), WHAT those versions
//! carry (content type, size, byte values, labels and descriptions), the profile's `Type`
//! (freeform and feature flags) and `LocationUri` (the hosted store and a store that
//! holds no hosted version at all), whether a deployment of the profile is in flight,
//! and which `DeletionProtectionCheck` the delete asks for — including `BYPASS`, which
//! bypasses deletion protection and not this rule. The row with a deployment in flight
//! asserts the refusal half only: this operation also models `ConflictException` (409)
//! for a conflicting current state, and nothing recorded says which answer AppConfig
//! gives for a profile a rollout is still using.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::{
    FEATURE_FLAGS_PROFILE_TYPE, FREEFORM_PROFILE_TYPE, PREDEFINED_DEPLOYMENT_STRATEGIES,
};

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, router, send,
    with_required_retrieval_role, Answer,
};

/// Where the configuration profiles of one application live, from this operation's
/// modelled URI
/// (`smithy_query(query="operation",operation="DeleteConfigurationProfile")`:
/// `/applications/{ApplicationId}/configurationprofiles/{ConfigurationProfileId}`).
fn profiles_uri(application: &str) -> String {
    format!("/applications/{application}/configurationprofiles")
}

/// Where the hosted configuration versions of one profile live.
fn hosted_uri(application: &str, profile: &str) -> String {
    format!("{}/{profile}/hostedconfigurationversions", profiles_uri(application))
}

/// One hosted configuration version a row writes before it tries the delete.
struct Version {
    content_type: &'static str,
    content: Vec<u8>,
    /// `Description` and `VersionLabel` travel as `@httpHeader` members of
    /// `CreateHostedConfigurationVersion`; a label must contain a non-digit character
    /// (the modelled `VersionLabel` pattern).
    description: Option<&'static str>,
    label: Option<&'static str>,
}

/// One row of the table: a profile, the versions written under it, and how the delete is
/// asked for.
struct Row {
    case: &'static str,
    /// The `CreateConfigurationProfile` body.
    profile: Value,
    versions: Vec<Version>,
    /// Whether a deployment of the profile's first version is started before the delete.
    deploy: bool,
    /// The `DeletionProtectionCheck` the delete sends, bound to
    /// `x-amzn-deletion-protection-check`.
    check: Option<&'static str>,
}

/// A freeform JSON payload of `size` bytes of content.
fn json_payload(size: usize) -> Vec<u8> {
    let padding = "p".repeat(size.saturating_sub(br#"{"pad":""}"#.len()));
    format!(r#"{{"pad":"{padding}"}}"#).into_bytes()
}

/// The feature-flags document a `AWS.AppConfig.FeatureFlags` profile accepts.
fn flags_document() -> Vec<u8> {
    json!({
        "flags": { "deletable-flag": { "name": "deletable-flag" } },
        "values": { "deletable-flag": { "enabled": true } },
        "version": "1",
    })
    .to_string()
    .into_bytes()
}

/// The rows the rule is asserted over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "hosted profile with no version ever written",
            profile: json!({ "Name": "no-versions", "LocationUri": "hosted" }),
            versions: Vec::new(),
            deploy: false,
            check: None,
        },
        Row {
            case: "profile whose configuration lives outside the hosted store",
            profile: json!({
                "Name": "elsewhere",
                "LocationUri": "ssm-parameter://appconfig-delete-profile",
            }),
            versions: Vec::new(),
            deploy: false,
            check: None,
        },
        Row {
            case: "one hosted version",
            profile: json!({ "Name": "one-version", "LocationUri": "hosted" }),
            versions: vec![Version {
                content_type: "application/json",
                content: br#"{"enabled":true}"#.to_vec(),
                description: None,
                label: None,
            }],
            deploy: false,
            check: None,
        },
        Row {
            case: "two hosted versions and a deployment of the first",
            profile: json!({
                "Name": "two-versions-deployed",
                "LocationUri": "hosted",
                "Description": "the shape the recorded check exercises",
                "Type": FREEFORM_PROFILE_TYPE,
            }),
            versions: vec![
                Version {
                    content_type: "application/json",
                    content: br#"{"enabled":true}"#.to_vec(),
                    description: Some("first"),
                    label: Some("v-one"),
                },
                Version {
                    content_type: "application/json",
                    content: br#"{"enabled":false}"#.to_vec(),
                    description: Some("second"),
                    label: Some("v-two"),
                },
            ],
            deploy: true,
            check: None,
        },
        Row {
            case: "five hosted versions of differing content, delete asking for BYPASS",
            profile: json!({ "Name": "five-versions", "LocationUri": "hosted" }),
            versions: vec![
                Version {
                    content_type: "application/json",
                    content: br#"{"enabled":true}"#.to_vec(),
                    description: None,
                    label: None,
                },
                Version {
                    content_type: "application/x-yaml",
                    content: b"enabled: true\nlimit: 7\n".to_vec(),
                    description: Some("yaml"),
                    label: None,
                },
                Version {
                    content_type: "text/plain",
                    content: vec![b'x'],
                    description: None,
                    label: Some("tiny"),
                },
                Version {
                    content_type: "application/octet-stream",
                    content: vec![0, 1, 2, 255, 0, 128],
                    description: None,
                    label: None,
                },
                Version {
                    content_type: "application/json",
                    content: json_payload(64 * 1024),
                    description: Some("large"),
                    label: None,
                },
            ],
            deploy: false,
            check: Some("BYPASS"),
        },
        Row {
            case: "feature-flags profile with one hosted version, delete asking for APPLY",
            profile: json!({
                "Name": "flagged",
                "LocationUri": "hosted",
                "Type": FEATURE_FLAGS_PROFILE_TYPE,
            }),
            versions: vec![Version {
                content_type: "application/json",
                content: flags_document(),
                description: None,
                label: None,
            }],
            deploy: false,
            check: Some("ACCOUNT_DEFAULT"),
        },
        Row {
            case: "profile named at the upper length bound of LongName, two versions",
            profile: json!({ "Name": "n".repeat(128), "LocationUri": "hosted" }),
            versions: vec![
                Version {
                    content_type: "text/plain",
                    content: b"one".to_vec(),
                    description: None,
                    label: None,
                },
                Version {
                    content_type: "text/plain",
                    content: b"two".to_vec(),
                    description: None,
                    label: None,
                },
            ],
            deploy: false,
            check: Some("APPLY"),
        },
    ]
}

/// `POST` one hosted configuration version and answer the version number it was given.
async fn write_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    case: &str,
    version: &Version,
) -> i32 {
    let mut headers: Vec<(&str, String)> = Vec::new();
    if let Some(description) = version.description {
        headers.push(("Description", description.to_string()));
    }
    if let Some(label) = version.label {
        headers.push(("VersionLabel", label.to_string()));
    }
    let created = send(
        router,
        "POST",
        &hosted_uri(application, profile),
        version.content_type,
        version.content.clone(),
        &headers,
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateHostedConfigurationVersion must answer 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .header("Version-Number")
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateHostedConfigurationVersion answered no Version-Number")
        })
        .parse()
        .unwrap_or_else(|error| {
            panic!("case {case:?}: Version-Number must be an integer, got {error}")
        })
}

/// `DELETE` one configuration profile, optionally asking for a deletion-protection check.
async fn delete_profile(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    check: Option<&str>,
) -> Answer {
    let headers: Vec<(&str, String)> = check
        .map(|value| vec![("x-amzn-deletion-protection-check", value.to_string())])
        .unwrap_or_default();
    send(
        router,
        "DELETE",
        &format!("{}/{profile}", profiles_uri(application)),
        "application/json",
        Vec::new(),
        &headers,
    )
    .await
}

/// `DELETE` one hosted configuration version, which must succeed.
async fn delete_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    case: &str,
    number: i32,
) {
    let answer = send(
        router,
        "DELETE",
        &format!("{}/{number}", hosted_uri(application, profile)),
        "application/json",
        Vec::new(),
        &[],
    )
    .await;
    assert_eq!(
        answer.status,
        204,
        "case {case:?}: DeleteHostedConfigurationVersion {number} must answer 204, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// `GET` one configuration profile.
async fn get_profile(router: &MultiServiceRouter, application: &str, profile: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("{}/{profile}", profiles_uri(application)),
        &Value::Null,
    )
    .await
}

/// The version numbers `ListHostedConfigurationVersions` reports for one profile.
async fn listed_versions(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    case: &str,
) -> Vec<i64> {
    let answer = json_call(
        router,
        "GET",
        &hosted_uri(application, profile),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListHostedConfigurationVersions must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item.get("VersionNumber")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| panic!("case {case:?}: a listed version must carry a number"))
        })
        .collect()
}

/// The profile ids `ListConfigurationProfiles` reports for one application.
async fn listed_profiles(router: &MultiServiceRouter, application: &str, case: &str) -> Vec<String> {
    let answer = json_call(router, "GET", &profiles_uri(application), &Value::Null).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListConfigurationProfiles must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item.get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a listed profile must carry an Id"))
                .to_string()
        })
        .collect()
}

/// A delete that must have been refused for the children beneath the profile: the
/// modelled `BadRequestException`, HTTP 400, with the error type on the wire where an SDK
/// classifies it from and a message for the caller.
fn assert_refused(case: &str, remaining: usize, answer: &Answer) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: a delete with {remaining} hosted versions left must answer 400, got \
         {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.error_code().contains("BadRequestException"),
        "case {case:?}: a delete with {remaining} hosted versions left must answer \
         BadRequestException, got {:?}",
        answer.error_code()
    );
    assert!(
        answer
            .header("x-amzn-errortype")
            .is_some_and(|value| value.contains("BadRequestException")),
        "case {case:?}: the refusal must set x-amzn-errortype, got {:?}",
        answer.header("x-amzn-errortype")
    );
    assert!(
        answer
            .string("Message")
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: the refusal must carry a Message, got {}",
        answer.json()
    );
}

/// A call that must miss: the modelled `ResourceNotFoundException`, HTTP 404.
fn assert_not_found(case: &str, operation: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        404,
        "case {case:?}: {operation} must answer 404 for a profile that is gone, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.error_code().contains("ResourceNotFoundException"),
        "case {case:?}: {operation} must answer ResourceNotFoundException, got {:?}",
        answer.error_code()
    );
    assert!(
        answer
            .header("x-amzn-errortype")
            .is_some_and(|value| value.contains("ResourceNotFoundException")),
        "case {case:?}: {operation} must set x-amzn-errortype, got {:?}",
        answer.header("x-amzn-errortype")
    );
}

/// A delete that must have succeeded: the modelled 204, with no body.
fn assert_deleted(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        204,
        "case {case:?}: DeleteConfigurationProfile must answer the modelled 204, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.body.is_empty(),
        "case {case:?}: the modelled 204 carries no body, got {:?}",
        String::from_utf8_lossy(&answer.body)
    );
}

/// RULE, both halves: while hosted versions remain the delete is refused with 400
/// `BadRequestException` and changes nothing; once the last one is gone it answers 204 and
/// the profile is gone with it.
#[tokio::test]
async fn a_profile_is_deletable_only_once_its_hosted_versions_are_gone() {
    let router = router();
    let application = create_application(&router, "profile-deletes").await;
    let environment = create_environment(&router, &application, "profile-deletes-env").await;

    // A sibling profile with a hosted version of its own: whatever the rows below do,
    // this profile and its version must be untouched at the end.
    let witness = create_profile(&router, &application, "witness").await;
    let witness_version = write_version(
        &router,
        &application,
        &witness,
        "witness",
        &Version {
            content_type: "application/json",
            content: br#"{"witness":1}"#.to_vec(),
            description: None,
            label: None,
        },
    )
    .await;

    for row in rows() {
        let case = row.case;
        let created = json_call(
            &router,
            "POST",
            &profiles_uri(&application),
            &with_required_retrieval_role(row.profile.clone()),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateConfigurationProfile must answer 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let profile = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: CreateConfigurationProfile answered no Id"));

        let mut numbers: Vec<i32> = Vec::new();
        for version in &row.versions {
            numbers.push(write_version(&router, &application, &profile, case, version).await);
        }

        if row.deploy {
            // A deployment of the profile is in flight: the predefined strategy with the
            // longest schedule, so the rollout has not finished when the delete runs.
            let slowest = PREDEFINED_DEPLOYMENT_STRATEGIES
                .iter()
                .max_by_key(|strategy| {
                    strategy.deployment_duration_in_minutes + strategy.final_bake_time_in_minutes
                })
                .expect("the predefined catalog is not empty");
            let started = json_call(
                &router,
                "POST",
                &format!("/applications/{application}/environments/{environment}/deployments"),
                &json!({
                    "DeploymentStrategyId": slowest.id,
                    "ConfigurationProfileId": profile.clone(),
                    "ConfigurationVersion": numbers
                        .first()
                        .expect("a deploying row writes a version")
                        .to_string(),
                }),
            )
            .await;
            assert_eq!(
                started.status,
                201,
                "case {case:?}: StartDeployment must answer 201, got {} / {}",
                started.status,
                String::from_utf8_lossy(&started.body)
            );
        }

        // While ANY version is left the delete is refused, and the refusal changes
        // nothing — asserted once per remaining version, so the rule is about what is
        // still there and not about how many were ever written.
        let described_before = get_profile(&router, &application, &profile).await.json();
        while !numbers.is_empty() {
            let refused = delete_profile(&router, &application, &profile, row.check).await;
            assert_refused(case, numbers.len(), &refused);

            let described = get_profile(&router, &application, &profile).await;
            assert_eq!(
                described.status,
                200,
                "case {case:?}: a refused delete leaves the profile describable, got {} / {}",
                described.status,
                String::from_utf8_lossy(&described.body)
            );
            assert_eq!(
                described.json(),
                described_before,
                "case {case:?}: a refused delete changes no member of the profile, expected \
                 {described_before}, got {}",
                described.json()
            );
            let mut still_there = listed_versions(&router, &application, &profile, case).await;
            still_there.sort_unstable();
            let mut expected: Vec<i64> = numbers.iter().map(|number| i64::from(*number)).collect();
            expected.sort_unstable();
            assert_eq!(
                still_there, expected,
                "case {case:?}: a refused delete leaves every hosted version listable, \
                 expected {expected:?}, got {still_there:?}"
            );
            assert!(
                listed_profiles(&router, &application, case)
                    .await
                    .contains(&profile),
                "case {case:?}: a refused delete leaves the profile listed"
            );

            let doomed = numbers.remove(0);
            delete_version(&router, &application, &profile, case, doomed).await;
        }

        // The children-first refusal is the rule under test and holds for every row,
        // including the deploying one. The SUCCESS half is asserted only where no
        // deployment is in flight: this operation also models `ConflictException` (409)
        // for a resource whose current state conflicts with the request
        // (`smithy_query(query="operation",operation="DeleteConfigurationProfile")`), and
        // nothing recorded says which of 204 and 409 AppConfig answers for a profile a
        // rollout is still using — so this suite pins neither for that row.
        if row.deploy {
            continue;
        }

        // No version is left, so the same call now succeeds — including for the profile
        // whose store never held one.
        let deleted = delete_profile(&router, &application, &profile, row.check).await;
        assert_deleted(case, &deleted);

        assert_not_found(
            case,
            "GetConfigurationProfile",
            &get_profile(&router, &application, &profile).await,
        );
        assert_not_found(
            case,
            "a repeated DeleteConfigurationProfile",
            &delete_profile(&router, &application, &profile, row.check).await,
        );
        assert!(
            !listed_profiles(&router, &application, case)
                .await
                .contains(&profile),
            "case {case:?}: a deleted profile is no longer listed"
        );
    }

    let surviving = listed_versions(&router, &application, &witness, "witness").await;
    assert_eq!(
        surviving,
        vec![i64::from(witness_version)],
        "case \"witness\": the sibling profile keeps its hosted version, got {surviving:?}"
    );
    assert_eq!(
        get_profile(&router, &application, &witness).await.status,
        200,
        "case \"witness\": the sibling profile is untouched by every delete"
    );
}

/// A delete that cannot resolve the application or the configuration profile answers 404
/// `ResourceNotFoundException` and removes nothing.
#[tokio::test]
async fn deleting_a_profile_the_store_does_not_hold_changes_nothing() {
    let router = router();
    let application = create_application(&router, "misses").await;
    let profile = create_profile(&router, &application, "held").await;
    let other_application = create_application(&router, "elsewhere").await;
    let other_profile = create_profile(&router, &other_application, "held-elsewhere").await;

    // Rows differ along WHICH level of the path is unresolvable and along how: an id
    // nobody minted, an id that belongs to the other kind of resource, and a profile that
    // exists but under a different application.
    let cases: Vec<(&str, String, String)> = vec![
        (
            "an application id nobody minted",
            "nosuchapp".to_string(),
            profile.clone(),
        ),
        (
            "a profile id nobody minted",
            application.clone(),
            "nosuchprofile".to_string(),
        ),
        (
            "an application id in the profile slot",
            application.clone(),
            application.clone(),
        ),
        (
            "a profile of another application",
            application.clone(),
            other_profile.clone(),
        ),
    ];

    for (case, target_application, target_profile) in cases {
        let answer = delete_profile(&router, &target_application, &target_profile, None).await;
        assert_not_found(case, "DeleteConfigurationProfile", &answer);

        // Nothing was removed: both held profiles still read back.
        assert_eq!(
            get_profile(&router, &application, &profile).await.status,
            200,
            "case {case:?}: a refused delete must leave the held profile describable"
        );
        assert_eq!(
            get_profile(&router, &other_application, &other_profile)
                .await
                .status,
            200,
            "case {case:?}: a refused delete must leave the other application's profile \
             describable"
        );
    }
}
