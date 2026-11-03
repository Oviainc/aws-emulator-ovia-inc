// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateProject`, driven over the wire the router serves, with
//! the same harness the project -> build suite uses (`crate::project_build_lifecycle`).
//!
//! RULE under test: `UpdateProject` identifies a stored project by name and replaces
//! exactly the settings the request carries — each member whole, a structure never
//! merged field by field — leaving every omitted setting and every member another
//! operation owns as stored, and moving `lastModified` only when the call succeeds. A
//! A setting the request replaces is stored with the members the SERVICE supplies
//! resolved — the rule `crate::project_effective_configuration` covers — so each
//! expectation below carries them. A
//! name no project holds is refused with `ResourceNotFoundException`, and a resulting
//! configuration the service cannot build from (a source declaring no input source code
//! yet carrying no inline buildspec) is refused with `InvalidInputException`, with
//! nothing persisted either way.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::SOURCE_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::project_build_lifecycle::{
    call, create_project, create_project_body, emulator, error_type,
};

/// The settings every project below is created with, so "a member the request did not
/// carry is unchanged" is an assertion with something to hold on to: each one is a
/// member `UpdateProject` can also replace.
fn stored_settings() -> Value {
    json!({
        "description": "the stored description",
        "sourceVersion": "refs/heads/main",
        "timeoutInMinutes": 30,
        "queuedTimeoutInMinutes": 15,
        "serviceRole": "arn:aws:iam::000000000000:role/service-role/stored",
        "encryptionKey": "alias/aws/s3",
        "concurrentBuildLimit": 3,
        "autoRetryLimit": 1,
        "badgeEnabled": false,
        "tags": [{"key": "team", "value": "build"}],
        "cache": {"type": "LOCAL", "modes": ["LOCAL_SOURCE_CACHE"]},
        "secondaryArtifacts": [{"type": "NO_ARTIFACTS", "artifactIdentifier": "second"}],
        "fileSystemLocations": [{
            "type": "EFS",
            "location": "fs-1.efs.us-west-2.amazonaws.com:/",
            "mountPoint": "/mnt/one",
            "identifier": "one",
        }],
        "vpcConfig": {"vpcId": "vpc-1", "subnets": ["subnet-1"], "securityGroupIds": ["sg-1"]},
        "logsConfig": {"cloudWatchLogs": {"status": "ENABLED", "groupName": "stored-group"}},
    })
}

/// Send one `UpdateProject` for `name` with `extra`'s members merged in.
async fn update_project(
    router: &MultiServiceRouter,
    name: &str,
    extra: Value,
) -> (u16, Value) {
    let mut body = json!({"name": name});
    if let (Value::Object(target), Value::Object(members)) = (&mut body, extra) {
        target.extend(members);
    }
    call(router, "UpdateProject", body).await
}

/// `UpdateProject`, asserting it succeeded, handing back the project it returned.
async fn update_ok(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    extra: Value,
) -> Value {
    let (status, body) = update_project(router, name, extra).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected UpdateProject to answer 200, got {status} with {body}"
    );
    body["project"].clone()
}

/// The project `name` as the read path reports it.
async fn fetch_project(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(router, "BatchGetProjects", json!({"names": [name]})).await;
    assert_eq!(
        status, 200,
        "BatchGetProjects {name:?}: expected 200, got {status} with {body}"
    );
    body["projects"][0].clone()
}

/// A timestamp member as the seconds-since-epoch number the protocol puts on the wire.
pub(crate) fn instant(project: &Value, member: &str) -> f64 {
    project[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} must be a timestamp, got {project}"))
}

/// Every member either record carries, so "nothing else changed" also catches a member
/// that APPEARED or vanished.
pub(crate) fn all_members(before: &Value, after: &Value) -> Vec<String> {
    let mut members: Vec<String> = before
        .as_object()
        .map(|held| held.keys().cloned().collect())
        .unwrap_or_default();
    if let Some(object) = after.as_object() {
        for member in object.keys() {
            if !members.contains(member) {
                members.push(member.clone());
            }
        }
    }
    members
}

/// RULE: a member the request carries replaces the stored setting whole; every member it
/// omits — and every member another operation owns — is reported exactly as stored, and
/// the read path hands back the same record. The table varies the member, its value, its
/// type (string, number, boolean, structure, list), its entry count and its form (set,
/// replaced, emptied).
#[tokio::test]
async fn update_project_replaces_the_members_it_was_given_and_keeps_the_rest() {
    let router = emulator();

    // (case, the members the request carries, the members that must change and to what)
    let cases: Vec<(&str, Value, Value)> = vec![
        ("nothing but the name", json!({}), json!({})),
        (
            "a replacement description",
            json!({"description": "a new description"}),
            json!({"description": "a new description"}),
        ),
        (
            "a description emptied",
            json!({"description": ""}),
            json!({"description": ""}),
        ),
        (
            "a new inline buildspec",
            json!({"source": {"type": "NO_SOURCE", "buildspec": "version: 0.2\nphases: {}"}}),
            // The replacement source is stored with the flags the service defaults
            // resolved, the same way the create resolved the source it replaces.
            json!({"source": {
                "type": "NO_SOURCE",
                "buildspec": "version: 0.2\nphases: {}",
                "insecureSsl": false,
            }}),
        ),
        (
            "a timeout at the modelled lower bound",
            json!({"timeoutInMinutes": 5}),
            json!({"timeoutInMinutes": 5}),
        ),
        (
            "a timeout at the modelled upper bound",
            json!({"timeoutInMinutes": 2160}),
            json!({"timeoutInMinutes": 2160}),
        ),
        (
            "a queued timeout at its modelled upper bound",
            json!({"queuedTimeoutInMinutes": 480}),
            json!({"queuedTimeoutInMinutes": 480}),
        ),
        (
            "a replacement service role",
            json!({"serviceRole": "arn:aws:iam::000000000000:role/service-role/replaced"}),
            json!({"serviceRole": "arn:aws:iam::000000000000:role/service-role/replaced"}),
        ),
        (
            "a replacement environment",
            json!({"environment": {
                "type": "ARM_CONTAINER",
                "image": "aws/codebuild/amazonlinux2-aarch64-standard:3.0",
                "computeType": "BUILD_GENERAL1_LARGE",
            }}),
            json!({"environment": {
                "type": "ARM_CONTAINER",
                "image": "aws/codebuild/amazonlinux2-aarch64-standard:3.0",
                "computeType": "BUILD_GENERAL1_LARGE",
                "privilegedMode": false,
                "imagePullCredentialsType": "CODEBUILD",
                "environmentVariables": [],
            }}),
        ),
        (
            "two tags replacing one",
            json!({"tags": [{"key": "a", "value": "1"}, {"key": "b", "value": "2"}]}),
            json!({"tags": [{"key": "a", "value": "1"}, {"key": "b", "value": "2"}]}),
        ),
        ("tags emptied", json!({"tags": []}), json!({"tags": []})),
        (
            "three secondary artifacts replacing one",
            json!({"secondaryArtifacts": [
                {"type": "NO_ARTIFACTS", "artifactIdentifier": "a"},
                {"type": "NO_ARTIFACTS", "artifactIdentifier": "b"},
                {"type": "S3", "location": "bucket", "artifactIdentifier": "c"},
            ]}),
            json!({"secondaryArtifacts": [
                {"type": "NO_ARTIFACTS", "artifactIdentifier": "a"},
                {"type": "NO_ARTIFACTS", "artifactIdentifier": "b"},
                {"type": "S3", "location": "bucket", "artifactIdentifier": "c"},
            ]}),
        ),
        (
            "the badge flag turned on",
            json!({"badgeEnabled": true}),
            json!({"badge": {"badgeEnabled": true}}),
        ),
        (
            "a bigger concurrent build limit",
            json!({"concurrentBuildLimit": 20}),
            json!({"concurrentBuildLimit": 20}),
        ),
        (
            "a replacement cache of a different type",
            json!({"cache": {"type": "NO_CACHE"}}),
            json!({"cache": {"type": "NO_CACHE"}}),
        ),
        (
            "a replacement source version",
            json!({"sourceVersion": "refs/tags/v2"}),
            json!({"sourceVersion": "refs/tags/v2"}),
        ),
        (
            "a replacement artifacts configuration",
            json!({"artifacts": {
                "type": "S3",
                "location": "artifact-bucket",
                "name": "out.zip",
                "packaging": "ZIP",
            }}),
            json!({"artifacts": {
                "type": "S3",
                "location": "artifact-bucket",
                "name": "out.zip",
                "packaging": "ZIP",
            }}),
        ),
        (
            "a replacement logs configuration",
            json!({"logsConfig": {"s3Logs": {"status": "ENABLED", "location": "bucket/logs"}}}),
            json!({"logsConfig": {"s3Logs": {"status": "ENABLED", "location": "bucket/logs"}}}),
        ),
        (
            "a replacement vpc configuration",
            json!({"vpcConfig": {
                "vpcId": "vpc-2",
                "subnets": ["subnet-2", "subnet-3"],
                "securityGroupIds": ["sg-2"],
            }}),
            json!({"vpcConfig": {
                "vpcId": "vpc-2",
                "subnets": ["subnet-2", "subnet-3"],
                "securityGroupIds": ["sg-2"],
            }}),
        ),
        (
            "two file system locations replacing one",
            json!({"fileSystemLocations": [
                {"type": "EFS", "location": "fs-2.efs.us-west-2.amazonaws.com:/",
                 "mountPoint": "/mnt/two", "identifier": "two"},
                {"type": "EFS", "location": "fs-3.efs.us-west-2.amazonaws.com:/",
                 "mountPoint": "/mnt/three", "identifier": "three"},
            ]}),
            json!({"fileSystemLocations": [
                {"type": "EFS", "location": "fs-2.efs.us-west-2.amazonaws.com:/",
                 "mountPoint": "/mnt/two", "identifier": "two"},
                {"type": "EFS", "location": "fs-3.efs.us-west-2.amazonaws.com:/",
                 "mountPoint": "/mnt/three", "identifier": "three"},
            ]}),
        ),
        (
            "secondary sources where there were none",
            json!({"secondarySources": [
                {"type": "S3", "location": "bucket/src.zip", "sourceIdentifier": "extra"},
            ]}),
            json!({"secondarySources": [
                {
                    "type": "S3",
                    "location": "bucket/src.zip",
                    "sourceIdentifier": "extra",
                    "insecureSsl": false,
                },
            ]}),
        ),
        (
            "a replacement encryption key",
            json!({"encryptionKey": "arn:aws:kms:us-west-2:000000000000:key/other"}),
            json!({"encryptionKey": "arn:aws:kms:us-west-2:000000000000:key/other"}),
        ),
        (
            "a replacement auto retry limit",
            json!({"autoRetryLimit": 5}),
            json!({"autoRetryLimit": 5}),
        ),
        (
            "two members at once",
            json!({"description": "both", "timeoutInMinutes": 90}),
            json!({"description": "both", "timeoutInMinutes": 90}),
        ),
    ];

    for (index, (case, request, expected)) in cases.iter().enumerate() {
        let name = format!("replace-{index:02}");
        let stored = create_project(&router, &name, stored_settings()).await;

        let updated = update_ok(&router, case, &name, request.clone()).await;

        let changed = expected.as_object().expect("the expectation is an object");
        for (member, want) in changed {
            assert_eq!(
                &updated[member], want,
                "case {case:?}: expected {member} = {want}, got {updated}"
            );
        }
        for member in all_members(&stored, &updated) {
            // `lastModified` is the one member a successful update always moves.
            if member == "lastModified" || changed.contains_key(&member) {
                continue;
            }
            assert_eq!(
                updated[&member], stored[&member],
                "case {case:?}: {member} was not in the request, so it must stay \
                 {:?}, got {:?}",
                stored[&member], updated[&member]
            );
        }
        assert!(
            instant(&updated, "lastModified") > instant(&stored, "lastModified"),
            "case {case:?}: a successful update must move lastModified past {}, got {}",
            instant(&stored, "lastModified"),
            instant(&updated, "lastModified")
        );

        let fetched = fetch_project(&router, &name).await;
        assert_eq!(
            fetched, updated,
            "case {case:?}: BatchGetProjects must report the updated record, got {fetched}"
        );
    }
}

/// RULE (invariant): `lastModified` is strictly increasing across the successful updates
/// of one project, while `created` and the project's identity (name, ARN, visibility)
/// never move — whatever the update carried.
#[tokio::test]
async fn repeated_updates_keep_the_identity_and_advance_last_modified() {
    let router = emulator();
    let stored = create_project(&router, "advance-proj", stored_settings()).await;
    let created = instant(&stored, "created");

    // Deliberately includes a repeat of the same value and an empty request: the stamp
    // records that the project was updated, not that a value differed.
    let requests: Vec<Value> = vec![
        json!({}),
        json!({"description": "first"}),
        json!({"description": "first"}),
        json!({"timeoutInMinutes": 45}),
        json!({"badgeEnabled": true}),
        json!({"concurrentBuildLimit": 9}),
    ];

    let mut previous = instant(&stored, "lastModified");
    for (index, request) in requests.iter().enumerate() {
        let case = format!("update #{index} with {request}");
        let updated = update_ok(&router, &case, "advance-proj", request.clone()).await;

        let moved = instant(&updated, "lastModified");
        assert!(
            moved > previous,
            "case {case:?}: lastModified must advance past {previous}, got {moved}"
        );
        assert_eq!(
            instant(&updated, "created"),
            created,
            "case {case:?}: created must stay {created}, got {}",
            instant(&updated, "created")
        );
        for member in ["name", "arn", "projectVisibility"] {
            assert_eq!(
                updated[member], stored[member],
                "case {case:?}: {member} must stay {:?}, got {:?}",
                stored[member], updated[member]
            );
        }
        let fetched = fetch_project(&router, "advance-proj").await;
        assert_eq!(
            fetched, updated,
            "case {case:?}: BatchGetProjects must report the updated record, got {fetched}"
        );
        previous = moved;
    }
}

/// RULE: a name no project holds is refused with HTTP 400 `ResourceNotFoundException`,
/// and the refusal creates nothing. The table varies how the name misses: never created,
/// created then deleted, and off by case or by suffix from one that exists.
#[tokio::test]
async fn updating_a_project_that_is_not_there_is_refused_and_stores_nothing() {
    let router = emulator();
    create_project(&router, "present-proj", stored_settings()).await;
    create_project(&router, "doomed-proj", stored_settings()).await;
    let (status, deleted) = call(&router, "DeleteProject", json!({"name": "doomed-proj"})).await;
    assert_eq!(
        status, 200,
        "DeleteProject: expected 200, got {status} with {deleted}"
    );

    // (case, the name to update)
    let cases: Vec<(&str, &str)> = vec![
        ("a name never created", "absent-proj"),
        ("a name that was deleted", "doomed-proj"),
        ("a name differing only in case", "PRESENT-PROJ"),
        ("a name differing by a suffix", "present-proj-2"),
        ("a single-character name", "x"),
    ];

    for (case, name) in cases {
        let (status, body) = update_project(&router, name, json!({"description": "x"})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {name:?}, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException for {name:?}, got {body}"
        );

        let (_, fetched) = call(&router, "BatchGetProjects", json!({"names": [name]})).await;
        assert_eq!(
            fetched["projectsNotFound"],
            json!([name]),
            "case {case:?}: a refused update must create nothing, got {fetched}"
        );
    }
}

/// RULE: a project source that declares it has no input source code must carry its
/// buildspec inline — there is no source tree for a buildspec file to live in — so that
/// combination is refused with HTTP 400 `InvalidInputException` wherever a project's
/// source is set, and the refused call changes nothing. Every other source type carries
/// its buildspec in the repository, so none of them needs one here.
///
/// The table walks `SourceType`'s whole modelled value list
/// (`ops::types::SOURCE_TYPE_VALUES`, generated from the Smithy enum), so it covers the
/// types the issue never cited as well as the one it did.
#[tokio::test]
async fn a_source_with_no_input_source_code_must_carry_an_inline_buildspec() {
    let router = emulator();

    for source_type in SOURCE_TYPE_VALUES {
        let case = format!("update source type {source_type} with no buildspec");
        let name = format!("src-{}", source_type.to_lowercase());
        let stored = create_project(&router, &name, stored_settings()).await;

        let mut source = json!({"type": source_type});
        if *source_type != "NO_SOURCE" {
            source["location"] = json!("bucket/src.zip");
        }
        let (status, body) = update_project(&router, &name, json!({"source": source})).await;

        if *source_type == "NO_SOURCE" {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {body}"
            );
            assert!(
                error_type(&body).contains("InvalidInputException"),
                "case {case:?}: expected InvalidInputException, got {body}"
            );
            // "keeps the version": a refused update leaves the whole record alone,
            // lastModified included.
            let fetched = fetch_project(&router, &name).await;
            assert_eq!(
                fetched, stored,
                "case {case:?}: a refused update must leave the stored project \
                 untouched, got {fetched}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a source type that keeps its buildspec in the \
                 repository needs none here, got {status} with {body}"
            );
            assert_eq!(
                body["project"]["source"]["type"], *source_type,
                "case {case:?}: expected the replacement source to be stored, got {body}"
            );
        }
    }

    // The forms of "no inline buildspec" the model names: absent, and an empty string.
    let absent_forms: Vec<(&str, Value)> = vec![
        ("buildspec omitted", json!({"type": "NO_SOURCE"})),
        ("buildspec empty", json!({"type": "NO_SOURCE", "buildspec": ""})),
        (
            "buildspec all whitespace",
            json!({"type": "NO_SOURCE", "buildspec": "   \n "}),
        ),
    ];
    for (index, (case, source)) in absent_forms.iter().enumerate() {
        let name = format!("nospec-{index}");
        let stored = create_project(&router, &name, stored_settings()).await;

        let (status, body) = update_project(&router, &name, json!({"source": source})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        assert_eq!(
            fetch_project(&router, &name).await,
            stored,
            "case {case:?}: a refused update must persist nothing"
        );

        // The same source WITH an inline buildspec is accepted and stored.
        let mut accepted = source.clone();
        accepted["buildspec"] = json!("version: 0.2");
        let updated = update_ok(&router, case, &name, json!({"source": accepted})).await;
        assert_eq!(
            updated["source"]["buildspec"], "version: 0.2",
            "case {case:?}: expected the inline buildspec to be stored, got {updated}"
        );
    }

    // The rule holds at creation too, not only on update.
    let (status, body) = call(
        &router,
        "CreateProject",
        create_project_body("create-nospec", json!({"source": {"type": "NO_SOURCE"}})),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"create with NO_SOURCE and no buildspec\": expected 400, got {status} \
         with {body}"
    );
    assert!(
        error_type(&body).contains("InvalidInputException"),
        "case \"create with NO_SOURCE and no buildspec\": expected \
         InvalidInputException, got {body}"
    );
    let (_, fetched) = call(
        &router,
        "BatchGetProjects",
        json!({"names": ["create-nospec"]}),
    )
    .await;
    assert_eq!(
        fetched["projectsNotFound"],
        json!(["create-nospec"]),
        "case \"create with NO_SOURCE and no buildspec\": nothing may be persisted, \
         got {fetched}"
    );
}

/// RULE: `concurrentBuildLimit` is documented on this operation as removable by the
/// value -1 ("To remove this limit, set this value to -1"), so -1 clears the setting
/// instead of storing a limit, and a later update sets one again.
#[tokio::test]
async fn the_concurrent_build_limit_is_removed_by_the_documented_minus_one() {
    let router = emulator();
    create_project(&router, "limit-proj", stored_settings()).await;

    // (case, the value sent, what a later read must report)
    let cases: Vec<(&str, i64, Value)> = vec![
        ("the smallest limit", 1, json!(1)),
        ("a larger limit", 20, json!(20)),
        ("the documented removal value", -1, Value::Null),
        ("a limit set again after removal", 7, json!(7)),
        ("removed again", -1, Value::Null),
    ];

    for (case, sent, expected) in cases {
        let updated = update_ok(
            &router,
            case,
            "limit-proj",
            json!({"concurrentBuildLimit": sent}),
        )
        .await;
        assert_eq!(
            updated["concurrentBuildLimit"], expected,
            "case {case:?}: sending {sent} must report {expected}, got {updated}"
        );
        let fetched = fetch_project(&router, "limit-proj").await;
        assert_eq!(
            fetched["concurrentBuildLimit"], expected,
            "case {case:?}: BatchGetProjects must report {expected}, got {fetched}"
        );
    }
}
