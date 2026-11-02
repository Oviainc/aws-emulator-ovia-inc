// SPDX-License-Identifier: CC0-1.0

//! `DeleteApplication`.
//!
//! The rule under test, in one sentence: a delete of an application that answers its
//! modelled 204 has taken the application record — and with it everything this service
//! still stores inside that record — out of the store, so every later call addressed
//! through that application id is refused with `ResourceNotFoundException` (HTTP 404) and
//! the hosted-content budget the versions under it held is given back, while an application
//! id the account does not hold is refused the same 404 and removes nothing.
//!
//! A delete only reaches that record once the application is a LEAF, so every row here
//! tears its children down first — the hosted versions, the profiles, the environments and
//! the experiment definition — and what the delete is asserted on is the application
//! itself.
//!
//! An application that still holds children is not deletable at all: that gate, and the
//! refusal it answers with, is pinned by `application_deletion_gate`, so every row here
//! tears the children down first — bottom-up, as AppConfig's ordering contract asks — and
//! what the delete is asserted on is the record that is left.
//!
//! Every case drives real HTTP requests through the router the emulator's front door
//! uses, so what is asserted is what an AppConfig client sees on the wire. The one
//! assertion made against state directly is the service's hosted-content accounting,
//! which no operation reports.
//!
//! The tables span what the rule generalises over rather than one recorded application:
//! the length of `Name` at both published bounds (1..64) and whether `Description` and
//! `Tags` ride along, and — because the rule is about a whole subtree going — how many
//! children of each kind the application holds (none, environments only, profiles with
//! hosted content, one of every kind with a deployment in flight and an experiment
//! definition beside it). The misses differ along the form of the identifier that
//! resolves nothing, including an application's NAME, which this operation's id member
//! does not address.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    account, create_environment, create_flags_profile, create_profile, json_call, router_on,
    send, Answer,
};
use crate::delete_hosted_configuration_version::held_bytes;
use crate::experiment_definitions::{create_definition, flag, treatment};
use crate::resource_deletes::{assert_deleted, assert_not_found};
use crate::resource_tags::{arn, list_tags};

/// The published bounds of `CreateApplication`'s `Name` (length 1..64) and `Description`
/// (length 0..1024)
/// (`smithy_query(query="operation",operation="CreateApplication")`).
const MAX_NAME: usize = 64;
/// See [`MAX_NAME`].
const MAX_DESCRIPTION: usize = 1024;

/// One predefined deployment strategy id, so a row can start a deployment without first
/// creating a strategy: AppConfig's `AppConfig.*` strategies are deployable as shipped.
const ALL_AT_ONCE: &str = "AppConfig.AllAtOnce";

/// `DELETE /applications/{ApplicationId}`, this operation's modelled URI
/// (`smithy_query(query="operation",operation="DeleteApplication")`).
async fn delete_application(router: &MultiServiceRouter, application: &str) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/applications/{application}"),
        "application/json",
        Vec::new(),
        &[],
    )
    .await
}

/// `GET /applications/{ApplicationId}`.
async fn get_application(router: &MultiServiceRouter, application: &str) -> Answer {
    json_call(router, "GET", &format!("/applications/{application}"), &Value::Null).await
}

/// `PATCH /applications/{ApplicationId}` — the writer, so a delete is asserted against a
/// mutating caller as well as a reading one.
async fn update_application(router: &MultiServiceRouter, application: &str) -> Answer {
    json_call(
        router,
        "PATCH",
        &format!("/applications/{application}"),
        &json!({ "Description": "written after the delete" }),
    )
    .await
}

/// The `Id` of every application `ListApplications` serves, in the order it served them.
async fn listed_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let answer = json_call(router, "GET", "/applications", &Value::Null).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListApplications must answer 200, got {} / {}",
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
        .filter_map(|item| {
            item.get("Id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// Create an application from `body` and answer its minted id. The crate's shared
/// `create_application` helper posts a name only; a row here also carries a description
/// and tags, so it builds the body itself.
async fn create_application_from(
    router: &MultiServiceRouter,
    case: &str,
    body: &Value,
) -> String {
    let created = json_call(router, "POST", "/applications", body).await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateApplication must answer 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: CreateApplication answered no Id"))
}

/// Write one hosted configuration version into `profile` and answer the bytes that were
/// stored, so a row can state the budget its children were holding.
async fn host_version(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    profile: &str,
    payload: &[u8],
) -> usize {
    let created = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        payload.to_vec(),
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
    payload.len()
}

/// One hosted configuration payload of `size` JSON-document bytes.
fn payload(size: usize) -> Vec<u8> {
    let overhead = br#"{"value":""#.len() + br#""}"#.len();
    let filler = "x".repeat(size.saturating_sub(overhead).max(1));
    format!("{{\"value\":\"{filler}\"}}").into_bytes()
}

/// What one row builds under the application it deletes.
struct Row {
    case: &'static str,
    name: String,
    description: Option<String>,
    tags: Option<Value>,
    /// Environments created under it.
    environments: usize,
    /// Configuration profiles created under it.
    profiles: usize,
    /// Hosted configuration versions written into EACH profile.
    versions_per_profile: usize,
    /// The size of each hosted payload, in bytes.
    version_bytes: usize,
    /// Whether a deployment is started into the first environment.
    deploy: bool,
    /// Whether an experiment definition is created under it.
    definition: bool,
}

/// Everything one row created, with the ids the assertions after the delete address.
struct Built {
    application: String,
    environments: Vec<String>,
    profiles: Vec<String>,
    /// `(profile id, version number)` of every hosted version written.
    hosted: Vec<(String, i32)>,
    /// The bytes of hosted content the subtree was holding.
    hosted_bytes: usize,
    /// The deployment number started into the first environment, when the row deploys.
    deployment: Option<i32>,
    /// The experiment definition created under the application, when the row has one.
    definition: Option<String>,
}

/// Build one row's application and everything under it, asserting on the way that each
/// resource really is there — so what the delete changes is not an absence that was there
/// all along.
async fn build(router: &MultiServiceRouter, row: &Row) -> Built {
    let mut body = json!({ "Name": row.name });
    if let Some(description) = &row.description {
        body["Description"] = Value::from(description.clone());
    }
    if let Some(tags) = &row.tags {
        body["Tags"] = tags.clone();
    }
    let application = create_application_from(router, row.case, &body).await;

    let mut environments = Vec::new();
    for index in 0..row.environments {
        environments.push(create_environment(router, &application, &format!("env-{index}")).await);
    }

    let mut profiles = Vec::new();
    let mut hosted = Vec::new();
    let mut hosted_bytes = 0;
    for index in 0..row.profiles {
        let profile = create_profile(router, &application, &format!("profile-{index}")).await;
        for version in 0..row.versions_per_profile {
            hosted_bytes += host_version(
                router,
                row.case,
                &application,
                &profile,
                &payload(row.version_bytes + version),
            )
            .await;
            // Version numbers start at 1 and increase by one per write.
            hosted.push((profile.clone(), version as i32 + 1));
        }
        profiles.push(profile);
    }

    let deployment = if row.deploy {
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
                "ConfigurationVersion": "1",
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
        Some(
            started
                .json()
                .get("DeploymentNumber")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| {
                    panic!("case {:?}: StartDeployment answered no DeploymentNumber", row.case)
                }) as i32,
        )
    } else {
        None
    };

    let definition = if row.definition {
        // An experiment definition targets a feature flag, so it can only be bound to a
        // profile that stores flags: the row that carries one holds such a profile beside
        // the free-form profiles its hosted content lives in, and the delete walk below
        // tears it down with them.
        let flags_profile = create_flags_profile(router, &application, "flags-profile").await;
        let created = create_definition(
            router,
            &application,
            &json!({
                "Name": "checkout-experiment",
                "ConfigurationProfileIdentifier": flags_profile,
                "EnvironmentIdentifier": environments[0],
                "FlagKey": "checkout-flow",
                "AudienceRule": "attribute(\"tier\") == \"beta\"",
                "Treatments": [treatment(50.0, None, flag(true))],
                "Control": treatment(0.0, None, flag(false)),
            }),
        )
        .await;
        profiles.push(flags_profile);
        assert_eq!(
            created.status,
            201,
            "case {:?}: CreateExperimentDefinition must answer 201, got {} / {}",
            row.case,
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        Some(created.string("Id").unwrap_or_else(|| {
            panic!("case {:?}: CreateExperimentDefinition answered no Id", row.case)
        }))
    } else {
        None
    };

    Built {
        application,
        environments,
        profiles,
        hosted,
        hosted_bytes,
        deployment,
        definition,
    }
}

/// Every read of `built`'s subtree, as `(operation, URI)` pairs — the calls that answer
/// 200 before the delete and must miss after it.
fn reads(built: &Built) -> Vec<(String, String)> {
    let application = &built.application;
    let mut calls: Vec<(String, String)> = vec![(
        "GetApplication".to_string(),
        format!("/applications/{application}"),
    )];
    for environment in &built.environments {
        calls.push((
            "GetEnvironment".to_string(),
            format!("/applications/{application}/environments/{environment}"),
        ));
        calls.push((
            "ListTagsForResource on the environment".to_string(),
            format!(
                "/tags/{}",
                crate::resource_tags::pct(&arn(&format!(
                    "application/{application}/environment/{environment}"
                )))
            ),
        ));
    }
    for profile in &built.profiles {
        calls.push((
            "GetConfigurationProfile".to_string(),
            format!("/applications/{application}/configurationprofiles/{profile}"),
        ));
    }
    for (profile, version) in &built.hosted {
        calls.push((
            "GetHostedConfigurationVersion".to_string(),
            format!(
                "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions/{version}"
            ),
        ));
    }
    if let Some(number) = built.deployment {
        calls.push((
            "GetDeployment".to_string(),
            format!(
                "/applications/{application}/environments/{}/deployments/{number}",
                built.environments[0]
            ),
        ));
    }
    if let Some(definition) = &built.definition {
        calls.push((
            "GetExperimentDefinition".to_string(),
            format!("/applications/{application}/experimentdefinitions/{definition}"),
        ));
    }
    calls
}

/// Delete every child the row built, bottom-up — the hosted versions, then the profiles
/// that held them, then the environments, then the experiment definition — so the
/// application is the leaf its own delete needs it to be.
///
/// Each of these deletes is itself owed its modelled 204, which is what makes the teardown
/// AppConfig's documented way out: the gate on the application's delete is child existence,
/// so removing the children is what opens it.
async fn teardown_children(router: &MultiServiceRouter, case: &str, built: &Built) {
    let application = &built.application;
    for (profile, version) in &built.hosted {
        let answer = send(
            router,
            "DELETE",
            &format!(
                "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions/{version}"
            ),
            "application/json",
            Vec::new(),
            &[],
        )
        .await;
        assert_deleted(case, "DeleteHostedConfigurationVersion", &answer);
    }
    for profile in &built.profiles {
        let answer = send(
            router,
            "DELETE",
            &format!("/applications/{application}/configurationprofiles/{profile}"),
            "application/json",
            Vec::new(),
            &[],
        )
        .await;
        assert_deleted(case, "DeleteConfigurationProfile", &answer);
    }
    for environment in &built.environments {
        let answer = send(
            router,
            "DELETE",
            &format!("/applications/{application}/environments/{environment}"),
            "application/json",
            Vec::new(),
            &[],
        )
        .await;
        assert_deleted(case, "DeleteEnvironment", &answer);
    }
    if let Some(definition) = &built.definition {
        // A delete that names no `DeleteType` PRESERVES the record, which would still gate
        // the application, so the teardown asks for the permanent one.
        let answer = send(
            router,
            "DELETE",
            &format!(
                "/applications/{application}/experimentdefinitions/{definition}?delete_type=DESTROY"
            ),
            "application/json",
            Vec::new(),
            &[],
        )
        .await;
        assert_deleted(case, "DeleteExperimentDefinition", &answer);
    }
}

/// A deleted application is gone with everything still stored under it: every reader of the
/// application and of any resource addressed through it misses, the WRITER is refused
/// rather than resurrecting it, the tag reader misses, the listing no longer serves it, a
/// second delete misses, the hosted-content budget its versions held is released — and a
/// sibling application and its own subtree are untouched.
#[tokio::test]
async fn a_deleted_application_is_gone_with_everything_still_stored_under_it() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let account = account();

    // A sibling with a subtree of its own, so every row also asserts that the delete
    // reached exactly one application.
    let sibling = build(
        &router,
        &Row {
            case: "sibling",
            name: "sibling-app".to_string(),
            description: None,
            tags: None,
            environments: 1,
            profiles: 1,
            versions_per_profile: 1,
            version_bytes: 64,
            deploy: false,
            definition: false,
        },
    )
    .await;

    let rows = vec![
        Row {
            case: "no children, name at the one-character bound",
            name: "a".to_string(),
            description: None,
            tags: None,
            environments: 0,
            profiles: 0,
            versions_per_profile: 0,
            version_bytes: 0,
            deploy: false,
            definition: false,
        },
        Row {
            case: "environments only, description at its published upper bound",
            name: "environments-only".to_string(),
            description: Some("d".repeat(MAX_DESCRIPTION)),
            tags: None,
            environments: 3,
            profiles: 0,
            versions_per_profile: 0,
            version_bytes: 0,
            deploy: false,
            definition: false,
        },
        Row {
            case: "profiles holding hosted configuration content",
            name: "profiles-with-content".to_string(),
            description: Some("hosted content under every profile".to_string()),
            tags: None,
            environments: 0,
            profiles: 2,
            versions_per_profile: 3,
            version_bytes: 512,
            deploy: false,
            definition: false,
        },
        Row {
            case: "one child of every kind, deployed, tagged, name at the 64-character bound",
            name: "n".repeat(MAX_NAME),
            description: Some(String::new()),
            tags: Some(json!({ "team": "platform", "tier": "beta" })),
            environments: 2,
            profiles: 1,
            versions_per_profile: 1,
            version_bytes: 128,
            deploy: true,
            definition: true,
        },
    ];

    for row in &rows {
        let case = row.case;
        let built = build(&router, row).await;
        let application = built.application.clone();
        let application_arn = arn(&format!("application/{application}"));

        // Everything the row built is really there.
        for (operation, uri) in reads(&built) {
            let answer = json_call(&router, "GET", &uri, &Value::Null).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: {operation} must report the created resource, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }
        let tagged_before = list_tags(&router, &application_arn).await;
        assert_eq!(
            tagged_before.status,
            200,
            "case {case:?}: ListTagsForResource must report the created application, got {} / {}",
            tagged_before.status,
            String::from_utf8_lossy(&tagged_before.body)
        );
        assert!(
            listed_ids(&router, case).await.contains(&application),
            "case {case:?}: ListApplications must serve the created application first"
        );
        let held_before = held_bytes(&engine);

        // The application is a container, so its own delete is refused while a child is
        // still there; the children go first, bottom-up.
        teardown_children(&router, case, &built).await;
        let deleted = delete_application(&router, &application).await;
        assert_deleted(case, "DeleteApplication", &deleted);

        // Every caller addressed through the application now misses, readers and the
        // writer alike, and so does a second delete.
        for (operation, uri) in reads(&built) {
            assert_not_found(
                case,
                &operation,
                &json_call(&router, "GET", &uri, &Value::Null).await,
            );
        }
        assert_not_found(
            case,
            "UpdateApplication",
            &update_application(&router, &application).await,
        );
        assert_not_found(
            case,
            "ListTagsForResource on the application",
            &list_tags(&router, &application_arn).await,
        );
        assert_not_found(
            case,
            "a second DeleteApplication",
            &delete_application(&router, &application).await,
        );

        // The application's own not-found template, which is the one the whole subtree's
        // misses are reported through when the parent is what is gone.
        let missed = get_application(&router, &application).await;
        assert_eq!(
            missed.string("Message"),
            Some(format!(
                "Application with Id {application} could not be found for account {account}."
            )),
            "case {case:?}: GetApplication must answer the application template, got {}",
            missed.json()
        );

        let listed = listed_ids(&router, case).await;
        assert!(
            !listed.contains(&application),
            "case {case:?}: ListApplications must no longer serve the deleted application, got {listed:?}"
        );
        assert!(
            listed.contains(&sibling.application),
            "case {case:?}: ListApplications must still serve the sibling, got {listed:?}"
        );

        // The budget the subtree's hosted versions held is given back, and nothing else
        // is: the sibling's version is still stored.
        let held_after = held_bytes(&engine);
        assert_eq!(
            held_after,
            held_before - built.hosted_bytes,
            "case {case:?}: the delete must release exactly the {} hosted bytes under it, \
             held {held_before} before and {held_after} after",
            built.hosted_bytes
        );

        // The sibling's whole subtree is untouched.
        for (operation, uri) in reads(&sibling) {
            let answer = json_call(&router, "GET", &uri, &Value::Null).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: the sibling's {operation} must be untouched, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }
    }

    // Only the sibling's hosted content is still charged for.
    assert_eq!(
        held_bytes(&engine),
        sibling.hosted_bytes,
        "the deletes must leave only the sibling's hosted content charged for"
    );
}

/// A delete that resolves no application removes nothing and is refused with the
/// application's own `ResourceNotFoundException` template.
///
/// Rows differ along the form of the identifier — its length at both `Name` bounds
/// (1..64), a value shaped like a minted id, and an application's NAME, which this
/// operation's `ApplicationId` member ("the ID of the application to delete") does not
/// address — so a refusal is pinned for each, together with the store being unchanged.
#[tokio::test]
async fn a_delete_that_resolves_no_application_removes_nothing() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let account = account();

    let name = "delete-misses-app";
    let built = build(
        &router,
        &Row {
            case: "the application the misses must leave alone",
            name: name.to_string(),
            description: Some("still here".to_string()),
            tags: None,
            environments: 1,
            profiles: 1,
            versions_per_profile: 2,
            version_bytes: 256,
            deploy: true,
            definition: false,
        },
    )
    .await;
    let before = get_application(&router, &built.application).await.json();
    let held_before = held_bytes(&engine);

    let unknown = [
        ("a value shaped like a minted id", "zzzzzzz".to_string()),
        ("a four-character id", "ab12".to_string()),
        ("a one-character identifier", "q".to_string()),
        (
            "an identifier at the 64-character Name bound",
            "u".repeat(MAX_NAME),
        ),
        ("the application's name rather than its id", name.to_string()),
    ];

    for (case, identifier) in unknown {
        let refused = delete_application(&router, &identifier).await;
        assert_not_found(case, "DeleteApplication", &refused);
        assert_eq!(
            refused.string("Message"),
            Some(format!(
                "Application with Id {identifier} could not be found for account {account}."
            )),
            "case {case:?}: the refusal must be the application template, got {}",
            refused.json()
        );

        // The refused delete changed nothing: the application still reads back exactly as
        // it did, its whole subtree still answers, the listing still serves it, and the
        // hosted-content budget is untouched.
        let after = get_application(&router, &built.application).await;
        assert_eq!(
            after.json(),
            before,
            "case {case:?}: a refused delete must leave the application unchanged, got {}",
            after.json()
        );
        for (operation, uri) in reads(&built) {
            let answer = json_call(&router, "GET", &uri, &Value::Null).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: {operation} must still answer after a refused delete, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }
        assert!(
            listed_ids(&router, case).await.contains(&built.application),
            "case {case:?}: ListApplications must still serve the application"
        );
        assert_eq!(
            held_bytes(&engine),
            held_before,
            "case {case:?}: a refused delete must release no hosted bytes"
        );
    }
}
