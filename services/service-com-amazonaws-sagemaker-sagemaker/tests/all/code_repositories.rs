// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's Git-repository resource:
//! `CreateCodeRepository` -> `DescribeCodeRepository`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness
//! (`Emulator`, `error_code`, `message`, `str_field`) is the one
//! `inference_deployment_lifecycle` already defines for this target.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every name shape and region, the create/describe round trip for every
//! combination of `GitConfig`'s optional members, the refusal of a name already in use,
//! the refusal of a name nothing created — and the message that refusal carries, which
//! the service builds from the request — and the modelled constraints on each bound.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// A minimal valid `GitConfig`: `RepositoryUrl` is its one required member
/// (smithy_query shape=GitConfig).
fn git_config(url: &str) -> Value {
    json!({ "RepositoryUrl": url })
}

/// The `GitConfig` every case that is not about `GitConfig` itself uses.
fn default_git_config() -> Value {
    git_config("https://github.com/aws/amazon-sagemaker-examples")
}

/// RULE: `CreateCodeRepository` mints
/// `arn:aws:sagemaker:<signed region>:<account>:code-repository/<CodeRepositoryName>`,
/// whatever the name's shape and whichever region the caller signed for — the emulator
/// default when the request carries no usable credential scope.
#[tokio::test]
async fn created_code_repository_arns_name_the_signed_region_and_the_stored_name() {
    // Rows differ along the name's shape, including both ends of the modelled
    // `EntityName` bound (len 1..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$ —
    // smithy_query operation=CreateCodeRepository).
    let longest_name = format!("a{}b", "1".repeat(61));
    let names = [
        "r".to_string(),
        "repo1".to_string(),
        "9".to_string(),
        "my-git-repo-with-hyphens".to_string(),
        longest_name,
    ];
    // `CodeRepositoryArn`'s pattern constrains the region component to
    // [a-z0-9\-]{9,16}, so rows sit at that lower bound (9) and above it.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in &names {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");

            let (status, body) = emulator
                .call_in(
                    region,
                    "CreateCodeRepository",
                    &json!({ "CodeRepositoryName": name, "GitConfig": default_git_config() }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200 from CreateCodeRepository, got {status} {body}"
            );
            assert_eq!(
                str_field(&body, "CodeRepositoryArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:code-repository/{name}"),
                "case {case}: CodeRepositoryArn, got {body:?}"
            );

            // The reader reports the same ARN, so the value was stored rather than
            // computed per call.
            let described = emulator
                .ok(
                    "DescribeCodeRepository",
                    &json!({ "CodeRepositoryName": name }),
                )
                .await;
            assert_eq!(
                str_field(&described, "CodeRepositoryArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:code-repository/{name}"),
                "case {case}: DescribeCodeRepository must replay the minted ARN, \
                 got {described:?}"
            );
            assert_eq!(
                str_field(&described, "CodeRepositoryName"),
                name.as_str(),
                "case {case}: CodeRepositoryName, got {described:?}"
            );
        }
    }

    // A credential scope whose region is not a region token cannot enter an ARN (the
    // region component is positional), so the emulator default stands in; and an
    // unsigned request names no region at all.
    for malformed in ["us east 1", "US-EAST-1", "us:east/1"] {
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call_in(
                malformed,
                "CreateCodeRepository",
                &json!({ "CodeRepositoryName": "odd-region", "GitConfig": default_git_config() }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case malformed-region={malformed}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            str_field(&body, "CodeRepositoryArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:code-repository/odd-region"),
            "case malformed-region={malformed}: the ARN must fall back to the default \
             region, got {body:?}"
        );
    }

    let emulator = Emulator::new();
    let (status, body) = emulator
        .call_unsigned(
            "CreateCodeRepository",
            &json!({ "CodeRepositoryName": "unsigned-repo", "GitConfig": default_git_config() }),
        )
        .await;
    assert_eq!(status, 200, "case unsigned: got {status} {body}");
    assert_eq!(
        str_field(&body, "CodeRepositoryArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:code-repository/unsigned-repo"),
        "case unsigned: CodeRepositoryArn, got {body:?}"
    );
}

/// RULE: `DescribeCodeRepository` replays exactly the `GitConfig`
/// `CreateCodeRepository` stored — every member the request carried and no member it
/// did not — and reports `CreationTime` and `LastModifiedTime` as the one instant the
/// create recorded, so a repository nobody has changed reports them equal.
#[tokio::test]
async fn describe_code_repository_replays_exactly_what_create_stored() {
    // Rows differ along which of `GitConfig`'s optional members are present and along
    // the shape of their values, including the published bounds: `RepositoryUrl`
    // len 11..1024 and `Branch` len 1..1024 (smithy_query shape=GitConfig).
    let max_url = format!("https://example.com/{}", "u".repeat(1024 - 20));
    let max_branch = "b".repeat(1024);
    let cases: Vec<(&str, Value)> = vec![
        ("url-only", git_config("https://github.com/aws/repo")),
        (
            "url-and-branch",
            json!({
                "RepositoryUrl": "https://github.com/aws/repo",
                "Branch": "main",
            }),
        ),
        (
            "url-and-secret",
            json!({
                "RepositoryUrl": "https://git-codecommit.us-east-1.amazonaws.com/v1/repos/demo",
                "SecretArn": format!("arn:aws:secretsmanager:us-east-1:{ACCOUNT}:secret:git-creds"),
            }),
        ),
        (
            "every-member",
            json!({
                "RepositoryUrl": "https://gitlab.com/group/sub/project.git",
                "Branch": "release/2026-01",
                "SecretArn": format!("arn:aws:secretsmanager:us-east-1:{ACCOUNT}:secret:other-creds"),
            }),
        ),
        // The shortest URL the modelled pattern ^https://([^/]+)/?.{3,1016}$ admits
        // while clearing the len 11 floor.
        ("shortest-url", git_config("https://g.io/abc")),
        (
            "longest-url-and-branch",
            json!({ "RepositoryUrl": max_url.clone(), "Branch": max_branch.clone() }),
        ),
    ];

    for (case, config) in cases {
        let emulator = Emulator::new();
        let created = emulator
            .ok(
                "CreateCodeRepository",
                &json!({ "CodeRepositoryName": case, "GitConfig": config.clone() }),
            )
            .await;
        let arn = str_field(&created, "CodeRepositoryArn");

        let described = emulator
            .ok(
                "DescribeCodeRepository",
                &json!({ "CodeRepositoryName": case }),
            )
            .await;
        assert_eq!(
            str_field(&described, "CodeRepositoryArn"),
            arn,
            "case {case}: the reader must replay the minted ARN, got {described:?}"
        );
        assert_eq!(
            described.get("GitConfig"),
            Some(&config),
            "case {case}: GitConfig must come back exactly as stored, got {described:?}"
        );

        let created_at = described
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: CreationTime, got {described:?}"));
        let modified_at = described
            .get("LastModifiedTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: LastModifiedTime, got {described:?}"));
        assert_eq!(
            created_at, modified_at,
            "case {case}: an untouched repository was last modified when it was created, \
             got CreationTime {created_at} and LastModifiedTime {modified_at}"
        );

        // A read changes nothing a later caller sees: the second describe is identical.
        let again = emulator
            .ok(
                "DescribeCodeRepository",
                &json!({ "CodeRepositoryName": case }),
            )
            .await;
        assert_eq!(
            again, described,
            "case {case}: reading twice must report the same repository, got {again:?}"
        );
    }
}

/// RULE: repositories are independent records keyed by `CodeRepositoryName`, so each one
/// replays its own `GitConfig` however many others the store holds.
#[tokio::test]
async fn every_stored_repository_replays_its_own_configuration() {
    let emulator = Emulator::new();
    // Rows differ in count: the fourth repository is read back after three siblings
    // exist, so no handler can be reading "the last create".
    let seeded = [
        ("alpha", "https://github.com/aws/alpha", "main"),
        ("beta", "https://github.com/aws/beta", "develop"),
        ("gamma", "https://gitlab.com/g/gamma.git", "trunk"),
        ("delta", "https://g.io/delta", "v1"),
    ];

    for (name, url, branch) in seeded {
        emulator
            .ok(
                "CreateCodeRepository",
                &json!({
                    "CodeRepositoryName": name,
                    "GitConfig": { "RepositoryUrl": url, "Branch": branch },
                }),
            )
            .await;
    }

    for (name, url, branch) in seeded {
        let described = emulator
            .ok(
                "DescribeCodeRepository",
                &json!({ "CodeRepositoryName": name }),
            )
            .await;
        assert_eq!(
            described.get("GitConfig"),
            Some(&json!({ "RepositoryUrl": url, "Branch": branch })),
            "case {name}: each repository replays its own GitConfig, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "CodeRepositoryArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:code-repository/{name}"),
            "case {name}: CodeRepositoryArn, got {described:?}"
        );
    }
}

/// RULE: a `CodeRepositoryName` already in use is refused with HTTP 400
/// `ValidationException`, and the stored repository is left exactly as it was.
#[tokio::test]
async fn creating_a_repository_name_already_in_use_is_refused() {
    // Rows differ in the name's shape and in how the second request differs from the
    // first (same configuration, a different URL, a new member).
    let cases: Vec<(&str, Value)> = vec![
        ("same-config", default_git_config()),
        ("different-url", git_config("https://github.com/other/repo")),
        (
            "extra-member",
            json!({ "RepositoryUrl": "https://github.com/other/repo", "Branch": "main" }),
        ),
    ];

    for (case, second_config) in cases {
        let emulator = Emulator::new();
        let first = emulator
            .ok(
                "CreateCodeRepository",
                &json!({ "CodeRepositoryName": case, "GitConfig": default_git_config() }),
            )
            .await;
        let arn = str_field(&first, "CodeRepositoryArn");

        let (status, parsed) = emulator
            .call(
                "CreateCodeRepository",
                &json!({ "CodeRepositoryName": case, "GitConfig": second_config }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a name already in use must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: error code, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(&arn),
            "case {case}: the refusal must name the existing repository, got {parsed:?}"
        );

        let described = emulator
            .ok(
                "DescribeCodeRepository",
                &json!({ "CodeRepositoryName": case }),
            )
            .await;
        assert_eq!(
            described.get("GitConfig"),
            Some(&default_git_config()),
            "case {case}: the refused create must not overwrite the stored \
             configuration, got {described:?}"
        );
    }
}

/// RULE: `DescribeCodeRepository` naming a repository that is not in state is refused
/// with HTTP 400 `ValidationException` — never a 200 with empty members, and never a
/// 501.
#[tokio::test]
async fn describing_a_repository_nothing_created_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateCodeRepository",
            &json!({ "CodeRepositoryName": "present", "GitConfig": default_git_config() }),
        )
        .await;

    // Rows: a name nothing created, a near-miss of a name that exists, and a name whose
    // only difference is case — the store is keyed by the exact name.
    for absent in ["absent", "present1", "Present", "p"] {
        let (status, parsed) = emulator
            .call(
                "DescribeCodeRepository",
                &json!({ "CodeRepositoryName": absent }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {absent}: expected HTTP 400 for an absent repository, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {absent}: error code, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(&format!("{ACCOUNT}:code-repository/{absent}")),
            "case {absent}: the refusal must name the repository it could not find, \
             got {parsed:?}"
        );
    }
}

/// RULE: the refusal for a name no repository holds carries the message SageMaker builds
/// from the request — `Cannot find CodeRepository: <arn>`, the ARN naming the requested
/// name and the region the caller signed for (the emulator default when the request
/// carries no usable credential scope).
#[tokio::test]
async fn the_refusal_names_the_repository_arn_the_request_asked_for() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateCodeRepository",
            &json!({ "CodeRepositoryName": "present", "GitConfig": default_git_config() }),
        )
        .await;

    // Rows differ along the signed region (including the 9-character lower bound of
    // `CodeRepositoryArn`'s [a-z0-9\-]{9,16} region component) and the requested name's
    // shape, so the message has to be built from the request rather than memorised.
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
                    "DescribeCodeRepository",
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
            "DescribeCodeRepository",
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

    // The stored repository is untouched by the refusals.
    let described = emulator
        .ok(
            "DescribeCodeRepository",
            &json!({ "CodeRepositoryName": "present" }),
        )
        .await;
    assert_eq!(
        str_field(&described, "CodeRepositoryName"),
        "present",
        "the stored repository must survive a refused read, got {described:?}"
    );
}

/// RULE: a request outside a member's modelled constraint is refused with HTTP 400
/// before any state is written.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();

    // Rows: one past each published bound these two operations state —
    // `CodeRepositoryName` len 1..63 and its pattern, `GitConfig` required,
    // `RepositoryUrl` required with len 11..1024 and pattern ^https://...
    // (smithy_query operation=CreateCodeRepository,DescribeCodeRepository).
    let too_long_name = format!("a{}b", "1".repeat(62));
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "name-one-past-the-63-character-maximum",
            "CreateCodeRepository",
            json!({
                "CodeRepositoryName": too_long_name,
                "GitConfig": default_git_config(),
            }),
        ),
        (
            "name-below-the-1-character-minimum",
            "CreateCodeRepository",
            json!({ "CodeRepositoryName": "", "GitConfig": default_git_config() }),
        ),
        (
            "name-outside-the-modelled-pattern",
            "CreateCodeRepository",
            json!({ "CodeRepositoryName": "under_score", "GitConfig": default_git_config() }),
        ),
        (
            "missing-the-required-git-config",
            "CreateCodeRepository",
            json!({ "CodeRepositoryName": "no-config" }),
        ),
        (
            "repository-url-below-the-11-character-minimum",
            "CreateCodeRepository",
            json!({ "CodeRepositoryName": "short-url", "GitConfig": git_config("https://a") }),
        ),
        (
            "repository-url-one-past-the-1024-character-maximum",
            "CreateCodeRepository",
            json!({
                "CodeRepositoryName": "long-url",
                "GitConfig": git_config(&format!("https://example.com/{}", "u".repeat(1024 - 19))),
            }),
        ),
        (
            "repository-url-outside-the-https-pattern",
            "CreateCodeRepository",
            json!({
                "CodeRepositoryName": "not-https",
                "GitConfig": git_config("http://github.com/aws/repo"),
            }),
        ),
        (
            "reader-name-one-past-the-63-character-maximum",
            "DescribeCodeRepository",
            json!({ "CodeRepositoryName": too_long_name }),
        ),
    ];

    for (case, target, body) in cases {
        let (status, parsed) = emulator.call(target, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: {target} must be refused, got {status} {parsed}"
        );
    }

    // Nothing above reached state: every name they named is still absent.
    for name in ["no-config", "short-url", "long-url", "not-https"] {
        let (status, parsed) = emulator
            .call(
                "DescribeCodeRepository",
                &json!({ "CodeRepositoryName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {name}: a refused create must store nothing, got {status} {parsed}"
        );
    }
}
