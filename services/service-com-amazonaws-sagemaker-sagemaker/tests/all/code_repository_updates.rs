// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's Git-repository MUTATION path:
//! `UpdateCodeRepository`, read back through `DescribeCodeRepository`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness
//! (`Emulator`, `error_code`, `message`, `str_field`) is the one
//! `inference_deployment_lifecycle` defines for this target.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! update applies the `GitConfig` members the request carries and leaves the rest of the
//! record as it was, it moves `LastModifiedTime`, it answers the repository's own ARN,
//! and a name this account holds no repository under — never created, or deleted — is
//! refused with the one sentence every code-repository operation words that fact with.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, f64_field, message, str_field, Emulator};

/// The repository URL every case registers, which no update member can change
/// (`GitConfigForUpdate` carries `SecretArn` alone — smithy_query
/// shape=GitConfigForUpdate).
const REPOSITORY_URL: &str = "https://github.com/aws/amazon-sagemaker-examples";

/// A `SecretArn` matching `GitConfig$SecretArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:secretsmanager:[a-z0-9\-]*:[0-9]{12}:secret:.*$`).
fn secret_arn(name: &str) -> String {
    format!("arn:aws:secretsmanager:us-east-1:000000000000:secret:{name}")
}

/// How every operation that cannot resolve a code repository words it, built here from
/// the request rather than imported, so the wording itself is under test.
fn not_found(region: &str, name: &str) -> String {
    format!(
        "Cannot find CodeRepository: \
         arn:aws:sagemaker:{region}:000000000000:code-repository/{name}"
    )
}

/// Register `name` in `region`, with the `GitConfig` members `extra` carries.
async fn create(emulator: &Emulator, region: &str, name: &str, extra: &Value) -> Value {
    let mut git_config = json!({ "RepositoryUrl": REPOSITORY_URL });
    for (member, value) in extra.as_object().expect("a JSON object of GitConfig members") {
        git_config[member] = value.clone();
    }
    let (status, created) = emulator
        .call_in(
            region,
            "CreateCodeRepository",
            &json!({ "CodeRepositoryName": name, "GitConfig": git_config }),
        )
        .await;
    assert_eq!(
        status, 200,
        "seeding {region}/{name}: CreateCodeRepository, got {status} with {created}"
    );
    created
}

/// One row of the update round trip: the case, the Region, the repository's name, the
/// `SecretArn` the create carried, the members the update sends and the `SecretArn` the
/// reader must report afterwards.
struct UpdateRow {
    case: &'static str,
    region: &'static str,
    name: String,
    seeded_secret: Option<String>,
    update: Value,
    expected_secret: Option<String>,
}

/// RULE: `UpdateCodeRepository` applies the `GitConfig` members the request carries to the
/// stored repository, leaves every member no update member addresses exactly as it was,
/// moves `LastModifiedTime` past `CreationTime` and answers the repository's own ARN.
#[tokio::test]
async fn an_update_applies_the_members_it_carries_and_answers_the_stored_arn() {
    // Rows differ along what the rule generalises over: the Region signed for, the name's
    // shape (`EntityName` is len 1..63, `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$` — smithy_query
    // operation=UpdateCodeRepository), whether the create carried a `SecretArn`, and what
    // the update carries (a new secret, no `GitConfig` at all, an empty `GitConfig`).
    let longest = format!("r{}x", "9".repeat(61));
    let rows = vec![
        UpdateRow {
            case: "a secret attached to a repository that had none",
            region: "us-east-1",
            name: "r".to_string(),
            seeded_secret: None,
            update: json!({ "GitConfig": { "SecretArn": secret_arn("first") } }),
            expected_secret: Some(secret_arn("first")),
        },
        UpdateRow {
            case: "a secret replaced",
            region: "eu-west-1",
            name: "repo-with-secret".to_string(),
            seeded_secret: Some(secret_arn("old")),
            update: json!({ "GitConfig": { "SecretArn": secret_arn("new") } }),
            expected_secret: Some(secret_arn("new")),
        },
        UpdateRow {
            case: "no GitConfig at all leaves the stored credentials",
            region: "ap-southeast-2",
            name: longest,
            seeded_secret: Some(secret_arn("kept")),
            update: json!({}),
            expected_secret: Some(secret_arn("kept")),
        },
        UpdateRow {
            case: "an empty GitConfig leaves the stored credentials",
            region: "us-east-1",
            name: "9".to_string(),
            seeded_secret: Some(secret_arn("also-kept")),
            update: json!({ "GitConfig": {} }),
            expected_secret: Some(secret_arn("also-kept")),
        },
        UpdateRow {
            case: "no GitConfig on a repository that never had a secret",
            region: "eu-west-1",
            name: "no-secret-at-all".to_string(),
            seeded_secret: None,
            update: json!({}),
            expected_secret: None,
        },
    ];

    for row in rows {
        let UpdateRow {
            case,
            region,
            name,
            seeded_secret,
            update,
            expected_secret,
        } = row;
        let emulator = Emulator::new();
        let created_extra = match &seeded_secret {
            Some(arn) => json!({ "SecretArn": arn }),
            None => json!({}),
        };
        let created = create(&emulator, region, &name, &created_extra).await;
        let arn = str_field(&created, "CodeRepositoryArn");
        let before = emulator
            .call_in(
                region,
                "DescribeCodeRepository",
                &json!({ "CodeRepositoryName": &name }),
            )
            .await
            .1;

        let mut body = json!({ "CodeRepositoryName": &name });
        for (member, value) in update.as_object().expect("a JSON object of update members") {
            body[member] = value.clone();
        }
        let (status, updated) = emulator.call_in(region, "UpdateCodeRepository", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateCodeRepository, got {status} \
             with {updated}"
        );
        assert_eq!(
            str_field(&updated, "CodeRepositoryArn"),
            arn,
            "case {case:?}: the update must answer the repository's own ARN, got {updated:?}"
        );

        // The round trip: the reader replays what the update left.
        let (status, described) = emulator
            .call_in(
                region,
                "DescribeCodeRepository",
                &json!({ "CodeRepositoryName": &name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeCodeRepository, got {status} with {described}"
        );
        assert_eq!(
            described
                .pointer("/GitConfig/SecretArn")
                .and_then(Value::as_str)
                .map(str::to_string),
            expected_secret,
            "case {case:?}: GitConfig.SecretArn after the update, got {described:?}"
        );
        assert_eq!(
            described
                .pointer("/GitConfig/RepositoryUrl")
                .and_then(Value::as_str),
            Some(REPOSITORY_URL),
            "case {case:?}: no update member addresses RepositoryUrl, so it must be \
             unchanged, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "CodeRepositoryArn"),
            arn,
            "case {case:?}: CodeRepositoryArn, got {described:?}"
        );
        assert_eq!(
            f64_field(&described, "CreationTime"),
            f64_field(&before, "CreationTime"),
            "case {case:?}: an update must not move CreationTime, got {described:?}"
        );
        assert!(
            f64_field(&described, "LastModifiedTime") >= f64_field(&before, "LastModifiedTime"),
            "case {case:?}: the update must move LastModifiedTime, got {described:?} \
             after {before:?}"
        );
    }
}

/// RULE: a `CodeRepositoryName` this account holds no repository under is refused with a
/// 400 `ValidationException` whose message names the ARN that name resolves to, whether
/// the name was never created or has just been deleted — and the update stores nothing, so
/// the reader still finds no repository.
#[tokio::test]
async fn an_update_of_a_repository_the_account_does_not_hold_is_refused() {
    // Rows differ along HOW the name misses: never created, deleted, or differing from a
    // stored one in case (the name is matched as a whole) — and along the Region, so the
    // ARN the refusal names is the one the caller signed for.
    for region in ["us-east-1", "eu-west-1"] {
        let emulator = Emulator::new();
        create(&emulator, region, "live-repo", &json!({})).await;
        create(&emulator, region, "doomed-repo", &json!({})).await;
        let deleted = emulator
            .call_in(
                region,
                "DeleteCodeRepository",
                &json!({ "CodeRepositoryName": "doomed-repo" }),
            )
            .await;
        assert_eq!(
            deleted.0, 200,
            "seeding {region}: DeleteCodeRepository, got {deleted:?}"
        );

        for name in ["never-created", "doomed-repo", "Live-Repo"] {
            let case = format!("{region}/{name}");
            let (status, refused) = emulator
                .call_in(
                    region,
                    "UpdateCodeRepository",
                    &json!({
                        "CodeRepositoryName": name,
                        "GitConfig": { "SecretArn": secret_arn("unused") },
                    }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case}: error code, got {refused:?}"
            );
            assert_eq!(
                message(&refused),
                not_found(region, name),
                "case {case}: the refusal must name the ARN the request resolves to, \
                 got {refused:?}"
            );

            // Nothing was created as a side effect: the reader words the same fact.
            let (read_status, read) = emulator
                .call_in(
                    region,
                    "DescribeCodeRepository",
                    &json!({ "CodeRepositoryName": name }),
                )
                .await;
            assert_eq!(
                (read_status, message(&read)),
                (400, not_found(region, name)),
                "case {case}: a refused update must store nothing, got {read_status} \
                 with {read}"
            );
        }

        // The repository that IS held still updates, so the refusals above are about the
        // name and not about the operation.
        let (status, updated) = emulator
            .call_in(
                region,
                "UpdateCodeRepository",
                &json!({
                    "CodeRepositoryName": "live-repo",
                    "GitConfig": { "SecretArn": secret_arn("live") },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {region}/live-repo: the held repository must still update, got {status} \
             with {updated}"
        );
    }
}
