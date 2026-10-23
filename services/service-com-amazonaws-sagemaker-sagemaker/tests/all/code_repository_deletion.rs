// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `DeleteCodeRepository`.
//!
//! RULE: `DeleteCodeRepository` removes exactly the Git-repository record its
//! `CodeRepositoryName` names and nothing else — the operation's model doc says it
//! "Deletes the specified Git repository from your account", and `CreateCodeRepository`'s
//! says the repository "persists independently from the lifecycle of any notebook
//! instances it is associated with". So after it: `DescribeCodeRepository` refuses that
//! name, a second delete of it is refused, every other repository and every notebook
//! instance referencing the deleted one is unchanged, and the freed name can be created
//! again. The operation models no errors, so a name nothing created is refused with the
//! common HTTP 400 `ValidationException` the sibling deletes use.
//!
//! The cases assert that rule over a TABLE whose rows differ along the axes it
//! generalises over: the repository's name shape (both ends of the modelled `EntityName`
//! bound), the region the caller signed for, the `GitConfig` the repository carries, and
//! how many sibling repositories the store holds when the delete runs.
//!
//! The harness (`Emulator`, `error_code`, `message`, `str_field`) is the one
//! `tests/all/inference_deployment_lifecycle.rs` already defines, so these cases go over
//! the same wire through the same `MultiServiceRouter` as every other suite here.

use serde_json::{json, Value};

use super::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// A minimal valid `GitConfig`: `RepositoryUrl` is its one required member
/// (smithy_query shape=GitConfig).
fn git_config(url: &str) -> Value {
    json!({ "RepositoryUrl": url })
}

/// The `GitConfig` a case that is not about `GitConfig` itself uses.
fn default_git_config() -> Value {
    git_config("https://github.com/aws/amazon-sagemaker-examples")
}

/// Create `count` repositories the delete under test must leave alone, returning their
/// names paired with the `GitConfig` each one stored.
async fn seed_siblings(emulator: &Emulator, count: usize) -> Vec<(String, Value)> {
    let mut seeded = Vec::new();
    for index in 0..count {
        let name = format!("sibling{index}");
        let config = json!({
            "RepositoryUrl": format!("https://github.com/aws/{name}"),
            "Branch": format!("branch-{index}"),
        });
        emulator
            .ok(
                "CreateCodeRepository",
                &json!({ "CodeRepositoryName": name, "GitConfig": config.clone() }),
            )
            .await;
        seeded.push((name, config));
    }
    seeded
}

/// RULE: `DeleteCodeRepository` removes exactly the named repository — the reader refuses
/// that name afterwards, a second delete of it is refused, every sibling repository
/// survives, and the freed name is creatable again — and it answers HTTP 200 with the
/// modelled empty output.
#[tokio::test]
async fn deleting_a_repository_removes_exactly_the_named_repository() {
    // `CodeRepositoryName` is modelled `EntityName`, len 1..63 with pattern
    // ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$ (smithy_query operation=DeleteCodeRepository),
    // so one row sits on each end of that bound.
    let longest_name = format!("a{}b", "1".repeat(61));
    // Rows: name shape x signed region x stored `GitConfig` x sibling count.
    let cases: Vec<(String, &str, Value, usize)> = vec![
        ("r".to_string(), "us-east-1", default_git_config(), 0),
        (
            "9".to_string(),
            "eu-west-1",
            json!({ "RepositoryUrl": "https://g.io/abc", "Branch": "main" }),
            1,
        ),
        (
            "my-git-repo-with-hyphens".to_string(),
            "ap-southeast-2",
            json!({
                "RepositoryUrl": "https://git-codecommit.us-east-1.amazonaws.com/v1/repos/demo",
                "Branch": "release/2026-01",
                "SecretArn": format!("arn:aws:secretsmanager:us-east-1:{ACCOUNT}:secret:git-creds"),
            }),
            3,
        ),
        (longest_name.clone(), "us-west-2", default_git_config(), 2),
    ];

    for (name, region, config, siblings) in cases {
        let emulator = Emulator::new();
        let case = format!("region={region} name={name} siblings={siblings}");
        let seeded = seed_siblings(&emulator, siblings).await;

        let (create_status, created) = emulator
            .call_in(
                region,
                "CreateCodeRepository",
                &json!({ "CodeRepositoryName": name, "GitConfig": config.clone() }),
            )
            .await;
        assert_eq!(
            create_status, 200,
            "case {case}: expected the create to succeed, got {create_status} {created}"
        );
        let arn = str_field(&created, "CodeRepositoryArn");

        let (delete_status, deleted) = emulator
            .call_in(
                region,
                "DeleteCodeRepository",
                &json!({ "CodeRepositoryName": name }),
            )
            .await;
        assert_eq!(
            delete_status, 200,
            "case {case}: expected HTTP 200 from the delete, got {delete_status} {deleted}"
        );
        assert_eq!(
            deleted,
            json!({}),
            "case {case}: the modelled output has no members, got {deleted:?}"
        );

        // The repository is gone: its reader refuses the name, naming the ARN.
        let (read_status, read_body) = emulator
            .call_in(
                region,
                "DescribeCodeRepository",
                &json!({ "CodeRepositoryName": name }),
            )
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: the deleted repository must be gone, got {read_status} {read_body}"
        );
        assert_eq!(
            error_code(&read_body),
            "ValidationException",
            "case {case}: error code after the delete, got {read_body:?}"
        );
        assert_eq!(
            message(&read_body),
            format!("Cannot find CodeRepository: {arn}"),
            "case {case}: the refusal must name the repository it could not find, got {:?}",
            message(&read_body)
        );

        // A second delete is refused the same way: the entry really went.
        let (repeat_status, repeat_body) = emulator
            .call_in(
                region,
                "DeleteCodeRepository",
                &json!({ "CodeRepositoryName": name }),
            )
            .await;
        assert_eq!(
            repeat_status, 400,
            "case {case}: deleting twice must be refused, got {repeat_status} {repeat_body}"
        );
        assert_eq!(
            error_code(&repeat_body),
            "ValidationException",
            "case {case}: error code for the second delete, got {repeat_body:?}"
        );
        assert!(
            message(&repeat_body).contains(&arn),
            "case {case}: the second delete must name the repository it could not find, \
             got {repeat_body:?}"
        );

        // Only that entry went: every sibling still replays its own configuration.
        for (sibling, sibling_config) in &seeded {
            let described = emulator
                .ok(
                    "DescribeCodeRepository",
                    &json!({ "CodeRepositoryName": sibling }),
                )
                .await;
            assert_eq!(
                described.get("GitConfig"),
                Some(sibling_config),
                "case {case}: sibling {sibling} must survive the delete unchanged, \
                 got {described:?}"
            );
        }

        // The name is released, so a create naming it succeeds again, mints the same ARN
        // and stores the NEW configuration — the delete left no remnant of the old one.
        let recreated_config = git_config("https://github.com/aws/recreated-after-delete");
        let (recreate_status, recreated) = emulator
            .call_in(
                region,
                "CreateCodeRepository",
                &json!({ "CodeRepositoryName": name, "GitConfig": recreated_config.clone() }),
            )
            .await;
        assert_eq!(
            recreate_status, 200,
            "case {case}: the freed name must be creatable again, got \
             {recreate_status} {recreated}"
        );
        assert_eq!(
            str_field(&recreated, "CodeRepositoryArn"),
            arn,
            "case {case}: the re-created repository keeps the same ARN, got {recreated:?}"
        );
        let (replay_status, replayed) = emulator
            .call_in(
                region,
                "DescribeCodeRepository",
                &json!({ "CodeRepositoryName": name }),
            )
            .await;
        assert_eq!(
            replay_status, 200,
            "case {case}: the re-created repository must be readable, got \
             {replay_status} {replayed}"
        );
        assert_eq!(
            replayed.get("GitConfig"),
            Some(&recreated_config),
            "case {case}: the re-created repository replays its own GitConfig, \
             got {replayed:?}"
        );
    }
}

/// RULE: `DeleteCodeRepository` naming a repository that is not in state is refused with
/// HTTP 400 `ValidationException` whose message names the ARN built from the request —
/// the requested name and the region the caller signed for (the emulator default when
/// the request carries no usable credential scope) — and nothing in state changes.
#[tokio::test]
async fn deleting_a_repository_nothing_created_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateCodeRepository",
            &json!({ "CodeRepositoryName": "present", "GitConfig": default_git_config() }),
        )
        .await;

    // Rows differ along the signed region (including the 9-character lower bound of
    // `CodeRepositoryArn`'s [a-z0-9\-]{9,16} region component) and the requested name's
    // shape: a name nothing created, near-misses of one that exists, and a name whose
    // only difference is case — the store is keyed by the exact name.
    let absent_names = [
        "absent",
        "present1",
        "Present",
        "p",
        "9",
        "a-name-with-hyphens",
    ];
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        for name in absent_names {
            let case = format!("region={region} name={name}");
            let (status, parsed) = emulator
                .call_in(
                    region,
                    "DeleteCodeRepository",
                    &json!({ "CodeRepositoryName": name }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {case}: error code, got {parsed:?}"
            );
            assert_eq!(
                message(&parsed),
                format!(
                    "Cannot find CodeRepository: \
                     arn:aws:sagemaker:{region}:{ACCOUNT}:code-repository/{name}"
                ),
                "case {case}: refusal message, got {:?}",
                message(&parsed)
            );
        }
    }

    // An unsigned request names no region, so the ARN in the message carries the
    // emulator's default.
    let (status, parsed) = emulator
        .call_unsigned(
            "DeleteCodeRepository",
            &json!({ "CodeRepositoryName": "absent" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case unsigned: expected HTTP 400, got {status} {parsed}"
    );
    assert_eq!(
        message(&parsed),
        format!(
            "Cannot find CodeRepository: \
             arn:aws:sagemaker:us-east-1:{ACCOUNT}:code-repository/absent"
        ),
        "case unsigned: refusal message, got {:?}",
        message(&parsed)
    );

    // Every refusal above left the one stored repository exactly as it was.
    let described = emulator
        .ok(
            "DescribeCodeRepository",
            &json!({ "CodeRepositoryName": "present" }),
        )
        .await;
    assert_eq!(
        described.get("GitConfig"),
        Some(&default_git_config()),
        "the stored repository must survive a refused delete, got {described:?}"
    );
}

/// RULE: a `DeleteCodeRepository` request outside the modelled constraint on
/// `CodeRepositoryName` is refused with HTTP 400 before any state is touched.
#[tokio::test]
async fn delete_requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateCodeRepository",
            &json!({ "CodeRepositoryName": "keepme", "GitConfig": default_git_config() }),
        )
        .await;

    // Rows: one past each published bound the operation states — `CodeRepositoryName`
    // required, len 1..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$ (smithy_query
    // operation=DeleteCodeRepository).
    let too_long_name = format!("a{}b", "1".repeat(62));
    let cases: Vec<(&str, Value)> = vec![
        (
            "name-one-past-the-63-character-maximum",
            json!({ "CodeRepositoryName": too_long_name }),
        ),
        (
            "name-below-the-1-character-minimum",
            json!({ "CodeRepositoryName": "" }),
        ),
        (
            "name-outside-the-modelled-pattern",
            json!({ "CodeRepositoryName": "under_score" }),
        ),
        ("missing-the-required-name", json!({})),
    ];

    for (case, body) in cases {
        let (status, parsed) = emulator.call("DeleteCodeRepository", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: DeleteCodeRepository must be refused, got {status} {parsed}"
        );
    }

    let described = emulator
        .ok(
            "DescribeCodeRepository",
            &json!({ "CodeRepositoryName": "keepme" }),
        )
        .await;
    assert_eq!(
        str_field(&described, "CodeRepositoryName"),
        "keepme",
        "a refused delete must touch no repository, got {described:?}"
    );
}

/// RULE: a repository's lifecycle is independent of the notebook instances that reference
/// it (`CreateCodeRepository`'s doc), so deleting it deletes no notebook instance and
/// leaves every member the notebook instance stored — including the repository names it
/// carries — unchanged.
#[tokio::test]
async fn deleting_a_repository_leaves_the_notebook_instances_that_reference_it() {
    // The three required members of `CreateNotebookInstance`; `RoleArn` satisfies its
    // modelled pattern ^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$ and
    // `InstanceType` is a member of the modelled `InstanceType` enum.
    let role = format!("arn:aws:iam::{ACCOUNT}:role/SageMakerNotebookRole");

    // Rows differ in which reference the notebook instance holds: the default
    // repository, the additional list, or both.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "default-only",
            "repo-default",
            json!({ "DefaultCodeRepository": "repo-default" }),
        ),
        (
            "additional-only",
            "repo-extra",
            json!({ "AdditionalCodeRepositories": ["repo-extra"] }),
        ),
        (
            "both",
            "repo-both",
            json!({
                "DefaultCodeRepository": "repo-both",
                "AdditionalCodeRepositories": ["repo-both", "repo-other"],
            }),
        ),
    ];

    for (case, repository, references) in cases {
        let emulator = Emulator::new();
        for name in [repository, "repo-other"] {
            emulator
                .ok(
                    "CreateCodeRepository",
                    &json!({ "CodeRepositoryName": name, "GitConfig": default_git_config() }),
                )
                .await;
        }

        let mut create_body = json!({
            "NotebookInstanceName": format!("nb-{case}"),
            "InstanceType": "ml.t3.medium",
            "RoleArn": role,
        });
        for (key, value) in references
            .as_object()
            .expect("the reference members are a JSON object")
        {
            create_body[key] = value.clone();
        }
        emulator.ok("CreateNotebookInstance", &create_body).await;
        let before = emulator
            .ok(
                "DescribeNotebookInstance",
                &json!({ "NotebookInstanceName": format!("nb-{case}") }),
            )
            .await;

        emulator
            .ok(
                "DeleteCodeRepository",
                &json!({ "CodeRepositoryName": repository }),
            )
            .await;

        let after = emulator
            .ok(
                "DescribeNotebookInstance",
                &json!({ "NotebookInstanceName": format!("nb-{case}") }),
            )
            .await;
        for member in [
            "NotebookInstanceName",
            "NotebookInstanceArn",
            "DefaultCodeRepository",
            "AdditionalCodeRepositories",
            "CreationTime",
        ] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case}: {member} must be unchanged by deleting the repository, \
                 got {after:?}"
            );
        }
    }
}
