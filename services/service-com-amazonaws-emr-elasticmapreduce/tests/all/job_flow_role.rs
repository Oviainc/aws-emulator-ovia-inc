// SPDX-License-Identifier: CC0-1.0

//! A cluster's `JobFlowRole` has to NAME an EC2 instance profile of this account.
//!
//! THE RULE: `RunJobFlow` resolves `JobFlowRole` — "also called instance profile and
//! Amazon EC2 role" (`RunJobFlow$JobFlowRole` doc) — to an EC2 instance profile IAM
//! holds in this account, and refuses the launch with HTTP 400 `ValidationException`
//! carrying `Invalid InstanceProfile: <value>.`, writing NO cluster, when the value
//! resolves to no such profile. A value that is not a profile name at all — the empty
//! string the member's `@length(0..10280)` admits, an ARN of another vendor, an ARN of
//! another IAM entity type, an ARN of another account, an ARN carrying a Region an IAM
//! ARN does not have, a malformed ARN — names nothing to resolve, so it is
//! refused without asking IAM; a value that does name a profile is resolved against
//! IAM, and a refusal of one is grounded in IAM's own answer.
//!
//! This suite carries only EMR, so there is no IAM here to answer: it owns the half of
//! the rule that does not depend on one — every value that names no instance profile is
//! refused, atomically, and a value that does name one is passed on to be resolved.
//! The IAM-grounded half (a profile IAM holds, a profile that was never created, a name
//! held only by a role) needs two services on one engine and lives in
//! `core/router/tests/all/emr_job_flow_role_instance_profile.rs`.
//!
//! The rows span what the rule generalises over: the form of the value (a friendly
//! name, an ARN, a malformed ARN, the empty string), which vendor, which account, which
//! Region field and which IAM entity type an ARN names, and whether a profile name can
//! be read out of it at all.
//! Every case drives the real HTTP path through the router's `dispatch`, and a launch
//! is only ever observed through `ListClusters` and `DescribeCluster`.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{assert_refused, at, Emr};

/// The error code EMR is recorded answering for an unresolvable instance profile.
const REFUSAL_CODE: &str = "ValidationException";

/// The account every ARN below names: the one this emulator serves.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// The error message, whichever casing the envelope carries it under.
fn error_message(payload: &Value) -> String {
    at(payload, &["Message"])
        .as_str()
        .or_else(|| at(payload, &["message"]).as_str())
        .unwrap_or_default()
        .to_string()
}

/// A `RunJobFlow` request naming `job_flow_role`.
fn request(name: &str, job_flow_role: &str) -> Value {
    json!({
        "Name": name,
        "ReleaseLabel": "emr-6.15.0",
        "ServiceRole": "EMR_DefaultRole",
        "Instances": {
            "InstanceCount": 1,
            "MasterInstanceType": "m5.xlarge",
            "KeepJobFlowAliveWhenNoSteps": true,
        },
        "JobFlowRole": job_flow_role,
    })
}

/// One row: the `JobFlowRole` a request names, and whether a profile name can be read
/// out of it (so the value is passed on to IAM) or not (so it is refused here).
struct Case {
    case: &'static str,
    /// The cluster's name, so each row writes — or fails to write — its own cluster.
    cluster: &'static str,
    job_flow_role: String,
    /// `true` when the value names a profile for IAM to resolve.
    names_a_profile: bool,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            // `@length(0..10280)` admits the empty string, which names nothing.
            case: "the empty JobFlowRole",
            cluster: "empty-role",
            job_flow_role: String::new(),
            names_a_profile: false,
        },
        Case {
            // An ARN of another vendor names nothing in IAM, whatever its last path
            // segment happens to spell.
            case: "an S3 ARN whose last segment spells a profile name",
            cluster: "s3-arn",
            job_flow_role: "arn:aws:s3:::bucket/EMR_EC2_DefaultRole".to_string(),
            names_a_profile: false,
        },
        Case {
            // An instance profile and a role are different IAM resources.
            case: "an IAM role ARN rather than an instance-profile ARN",
            cluster: "role-arn",
            job_flow_role: format!("arn:aws:iam::{}:role/EMR_EC2_DefaultRole", account()),
            names_a_profile: false,
        },
        Case {
            case: "an IAM ARN of a third resource type",
            cluster: "policy-arn",
            job_flow_role: format!("arn:aws:iam::{}:policy/EMRPolicy", account()),
            names_a_profile: false,
        },
        Case {
            case: "an instance-profile ARN whose name is empty",
            cluster: "nameless-arn",
            job_flow_role: format!("arn:aws:iam::{}:instance-profile/", account()),
            names_a_profile: false,
        },
        Case {
            // An IAM ARN is partition-global: it carries no Region, so a value with one
            // names no IAM resource, whatever its last path segment spells.
            case: "an instance-profile ARN carrying a Region",
            cluster: "region-bearing-arn",
            job_flow_role: format!(
                "arn:aws:iam:us-east-1:{}:instance-profile/EMR_EC2_DefaultRole",
                account()
            ),
            names_a_profile: false,
        },
        Case {
            // Another account's instance profile is not this account's to resolve.
            case: "an instance-profile ARN of another account",
            cluster: "cross-account-arn",
            job_flow_role: "arn:aws:iam::123456789012:instance-profile/EMR_EC2_DefaultRole"
                .to_string(),
            names_a_profile: false,
        },
        Case {
            // Six colon-separated fields or it is no ARN at all.
            case: "a value that starts with arn: but carries too few fields",
            cluster: "malformed-arn",
            job_flow_role: "arn:aws:iam:EMR_EC2_DefaultRole".to_string(),
            names_a_profile: false,
        },
        Case {
            case: "a friendly instance profile name",
            cluster: "friendly-name",
            job_flow_role: "EMR_EC2_DefaultRole".to_string(),
            names_a_profile: true,
        },
        Case {
            case: "a well-formed instance-profile ARN",
            cluster: "profile-arn",
            job_flow_role: format!(
                "arn:aws:iam::{}:instance-profile/EMR_EC2_DefaultRole",
                account()
            ),
            names_a_profile: true,
        },
        Case {
            case: "a well-formed instance-profile ARN under a path",
            cluster: "pathed-profile-arn",
            job_flow_role: format!(
                "arn:aws:iam::{}:instance-profile/service-role/EMR_EC2_DefaultRole",
                account()
            ),
            names_a_profile: true,
        },
    ]
}

/// Every cluster `ListClusters` reports, by name.
async fn cluster_names(emr: &Emr) -> Vec<String> {
    let listed = emr.ok("ListClusters", json!({})).await;
    at(&listed, &["Clusters"])
        .as_array()
        .map(|clusters| {
            clusters
                .iter()
                .filter_map(|cluster| at(cluster, &["Name"]).as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_job_flow_role_that_names_no_instance_profile_is_refused_atomically() {
    let emr = Emr::new();
    for case in cases() {
        let label = case.case;
        let (status, payload) = emr
            .call("RunJobFlow", request(case.cluster, &case.job_flow_role))
            .await;
        let listed = cluster_names(&emr).await;
        if case.names_a_profile {
            // Nothing here can say this profile is absent — there is no IAM in this
            // router to ask — so the launch stands, and the cluster reports back the
            // role it was given.
            assert_eq!(
                status, 200,
                "case {label:?}: expected the launch to stand, got {status} with {payload}"
            );
            let cluster_id = at(&payload, &["JobFlowId"])
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !cluster_id.is_empty(),
                "case {label:?}: expected a minted JobFlowId, got {payload}"
            );
            assert!(
                listed.iter().any(|name| name == case.cluster),
                "case {label:?}: expected the cluster to be listed, got {listed:?}"
            );
            let cluster = emr.describe_cluster(&cluster_id).await;
            assert_eq!(
                at(&cluster, &["Ec2InstanceAttributes", "IamInstanceProfile"]).as_str(),
                Some(case.job_flow_role.as_str()),
                "case {label:?}: expected the cluster to report the JobFlowRole it was \
                 given, got {cluster}"
            );
        } else {
            assert_refused(label, status, &payload, REFUSAL_CODE);
            assert_eq!(
                error_message(&payload),
                format!("Invalid InstanceProfile: {}.", case.job_flow_role),
                "case {label:?}: expected the instance-profile refusal, got {payload}"
            );
            assert!(
                !listed.iter().any(|name| name == case.cluster),
                "case {label:?}: expected a refused launch to write no cluster, got \
                 {listed:?}"
            );
        }
    }
}
