// SPDX-License-Identifier: CC0-1.0

//! A cluster launch has to NAME the IAM roles it runs under, and its `JobFlowRole` has to
//! name an EC2 instance profile of this account.
//!
//! THE RULE: every Amazon EC2 instance of a cluster assumes the instance profile the
//! launch names — "also called instance profile and Amazon EC2 role. An IAM role for an
//! Amazon EMR cluster. The Amazon EC2 instances of the cluster assume this role"
//! (`RunJobFlow$JobFlowRole` doc) — and Amazon EMR itself acts as the launch's
//! `ServiceRole`, so `RunJobFlow` requires BOTH members and then resolves the instance
//! profile against IAM, writing NO cluster when any of those fails. Three error classes
//! follow, and a client branching on the code learns which it has to fix:
//!
//! 1. a launch naming none of a role it requires is a malformed REQUEST: HTTP 400
//!    `ValidationException` carrying `<role> is required for creating cluster.`, where
//!    `<role>` is `InstanceProfile` for `JobFlowRole` and `ServiceRole` for `ServiceRole`
//!    — the member absent, or the empty string its `@length(0..10280)` admits, name no
//!    role alike;
//! 2. a `JobFlowRole` naming no instance profile this account holds is a malformed
//!    request too: HTTP 400 `ValidationException` carrying `Invalid InstanceProfile:
//!    <value>.`, whose one variable part is the value the request sent;
//! 3. a profile that EXISTS and cannot be authorized is an authorization failure for an
//!    otherwise well-formed request: HTTP 400 `AccessDeniedException` carrying `Failed to
//!    authorize instance profile <arn>`.
//!
//! A value that is not a profile name at all — an ARN of another vendor, an ARN of
//! another IAM entity type, an ARN of another account, an ARN carrying a Region an IAM
//! ARN does not have, a malformed ARN — names nothing IAM could hold a profile under, so
//! it falls in class 2 without asking IAM; a value that does name a profile is resolved
//! against IAM, and a refusal of one is grounded in IAM's own answer.
//!
//! This suite carries only EMR, so there is no IAM here to answer: it owns the classes
//! that do not depend on one — the two required members, and every value that names no
//! instance profile — plus the fact that a value that DOES name one is passed on to be
//! resolved. The IAM-grounded classes (a profile IAM holds, a profile that was never
//! created, a name held only by a role, a profile carrying no role EC2 may assume) need
//! two services on one engine and live in
//! `core/router/tests/all/emr_job_flow_role_instance_profile.rs`.
//!
//! The rows span what the rule generalises over: which of the two roles is missing,
//! whether the member is carried at all or carried empty, the form of the value (a
//! friendly name, an ARN, a malformed ARN), and which vendor, which account, which Region
//! field and which IAM entity type an ARN names.
//! Every case drives the real HTTP path through the router's `dispatch`, and a launch
//! is only ever observed through `ListClusters` and `DescribeCluster`.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{at, Emr, INSTANCE_PROFILE, SERVICE_ROLE};

/// The error code EMR answers a malformed launch request with.
const REQUEST_VALIDATION: &str = "ValidationException";

/// The account every ARN below names: the one this emulator serves.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// The awsJson1_1 error code, without the namespace the envelope may prefix.
fn error_code(payload: &Value) -> String {
    at(payload, &["__type"])
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The error message, whichever casing the envelope carries it under.
fn error_message(payload: &Value) -> String {
    at(payload, &["Message"])
        .as_str()
        .or_else(|| at(payload, &["message"]).as_str())
        .unwrap_or_default()
        .to_string()
}

/// A `RunJobFlow` request carrying the roles a row names, each only when it is `Some`.
fn request(name: &str, job_flow_role: Option<&str>, service_role: Option<&str>) -> Value {
    let mut body = json!({
        "Name": name,
        "ReleaseLabel": "emr-6.15.0",
        "Instances": {
            "InstanceCount": 1,
            "MasterInstanceType": "m5.xlarge",
            "KeepJobFlowAliveWhenNoSteps": true,
        },
    });
    if let Some(role) = job_flow_role {
        body["JobFlowRole"] = Value::String(role.to_string());
    }
    if let Some(role) = service_role {
        body["ServiceRole"] = Value::String(role.to_string());
    }
    body
}

/// What a row expects of its launch.
#[derive(Debug, PartialEq, Eq)]
enum Expected {
    /// The value names a profile for IAM to resolve, so the launch stands here.
    Launched,
    /// A role the launch requires was not named; the one the sentence states.
    Required(&'static str),
    /// The value names no instance profile of this account; the value the sentence
    /// echoes.
    Invalid(String),
}

/// One row: the roles a request carries, and what the launch is answered with.
struct Case {
    case: &'static str,
    /// The cluster's name, so each row writes — or fails to write — its own cluster.
    cluster: &'static str,
    job_flow_role: Option<String>,
    service_role: Option<String>,
    expected: Expected,
}

/// A row whose `ServiceRole` is the one every launch that is meant to get past it names.
fn with_profile(
    case: &'static str,
    cluster: &'static str,
    job_flow_role: Option<String>,
    expected: Expected,
) -> Case {
    Case {
        case,
        cluster,
        job_flow_role,
        service_role: Some(SERVICE_ROLE.to_string()),
        expected,
    }
}

fn cases() -> Vec<Case> {
    vec![
        with_profile(
            "no JobFlowRole member at all",
            "absent-role",
            None,
            Expected::Required("InstanceProfile"),
        ),
        with_profile(
            // `@length(0..10280)` admits the empty string, which names nothing.
            "the empty JobFlowRole",
            "empty-role",
            Some(String::new()),
            Expected::Required("InstanceProfile"),
        ),
        Case {
            case: "no ServiceRole member at all",
            cluster: "absent-service-role",
            job_flow_role: Some(INSTANCE_PROFILE.to_string()),
            service_role: None,
            expected: Expected::Required("ServiceRole"),
        },
        Case {
            case: "the empty ServiceRole",
            cluster: "empty-service-role",
            job_flow_role: Some(INSTANCE_PROFILE.to_string()),
            service_role: Some(String::new()),
            expected: Expected::Required("ServiceRole"),
        },
        Case {
            // Neither role named: the roles are checked in order, so the instance
            // profile is the one the refusal states.
            case: "neither role named",
            cluster: "no-roles",
            job_flow_role: None,
            service_role: None,
            expected: Expected::Required("InstanceProfile"),
        },
        with_profile(
            // An ARN of another vendor names nothing in IAM, whatever its last path
            // segment happens to spell.
            "an S3 ARN whose last segment spells a profile name",
            "s3-arn",
            Some("arn:aws:s3:::bucket/EMR_EC2_DefaultRole".to_string()),
            Expected::Invalid("arn:aws:s3:::bucket/EMR_EC2_DefaultRole".to_string()),
        ),
        with_profile(
            // An instance profile and a role are different IAM resources.
            "an IAM role ARN rather than an instance-profile ARN",
            "role-arn",
            Some(format!("arn:aws:iam::{}:role/EMR_EC2_DefaultRole", account())),
            Expected::Invalid(format!(
                "arn:aws:iam::{}:role/EMR_EC2_DefaultRole",
                account()
            )),
        ),
        with_profile(
            "an IAM ARN of a third resource type",
            "policy-arn",
            Some(format!("arn:aws:iam::{}:policy/EMRPolicy", account())),
            Expected::Invalid(format!("arn:aws:iam::{}:policy/EMRPolicy", account())),
        ),
        with_profile(
            "an instance-profile ARN whose name is empty",
            "nameless-arn",
            Some(format!("arn:aws:iam::{}:instance-profile/", account())),
            Expected::Invalid(format!("arn:aws:iam::{}:instance-profile/", account())),
        ),
        with_profile(
            // An IAM ARN is partition-global: it carries no Region, so a value with one
            // names no IAM resource, whatever its last path segment spells.
            "an instance-profile ARN carrying a Region",
            "region-bearing-arn",
            Some(format!(
                "arn:aws:iam:us-east-1:{}:instance-profile/EMR_EC2_DefaultRole",
                account()
            )),
            Expected::Invalid(format!(
                "arn:aws:iam:us-east-1:{}:instance-profile/EMR_EC2_DefaultRole",
                account()
            )),
        ),
        with_profile(
            // Another account's instance profile is not this account's to resolve.
            "an instance-profile ARN of another account",
            "cross-account-arn",
            Some("arn:aws:iam::123456789012:instance-profile/EMR_EC2_DefaultRole".to_string()),
            Expected::Invalid(
                "arn:aws:iam::123456789012:instance-profile/EMR_EC2_DefaultRole".to_string(),
            ),
        ),
        with_profile(
            // Six colon-separated fields or it is no ARN at all.
            "a value that starts with arn: but carries too few fields",
            "malformed-arn",
            Some("arn:aws:iam:EMR_EC2_DefaultRole".to_string()),
            Expected::Invalid("arn:aws:iam:EMR_EC2_DefaultRole".to_string()),
        ),
        with_profile(
            "a friendly instance profile name",
            "friendly-name",
            Some(INSTANCE_PROFILE.to_string()),
            Expected::Launched,
        ),
        with_profile(
            "a well-formed instance-profile ARN",
            "profile-arn",
            Some(format!(
                "arn:aws:iam::{}:instance-profile/{INSTANCE_PROFILE}",
                account()
            )),
            Expected::Launched,
        ),
        with_profile(
            "a well-formed instance-profile ARN under a path",
            "pathed-profile-arn",
            Some(format!(
                "arn:aws:iam::{}:instance-profile/service-role/{INSTANCE_PROFILE}",
                account()
            )),
            Expected::Launched,
        ),
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
            .call(
                "RunJobFlow",
                request(
                    case.cluster,
                    case.job_flow_role.as_deref(),
                    case.service_role.as_deref(),
                ),
            )
            .await;
        let listed = cluster_names(&emr).await;
        match &case.expected {
            Expected::Launched => {
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
                    case.job_flow_role.as_deref(),
                    "case {label:?}: expected the cluster to report the JobFlowRole it was \
                     given, got {cluster}"
                );
            }
            Expected::Required(role) => {
                assert_eq!(
                    (status, error_code(&payload).as_str()),
                    (400, REQUEST_VALIDATION),
                    "case {label:?}: a launch naming no {role} must be refused 400 \
                     {REQUEST_VALIDATION}, got {status} {payload}"
                );
                assert_eq!(
                    error_message(&payload),
                    format!("{role} is required for creating cluster."),
                    "case {label:?}: expected the required-role refusal naming {role:?}, \
                     got {payload}"
                );
            }
            Expected::Invalid(value) => {
                assert_eq!(
                    (status, error_code(&payload).as_str()),
                    (400, REQUEST_VALIDATION),
                    "case {label:?}: a value naming no instance profile must be refused \
                     400 {REQUEST_VALIDATION}, got {status} {payload}"
                );
                assert_eq!(
                    error_message(&payload),
                    format!("Invalid InstanceProfile: {value}."),
                    "case {label:?}: expected the unresolvable-profile refusal echoing \
                     {value:?}, got {payload}"
                );
            }
        }
        if case.expected != Expected::Launched {
            assert!(
                !listed.iter().any(|name| name == case.cluster),
                "case {label:?}: expected a refused launch to write no cluster, got \
                 {listed:?}"
            );
        }
    }
}
