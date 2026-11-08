// SPDX-License-Identifier: CC0-1.0

//! `UpdateApplication`: renaming a stored application, driven over the wire.
//!
//! RULE under test: `UpdateApplication` renames the application `applicationName`
//! identifies to `newApplicationName` — the SAME resource under a new name, so its
//! identity (`applicationId`, compute platform, creation time) and everything that
//! references it (its deployment groups, its deployment history, its position in a
//! listing) follow the new name and the old name stops resolving; a name another
//! application already holds is refused, and a request that names no application at
//! all is refused.
//!
//! Every case goes through the same front door a client uses, reusing the router
//! harness of `codedeploy_lifecycle`, so the boundary validator, this service's
//! refusal wording and the awsJson1_1 envelope are all exercised.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, s3_revision, Emulator};

/// The deployment group every seeded application here carries.
const GROUP: &str = "dg";

/// What an application and its dependents answered BEFORE a rename, so the rename
/// can be checked to have carried them rather than re-minted them.
struct Seeded {
    application_id: String,
    create_time: Value,
    group_id: String,
    deployment_id: String,
    /// The revision `RegisterApplicationRevision` filed against the application,
    /// which is the key `GetApplicationRevision` reads it back by.
    registered_revision: Value,
}

/// Create `application` on `platform` with one deployment group, one registered
/// application revision and one deployment, answering what they reported.
async fn seed(emulator: &Emulator, application: &str, platform: &str) -> Seeded {
    let created = emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": platform}),
        )
        .await;
    let application_id = created["applicationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !application_id.is_empty(),
        "case {application}: expected a minted applicationId, got {created:?}"
    );

    let group = emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": application,
                "deploymentGroupName": GROUP,
                "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
            }),
        )
        .await;
    let deployment = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": GROUP,
                "revision": s3_revision("my-bucket", "app.zip"),
            }),
        )
        .await;

    let registered_revision = s3_revision("my-bucket", "registered.zip");
    emulator
        .ok(
            "RegisterApplicationRevision",
            json!({
                "applicationName": application,
                "revision": registered_revision,
                "description": "seeded before the rename",
            }),
        )
        .await;

    let fetched = emulator
        .ok("GetApplication", json!({"applicationName": application}))
        .await;

    Seeded {
        application_id,
        registered_revision,
        create_time: fetched["application"]["createTime"].clone(),
        group_id: group["deploymentGroupId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        deployment_id: deployment["deploymentId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    }
}

/// The deployment ids of a `ListDeployments` body.
fn deployment_ids(body: &Value) -> Vec<String> {
    body["deployments"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

/// The message of a failure body: the modelled `message` member, which the
/// awsJson envelope renders under the casing the model gives it.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string()
}

/// RULE: a renamed application is the same resource under a new name — its id,
/// compute platform and creation time are unchanged, its deployment group and its
/// deployment history answer under the new name, and the old name no longer
/// resolves.
#[tokio::test]
async fn renaming_an_application_moves_the_record_and_everything_that_names_it() {
    let emulator = Emulator::new();
    // Rows differ along the compute platform and along the FORM of the new name,
    // spanning the `ApplicationName` constraints the model states — length 1..100,
    // pattern ^[A-Za-z0-9+=,.@_-]*$, from
    // smithy_query(query="operation",operation="UpdateApplication"): a plain name,
    // every punctuation character the pattern admits, the published minimum length
    // and the published maximum length.
    let longest = "m".repeat(100);
    let cases: &[(&str, &str, &str, &str)] = &[
        ("a plain name", "Server", "plain-old", "plain-new"),
        (
            "every punctuation character the pattern admits",
            "Lambda",
            "punctuated.old",
            "renamed+=,.@_-0",
        ),
        ("the published minimum length of 1", "ECS", "min-old", "x"),
        (
            "the published maximum length of 100",
            "Server",
            "max-old",
            longest.as_str(),
        ),
    ];

    for (case, platform, old, new) in cases {
        let seeded = seed(&emulator, old, platform).await;

        let (status, body) = emulator
            .call(
                "UpdateApplication",
                json!({"applicationName": old, "newApplicationName": new}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from UpdateApplication, got {status} {body:?}"
        );

        // The record kept its identity and is answered under the new name.
        let fetched = emulator
            .ok("GetApplication", json!({"applicationName": new}))
            .await;
        let info = &fetched["application"];
        assert_eq!(
            info["applicationName"].as_str(),
            Some(*new),
            "case {case}: expected the new name on the stored record, got {info:?}"
        );
        assert_eq!(
            info["applicationId"].as_str(),
            Some(seeded.application_id.as_str()),
            "case {case}: expected the applicationId to survive the rename, got {info:?}"
        );
        assert_eq!(
            info["computePlatform"].as_str(),
            Some(*platform),
            "case {case}: expected the compute platform to survive the rename, got {info:?}"
        );
        assert_eq!(
            info["createTime"], seeded.create_time,
            "case {case}: expected the creation time to survive the rename, got {info:?}"
        );

        // The old name stops resolving, in the listing as well as in the read.
        let (status, body) = emulator
            .call("GetApplication", json!({"applicationName": old}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ApplicationDoesNotExistException"),
            "case {case}: expected the old name gone, got {status} {body:?}"
        );
        let listed = emulator.ok("ListApplications", json!({})).await;
        let names = listed["applications"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            names.iter().any(|name| name.as_str() == Some(*new)),
            "case {case}: expected ListApplications to carry the new name, got {names:?}"
        );
        assert!(
            names.iter().all(|name| name.as_str() != Some(*old)),
            "case {case}: expected ListApplications to drop the old name, got {names:?}"
        );

        // The deployment group came with it, same id, under the new name.
        let group = emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": new, "deploymentGroupName": GROUP}),
            )
            .await;
        let group_info = &group["deploymentGroupInfo"];
        assert_eq!(
            group_info["deploymentGroupId"].as_str(),
            Some(seeded.group_id.as_str()),
            "case {case}: expected the group's id to survive the rename, got {group_info:?}"
        );
        assert_eq!(
            group_info["applicationName"].as_str(),
            Some(*new),
            "case {case}: expected the group to report the new name, got {group_info:?}"
        );
        assert_eq!(
            group_info["lastAttemptedDeployment"]["deploymentId"].as_str(),
            Some(seeded.deployment_id.as_str()),
            "case {case}: expected the group to still report its last deployment, got {group_info:?}"
        );
        let (status, body) = emulator
            .call(
                "GetDeploymentGroup",
                json!({"applicationName": old, "deploymentGroupName": GROUP}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ApplicationDoesNotExistException"),
            "case {case}: expected the group unreachable under the old name, got {status} {body:?}"
        );

        // So did the deployment history.
        let deployment = emulator
            .ok(
                "GetDeployment",
                json!({"deploymentId": seeded.deployment_id}),
            )
            .await;
        assert_eq!(
            deployment["deploymentInfo"]["applicationName"].as_str(),
            Some(*new),
            "case {case}: expected the deployment to report the new name, got {deployment:?}"
        );
        let listed = emulator
            .ok(
                "ListDeployments",
                json!({"applicationName": new, "deploymentGroupName": GROUP}),
            )
            .await;
        assert_eq!(
            deployment_ids(&listed),
            vec![seeded.deployment_id.clone()],
            "case {case}: expected the deployment listed under the new name, got {listed:?}"
        );
        let (status, body) = emulator
            .call(
                "ListDeployments",
                // The two filters are a pair the model requires together, so the
                // old name is asked about with the group it used to hold.
                json!({"applicationName": old, "deploymentGroupName": GROUP}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ApplicationDoesNotExistException"),
            "case {case}: expected no listing under the old name, got {status} {body:?}"
        );

        // And so did the registered application revisions.
        let revision = emulator
            .ok(
                "GetApplicationRevision",
                json!({"applicationName": new, "revision": seeded.registered_revision}),
            )
            .await;
        assert_eq!(
            revision["applicationName"].as_str(),
            Some(*new),
            "case {case}: expected the revision under the new name, got {revision:?}"
        );
        assert_eq!(
            revision["revision"], seeded.registered_revision,
            "case {case}: expected the revision verbatim, got {revision:?}"
        );
        let listed = emulator
            .ok("ListApplicationRevisions", json!({"applicationName": new}))
            .await;
        assert!(
            listed["revisions"]
                .as_array()
                .is_some_and(|revisions| revisions.contains(&seeded.registered_revision)),
            "case {case}: expected the revision listed under the new name, got {listed:?}"
        );
        let (status, body) = emulator
            .call(
                "GetApplicationRevision",
                json!({"applicationName": old, "revision": seeded.registered_revision}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ApplicationDoesNotExistException"),
            "case {case}: expected no revision under the old name, got {status} {body:?}"
        );
    }
}

/// RULE: a rename that asks for no new name changes nothing — a request carrying
/// no `newApplicationName`, and one naming the application's own current name,
/// both succeed and leave the stored record exactly as it was.
#[tokio::test]
async fn a_rename_that_asks_for_no_change_leaves_the_record_alone() {
    let emulator = Emulator::new();
    // Rows differ along HOW the request asks for nothing: the member omitted, and
    // the member sent as the name the application already has.
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("newApplicationName omitted", "unchanged-omitted", None),
        (
            "newApplicationName equal to the current name",
            "unchanged-same",
            Some("unchanged-same"),
        ),
    ];

    for (case, name, new) in cases {
        let seeded = seed(&emulator, name, "Server").await;
        let before = emulator
            .ok("GetApplication", json!({"applicationName": name}))
            .await;

        let mut request = json!({"applicationName": name});
        if let Some(new) = new {
            request["newApplicationName"] = json!(new);
        }
        let (status, body) = emulator.call("UpdateApplication", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from {request}, got {status} {body:?}"
        );

        let after = emulator
            .ok("GetApplication", json!({"applicationName": name}))
            .await;
        assert_eq!(
            after, before,
            "case {case}: expected the record untouched, got {after:?}"
        );
        assert_eq!(
            after["application"]["applicationId"].as_str(),
            Some(seeded.application_id.as_str()),
            "case {case}: expected the applicationId untouched, got {after:?}"
        );

        // Its dependents are still reachable under the one name it has.
        let group = emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": name, "deploymentGroupName": GROUP}),
            )
            .await;
        assert_eq!(
            group["deploymentGroupInfo"]["deploymentGroupId"].as_str(),
            Some(seeded.group_id.as_str()),
            "case {case}: expected the group untouched, got {group:?}"
        );
    }
}

/// RULE: renaming does not reorder the account's applications — a listing reports
/// the renamed application in the position it was created in.
#[tokio::test]
async fn renaming_keeps_the_applications_listing_order() {
    let emulator = Emulator::new();
    for name in ["order-a", "order-b", "order-c"] {
        emulator
            .ok("CreateApplication", json!({"applicationName": name}))
            .await;
    }

    emulator
        .ok(
            "UpdateApplication",
            json!({"applicationName": "order-b", "newApplicationName": "order-b-renamed"}),
        )
        .await;

    let listed = emulator.ok("ListApplications", json!({})).await;
    let names: Vec<String> = listed["applications"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|name| name.as_str().map(str::to_string))
        .collect();
    assert_eq!(
        names,
        vec![
            "order-a".to_string(),
            "order-b-renamed".to_string(),
            "order-c".to_string()
        ],
        "case renamed in place: expected the creation order kept, got {names:?}"
    );
}

/// RULE: a rename is refused — with the modelled code for what is wrong with the
/// request — when it names no application, names one that does not exist, or asks
/// for a name another application already holds; and a refused rename changes
/// nothing.
#[tokio::test]
async fn a_refused_rename_names_the_modelled_code_and_changes_nothing() {
    let emulator = Emulator::new();
    for name in ["occupied-a", "occupied-b"] {
        emulator
            .ok("CreateApplication", json!({"applicationName": name}))
            .await;
    }

    // One row per way the request is wrong. The length rows sit on the published
    // bounds of `ApplicationName` (1..100) and one character past the maximum; the
    // pattern rows carry a character ^[A-Za-z0-9+=,.@_-]*$ does not admit. A
    // `@length` minimum failure is "you did not give me this name" to CodeDeploy,
    // which is the same refusal as omitting the member. Where the refusal is the
    // service's own, the row also pins AWS's sentence for that error shape; the
    // rows the boundary refuses keep the constraint clause it builds, so they
    // carry no expected sentence here.
    let too_long = "n".repeat(101);
    let cases: &[(&str, Value, &str, Option<&str>)] = &[
        (
            "no applicationName at all",
            json!({"newApplicationName": "free-name"}),
            "ApplicationNameRequiredException",
            Some("Application name is missing"),
        ),
        (
            "an empty applicationName",
            json!({"applicationName": "", "newApplicationName": "free-name"}),
            "ApplicationNameRequiredException",
            None,
        ),
        (
            "an empty newApplicationName",
            json!({"applicationName": "occupied-a", "newApplicationName": ""}),
            "ApplicationNameRequiredException",
            None,
        ),
        (
            "an applicationName the pattern does not admit",
            json!({"applicationName": "has spaces", "newApplicationName": "free-name"}),
            "InvalidApplicationNameException",
            None,
        ),
        (
            "a newApplicationName the pattern does not admit",
            json!({"applicationName": "occupied-a", "newApplicationName": "has spaces"}),
            "InvalidApplicationNameException",
            None,
        ),
        (
            "a newApplicationName one character past the maximum length",
            json!({"applicationName": "occupied-a", "newApplicationName": too_long.as_str()}),
            "InvalidApplicationNameException",
            None,
        ),
        (
            "an applicationName one character past the maximum length",
            json!({"applicationName": too_long.as_str(), "newApplicationName": "free-name"}),
            "InvalidApplicationNameException",
            None,
        ),
        (
            "an application that does not exist",
            json!({"applicationName": "ghost", "newApplicationName": "free-name"}),
            "ApplicationDoesNotExistException",
            Some("No application found for name: ghost"),
        ),
        (
            "a new name another application already holds",
            json!({"applicationName": "occupied-a", "newApplicationName": "occupied-b"}),
            "ApplicationAlreadyExistsException",
            // The RENAME names the member that conflicts: the name the request
            // asked to become, not the one it asked to create.
            Some("The application name: occupied-b you want to update to already exist"),
        ),
    ];

    for (case, request, expected, expected_message) in cases {
        let (status, body) = emulator.call("UpdateApplication", request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {case}: expected HTTP 400 {expected}, got {status} {body:?}"
        );
        if let Some(expected_message) = expected_message {
            assert_eq!(
                error_message(&body).as_str(),
                *expected_message,
                "case {case}: expected the message {expected_message:?}, got {body:?}"
            );
        }
    }

    // Nothing a refusal touched: both applications still answer under their own
    // names, and no new name was minted.
    for name in ["occupied-a", "occupied-b"] {
        let fetched = emulator
            .ok("GetApplication", json!({"applicationName": name}))
            .await;
        assert_eq!(
            fetched["application"]["applicationName"].as_str(),
            Some(name),
            "case refusals leave state alone: expected {name} intact, got {fetched:?}"
        );
    }
    let listed = emulator.ok("ListApplications", json!({})).await;
    let names = listed["applications"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        names.len(),
        2,
        "case refusals leave state alone: expected exactly the two created applications, got {names:?}"
    );
    assert!(
        names.iter().all(|name| name.as_str() != Some("free-name")),
        "case refusals leave state alone: expected no renamed application, got {names:?}"
    );
}
