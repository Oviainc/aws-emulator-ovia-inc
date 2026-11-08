// SPDX-License-Identifier: CC0-1.0

//! How the policy-lockout safety check reads a statement's PRINCIPAL set, over both
//! entry points that write a cluster's resource-based policy.
//!
//! RULE: a statement states its principal set in one of two forms — `Principal`,
//! the principals it names, or `NotPrincipal`, its complement: every principal the
//! element does NOT name. So a `Deny` of the policy-management action on this
//! cluster, with no narrowing `Condition`, locks everybody out when its principal
//! set is `Principal` covering the wildcard, and equally when it is `NotPrincipal`
//! exempting nobody who could still call that action — the wildcard, or a principal
//! of the account that owns the cluster. It leaves the policy replaceable when the
//! exemption names such a principal, because that one principal can still replace
//! the document.
//!
//! The table spans the FORM of the principal element (the bare `*` string, an `AWS`
//! block, a `Service` block, an empty block, a list, a bare account id, a role ARN,
//! a root ARN), its POLARITY (`Principal` / `NotPrincipal`), the ACCOUNT the named
//! principal belongs to (this cluster's own, another one) and the crossing of a
//! negated principal set with the other conjuncts of the check (effect and its
//! spelling, `Action` / `NotAction`, `Resource` / `NotResource`, `Condition`).
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::{json, Value};

use crate::harness::{expected_arn, Dsql};

/// The action a principal needs to replace a cluster's policy, which is what the
/// lockout safety check is about. Source: `bypassPolicyLockoutSafetyCheck`'s own
/// documentation (`smithy_query(query="doc", shape="PutClusterPolicyInput",
/// member="bypassPolicyLockoutSafetyCheck")` — "a policy that might lock you out of
/// the cluster") together with `PutClusterPolicy`, the operation that applies one.
const POLICY_ACTION: &str = "dsql:PutClusterPolicy";

/// A 12-digit account id that is NOT the account this emulator serves, so a
/// principal of it could never call [`POLICY_ACTION`] on this cluster.
const OTHER_ACCOUNT: &str = "111122223333";

/// A second foreign account, for a list that exempts none of this account's
/// principals.
const THIRD_ACCOUNT: &str = "444455556666";

/// A role ARN of `account`, the form IAM names an AWS principal by.
fn role_arn(account: &str) -> String {
    format!("arn:aws:iam::{account}:role/dsql-admin")
}

/// The root-user ARN of `account`, which stands for the account itself.
fn root_arn(account: &str) -> String {
    format!("arn:aws:iam::{account}:root")
}

/// The account this emulator serves — the one every cluster it mints belongs to.
fn own_account() -> String {
    engine::EMULATOR_ACCOUNT_ID.to_string()
}

/// A cluster ARN of this account naming a cluster that is NOT the one under test.
fn another_cluster_arn(region: &str) -> String {
    expected_arn(region, "aaaaaaaaaaaaaaaaaaaaaaaaaa")
}

/// One statement and the verdict the rule gives it.
struct PrincipalCase {
    name: &'static str,
    statement: Value,
    /// Whether the statement leaves nobody able to replace the policy, so the check
    /// must refuse the document.
    locks_out: bool,
}

/// The rows, given the region the requests are signed for (the `Resource` rows name
/// a cluster ARN of that region).
///
/// The principal element's forms are IAM's: the whole element as the string `*`, or
/// a block keyed by principal type (`AWS`, `Service`, `Federated`, `CanonicalUser`)
/// whose value is one string or a list of them; only an AWS principal of an account
/// is an identity that calls this API.
fn principal_cases(region: &str) -> Vec<PrincipalCase> {
    let own = own_account();
    vec![
        // --- the positive form: the set is what it names ----------------------
        PrincipalCase {
            name: "Principal as the bare wildcard string",
            statement: json!({
                "Effect": "Deny",
                "Principal": "*",
                "Action": POLICY_ACTION,
                "Resource": "*",
            }),
            locks_out: true,
        },
        PrincipalCase {
            name: "Principal as an AWS block holding the wildcard",
            statement: json!({
                "Effect": "Deny",
                "Principal": {"AWS": "*"},
                "Action": "dsql:*",
                "Resource": "*",
            }),
            locks_out: true,
        },
        PrincipalCase {
            name: "Principal as a list carrying the wildcard beside a role",
            statement: json!({
                "Effect": "Deny",
                "Principal": {"AWS": [role_arn(&own), "*"]},
                "Action": "dsql:*",
                "Resource": "*",
            }),
            locks_out: true,
        },
        PrincipalCase {
            name: "Principal naming one role of this account only",
            statement: json!({
                "Effect": "Deny",
                "Principal": {"AWS": role_arn(&own)},
                "Action": "dsql:*",
                "Resource": "*",
            }),
            locks_out: false,
        },
        // --- the negated form: the set is everything it does NOT name ---------
        PrincipalCase {
            name: "NotPrincipal exempting one role of another account",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": role_arn(OTHER_ACCOUNT)},
                "Action": "dsql:*",
                "Resource": "*",
            }),
            locks_out: true,
        },
        PrincipalCase {
            name: "NotPrincipal exempting a list of two other accounts",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": [root_arn(OTHER_ACCOUNT), role_arn(THIRD_ACCOUNT)]},
                "Action": POLICY_ACTION,
                "Resource": "*",
            }),
            locks_out: true,
        },
        PrincipalCase {
            name: "NotPrincipal exempting a service principal, which calls no API here",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"Service": "dsql.amazonaws.com"},
                "Action": "dsql:*",
                "Resource": "*",
            }),
            locks_out: true,
        },
        PrincipalCase {
            name: "NotPrincipal as an empty block, exempting nobody",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {},
                "Action": "dsql:*",
                "Resource": "*",
            }),
            locks_out: true,
        },
        PrincipalCase {
            name: "NotPrincipal with the effect spelled in lower case",
            statement: json!({
                "Effect": "deny",
                "NotPrincipal": {"AWS": role_arn(OTHER_ACCOUNT)},
                "Action": "dsql:*",
                "Resource": "*",
            }),
            locks_out: true,
        },
        PrincipalCase {
            name: "NotPrincipal of another account beside a NotAction over another action",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": role_arn(OTHER_ACCOUNT)},
                "NotAction": "dsql:DbConnect",
                "Resource": "*",
            }),
            locks_out: true,
        },
        PrincipalCase {
            name: "all three sets negated: NotPrincipal, NotAction, NotResource",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": role_arn(OTHER_ACCOUNT)},
                "NotAction": "dsql:DbConnectAdmin",
                "NotResource": another_cluster_arn(region),
            }),
            locks_out: true,
        },
        // --- the negated form, where the exemption keeps the policy replaceable
        PrincipalCase {
            name: "NotPrincipal as the bare wildcard string, which denies nobody",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": "*",
                "Action": "dsql:*",
                "Resource": "*",
            }),
            locks_out: false,
        },
        PrincipalCase {
            name: "NotPrincipal as an AWS block holding the wildcard",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": "*"},
                "Action": "dsql:*",
                "Resource": "*",
            }),
            locks_out: false,
        },
        PrincipalCase {
            name: "NotPrincipal exempting this account's root user",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": root_arn(&own)},
                "Action": "dsql:*",
                "Resource": "*",
            }),
            locks_out: false,
        },
        PrincipalCase {
            name: "NotPrincipal exempting one role of this account",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": role_arn(&own)},
                "Action": POLICY_ACTION,
                "Resource": "*",
            }),
            locks_out: false,
        },
        PrincipalCase {
            name: "NotPrincipal exempting this account by its bare id",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": own.clone()},
                "Action": "dsql:*",
                "Resource": "*",
            }),
            locks_out: false,
        },
        PrincipalCase {
            name: "NotPrincipal exempting a foreign role and one of this account's",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": [role_arn(OTHER_ACCOUNT), role_arn(&own)]},
                "Action": "dsql:*",
                "Resource": "*",
            }),
            locks_out: false,
        },
        PrincipalCase {
            name: "NotPrincipal of another account, but NotAction names the action itself",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": role_arn(OTHER_ACCOUNT)},
                "NotAction": POLICY_ACTION,
                "Resource": "*",
            }),
            locks_out: false,
        },
        PrincipalCase {
            name: "NotPrincipal of another account denied a different action",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": role_arn(OTHER_ACCOUNT)},
                "Action": "dsql:DbConnect",
                "Resource": "*",
            }),
            locks_out: false,
        },
        PrincipalCase {
            name: "NotPrincipal of another account on another cluster's ARN",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": role_arn(OTHER_ACCOUNT)},
                "Action": "dsql:*",
                "Resource": another_cluster_arn(region),
            }),
            locks_out: false,
        },
        PrincipalCase {
            name: "NotPrincipal of another account, narrowed by a Condition",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": role_arn(OTHER_ACCOUNT)},
                "Action": "dsql:*",
                "Resource": "*",
                "Condition": {"Bool": {"aws:SecureTransport": "false"}},
            }),
            locks_out: false,
        },
        PrincipalCase {
            name: "NotPrincipal of another account, allowed rather than denied",
            statement: json!({
                "Effect": "Allow",
                "NotPrincipal": {"AWS": role_arn(OTHER_ACCOUNT)},
                "Action": "dsql:*",
                "Resource": "*",
            }),
            locks_out: false,
        },
    ]
}

/// The one sentence a document nobody could replace is refused with: it names the
/// action the check is about and the flag that accepts the risk.
fn lockout_message() -> String {
    format!(
        "The policy would prevent every principal from calling {POLICY_ACTION} on this cluster. \
         Set bypassPolicyLockoutSafetyCheck to true to apply it anyway."
    )
}

/// The document one row's statement travels in, as the `policy` member carries it:
/// a STRING holding a JSON policy document.
fn document(statement: &Value) -> String {
    serde_json::to_string(&json!({"Version": "2012-10-17", "Statement": [statement]}))
        .expect("a policy document serializes")
}

/// RULE, over `PutClusterPolicy`: a `Deny` whose principal set covers every
/// principal that could replace the policy — spelled positively or as the
/// complement `NotPrincipal` — is refused 400 `ValidationException` with reason
/// `other` and nothing attached, while one that leaves such a principal out of the
/// deny is attached and reads back verbatim.
#[tokio::test]
async fn the_lockout_check_reads_notprincipal_as_the_complement_of_the_principals_it_names() {
    let dsql = Dsql::new();
    let cases = principal_cases(dsql.region());
    let clusters = dsql.create_settled_many(cases.len()).await;

    for (case, identifier) in cases.iter().zip(&clusters) {
        let document = document(&case.statement);
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
                answer.str("reason"),
                "other",
                "case {:?}: expected reason \"other\", got {:?}",
                case.name,
                answer.str("reason")
            );
            assert_eq!(
                answer.message(),
                lockout_message(),
                "case {:?}: expected the lockout sentence, got {:?}",
                case.name,
                answer.message()
            );
            assert_eq!(
                dsql.get_policy(identifier).await.status,
                404,
                "case {:?}: a refused put must attach nothing, got {:?}",
                case.name,
                dsql.get_policy(identifier).await.body
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
                "case {:?}: the bypassed document must read back verbatim, got {:?}",
                case.name,
                dsql.get_policy(identifier).await.str("policy")
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {:?}: a policy that leaves the action reachable must be attached, \
                 got {} {:?}",
                case.name, answer.status, answer.body
            );
            let read = dsql.get_policy(identifier).await;
            assert_eq!(
                read.str("policy"),
                document,
                "case {:?}: the attached document must read back verbatim, got {:?}",
                case.name,
                read.str("policy")
            );
            assert!(
                !read.str("policyVersion").is_empty(),
                "case {:?}: the attached policy must carry a version, got {:?}",
                case.name,
                read.body
            );
        }
    }
}

/// RULE, over `CreateCluster`'s inline `policy`: the verdict belongs to the
/// DOCUMENT, so the same principal sets are refused or attached identically there —
/// and a refused create mints no cluster.
#[tokio::test]
async fn create_cluster_reads_the_principal_set_the_same_way() {
    for case in principal_cases("us-east-1") {
        // One store per row, so "a refused create creates nothing" is asserted
        // against a store that holds nothing else.
        let dsql = Dsql::new();
        let document = document(&case.statement);
        let created = dsql
            .create(json!({"deletionProtectionEnabled": false, "policy": document.clone()}))
            .await;

        if case.locks_out {
            assert_eq!(
                created.status, 400,
                "case {:?}: the create must be refused with 400, got {} {:?}",
                case.name, created.status, created.body
            );
            assert_eq!(
                created.str("reason"),
                "other",
                "case {:?}: expected reason \"other\", got {:?}",
                case.name,
                created.str("reason")
            );
            assert_eq!(
                created.message(),
                lockout_message(),
                "case {:?}: expected the lockout sentence, got {:?}",
                case.name,
                created.message()
            );
            let listed = dsql.list("").await;
            assert_eq!(
                listed.get("clusters"),
                json!([]),
                "case {:?}: a refused create must leave the store empty, got {:?}",
                case.name,
                listed.get("clusters")
            );
        } else {
            assert_eq!(
                created.status, 200,
                "case {:?}: an acceptable document must be attached, got {} {:?}",
                case.name, created.status, created.body
            );
            let read = dsql.get_policy(&created.str("identifier")).await;
            assert_eq!(
                read.str("policy"),
                document,
                "case {:?}: the inline document must read back verbatim, got {:?}",
                case.name,
                read.str("policy")
            );
        }
    }
}
