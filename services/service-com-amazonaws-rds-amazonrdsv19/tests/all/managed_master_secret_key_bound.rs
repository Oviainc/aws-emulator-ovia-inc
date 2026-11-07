// SPDX-License-Identifier: CC0-1.0

//! The bound on `MasterUserSecretKmsKeyId`, the one member of the managed-master-password
//! path whose value this service STORES on the resource.
//!
//! The RULE under test: a caller-supplied free-form value this service stores is bounded
//! where it enters the store — a value longer than [`st::MAX_FREE_FORM_LEN`] is refused
//! with `InvalidParameterValue` naming the member, and nothing is created for it — exactly
//! as every other stored free-form member of `CreateDBCluster` is (`CharacterSetName`,
//! `KmsKeyId`, `MonitoringRoleArn`, …). `MasterUserSecretKmsKeyId` publishes no length of
//! its own, so this service's own ceiling is the bound it is held to; the member's value is
//! stored on the cluster's `MasterUserSecret` and sent on to Secrets Manager, so an
//! unbounded value would enter two stores per request.
//!
//! The rows span the ceiling and one past it: a create that names no key at all (served,
//! and the control that this path really mints a secret), one naming a key identifier
//! exactly as long as this service stores (whatever becomes of the key it names, the answer
//! is not about the value's LENGTH), and one a single character longer (refused for its
//! length, with nothing created).
//!
//! The emulator and the Secrets Manager client are the ones `managed_master_user_password`
//! mounts, because this drives the same path.

use service_com_amazonaws_rds_amazonrdsv19::state as st;

use crate::db_clusters::the_cluster;
use crate::db_instances::{call, expect_error, expect_ok, subtrees, text};
use crate::managed_master_user_password::{emulator, secrets_manager};

/// The names of every secret the mounted Secrets Manager holds, sorted: what a row compares
/// across a create to see whether one was minted.
async fn held_secret_names(secrets: &aws_sdk_secretsmanager::Client) -> Vec<String> {
    let listed = secrets
        .list_secrets()
        .send()
        .await
        .expect("the mounted Secrets Manager lists its secrets");
    let mut names: Vec<String> = listed
        .secret_list()
        .iter()
        .filter_map(|secret| secret.name().map(str::to_string))
        .collect();
    names.sort();
    names
}

/// What a row expects of its create.
enum Expected {
    /// Served: the cluster reports a managed secret, and one secret was minted.
    Served,
    /// Whatever the answer is, it is not about the value's LENGTH.
    NotAboutLength,
    /// Refused for the value's length, with nothing created.
    RefusedForLength,
}

#[tokio::test]
async fn the_managed_secrets_kms_key_identifier_is_bounded_where_it_enters_the_store() {
    let router = emulator();
    let secrets = secrets_manager(&router);
    let longest = "k".repeat(st::MAX_FREE_FORM_LEN);
    let too_long = "k".repeat(st::MAX_FREE_FORM_LEN + 1);
    let ceiling = st::MAX_FREE_FORM_LEN.to_string();
    // (case, the MasterUserSecretKmsKeyId the request names, what the create must answer)
    let rows: [(&str, Option<&str>, Expected); 3] = [
        (
            "no key named at all, so the secret is left on the default key",
            None,
            Expected::Served,
        ),
        (
            "a key identifier exactly as long as this service stores",
            Some(longest.as_str()),
            Expected::NotAboutLength,
        ),
        (
            "one character longer than this service stores",
            Some(too_long.as_str()),
            Expected::RefusedForLength,
        ),
    ];
    for (index, (case, key, expected)) in rows.iter().enumerate() {
        let identifier = format!("emu-secret-key-bound-{index}");
        let held_before = held_secret_names(&secrets).await;
        let mut params: Vec<(&str, String)> = vec![
            ("DBClusterIdentifier", identifier.clone()),
            ("Engine", "aurora-mysql".to_string()),
            ("MasterUsername", "dbadmin".to_string()),
            ("ManageMasterUserPassword", "true".to_string()),
        ];
        if let Some(key) = key {
            params.push(("MasterUserSecretKmsKeyId", (*key).to_string()));
        }
        let (status, body) = call(&router, "CreateDBCluster", &params).await;
        let held_after = held_secret_names(&secrets).await;
        match expected {
            Expected::Served => {
                let body = expect_ok(case, (status, body));
                let cluster = the_cluster(case, &body);
                assert!(
                    !subtrees(&cluster, "MasterUserSecret").is_empty(),
                    "case {case:?}: a served managed-password create reports its \
                     MasterUserSecret: {cluster}"
                );
                assert_eq!(
                    held_after.len(),
                    held_before.len() + 1,
                    "case {case:?}: a served create mints exactly one secret, {held_before:?} \
                     -> {held_after:?}"
                );
            }
            Expected::NotAboutLength => {
                // The guard is what is under test, not the key: a value AT the ceiling
                // passes it, so whatever this create is answered with cannot be the length
                // refusal — neither its code nor its sentence.
                if status != 200 {
                    let code = text(&body, "Code");
                    let message = text(&body, "Message");
                    assert_ne!(
                        code, "InvalidParameterValue",
                        "case {case:?}: a value at the ceiling must not be refused for its \
                         length, got {body}"
                    );
                    assert!(
                        !message.contains(&ceiling),
                        "case {case:?}: a value at the ceiling must not be refused against the \
                         ceiling, got {message:?}"
                    );
                }
            }
            Expected::RefusedForLength => {
                let body = expect_error(case, (status, body), 400, "InvalidParameterValue");
                let message = text(&body, "Message");
                assert!(
                    message.contains("MasterUserSecretKmsKeyId") && message.contains(&ceiling),
                    "case {case:?}: the refusal must name the member and the bound it broke, \
                     got {message:?}"
                );
                assert_eq!(
                    held_after, held_before,
                    "case {case:?}: a refused create mints no secret, but Secrets Manager now \
                     holds {held_after:?} instead of {held_before:?}"
                );
                let (status, listing) = call(
                    &router,
                    "DescribeDBClusters",
                    &[("DBClusterIdentifier", identifier.as_str())],
                )
                .await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: a refused create stores no cluster, got {listing}"
                );
            }
        }
    }
}
