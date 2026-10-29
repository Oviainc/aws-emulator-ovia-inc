// SPDX-License-Identifier: CC0-1.0

//! The gate AppConfig puts on deleting a CONTAINER resource.
//!
//! The rule under test, in one sentence: AppConfig will not take a container resource out
//! of its tree while a resource that container owns still exists, so the delete is refused
//! with `BadRequestException` (HTTP 400) naming the kind of child in the way, and the
//! refusal changes nothing — the container and its children stay exactly as readable and
//! as listable as they were — while the same call succeeds with its modelled 204 once the
//! children have been removed bottom-up.
//!
//! The gate is on child EXISTENCE and not on deployment state: a deployment in flight
//! neither blocks a delete nor excuses one, so a LEAF under a container with a running
//! deployment is deletable while its parent is not.
//!
//! Every row drives real HTTP requests through the router the emulator's front door uses,
//! so what is asserted is what an AppConfig client sees on the wire.
//!
//! The table spans what the rule generalises over rather than one recorded application:
//! which kind of child is in the way and how many of them there are, whether the children
//! hold children of their own (hosted configuration versions under a profile), and whether
//! a deployment is in flight into one of them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    account, create_application, create_environment, create_profile, json_call, router, send,
    Answer,
};

/// One predefined deployment strategy id: AppConfig's `AppConfig.*` strategies are
/// deployable as shipped.
const ALL_AT_ONCE: &str = "AppConfig.AllAtOnce";

/// `DELETE` one URI with no body, which is how every delete in this suite is driven.
async fn delete(router: &MultiServiceRouter, uri: &str) -> Answer {
    send(router, "DELETE", uri, "application/json", Vec::new(), &[]).await
}

/// A delete that must have succeeded: the modelled 204 with no body.
fn assert_deleted(case: &str, operation: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        204,
        "case {case:?}: {operation} must answer the modelled 204, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.body.is_empty(),
        "case {case:?}: {operation} answers no body, got {:?}",
        String::from_utf8_lossy(&answer.body)
    );
}

/// A delete that must have been refused by the child gate: the modelled
/// `BadRequestException` (HTTP 400) carrying `expected` as its `Message`.
fn assert_gated(case: &str, operation: &str, answer: &Answer, expected: &str) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: {operation} must answer 400 while children exist, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.error_code().contains("BadRequestException"),
        "case {case:?}: {operation} must answer BadRequestException, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        answer.string("Message").as_deref(),
        Some(expected),
        "case {case:?}: {operation} must answer the child-gate template, got {}",
        answer.json()
    );
}

/// The ids one application's `ListEnvironments` serves.
async fn listed_environments(router: &MultiServiceRouter, application: &str) -> Vec<String> {
    listed_ids(
        router,
        &format!("/applications/{application}/environments"),
    )
    .await
}

/// The ids one application's `ListConfigurationProfiles` serves.
async fn listed_profiles(router: &MultiServiceRouter, application: &str) -> Vec<String> {
    listed_ids(
        router,
        &format!("/applications/{application}/configurationprofiles"),
    )
    .await
}

/// The `Id` of every item one listing serves, sorted so a row compares sets rather than
/// the order a listing happens to use.
async fn listed_ids(router: &MultiServiceRouter, uri: &str) -> Vec<String> {
    let answer = json_call(router, "GET", uri, &Value::Null).await;
    assert_eq!(
        answer.status,
        200,
        "the listing {uri:?} must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let mut ids: Vec<String> = answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            item.get("Id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    ids.sort();
    ids
}

/// Write one hosted configuration version into `profile` and answer its version number.
async fn given_version(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    profile: &str,
    content: &[u8],
) -> String {
    let created = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        content.to_vec(),
        &[],
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
        .unwrap_or_else(|| panic!("case {case:?}: no Version-Number was answered"))
}

/// What one row builds under the application whose delete it drives.
struct Row {
    case: &'static str,
    /// Environments created under the application.
    environments: usize,
    /// Configuration profiles created under the application.
    profiles: usize,
    /// Hosted configuration versions written into each profile.
    versions_per_profile: usize,
    /// Whether a deployment is started into the first environment.
    deploy: bool,
    /// The child kind the refusal must name, as the application's own refusal spells it.
    children: &'static str,
}

/// The ids one row built, so the assertions can address them.
struct Built {
    application: String,
    environments: Vec<String>,
    profiles: Vec<String>,
    /// `(profile id, version number)` of every hosted version written.
    hosted: Vec<(String, String)>,
}

/// Build one row's application and the children under it.
async fn build(router: &MultiServiceRouter, row: &Row) -> Built {
    let application = create_application(router, row.case).await;
    let mut environments = Vec::new();
    for index in 0..row.environments {
        environments.push(create_environment(router, &application, &format!("env-{index}")).await);
    }
    let mut profiles = Vec::new();
    let mut hosted = Vec::new();
    for index in 0..row.profiles {
        let profile = create_profile(router, &application, &format!("profile-{index}")).await;
        for version in 0..row.versions_per_profile {
            let number = given_version(
                router,
                row.case,
                &application,
                &profile,
                format!(r#"{{"version":{version}}}"#).as_bytes(),
            )
            .await;
            hosted.push((profile.clone(), number));
        }
        profiles.push(profile);
    }
    if row.deploy {
        let started = json_call(
            router,
            "POST",
            &format!(
                "/applications/{application}/environments/{}/deployments",
                environments[0]
            ),
            &json!({
                "DeploymentStrategyId": ALL_AT_ONCE,
                "ConfigurationProfileId": profiles[0],
                "ConfigurationVersion": hosted[0].1,
            }),
        )
        .await;
        assert_eq!(
            started.status,
            201,
            "case {:?}: StartDeployment must answer 201, got {} / {}",
            row.case,
            started.status,
            String::from_utf8_lossy(&started.body)
        );
    }
    Built {
        application,
        environments,
        profiles,
        hosted,
    }
}

/// An application delete is refused while a child of any gated kind exists under it, the
/// refusal changes nothing, and the same call succeeds once the children are gone.
#[tokio::test]
async fn an_application_delete_is_refused_while_children_exist_and_changes_nothing() {
    let router = router();
    let account = account();

    let rows = [
        Row {
            case: "one environment",
            environments: 1,
            profiles: 0,
            versions_per_profile: 0,
            deploy: false,
            children: "environments",
        },
        Row {
            case: "three environments",
            environments: 3,
            profiles: 0,
            versions_per_profile: 0,
            deploy: false,
            children: "environments",
        },
        Row {
            case: "one child of each kind with a deployment in flight",
            environments: 1,
            profiles: 1,
            versions_per_profile: 1,
            deploy: true,
            children: "environments",
        },
        Row {
            case: "a configuration profile and no environment",
            environments: 0,
            profiles: 1,
            versions_per_profile: 0,
            deploy: false,
            children: "configuration profiles",
        },
        Row {
            case: "two profiles holding hosted versions",
            environments: 0,
            profiles: 2,
            versions_per_profile: 2,
            deploy: false,
            children: "configuration profiles",
        },
    ];

    for row in &rows {
        let case = row.case;
        let built = build(&router, row).await;
        let application = &built.application;
        let uri = format!("/applications/{application}");
        let before = json_call(&router, "GET", &uri, &Value::Null).await.json();
        let environments_before = listed_environments(&router, application).await;
        let profiles_before = listed_profiles(&router, application).await;

        let refused = delete(&router, &uri).await;
        assert_gated(
            case,
            "DeleteApplication",
            &refused,
            &format!(
                "Cannot delete application {application}, because there are still {} existing \
                 under it.",
                row.children
            ),
        );

        // The refusal had no effect: the application still reads back exactly as it did
        // and every child is still listed under it.
        let after = json_call(&router, "GET", &uri, &Value::Null).await;
        assert_eq!(
            after.status,
            200,
            "case {case:?}: a refused delete leaves the application readable, got {} / {}",
            after.status,
            String::from_utf8_lossy(&after.body)
        );
        assert_eq!(
            after.json(),
            before,
            "case {case:?}: a refused delete must leave the application unchanged, got {}",
            after.json()
        );
        assert_eq!(
            listed_environments(&router, application).await,
            environments_before,
            "case {case:?}: a refused delete must leave every environment listed"
        );
        assert_eq!(
            listed_profiles(&router, application).await,
            profiles_before,
            "case {case:?}: a refused delete must leave every configuration profile listed"
        );

        // Taken bottom-up the children go, and then so does the application: the gate is
        // an ORDERING contract, not a resource that cannot be deleted.
        for (profile, version) in &built.hosted {
            let answer = delete(
                &router,
                &format!(
                    "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions/{version}"
                ),
            )
            .await;
            assert_deleted(case, "DeleteHostedConfigurationVersion", &answer);
        }
        for profile in &built.profiles {
            let answer = delete(
                &router,
                &format!("/applications/{application}/configurationprofiles/{profile}"),
            )
            .await;
            assert_deleted(case, "DeleteConfigurationProfile", &answer);
        }
        // With the profiles gone the environments are still in the way, so the gate still
        // refuses the application by the kind that is left.
        if !built.environments.is_empty() {
            let still_refused = delete(&router, &uri).await;
            assert_gated(
                case,
                "DeleteApplication with only environments left",
                &still_refused,
                &format!(
                    "Cannot delete application {application}, because there are still \
                     environments existing under it."
                ),
            );
        }
        for environment in &built.environments {
            let answer = delete(
                &router,
                &format!("/applications/{application}/environments/{environment}"),
            )
            .await;
            assert_deleted(case, "DeleteEnvironment", &answer);
        }

        let deleted = delete(&router, &uri).await;
        assert_deleted(case, "DeleteApplication on the leaf application", &deleted);
        let missed = json_call(&router, "GET", &uri, &Value::Null).await;
        assert_eq!(
            missed.status,
            404,
            "case {case:?}: the deleted application must be gone, got {} / {}",
            missed.status,
            String::from_utf8_lossy(&missed.body)
        );
        assert_eq!(
            missed.string("Message"),
            Some(format!(
                "Application with Id {application} could not be found for account {account}."
            )),
            "case {case:?}: GetApplication must answer the application template, got {}",
            missed.json()
        );
    }
}

/// A LEAF is deletable while a deployment is in flight, and the deployment keeps reporting
/// the configuration version it was started with — the gate is child existence, not
/// deployment state.
///
/// The same pass pins the sibling gate one level down: a configuration profile is refused
/// while it still holds a hosted configuration version, and succeeds once that version has
/// been deleted — including when the version deleted is the one being deployed.
#[tokio::test]
async fn a_leaf_is_deletable_while_a_deployment_is_in_flight() {
    let router = router();
    let case = "a deployment in flight";
    let application = create_application(&router, "in-flight-teardown").await;
    let environment = create_environment(&router, &application, "in-flight-env").await;
    let profile = create_profile(&router, &application, "in-flight-profile").await;
    let version = given_version(&router, case, &application, &profile, br#"{"v":1}"#).await;

    // A strategy that takes twenty minutes to roll out, so the deployment is still in
    // flight for every call below without any test waiting.
    let strategy = json_call(
        &router,
        "POST",
        "/deploymentstrategies",
        &json!({
            "Name": "slow-rollout",
            "DeploymentDurationInMinutes": 20,
            "FinalBakeTimeInMinutes": 0,
            "GrowthFactor": 25.0,
            "GrowthType": "LINEAR",
            "ReplicateTo": "NONE",
        }),
    )
    .await
    .string("Id")
    .expect("CreateDeploymentStrategy answers an Id");

    let started = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": strategy,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": version,
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
    let number = started.json()["DeploymentNumber"]
        .as_i64()
        .expect("StartDeployment answers a DeploymentNumber");
    let environment_state = json_call(
        &router,
        "GET",
        &format!("/applications/{application}/environments/{environment}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        environment_state.string("State").as_deref(),
        Some("Deploying"),
        "case {case:?}: the environment must be Deploying, got {}",
        environment_state.json()
    );

    // The profile still holds the version being deployed, so its own delete is gated.
    let profile_uri = format!("/applications/{application}/configurationprofiles/{profile}");
    assert_gated(
        case,
        "DeleteConfigurationProfile",
        &delete(&router, &profile_uri).await,
        &format!(
            "Cannot delete configuration profile {profile} because there are still hosted \
             configuration versions existing under it."
        ),
    );

    // The leaf — the very version being deployed — is deletable, and the deployment keeps
    // naming it.
    assert_deleted(
        case,
        "DeleteHostedConfigurationVersion of the version being deployed",
        &delete(
            &router,
            &format!("{profile_uri}/hostedconfigurationversions/{version}"),
        )
        .await,
    );
    let deployment = json_call(
        &router,
        "GET",
        &format!(
            "/applications/{application}/environments/{environment}/deployments/{number}"
        ),
        &Value::Null,
    )
    .await;
    assert_eq!(
        deployment.string("ConfigurationVersion").as_deref(),
        Some(version.as_str()),
        "case {case:?}: the deployment must keep naming the deleted version, got {}",
        deployment.json()
    );
    assert_ne!(
        deployment.string("State").as_deref(),
        Some("COMPLETE"),
        "case {case:?}: the deployment must still be in flight, got {}",
        deployment.json()
    );

    // Now a leaf, the profile goes; the environment holding the deployment still blocks
    // the application, and once it goes too the application is deletable.
    assert_deleted(
        case,
        "DeleteConfigurationProfile once it is a leaf",
        &delete(&router, &profile_uri).await,
    );
    let application_uri = format!("/applications/{application}");
    assert_gated(
        case,
        "DeleteApplication",
        &delete(&router, &application_uri).await,
        &format!(
            "Cannot delete application {application}, because there are still environments \
             existing under it."
        ),
    );
    assert_deleted(
        case,
        "DeleteEnvironment with a deployment in flight",
        &delete(
            &router,
            &format!("/applications/{application}/environments/{environment}"),
        )
        .await,
    );
    assert_deleted(
        case,
        "DeleteApplication on the leaf application",
        &delete(&router, &application_uri).await,
    );
}
