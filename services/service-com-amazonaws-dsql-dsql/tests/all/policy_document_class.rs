// SPDX-License-Identifier: CC0-1.0

//! `GetClusterPolicy`: the CLASS of documents a cluster's policy can be.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with a
//! signed-looking restJson1 request — through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client, so the
//! `GET /cluster/{identifier}/policy` route, the `@httpLabel` binding, the boundary
//! constraint checks and the handler are exercised the way a client exercises them.
//!
//! `error_contract.rs` pins the policy LIFECYCLE (put, version token, delete,
//! lockout safety) and the wording of its errors, and `cluster_policy.rs` the
//! idempotency-token rules; this suite pins the DOCUMENT class. The table asserts
//! the RULE over a class of policy documents — differing in size (from the shortest
//! well-formed document to `PolicyDocument`'s published maximum of 20480
//! characters), in nesting, in member count, in whitespace and in character set —
//! not over one recorded document: the read reports back exactly the bytes that
//! were persisted, never a re-serialised or normalised rendering of them. Every row
//! is a parseable JSON document, which is what `PutClusterPolicy` accepts, so no
//! row pins either entry point's answer to an unparseable one.

use serde_json::json;

use crate::harness::{absent_identifier, wait_for_the_clock, Dsql};

/// `PolicyDocument`'s published length bounds. Source: the service's own model,
/// `smithy_query(query="shape", shape="PolicyDocument")` — `@length(min: 1, max:
/// 20480)`.
const POLICY_MIN_LEN: usize = 1;
const POLICY_MAX_LEN: usize = 20480;

/// A JSON policy document of exactly `len` characters, padded inside a `Sid`, so a
/// case can sit on a length bound and still be a well-formed document.
fn policy_of_length(len: usize) -> String {
    let prefix = r#"{"Version":"2012-10-17","Statement":[{"Sid":""#;
    let suffix = r#"","Effect":"Allow","Principal":"*","Action":"dsql:DbConnect","Resource":"*"}]}"#;
    let fixed = prefix.len() + suffix.len();
    assert!(
        len >= fixed,
        "a padded document cannot be shorter than its own {fixed} fixed characters"
    );
    format!("{prefix}{}{suffix}", "s".repeat(len - fixed))
}

/// One persisted policy document the read has to report back verbatim.
struct PolicyCase {
    name: &'static str,
    policy: String,
}

fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            // The shortest document that is still JSON. `PolicyDocument`'s modelled
            // minimum is POLICY_MIN_LEN character, but no one-character string
            // parses, and `PutClusterPolicy` refuses an unparseable document (pinned
            // by `error_contract.rs`), so a one-character document is not a case the
            // verbatim round trip should freeze.
            name: "the shortest well-formed document: an empty JSON object",
            policy: "{}".to_string(),
        },
        PolicyCase {
            name: "one statement, compact",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": "arn:aws:iam::111122223333:role/reader"},
                    "Action": "dsql:DbConnect",
                    "Resource": "*"
                }]
            })
            .to_string(),
        },
        PolicyCase {
            name: "several statements, nested conditions and a list of actions",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "AllowConnect",
                        "Effect": "Allow",
                        "Principal": {"AWS": ["arn:aws:iam::111122223333:root"]},
                        "Action": ["dsql:DbConnect", "dsql:DbConnectAdmin"],
                        "Resource": "*",
                        "Condition": {"StringEquals": {"aws:PrincipalOrgID": "o-abc123"}}
                    },
                    {
                        "Sid": "DenyEverythingElse",
                        "Effect": "Deny",
                        "Principal": "*",
                        "NotAction": "dsql:DbConnect",
                        "Resource": "*"
                    }
                ]
            })
            .to_string(),
        },
        PolicyCase {
            name: "pretty-printed, so the stored whitespace must survive the read",
            policy: "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n      \
                     \"Effect\": \"Allow\",\n      \"Principal\": \"*\",\n      \
                     \"Action\": \"dsql:DbConnect\",\n      \"Resource\": \"*\"\n    }\n  ]\n}"
                .to_string(),
        },
        PolicyCase {
            name: "non-ASCII characters in a Sid",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "lectüre-日本語-Σ",
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "dsql:DbConnect",
                    "Resource": "*"
                }]
            })
            .to_string(),
        },
        PolicyCase {
            name: "a document just inside the published maximum length",
            policy: policy_of_length(POLICY_MAX_LEN - 1),
        },
        PolicyCase {
            name: "a document at the published maximum length",
            policy: policy_of_length(POLICY_MAX_LEN),
        },
    ]
}

/// RULE: `GetClusterPolicy` reports back the resource-based policy document
/// persisted against the named cluster, byte for byte, together with the version
/// that stored document carries — for every document the `PolicyDocument`
/// constraint admits.
#[tokio::test]
async fn get_cluster_policy_reports_the_persisted_document_verbatim() {
    for case in policy_cases() {
        let dsql = Dsql::new();
        let created = dsql
            .create(json!({"deletionProtectionEnabled": false, "policy": case.policy}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateCluster with a policy must answer 200, got {} {:?}",
            case.name, created.status, created.body
        );
        let identifier = created.str("identifier");

        let answer = dsql.get_policy(&identifier).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: GetClusterPolicy must answer 200, got {} {:?}",
            case.name, answer.status, answer.body
        );
        assert_eq!(
            answer.str("policy"),
            case.policy,
            "case {:?}: the reported policy must be the persisted document verbatim, got {:?}",
            case.name,
            answer.str("policy")
        );
        let version = answer.str("policyVersion");
        assert!(
            !version.is_empty(),
            "case {:?}: policyVersion is @required, got {:?}",
            case.name,
            answer.get("policyVersion")
        );

        // A read must not change what a later caller sees: the same call answers
        // the same document and the same version, and the cluster itself is
        // untouched by having had its policy read.
        let again = dsql.get_policy(&identifier).await;
        assert_eq!(
            (again.status, again.str("policy"), again.str("policyVersion")),
            (200, case.policy.clone(), version),
            "case {:?}: a repeated read must answer identically, got {} {:?}",
            case.name,
            again.status,
            again.body
        );
        let cluster = dsql.get(&identifier).await;
        assert_eq!(
            cluster.status, 200,
            "case {:?}: GetCluster must still report the cluster after its policy was read, \
             got {} {:?}",
            case.name, cluster.status, cluster.body
        );
    }
}

/// RULE: the policy is readable for as long as the cluster holds it, in whatever
/// status the cluster is in — a read is not gated on the cluster having settled.
#[tokio::test]
async fn the_policy_is_readable_while_the_cluster_is_still_creating() {
    let policy = r#"{"Version":"2012-10-17","Statement":[]}"#;
    let dsql = Dsql::new();
    let created = dsql
        .create(json!({"deletionProtectionEnabled": false, "policy": policy}))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateCluster must succeed, got {} {:?}",
        created.status, created.body
    );
    let identifier = created.str("identifier");
    // The create's own answer names the status the cluster starts in, so this case
    // is self-describing without racing the clock a second time.
    assert_eq!(
        created.str("status"),
        "CREATING",
        "CreateCluster must start the cluster in CREATING, got {:?}",
        created.str("status")
    );

    // Straight after the create, while the clock still owes CREATING -> ACTIVE.
    let creating = dsql.get_policy(&identifier).await;
    assert_eq!(
        (creating.status, creating.str("policy")),
        (200, policy.to_string()),
        "case CREATING: the policy must be readable, got {} {:?}",
        creating.status,
        creating.body
    );

    wait_for_the_clock().await;
    let settled = dsql.get_policy(&identifier).await;
    assert_eq!(
        (settled.status, settled.str("policy")),
        (200, policy.to_string()),
        "case ACTIVE: the policy must still be readable, got {} {:?}",
        settled.status,
        settled.body
    );
    assert_eq!(
        settled.str("policyVersion"),
        creating.str("policyVersion"),
        "the version must not move because the cluster settled, got {:?} then {:?}",
        creating.str("policyVersion"),
        settled.str("policyVersion")
    );
}

/// RULE: both output members are `@required`, so there is no success response that
/// means "no policy": every cluster/policy pair this service does not hold is the
/// modelled `ResourceNotFoundException` (404).
#[tokio::test]
async fn a_policy_this_service_does_not_hold_is_not_found() {
    let dsql = Dsql::new();

    // A cluster that was never created.
    let absent = absent_identifier();

    // A cluster created without the optional `policy` member.
    let no_policy = dsql.create(json!({"deletionProtectionEnabled": false})).await;
    assert_eq!(
        no_policy.status, 200,
        "CreateCluster must succeed, got {} {:?}",
        no_policy.status, no_policy.body
    );
    let without_policy = no_policy.str("identifier");

    // A cluster that held a policy and has since been deleted by the clock.
    let had_policy = dsql
        .create(json!({
            "deletionProtectionEnabled": false,
            "policy": r#"{"Version":"2012-10-17","Statement":[]}"#
        }))
        .await;
    assert_eq!(
        had_policy.status, 200,
        "CreateCluster with a policy must succeed, got {} {:?}",
        had_policy.status, had_policy.body
    );
    let deleted = had_policy.str("identifier");
    wait_for_the_clock().await;
    let delete = dsql.delete(&deleted).await;
    assert_eq!(
        delete.status, 200,
        "DeleteCluster must be accepted, got {} {:?}",
        delete.status, delete.body
    );
    wait_for_the_clock().await;

    for (name, identifier) in [
        ("a cluster that was never created", absent.as_str()),
        ("a cluster created without a policy", without_policy.as_str()),
        ("a cluster whose deletion has completed", deleted.as_str()),
    ] {
        let answer = dsql.get_policy(identifier).await;
        assert_eq!(
            answer.status, 404,
            "case {name:?}: GetClusterPolicy must answer 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {name:?}: the 404 must be a ResourceNotFoundException, got {:?}",
            answer.error
        );
        // Every member of the modelled shape is @required. `message` is read
        // through the harness, which accepts either envelope casing.
        assert!(
            !answer.message().is_empty(),
            "case {name:?}: the error must carry a message, got {:?}",
            answer.body
        );
        for member in ["resourceId", "resourceType"] {
            assert!(
                !answer.str(member).is_empty(),
                "case {name:?}: the error must carry a {member}, got {:?}",
                answer.body
            );
        }
        assert_eq!(
            answer.str("resourceId"),
            identifier,
            "case {name:?}: resourceId must name the cluster asked for, got {:?}",
            answer.str("resourceId")
        );
    }
}

/// RULE: `PolicyDocument`'s published length bounds are enforced at the request
/// boundary, so no document outside them is ever persisted for the read to report.
#[tokio::test]
async fn a_document_outside_the_published_length_bounds_is_never_persisted() {
    let dsql = Dsql::new();
    for (name, policy) in [
        ("one character short of the minimum", String::new()),
        (
            "one character past the maximum",
            policy_of_length(POLICY_MAX_LEN + 1),
        ),
    ] {
        let created = dsql
            .create(json!({"deletionProtectionEnabled": false, "policy": policy}))
            .await;
        assert_eq!(
            created.status, 400,
            "case {name:?}: a policy outside {POLICY_MIN_LEN}..={POLICY_MAX_LEN} characters \
             must be refused with 400, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.error.as_deref(),
            Some("ValidationException"),
            "case {name:?}: the @length refusal must be a ValidationException, got {:?}",
            created.error
        );
        assert!(
            created.str("identifier").is_empty(),
            "case {name:?}: a refused create must mint no cluster, got {:?}",
            created.body
        );
        let listed = dsql.list("").await;
        assert_eq!(
            listed.get("clusters"),
            json!([]),
            "case {name:?}: a refused create must leave the store empty, got {:?}",
            listed.get("clusters")
        );
    }
}

/// RULE: an `identifier` outside `ClusterId`'s `^[a-z0-9]{26}$` is refused as
/// invalid input (400 `ValidationException`) rather than looked up.
///
/// `cluster_lifecycle.rs` asserts the same rule over the cluster operations; these
/// are the rows for the policy route, whose `@httpLabel` carries the same
/// constraint.
#[tokio::test]
async fn a_malformed_identifier_is_refused_by_get_cluster_policy() {
    let dsql = Dsql::new();
    for (name, identifier) in [
        ("too short", "abc"),
        ("one char short", "aaaaaaaaaaaaaaaaaaaaaaaaa"),
        ("one char long", "aaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ("uppercase", "AAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ("hyphenated", "aaaaaaaaaaaa-aaaaaaaaaaaaa"),
        ("underscored", "aaaaaaaaaaaa_aaaaaaaaaaaaa"),
    ] {
        let answer = dsql.get_policy(identifier).await;
        assert_eq!(
            answer.status, 400,
            "case {name:?}: a malformed identifier must be refused with 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ValidationException"),
            "case {name:?}: the refusal must be a ValidationException, got {:?}",
            answer.error
        );
    }
}
