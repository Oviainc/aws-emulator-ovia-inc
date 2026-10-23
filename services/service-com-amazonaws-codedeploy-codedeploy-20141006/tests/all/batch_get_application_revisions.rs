// SPDX-License-Identifier: CC0-1.0

//! `BatchGetApplicationRevisions`, driven over the wire.
//!
//! RULE under test: `BatchGetApplicationRevisions` answers one `RevisionInfo` per
//! `RevisionLocation` the request carries, in the order the request named them,
//! each holding exactly what `GetApplicationRevision` answers for that one
//! revision of that application; it refuses a batch naming nothing with
//! `RevisionRequiredException`, one past the documented maximum of 25 revisions
//! with `BatchLimitExceededException`, a revision that names no location with
//! `InvalidRevisionException` and an application that is not stored with
//! `ApplicationDoesNotExistException`.
//!
//! The harness (`Emulator`, `error_code`, `s3_revision`, `seeded_group`,
//! `terminal_deployment`) is the lifecycle suite's, so every case here goes
//! through the same front door a client uses.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{
    error_code, s3_revision, seeded_group, terminal_deployment, Emulator,
};

/// The maximum batch size CodeDeploy documents for this operation: "The maximum
/// number of `RevisionLocation` objects you can specify is 25"
/// (`smithy_query(query="doc",shape="BatchGetApplicationRevisionsInput",member="revisions")`;
/// the operation models `BatchLimitExceededException` as the refusal past it).
///
/// Deliberately a SECOND copy of the handler's constant: this suite asserts the
/// bound AWS publishes, so importing the implementation's literal would make the
/// test agree with whatever the handler happens to say.
const MAX_BATCH_REVISIONS: usize = 25;

/// The `revisions` list of a successful answer.
fn revisions(body: &Value, case: &str) -> Vec<Value> {
    body["revisions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a revisions list, got {body:?}"))
        .clone()
}

/// Create `name` as a `Server` application.
async fn application(emulator: &Emulator, name: &str) -> String {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": name, "computePlatform": "Server"}),
        )
        .await;
    name.to_string()
}

/// Register `revision` with `name`, with `description`.
async fn register(emulator: &Emulator, name: &str, revision: &Value, description: &str) {
    emulator
        .ok(
            "RegisterApplicationRevision",
            json!({
                "applicationName": name,
                "revision": revision,
                "description": description,
            }),
        )
        .await;
}

/// Every form the model's `RevisionLocation` carries a location for
/// (`smithy_query(query="shape",shape="RevisionLocation")`: s3Location,
/// gitHubLocation, string, appSpecContent), so no row below depends on one of
/// them.
fn revision_forms() -> Vec<(&'static str, Value)> {
    vec![
        ("S3", s3_revision("artifacts", "prod/app.zip")),
        (
            "S3 with a version and an ETag",
            json!({
                "revisionType": "S3",
                "s3Location": {
                    "bucket": "artifacts",
                    "key": "prod/other.zip",
                    "bundleType": "tar",
                    "version": "2sL4kqtJlcpXroDTDmJ+rmSpXd3dIbrHY",
                    "eTag": "\"7b2cf535f27731c974343645a3985328\"",
                },
            }),
        ),
        (
            "GitHub",
            json!({
                "revisionType": "GitHub",
                "gitHubLocation": {
                    "repository": "my-account/my-app",
                    "commitId": "f3e4a1b2c3d4e5f60718293a4b5c6d7e8f901234",
                },
            }),
        ),
        (
            "String",
            json!({
                "revisionType": "String",
                "string": {"content": "version: 0.0\nos: linux\n", "sha256": "abc123"},
            }),
        ),
        (
            "AppSpecContent",
            json!({
                "revisionType": "AppSpecContent",
                "appSpecContent": {"content": "{\"version\":\"0.0\"}", "sha256": "def456"},
            }),
        ),
    ]
}

/// RULE: every entry of the batch is exactly what `GetApplicationRevision`
/// answers for that one revision — whatever the batch's size, the order it asks
/// in, whether it repeats a revision, and which form of `RevisionLocation` each
/// one carries.
#[tokio::test]
async fn every_batch_entry_equals_the_single_read_of_that_revision() {
    let emulator = Emulator::new();
    let name = application(&emulator, "batch-revisions").await;
    let forms = revision_forms();
    for (case, revision) in &forms {
        register(&emulator, &name, revision, &format!("registered {case}")).await;
    }

    // Rows differ along batch SIZE, the ORDER the revisions are asked in
    // (registration order, reversed, interleaved) and whether one REPEATS.
    let cases: Vec<Vec<usize>> = vec![
        vec![0],
        vec![0, 1],
        vec![4, 3, 2, 1, 0],
        vec![2, 0, 4],
        vec![3, 3],
        (0..forms.len()).collect(),
    ];

    for positions in &cases {
        let case = format!("{positions:?}");
        let asked: Vec<Value> = positions
            .iter()
            .map(|index| forms[*index].1.clone())
            .collect();
        let body = emulator
            .ok(
                "BatchGetApplicationRevisions",
                json!({"applicationName": name, "revisions": asked}),
            )
            .await;
        assert_eq!(
            body["applicationName"].as_str(),
            Some(name.as_str()),
            "case {case}: expected the application name back, got {body:?}"
        );
        let answered = revisions(&body, &case);
        assert_eq!(
            answered.len(),
            asked.len(),
            "case {case}: expected one entry per requested revision, got {answered:?}"
        );

        for (position, index) in positions.iter().enumerate() {
            let (form, revision) = &forms[*index];
            let entry = &answered[position];
            let single = emulator
                .ok(
                    "GetApplicationRevision",
                    json!({"applicationName": name, "revision": revision}),
                )
                .await;
            assert_eq!(
                entry["revisionLocation"], single["revision"],
                "case {case}: expected position {position} ({form}) to carry \
                 GetApplicationRevision's revision {:?}, got {:?}",
                single["revision"], entry["revisionLocation"]
            );
            assert_eq!(
                entry["genericRevisionInfo"], single["revisionInfo"],
                "case {case}: expected position {position} ({form}) to carry \
                 GetApplicationRevision's revisionInfo {:?}, got {:?}",
                single["revisionInfo"], entry["genericRevisionInfo"]
            );
            assert_eq!(
                entry["genericRevisionInfo"]["description"].as_str(),
                Some(format!("registered {form}").as_str()),
                "case {case}: expected the registered description at position {position}, got \
                 {entry:?}"
            );
            assert!(
                entry["genericRevisionInfo"]["registerTime"]
                    .as_f64()
                    .unwrap_or_default()
                    > 0.0,
                "case {case}: expected a registerTime at position {position}, got {entry:?}"
            );
        }
    }
}

/// RULE: a revision the application's registry does not hold is still reported,
/// at the position the request named it, carrying the location the request gave
/// and an empty `GenericRevisionInfo` — this operation models no "revision does
/// not exist" refusal, so one unknown revision does not hide the known ones
/// beside it.
#[tokio::test]
async fn an_unregistered_revision_is_reported_with_no_revision_info() {
    let emulator = Emulator::new();
    let name = application(&emulator, "batch-partial").await;
    let known = s3_revision("artifacts", "known.zip");
    register(&emulator, &name, &known, "the known one").await;

    // Rows differ along the FORM of the unregistered revision, so the answer does
    // not depend on which kind of location is missing.
    let unknown_forms = revision_forms();

    for (form, unknown) in &unknown_forms {
        let case = format!("unregistered {form}");
        let body = emulator
            .ok(
                "BatchGetApplicationRevisions",
                json!({"applicationName": name, "revisions": [known, unknown]}),
            )
            .await;
        let answered = revisions(&body, &case);
        assert_eq!(
            answered.len(),
            2,
            "case {case:?}: expected both requested revisions back, got {answered:?}"
        );
        assert!(
            answered[0]["genericRevisionInfo"]["registerTime"]
                .as_f64()
                .unwrap_or_default()
                > 0.0,
            "case {case:?}: expected the registered revision to keep its registerTime, got {:?}",
            answered[0]
        );
        assert_eq!(
            answered[1]["revisionLocation"], *unknown,
            "case {case:?}: expected the requested location back verbatim, got {:?}",
            answered[1]
        );
        let info = &answered[1]["genericRevisionInfo"];
        assert!(
            info["registerTime"].is_null()
                && info["firstUsedTime"].is_null()
                && info["lastUsedTime"].is_null()
                && info["description"].is_null(),
            "case {case:?}: expected no revision info for an unregistered revision, got {info:?}"
        );

        // The read registered nothing: the revision is still unknown, and a
        // single read of it is still refused.
        let (status, single) = emulator
            .call(
                "GetApplicationRevision",
                json!({"applicationName": name, "revision": unknown}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the batch read to register nothing, got {status} {single:?}"
        );
        assert_eq!(
            error_code(&single),
            "RevisionDoesNotExistException",
            "case {case:?}: expected RevisionDoesNotExistException, got {single:?}"
        );
    }
}

/// RULE: the batch reports what is stored NOW — a revision registered after an
/// earlier batch is reported with its record by a later one, and a revision a
/// deployment used carries that use in the same batch the registry does.
#[tokio::test]
async fn a_batch_follows_the_registry_as_it_changes() {
    let emulator = Emulator::new();
    let name = "batch-tracking".to_string();
    let group = seeded_group(&emulator, &name, "Server").await;
    let deployed = s3_revision("artifacts", "deployed.zip");
    let later = s3_revision("artifacts", "later.zip");

    // Before anything is registered: both revisions are reported, neither with a
    // record.
    let body = emulator
        .ok(
            "BatchGetApplicationRevisions",
            json!({"applicationName": name, "revisions": [deployed, later]}),
        )
        .await;
    for (position, entry) in revisions(&body, "before").iter().enumerate() {
        assert!(
            entry["genericRevisionInfo"]["registerTime"].is_null(),
            "case before: expected position {position} to carry no registerTime, got {entry:?}"
        );
    }

    // A deployment is how CodeDeploy learns a revision nobody registered, and it
    // is what makes that revision a deployment group's target revision.
    let created = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": name,
                "deploymentGroupName": group,
                "revision": deployed,
            }),
        )
        .await;
    let deployment_id = created["deploymentId"]
        .as_str()
        .expect("CreateDeployment mints a deploymentId")
        .to_string();
    terminal_deployment(&emulator, &deployment_id, "deployed").await;
    register(&emulator, &name, &later, "registered after the batch").await;

    let body = emulator
        .ok(
            "BatchGetApplicationRevisions",
            json!({"applicationName": name, "revisions": [deployed, later]}),
        )
        .await;
    let answered = revisions(&body, "after");
    let used = &answered[0]["genericRevisionInfo"];
    assert!(
        used["firstUsedTime"].as_f64().unwrap_or_default() > 0.0
            && used["lastUsedTime"].as_f64().unwrap_or_default() > 0.0,
        "case after: expected use times for the deployed revision, got {used:?}"
    );
    assert_eq!(
        used["deploymentGroups"].as_array().map(Vec::as_slice),
        Some([json!(group)].as_slice()),
        "case after: expected the deployed revision to target {group}, got {used:?}"
    );
    let registered = &answered[1]["genericRevisionInfo"];
    assert_eq!(
        registered["description"].as_str(),
        Some("registered after the batch"),
        "case after: expected the newly registered description, got {registered:?}"
    );
    assert!(
        registered["firstUsedTime"].is_null(),
        "case after: expected no use times for a revision no deployment used, got {registered:?}"
    );

    // Reading the batch changed nothing: the same request answers the same thing.
    let again = emulator
        .ok(
            "BatchGetApplicationRevisions",
            json!({"applicationName": name, "revisions": [deployed, later]}),
        )
        .await;
    assert_eq!(
        again["revisions"], body["revisions"],
        "case repeat: expected the batch to be a read, got {:?} then {:?}",
        body["revisions"], again["revisions"]
    );
}

/// RULE: the documented batch maximum is 25 revisions — a batch of exactly 25 is
/// answered, and one past it is refused with `BatchLimitExceededException` even
/// when every revision it carries is registered.
#[tokio::test]
async fn the_documented_batch_maximum_is_enforced() {
    let emulator = Emulator::new();
    let name = application(&emulator, "batch-bounded").await;
    let locations: Vec<Value> = (0..=MAX_BATCH_REVISIONS)
        .map(|index| s3_revision("artifacts", &format!("bounded-{index:03}.zip")))
        .collect();
    for (index, revision) in locations.iter().enumerate() {
        register(&emulator, &name, revision, &format!("bounded {index}")).await;
    }

    // Each published bound, and one past it.
    for size in [1usize, MAX_BATCH_REVISIONS - 1, MAX_BATCH_REVISIONS] {
        let asked = &locations[..size];
        let body = emulator
            .ok(
                "BatchGetApplicationRevisions",
                json!({"applicationName": name, "revisions": asked}),
            )
            .await;
        let answered = revisions(&body, &format!("size {size}"));
        assert_eq!(
            answered.len(),
            size,
            "case size {size}: expected {size} entries, got {}",
            answered.len()
        );
    }

    let (status, body) = emulator
        .call(
            "BatchGetApplicationRevisions",
            json!({"applicationName": name, "revisions": locations.clone()}),
        )
        .await;
    assert_eq!(
        status,
        400,
        "case size {}: expected HTTP 400 past the maximum, got {status} {body:?}",
        locations.len()
    );
    assert_eq!(
        error_code(&body),
        "BatchLimitExceededException",
        "case size {}: expected BatchLimitExceededException, got {body:?}",
        locations.len()
    );
}

/// RULE: every way a batch can be unacceptable is answered with CodeDeploy's own
/// code for it — the revision-required code for a batch naming no revision, the
/// invalid-revision code for one that names no location, the
/// name-required/invalid-name codes for an application name outside the modelled
/// form, and the does-not-exist code for an application that is simply not
/// stored.
#[tokio::test]
async fn an_unacceptable_batch_is_answered_with_codedeploys_code_for_it() {
    let emulator = Emulator::new();
    let name = application(&emulator, "batch-refusals").await;
    let stored = s3_revision("artifacts", "stored.zip");
    register(&emulator, &name, &stored, "the stored one").await;

    let too_long = "n".repeat(101);
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "absent revisions member",
            json!({"applicationName": name}),
            "RevisionRequiredException",
        ),
        (
            "empty batch",
            json!({"applicationName": name, "revisions": []}),
            "RevisionRequiredException",
        ),
        (
            "a revisionType with no location",
            json!({"applicationName": name, "revisions": [{"revisionType": "S3"}]}),
            "InvalidRevisionException",
        ),
        (
            "a revision with no location beside a stored one",
            json!({"applicationName": name, "revisions": [stored, {}]}),
            "InvalidRevisionException",
        ),
        (
            "absent application name",
            json!({"revisions": [stored]}),
            "ApplicationNameRequiredException",
        ),
        (
            "empty application name",
            json!({"applicationName": "", "revisions": [stored]}),
            "ApplicationNameRequiredException",
        ),
        (
            "application name outside the pattern",
            json!({"applicationName": "has spaces", "revisions": [stored]}),
            "InvalidApplicationNameException",
        ),
        (
            "application name past the length maximum",
            json!({"applicationName": too_long, "revisions": [stored]}),
            "InvalidApplicationNameException",
        ),
        (
            "unstored application",
            json!({"applicationName": "not-stored", "revisions": [stored]}),
            "ApplicationDoesNotExistException",
        ),
    ];

    for (case, request, expected) in &cases {
        let (status, body) = emulator
            .call("BatchGetApplicationRevisions", request.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            *expected,
            "case {case:?}: expected {expected}, got {body:?}"
        );
    }
}
