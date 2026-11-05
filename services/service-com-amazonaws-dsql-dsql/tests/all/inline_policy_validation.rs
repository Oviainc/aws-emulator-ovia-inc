// SPDX-License-Identifier: CC0-1.0

//! The documents this service will hold as a cluster's resource-based policy, over
//! BOTH entry points that write one.
//!
//! RULE: a request that asks this service to attach a resource-based policy to a
//! cluster is refused with 400 `ValidationException` — and nothing is created or
//! attached — when the document is not a JSON policy document (reason
//! `cannotParse`), or, unless the request sets `bypassPolicyLockoutSafetyCheck`,
//! when it denies every principal the action that replaces the policy on that
//! cluster (reason `other`); every other document is persisted and read back
//! verbatim. The rule is about the DOCUMENT, not about which operation carries it,
//! so `CreateCluster`'s inline `policy` and `PutClusterPolicy`'s `policy` answer
//! identically for the same document.
//!
//! The table spans the document's TYPE (object, array, string, number, boolean,
//! null, truncated text), its STATEMENT COUNT and form (absent, empty list, a bare
//! statement object, a list), the statement's effect spelling, the shape of its
//! action set (`Action`, a wildcard family, `NotAction`), of its principal set (a
//! string wildcard, an `AWS` block, one named role) and of its resource set
//! (absent, `*`, a wildcard over the account's clusters, another cluster's ARN) —
//! `tests/all/policy_version_conflict_and_lockout.rs` spans the `Resource` forms
//! that name the cluster's own ARN, which a create cannot name because the ARN is
//! minted by the call itself.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::{json, Value};

use crate::harness::{expected_arn, Dsql};

/// The action a principal needs to replace a cluster's policy, which is what the
/// lockout safety check is about. Source: the operations' own documentation for
/// `bypassPolicyLockoutSafetyCheck` (`smithy_query(query="doc",
/// shape="CreateClusterInput", member="bypassPolicyLockoutSafetyCheck")`) together
/// with `PutClusterPolicy`, the operation that applies a policy.
const POLICY_ACTION: &str = "dsql:PutClusterPolicy";

/// `ValidationExceptionReason` values a policy refusal carries. Source:
/// `smithy_query(query="shape", shape="ValidationExceptionReason")` —
/// unknownOperation | cannotParse | fieldValidationFailed |
/// deletionProtectionEnabled | other.
const REASON_CANNOT_PARSE: &str = "cannotParse";
/// See [`REASON_CANNOT_PARSE`].
const REASON_OTHER: &str = "other";

/// What this service must answer for one document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The document is persisted and read back verbatim.
    Attached,
    /// The document is not a policy document at all.
    CannotParse,
    /// The document would leave nobody able to replace it, so it is refused
    /// unless the request accepts that risk.
    LocksEveryoneOut,
}

/// One policy document and the verdict the rule gives it.
struct PolicyCase {
    name: &'static str,
    document: String,
    verdict: Verdict,
}

/// An ARN of this account's partition naming a cluster that is NOT the one under
/// test, in the harness's own ARN form.
fn another_cluster_arn(region: &str) -> String {
    expected_arn(region, "aaaaaaaaaaaaaaaaaaaaaaaaaa")
}

/// One named principal of this account, as a `Principal` block names one.
fn named_principal() -> Value {
    json!({"AWS": format!("arn:aws:iam::{}:role/dsql-admin", engine::EMULATOR_ACCOUNT_ID)})
}

/// A document wrapping `statement` (a statement, or a list of them) in the policy
/// envelope, serialized the way the `policy` member carries it: a STRING holding a
/// JSON document.
fn document(statement: Value) -> String {
    serde_json::to_string(&json!({"Version": "2012-10-17", "Statement": statement}))
        .expect("a policy document serializes")
}

fn policy_cases(region: &str) -> Vec<PolicyCase> {
    let account_clusters = expected_arn(region, "*");
    vec![
        // --- documents this service holds -------------------------------------
        PolicyCase {
            name: "an allow of one action to one named principal",
            document: document(json!([{
                "Effect": "Allow",
                "Principal": named_principal(),
                "Action": "dsql:DbConnect",
                "Resource": "*",
            }])),
            verdict: Verdict::Attached,
        },
        PolicyCase {
            name: "no statement at all, so nothing is denied",
            document: "{}".to_string(),
            verdict: Verdict::Attached,
        },
        PolicyCase {
            name: "an empty statement list",
            document: document(json!([])),
            verdict: Verdict::Attached,
        },
        PolicyCase {
            name: "a deny of every principal, but of another action",
            document: document(json!([{
                "Effect": "Deny",
                "Principal": "*",
                "Action": "dsql:DbConnect",
                "Resource": "*",
            }])),
            verdict: Verdict::Attached,
        },
        PolicyCase {
            name: "a deny of the policy action to every principal, under a condition",
            document: document(json!([{
                "Effect": "Deny",
                "Principal": "*",
                "Action": "dsql:*",
                "Resource": "*",
                "Condition": {"Bool": {"aws:SecureTransport": "false"}},
            }])),
            verdict: Verdict::Attached,
        },
        PolicyCase {
            name: "a deny of the policy action to one named principal only",
            document: document(json!([{
                "Effect": "Deny",
                "Principal": named_principal(),
                "Action": "dsql:*",
                "Resource": "*",
            }])),
            verdict: Verdict::Attached,
        },
        PolicyCase {
            name: "a deny of every principal on ANOTHER cluster's ARN",
            document: document(json!([{
                "Effect": "Deny",
                "Principal": "*",
                "Action": "dsql:*",
                "Resource": another_cluster_arn(region),
            }])),
            verdict: Verdict::Attached,
        },
        PolicyCase {
            name: "a NotAction that names the policy action, so it stays reachable",
            document: document(json!([{
                "Effect": "Deny",
                "Principal": "*",
                "NotAction": POLICY_ACTION,
                "Resource": "*",
            }])),
            verdict: Verdict::Attached,
        },
        PolicyCase {
            name: "an allow beside a deny of another action, two statements",
            document: document(json!([
                {
                    "Effect": "Allow",
                    "Principal": named_principal(),
                    "Action": ["dsql:DbConnect", POLICY_ACTION],
                    "Resource": "*",
                },
                {
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "dsql:DbConnectAdmin",
                    "Resource": "*",
                },
            ])),
            verdict: Verdict::Attached,
        },
        // --- documents nobody could replace -----------------------------------
        PolicyCase {
            name: "deny every principal every action on every resource",
            document: document(json!([{
                "Effect": "Deny",
                "Principal": "*",
                "Action": "*",
                "Resource": "*",
            }])),
            verdict: Verdict::LocksEveryoneOut,
        },
        PolicyCase {
            name: "deny every principal the policy action by name",
            document: document(json!([{
                "Effect": "Deny",
                "Principal": "*",
                "Action": POLICY_ACTION,
                "Resource": "*",
            }])),
            verdict: Verdict::LocksEveryoneOut,
        },
        PolicyCase {
            name: "deny the AWS principal wildcard a wildcarded action family",
            document: document(json!([{
                "Effect": "Deny",
                "Principal": {"AWS": "*"},
                "Action": "dsql:Put*",
                "Resource": "*",
            }])),
            verdict: Verdict::LocksEveryoneOut,
        },
        PolicyCase {
            name: "deny every principal with no Resource, which is this cluster",
            document: document(json!([{
                "Effect": "Deny",
                "Principal": "*",
                "Action": "dsql:*",
            }])),
            verdict: Verdict::LocksEveryoneOut,
        },
        PolicyCase {
            name: "deny every principal over every cluster of the account",
            document: document(json!([{
                "Effect": "Deny",
                "Principal": "*",
                "Action": "dsql:*",
                "Resource": account_clusters,
            }])),
            verdict: Verdict::LocksEveryoneOut,
        },
        PolicyCase {
            name: "deny every principal everything but one other action",
            document: document(json!([{
                "Effect": "Deny",
                "Principal": "*",
                "NotAction": "dsql:DbConnect",
                "Resource": "*",
            }])),
            verdict: Verdict::LocksEveryoneOut,
        },
        PolicyCase {
            name: "a bare statement object rather than a list of them",
            document: document(json!({
                "Effect": "Deny",
                "Principal": "*",
                "Action": "dsql:*",
                "Resource": "*",
            })),
            verdict: Verdict::LocksEveryoneOut,
        },
        PolicyCase {
            name: "the effect spelled in lower case",
            document: document(json!([{
                "Effect": "deny",
                "Principal": "*",
                "Action": "dsql:*",
                "Resource": "*",
            }])),
            verdict: Verdict::LocksEveryoneOut,
        },
        PolicyCase {
            name: "a benign statement followed by a deny-everyone one",
            document: document(json!([
                {
                    "Effect": "Allow",
                    "Principal": named_principal(),
                    "Action": POLICY_ACTION,
                    "Resource": "*",
                },
                {
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "dsql:*",
                    "Resource": "*",
                },
            ])),
            verdict: Verdict::LocksEveryoneOut,
        },
        // --- values that are not policy documents ------------------------------
        PolicyCase {
            name: "plain text that is not JSON",
            document: "this is not json".to_string(),
            verdict: Verdict::CannotParse,
        },
        PolicyCase {
            name: "a JSON array",
            document: "[]".to_string(),
            verdict: Verdict::CannotParse,
        },
        PolicyCase {
            name: "a JSON string",
            document: "\"a string\"".to_string(),
            verdict: Verdict::CannotParse,
        },
        PolicyCase {
            name: "a JSON number",
            document: "7".to_string(),
            verdict: Verdict::CannotParse,
        },
        PolicyCase {
            name: "a JSON boolean",
            document: "true".to_string(),
            verdict: Verdict::CannotParse,
        },
        PolicyCase {
            name: "JSON null",
            document: "null".to_string(),
            verdict: Verdict::CannotParse,
        },
        PolicyCase {
            name: "a truncated object",
            document: "{\"Version\":\"2012-10-17\",\"Statement\":[".to_string(),
            verdict: Verdict::CannotParse,
        },
        PolicyCase {
            name: "two documents concatenated",
            document: "{}{}".to_string(),
            verdict: Verdict::CannotParse,
        },
    ]
}

/// The one sentence a document that is not a policy document is refused with: the
/// fixed text Aurora DSQL words that error class with, naming the member and the
/// form it had to be in and echoing nothing the caller sent.
fn cannot_parse_message() -> String {
    "Aurora DSQL can't process the request because the policy that you specified isn't \
     valid. Provide the policy in a valid format and try again."
        .to_string()
}

/// The one sentence a document nobody could replace is refused with: it names the
/// action the check is about and the flag that accepts the risk.
fn lockout_message() -> String {
    format!(
        "The policy would prevent every principal from calling {POLICY_ACTION} on this cluster. \
         Set bypassPolicyLockoutSafetyCheck to true to apply it anyway."
    )
}

/// The status, error code, reason and message the rule gives `verdict`.
fn expected_refusal(verdict: Verdict) -> Option<(&'static str, String)> {
    match verdict {
        Verdict::Attached => None,
        Verdict::CannotParse => Some((REASON_CANNOT_PARSE, cannot_parse_message())),
        Verdict::LocksEveryoneOut => Some((REASON_OTHER, lockout_message())),
    }
}

/// RULE, over `CreateCluster`'s inline `policy`: a document this service may not
/// hold is refused 400 `ValidationException` with the reason and sentence the rule
/// gives it, and NO cluster is created; an acceptable one is persisted and reads
/// back verbatim through `GetClusterPolicy`.
#[tokio::test]
async fn create_cluster_validates_its_inline_policy_before_it_mints_a_cluster() {
    for case in policy_cases("us-east-1") {
        // One store per row, so "a refused create creates nothing" is asserted
        // against a store that holds nothing else.
        let dsql = Dsql::new();
        let created = dsql
            .create(json!({"deletionProtectionEnabled": false, "policy": case.document.clone()}))
            .await;

        match expected_refusal(case.verdict) {
            Some((reason, message)) => {
                assert_eq!(
                    created.status, 400,
                    "case {:?}: the create must be refused with 400, got {} {:?}",
                    case.name, created.status, created.body
                );
                assert_eq!(
                    created.error.as_deref(),
                    Some("ValidationException"),
                    "case {:?}: the refusal must be a ValidationException, got {:?}",
                    case.name,
                    created.error
                );
                assert_eq!(
                    created.str("reason"),
                    reason,
                    "case {:?}: expected reason {reason:?}, got {:?}",
                    case.name,
                    created.str("reason")
                );
                assert_eq!(
                    created.message(),
                    message,
                    "case {:?}: expected the refusal sentence {message:?}, got {:?}",
                    case.name,
                    created.message()
                );
                assert!(
                    created.str("identifier").is_empty(),
                    "case {:?}: a refused create must mint no cluster, got {:?}",
                    case.name,
                    created.body
                );
                let listed = dsql.list("").await;
                assert_eq!(
                    listed.get("clusters"),
                    json!([]),
                    "case {:?}: a refused create must leave the store empty, got {:?}",
                    case.name,
                    listed.get("clusters")
                );
            }
            None => {
                assert_eq!(
                    created.status, 200,
                    "case {:?}: an acceptable document must be attached, got {} {:?}",
                    case.name, created.status, created.body
                );
                let identifier = created.str("identifier");
                let read = dsql.get_policy(&identifier).await;
                assert_eq!(
                    read.status, 200,
                    "case {:?}: the inline policy must be readable, got {} {:?}",
                    case.name, read.status, read.body
                );
                assert_eq!(
                    read.str("policy"),
                    case.document,
                    "case {:?}: the reader must report the document verbatim, got {:?}",
                    case.name,
                    read.str("policy")
                );
                assert!(
                    !read.str("policyVersion").is_empty(),
                    "case {:?}: the inline policy must carry a version, got {:?}",
                    case.name,
                    read.body
                );
            }
        }
    }
}

/// RULE, over the two entry points together: the verdict belongs to the DOCUMENT,
/// so `PutClusterPolicy` answers each row with the same status, error code, reason
/// and sentence `CreateCluster` answers it with.
#[tokio::test]
async fn both_entry_points_answer_the_same_document_the_same_way() {
    let dsql = Dsql::new();
    let cases = policy_cases(dsql.region());
    // One settled cluster per row, created before the single clock wait: a put
    // against a cluster the clock still owes a transition is a conflict, which is
    // a different rule (`tests/all/cluster_policy_mutation_status_gate.rs`).
    let clusters = dsql.create_settled_many(cases.len()).await;

    for (case, identifier) in cases.iter().zip(&clusters) {
        let attached = dsql
            .put_policy(identifier, json!({"policy": case.document.clone()}))
            .await;
        // The same document through the create path, in its own store.
        let fresh = Dsql::new();
        let created = fresh
            .create(json!({"deletionProtectionEnabled": false, "policy": case.document.clone()}))
            .await;

        assert_eq!(
            (attached.status, created.status),
            match case.verdict {
                Verdict::Attached => (200, 200),
                _ => (400, 400),
            },
            "case {:?}: both entry points must answer the same status, got put {} and create {}",
            case.name,
            attached.status,
            created.status
        );
        assert_eq!(
            attached.error, created.error,
            "case {:?}: both entry points must answer the same error code, got {:?} and {:?}",
            case.name, attached.error, created.error
        );
        assert_eq!(
            attached.str("reason"),
            created.str("reason"),
            "case {:?}: both entry points must answer the same reason, got {:?} and {:?}",
            case.name,
            attached.str("reason"),
            created.str("reason")
        );
        assert_eq!(
            attached.message(),
            created.message(),
            "case {:?}: both entry points must answer the same sentence, got {:?} and {:?}",
            case.name,
            attached.message(),
            created.message()
        );
    }
}

/// RULE: `bypassPolicyLockoutSafetyCheck` is about the LOCKOUT check only — set, it
/// applies a document nobody could replace, at either entry point; it does not make
/// a value that is not a policy document acceptable.
#[tokio::test]
async fn the_bypass_flag_waives_only_the_lockout_check() {
    let dsql = Dsql::new();
    let cases = policy_cases(dsql.region());
    let bypassable: Vec<&PolicyCase> = cases
        .iter()
        .filter(|case| case.verdict != Verdict::Attached)
        .collect();
    let clusters = dsql.create_settled_many(bypassable.len()).await;

    for (case, identifier) in bypassable.iter().zip(&clusters) {
        let fresh = Dsql::new();
        let created = fresh
            .create(json!({
                "deletionProtectionEnabled": false,
                "policy": case.document.clone(),
                "bypassPolicyLockoutSafetyCheck": true,
            }))
            .await;
        let attached = dsql
            .put_policy(
                identifier,
                json!({
                    "policy": case.document.clone(),
                    "bypassPolicyLockoutSafetyCheck": true,
                }),
            )
            .await;

        if case.verdict == Verdict::CannotParse {
            assert_eq!(
                (created.status, attached.status),
                (400, 400),
                "case {:?}: the bypass must not excuse a document that is not a policy, \
                 got create {} and put {}",
                case.name,
                created.status,
                attached.status
            );
            assert_eq!(
                (created.str("reason"), attached.str("reason")),
                (REASON_CANNOT_PARSE.to_string(), REASON_CANNOT_PARSE.to_string()),
                "case {:?}: the refusal must still name the unparseable document, got {:?} and {:?}",
                case.name,
                created.str("reason"),
                attached.str("reason")
            );
            continue;
        }

        assert_eq!(
            created.status, 200,
            "case {:?}: the bypass must let the create apply the document, got {} {:?}",
            case.name, created.status, created.body
        );
        assert_eq!(
            attached.status, 200,
            "case {:?}: the bypass must let the put apply the document, got {} {:?}",
            case.name, attached.status, attached.body
        );
        let inline = fresh.get_policy(&created.str("identifier")).await;
        assert_eq!(
            inline.str("policy"),
            case.document,
            "case {:?}: the bypassed inline document must read back verbatim, got {:?}",
            case.name,
            inline.str("policy")
        );
        let put = dsql.get_policy(identifier).await;
        assert_eq!(
            put.str("policy"),
            case.document,
            "case {:?}: the bypassed attached document must read back verbatim, got {:?}",
            case.name,
            put.str("policy")
        );
    }
}
