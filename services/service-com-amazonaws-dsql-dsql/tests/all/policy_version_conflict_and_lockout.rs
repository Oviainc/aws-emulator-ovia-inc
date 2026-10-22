// SPDX-License-Identifier: CC0-1.0

//! Two rules of a cluster's resource-based policy, end to end over the wire.
//!
//! * RULE A — every refusal of a call whose `expectedPolicyVersion` does not name
//!   the version currently attached is worded with ONE request-derived sentence,
//!   naming the version the caller claimed and the cluster it claimed it for,
//!   whether the refusal is the 409 `ConflictException` (a policy is attached, at
//!   another version) or the 404 `ResourceNotFoundException` (no policy is attached
//!   at all), and whether the claim arrives on `PutClusterPolicy`'s body member or
//!   `DeleteClusterPolicy`'s query parameter. The table spans the FORM of the
//!   claimed token (a superseded version, a version never issued, a non-numeric
//!   token, a token past a 64-bit counter) and the state of the cluster (nothing
//!   attached, several versions deep, a policy since removed); one recorded
//!   superseded-version case is a single row.
//!
//! * RULE B — the policy-lockout safety check reads a statement's `Resource` set as
//!   IAM resource patterns against the ARN of the cluster the policy is being
//!   attached to, so the cluster's OWN ARN covers it exactly as a wildcard spanning
//!   it does, and an ARN naming another resource covers nothing. The table spans
//!   the form of the `Resource` value (the literal ARN, `*`, a wildcard suffix, a
//!   single-character wildcard, a list, a nested resource, absent, `NotResource`)
//!   and the other three conjuncts of the check (effect, action, condition).
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::{json, Value};

use crate::harness::{expected_arn, Dsql};

/// A policy document the lockout safety check has no reason to refuse: one named
/// principal, one action, on every resource. `round` varies the action so two
/// successive attachments are different documents.
fn benign_document(round: usize) -> String {
    let action = if round.is_multiple_of(2) {
        "dsql:DbConnect"
    } else {
        "dsql:DbConnectAdmin"
    };
    serde_json::to_string(&json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::EMULATOR_ACCOUNT_ID)},
            "Action": action,
            "Resource": "*",
        }],
    }))
    .expect("a policy document serializes")
}

/// Attach one more policy to `identifier` and answer with the version it got.
async fn attach(dsql: &Dsql, identifier: &str, round: usize) -> String {
    let answer = dsql
        .put_policy(identifier, json!({"policy": benign_document(round)}))
        .await;
    assert_eq!(
        answer.status, 200,
        "attaching policy #{round} to {identifier} must answer 200, got {} {:?}",
        answer.status, answer.body
    );
    let version = answer.str("policyVersion");
    assert!(
        !version.is_empty(),
        "attaching policy #{round} must answer a policyVersion, got {:?}",
        answer.body
    );
    version
}

/// The one sentence this service words a claim about a policy version the cluster
/// does not hold with — built from the request, naming the claimed version and the
/// cluster the call addressed.
fn version_message(identifier: &str, claimed: &str) -> String {
    format!(
        "Aurora DSQL can't find policy version {claimed} for resource {identifier}. \
         Verify that the policy exists and that the version is correct."
    )
}

/// One conditional policy call whose `expectedPolicyVersion` does not name the
/// attached version.
struct VersionCase {
    name: &'static str,
    /// How many policies to attach before the conditional call.
    attached: usize,
    /// Whether the attached policy is removed again before the call, so the claim
    /// names a version that is gone.
    removed: bool,
    /// The version the caller claims, given the versions issued so far.
    claimed: fn(&[String]) -> String,
    /// The status the class answers with, and the modelled error that carries it.
    status: u16,
    error: &'static str,
}

fn version_cases() -> Vec<VersionCase> {
    vec![
        VersionCase {
            name: "a superseded version, two policies deep",
            attached: 2,
            removed: false,
            claimed: |issued| issued[0].clone(),
            status: 409,
            error: "ConflictException",
        },
        VersionCase {
            name: "the version before the latest of three",
            attached: 3,
            removed: false,
            claimed: |issued| issued[1].clone(),
            status: 409,
            error: "ConflictException",
        },
        VersionCase {
            name: "a version this cluster has never issued",
            attached: 1,
            removed: false,
            claimed: |_| "99".to_string(),
            status: 409,
            error: "ConflictException",
        },
        VersionCase {
            name: "a token that is not a number at all",
            attached: 1,
            removed: false,
            claimed: |_| "notaversion".to_string(),
            status: 409,
            error: "ConflictException",
        },
        VersionCase {
            name: "a token past what a 64-bit counter reaches",
            attached: 1,
            removed: false,
            claimed: |_| "18446744073709551616".to_string(),
            status: 409,
            error: "ConflictException",
        },
        VersionCase {
            name: "zero, which is no version this service issues",
            attached: 1,
            removed: false,
            claimed: |_| "0".to_string(),
            status: 409,
            error: "ConflictException",
        },
        VersionCase {
            name: "a claim about a cluster with no policy attached",
            attached: 0,
            removed: false,
            claimed: |_| "1".to_string(),
            status: 404,
            error: "ResourceNotFoundException",
        },
        VersionCase {
            name: "a dotted token against a cluster with no policy attached",
            attached: 0,
            removed: false,
            claimed: |_| "v1.0".to_string(),
            status: 404,
            error: "ResourceNotFoundException",
        },
        VersionCase {
            name: "a claim about a policy that has since been removed",
            attached: 1,
            removed: true,
            claimed: |issued| issued[0].clone(),
            status: 404,
            error: "ResourceNotFoundException",
        },
    ]
}

/// RULE A, over `PutClusterPolicy` (the `expectedPolicyVersion` body member) and
/// `DeleteClusterPolicy` (the `expected-policy-version` query parameter): the
/// refusal carries the request-derived sentence naming the claimed version and the
/// addressed cluster, and changes nothing.
#[tokio::test]
async fn a_claim_about_a_version_the_cluster_does_not_hold_is_worded_from_the_request() {
    let dsql = Dsql::new();
    let cases = version_cases();
    // Two clusters per case — one for each entry point the claim arrives on — all
    // created up front so the clock's CREATING -> ACTIVE transition is waited for
    // once rather than per case.
    let clusters = dsql.create_settled_many(cases.len() * 2).await;

    for (index, case) in cases.iter().enumerate() {
        for (entry, identifier) in [
            ("PutClusterPolicy", &clusters[index * 2]),
            ("DeleteClusterPolicy", &clusters[index * 2 + 1]),
        ] {
            let mut issued: Vec<String> = Vec::new();
            for round in 0..case.attached {
                issued.push(attach(&dsql, identifier, round).await);
            }
            if case.removed {
                let removed = dsql.delete_policy(identifier, "").await;
                assert_eq!(
                    removed.status, 200,
                    "case {:?} on {entry}: the setup delete must answer 200, got {} {:?}",
                    case.name, removed.status, removed.body
                );
            }
            let current = if case.removed { None } else { issued.last().cloned() };
            let claimed = (case.claimed)(&issued);

            let answer = match entry {
                "PutClusterPolicy" => {
                    dsql.put_policy(
                        identifier,
                        json!({
                            "policy": benign_document(9),
                            "expectedPolicyVersion": claimed.clone(),
                        }),
                    )
                    .await
                }
                _ => {
                    dsql.delete_policy(
                        identifier,
                        &format!("?expected-policy-version={claimed}"),
                    )
                    .await
                }
            };

            assert_eq!(
                answer.status, case.status,
                "case {:?} on {entry}: expected status {}, got {} {:?}",
                case.name, case.status, answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some(case.error),
                "case {:?} on {entry}: expected error {:?}, got {:?}",
                case.name,
                case.error,
                answer.error
            );
            assert_eq!(
                answer.message(),
                version_message(identifier, &claimed),
                "case {:?} on {entry}: expected the request-derived sentence, got {:?}",
                case.name,
                answer.message()
            );
            assert_eq!(
                answer.str("resourceId"),
                *identifier,
                "case {:?} on {entry}: the refusal must name the cluster asked for, got {:?}",
                case.name,
                answer.str("resourceId")
            );
            assert_eq!(
                answer.str("resourceType"),
                "policy",
                "case {:?} on {entry}: the refusal is about the policy, got {:?}",
                case.name,
                answer.str("resourceType")
            );

            // A refused claim neither attaches nor removes anything.
            let read = dsql.get_policy(identifier).await;
            match current {
                Some(version) => assert_eq!(
                    (read.status, read.str("policyVersion")),
                    (200, version.clone()),
                    "case {:?} on {entry}: the attached policy must still be {version:?}, \
                     got {} {:?}",
                    case.name,
                    read.status,
                    read.body
                ),
                None => assert_eq!(
                    read.status, 404,
                    "case {:?} on {entry}: no policy may have been attached, got {} {:?}",
                    case.name, read.status, read.body
                ),
            }
        }
    }
}

/// RULE A keeps holding where the claim IS the attached version: that call is
/// applied, which is what makes the refusals above a conflict rather than a
/// blanket rejection of `expectedPolicyVersion`.
#[tokio::test]
async fn a_claim_naming_the_attached_version_is_applied() {
    let dsql = Dsql::new();
    let clusters = dsql.create_settled_many(2).await;

    // The write path: the conditional put lands and mints a new version.
    let put_cluster = &clusters[0];
    let first = attach(&dsql, put_cluster, 0).await;
    let replaced = dsql
        .put_policy(
            put_cluster,
            json!({"policy": benign_document(1), "expectedPolicyVersion": first.clone()}),
        )
        .await;
    assert_eq!(
        replaced.status, 200,
        "a put naming the attached version must be applied, got {} {:?}",
        replaced.status, replaced.body
    );
    assert_ne!(
        replaced.str("policyVersion"),
        first,
        "the replacement must carry a version other than {first:?}"
    );
    assert_eq!(
        dsql.get_policy(put_cluster).await.str("policy"),
        benign_document(1),
        "the replacement must be the stored document"
    );

    // The delete path: the conditional delete removes exactly that version.
    let delete_cluster = &clusters[1];
    let attached = attach(&dsql, delete_cluster, 0).await;
    let removed = dsql
        .delete_policy(
            delete_cluster,
            &format!("?expected-policy-version={attached}"),
        )
        .await;
    assert_eq!(
        (removed.status, removed.str("policyVersion")),
        (200, attached.clone()),
        "a delete naming the attached version must remove it, got {} {:?}",
        removed.status,
        removed.body
    );
    assert_eq!(
        dsql.get_policy(delete_cluster).await.status,
        404,
        "the removed policy must read back as 404"
    );
}

/// One `Resource`/`NotResource` shape the lockout safety check is read against,
/// built from the ARN of the cluster the policy is being attached to.
struct LockoutCase {
    name: &'static str,
    /// The single `Statement` of the document, given that cluster's own ARN.
    statement: fn(&str) -> Value,
    /// Whether the statement leaves nobody able to replace the policy, so the
    /// check must refuse it.
    locks_out: bool,
}

/// A cluster ARN of this account that is NOT the cluster under test: the ARN form
/// is `expected_arn`'s, which is what `ClusterArn`'s pattern requires.
fn another_cluster_arn() -> String {
    expected_arn("us-east-1", "aaaaaaaaaaaaaaaaaaaaaaaaaa")
}

fn lockout_cases() -> Vec<LockoutCase> {
    vec![
        LockoutCase {
            name: "the cluster's own ARN, named literally",
            statement: |arn| {
                json!({"Effect": "Deny", "Principal": "*", "Action": "dsql:*", "Resource": arn})
            },
            locks_out: true,
        },
        LockoutCase {
            name: "every resource",
            statement: |_| {
                json!({"Effect": "Deny", "Principal": "*", "Action": "dsql:*", "Resource": "*"})
            },
            locks_out: true,
        },
        LockoutCase {
            name: "every cluster of the account, by wildcard suffix",
            statement: |arn| {
                // Everything up to and including `cluster/`, then the wildcard:
                // a `ClusterId` is 26 characters (`^[a-z0-9]{26}$`).
                let prefix = &arn[..arn.len() - 26];
                json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "dsql:*",
                    "Resource": format!("{prefix}*"),
                })
            },
            locks_out: true,
        },
        LockoutCase {
            name: "the ARN with its last character wildcarded by ?",
            statement: |arn| {
                let prefix = &arn[..arn.len() - 1];
                json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "dsql:*",
                    "Resource": format!("{prefix}?"),
                })
            },
            locks_out: true,
        },
        LockoutCase {
            name: "a list naming another cluster and this one",
            statement: |arn| {
                json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "dsql:PutClusterPolicy",
                    "Resource": [another_cluster_arn(), arn.to_string()],
                })
            },
            locks_out: true,
        },
        LockoutCase {
            name: "no Resource at all, which in a resource policy is this cluster",
            statement: |_| json!({"Effect": "Deny", "Principal": "*", "Action": "dsql:*"}),
            locks_out: true,
        },
        LockoutCase {
            name: "NotResource naming another cluster, so this one is covered",
            statement: |_| {
                json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "dsql:*",
                    "NotResource": another_cluster_arn(),
                })
            },
            locks_out: true,
        },
        LockoutCase {
            name: "only another cluster's ARN",
            statement: |_| {
                json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "dsql:*",
                    "Resource": another_cluster_arn(),
                })
            },
            locks_out: false,
        },
        LockoutCase {
            name: "a resource nested under this cluster, which is not this cluster",
            statement: |arn| {
                json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "dsql:*",
                    "Resource": format!("{arn}/stream/*"),
                })
            },
            locks_out: false,
        },
        LockoutCase {
            name: "NotResource naming this cluster, which excludes it",
            statement: |arn| {
                json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "dsql:*",
                    "NotResource": arn,
                })
            },
            locks_out: false,
        },
        LockoutCase {
            name: "the own ARN allowed rather than denied",
            statement: |arn| {
                json!({"Effect": "Allow", "Principal": "*", "Action": "dsql:*", "Resource": arn})
            },
            locks_out: false,
        },
        LockoutCase {
            name: "the own ARN denied for another action",
            statement: |arn| {
                json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "dsql:DbConnect",
                    "Resource": arn,
                })
            },
            locks_out: false,
        },
        LockoutCase {
            name: "the own ARN denied only under a condition",
            statement: |arn| {
                json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "dsql:*",
                    "Resource": arn,
                    "Condition": {"Bool": {"aws:SecureTransport": "false"}},
                })
            },
            locks_out: false,
        },
        LockoutCase {
            name: "the own ARN denied to one named principal only",
            statement: |arn| {
                json!({
                    "Effect": "Deny",
                    "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::EMULATOR_ACCOUNT_ID)},
                    "Action": "dsql:*",
                    "Resource": arn,
                })
            },
            locks_out: false,
        },
    ]
}

/// RULE B: the lockout safety check's resource-coverage test matches a statement's
/// `Resource` set against the ARN of the cluster the policy is being attached to,
/// so a deny-all statement naming that cluster's own ARN is refused (400
/// `ValidationException`, nothing attached) just as a wildcard one is, while a
/// statement naming any other resource is attached.
#[tokio::test]
async fn the_lockout_check_reads_resource_against_the_clusters_own_arn() {
    let dsql = Dsql::new();
    let cases = lockout_cases();
    let clusters = dsql.create_settled_many(cases.len()).await;

    for (case, identifier) in cases.iter().zip(&clusters) {
        // The ARN the cluster itself reports, which is the resource the policy is
        // being attached to.
        let described = dsql.get(identifier).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: GetCluster must report the cluster, got {} {:?}",
            case.name, described.status, described.body
        );
        let arn = described.str("arn");
        assert_eq!(
            arn,
            expected_arn(dsql.region(), identifier),
            "case {:?}: the cluster must report its own ARN, got {arn:?}",
            case.name
        );

        let document = serde_json::to_string(&json!({
            "Version": "2012-10-17",
            "Statement": [(case.statement)(&arn)],
        }))
        .expect("a policy document serializes");

        let answer = dsql
            .put_policy(identifier, json!({"policy": document.clone()}))
            .await;

        if case.locks_out {
            assert_eq!(
                answer.status, 400,
                "case {:?}: a policy nobody could replace must be refused with 400, got {} {:?}",
                case.name, answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {:?}: the refusal must be a ValidationException, got {:?}",
                case.name,
                answer.error
            );
            assert_eq!(
                dsql.get_policy(identifier).await.status,
                404,
                "case {:?}: a refused put must attach nothing",
                case.name
            );
            // The caller can accept the risk explicitly, which is what the flag is
            // for: the identical document then lands.
            let bypassed = dsql
                .put_policy(
                    identifier,
                    json!({"policy": document.clone(), "bypassPolicyLockoutSafetyCheck": true}),
                )
                .await;
            assert_eq!(
                bypassed.status, 200,
                "case {:?}: bypassPolicyLockoutSafetyCheck must apply it anyway, got {} {:?}",
                case.name, bypassed.status, bypassed.body
            );
            assert_eq!(
                dsql.get_policy(identifier).await.str("policy"),
                document,
                "case {:?}: the bypassed document must read back verbatim",
                case.name
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {:?}: a policy that leaves the action reachable must be attached, \
                 got {} {:?}",
                case.name, answer.status, answer.body
            );
            assert_eq!(
                dsql.get_policy(identifier).await.str("policy"),
                document,
                "case {:?}: the attached document must read back verbatim",
                case.name
            );
        }
    }
}
