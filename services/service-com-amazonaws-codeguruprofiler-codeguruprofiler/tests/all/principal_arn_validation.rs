// SPDX-License-Identifier: CC0-1.0

//! What a `PutPermission` grant may name, end to end over HTTP through the router this
//! service mounts in production.
//!
//! The rule every case here asserts: a member documented as the ARN of a user or role —
//! a resource-based policy's PRINCIPAL — is granted to only when it is the ARN of an IAM
//! principal (an account root, a user, a role, a federated user or an assumed-role
//! session: no Region, a numeric account, no wildcard), and any other value is refused
//! with a 400 `ValidationException` that leaves the group's policy exactly as it was.
//!
//! The rows differ along what that rule generalises over: which identity form the value
//! takes, which vendor and partition it carries, which ARN field is wrong, whether it is
//! an ARN at all, where a wildcard sits, and at which position in the submitted list the
//! bad element is. The catalogue of ACCEPTED forms is `service_common::arn`'s
//! (`IDENTITY_RESOURCES`), itself the five forms IAM and STS write; none of the rows is
//! the value the issue's recording used.
//!
//! The write and read helpers are [`crate::permission_writes`]', and the harness
//! [`crate::profiling_group_lifecycle`]', so this suite drives the same emulator rather
//! than forking one.

use crate::permission_writes::{get_policy, put_permission, string_member};
use crate::profiling_group_lifecycle::{ACCOUNT, Harness};

/// The only value the modelled `ActionGroup` enum declares.
const ACTION_GROUP: &str = "agentPermissions";

/// One principal row: the value submitted and whether a grant may name it.
struct PrincipalCase {
    label: &'static str,
    value: String,
    granted: bool,
}

/// The table. The accepted rows are the identity forms IAM and STS write, across
/// partitions and with and without a path; the refused rows are a value that is no ARN, a
/// bare account id, another vendor's resource, an IAM resource that is no identity, each
/// ARN field written in a way IAM never writes it, and wildcards.
fn principal_cases() -> Vec<PrincipalCase> {
    let rows: Vec<(&'static str, String, bool)> = vec![
        // Accepted: the five forms, plus a path and another partition.
        ("the account root", format!("arn:aws:iam::{ACCOUNT}:root"), true),
        (
            "an IAM user",
            format!("arn:aws:iam::{ACCOUNT}:user/profiler-analyst"),
            true,
        ),
        (
            "an IAM user under a path",
            format!("arn:aws:iam::{ACCOUNT}:user/engineering/profiler-analyst"),
            true,
        ),
        (
            "an IAM role",
            format!("arn:aws:iam::{ACCOUNT}:role/ProfilerAgent"),
            true,
        ),
        (
            "a role under a path, as a service-linked role is written",
            format!("arn:aws:iam::{ACCOUNT}:role/aws-service-role/profiler.amazonaws.com/Agent"),
            true,
        ),
        (
            "an assumed-role session",
            format!("arn:aws:sts::{ACCOUNT}:assumed-role/ProfilerAgent/deploy-1"),
            true,
        ),
        (
            "a federated user",
            format!("arn:aws:sts::{ACCOUNT}:federated-user/Carol"),
            true,
        ),
        (
            "an identity in another partition",
            format!("arn:aws-us-gov:iam::{ACCOUNT}:role/ProfilerAgent"),
            true,
        ),
        // Refused: not an ARN, or an ARN that names no identity.
        ("a bare word", "profiler-agent".to_string(), false),
        ("a bare account id", ACCOUNT.to_string(), false),
        ("blank text", "   ".to_string(), false),
        ("the empty string", String::new(), false),
        (
            "another vendor's resource",
            "arn:aws:s3:::profiler-bucket".to_string(),
            false,
        ),
        (
            "this service's own resource",
            format!("arn:aws:codeguru-profiler:us-east-1:{ACCOUNT}:profilingGroup/pg"),
            false,
        ),
        (
            "an IAM resource that is no identity",
            format!("arn:aws:iam::{ACCOUNT}:policy/ReadOnlyAccess"),
            false,
        ),
        (
            "an identity type of the other vendor",
            format!("arn:aws:iam::{ACCOUNT}:assumed-role/ProfilerAgent/deploy-1"),
            false,
        ),
        (
            "a root the other vendor does not write",
            format!("arn:aws:sts::{ACCOUNT}:root"),
            false,
        ),
        (
            "a Region, which an identity ARN never carries",
            format!("arn:aws:iam:us-east-1:{ACCOUNT}:role/ProfilerAgent"),
            false,
        ),
        (
            "an empty account",
            "arn:aws:iam:::role/ProfilerAgent".to_string(),
            false,
        ),
        (
            "an account that is not a number",
            "arn:aws:iam::my-account:role/ProfilerAgent".to_string(),
            false,
        ),
        (
            "an account id shorter than twelve digits",
            "arn:aws:iam::1:role/ProfilerAgent".to_string(),
            false,
        ),
        (
            "a colon inside the name, which IAM does not write",
            format!("arn:aws:iam::{ACCOUNT}:role/Profiler:Agent"),
            false,
        ),
        (
            "a role with no name",
            format!("arn:aws:iam::{ACCOUNT}:role/"),
            false,
        ),
        (
            "a session with no session name",
            format!("arn:aws:sts::{ACCOUNT}:assumed-role/ProfilerAgent"),
            false,
        ),
        (
            "the colon form, which IAM does not write for an identity",
            format!("arn:aws:iam::{ACCOUNT}:role:ProfilerAgent"),
            false,
        ),
        (
            "a wildcard for the whole name",
            format!("arn:aws:iam::{ACCOUNT}:role/*"),
            false,
        ),
        (
            "a wildcard inside the name",
            format!("arn:aws:iam::{ACCOUNT}:role/Profiler*"),
            false,
        ),
        (
            "a single-character wildcard",
            format!("arn:aws:iam::{ACCOUNT}:user/analys?"),
            false,
        ),
        ("a bare wildcard", "*".to_string(), false),
    ];
    rows.into_iter()
        .map(|(label, value, granted)| PrincipalCase {
            label,
            value,
            granted,
        })
        .collect()
}

/// Create one profiling group, so each row starts from a group that holds no policy.
async fn create_group(harness: &Harness, name: &str) {
    let created = harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken={name}-token"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "profilingGroupName": name }))
                .expect("a serializable body"),
        )
        .await;
    assert_eq!(
        created.status,
        201,
        "case {name}: the group must be created first, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
}

/// The `Principal.AWS` of the one statement a policy document carries.
fn granted_principal(policy: &str) -> serde_json::Value {
    let document: serde_json::Value = serde_json::from_str(policy)
        .unwrap_or_else(|error| panic!("policy must be a JSON document, got {error}: {policy}"));
    document
        .pointer("/Statement/0/Principal/AWS")
        .cloned()
        .unwrap_or_else(|| panic!("no Statement[0].Principal.AWS in {document}"))
}

#[tokio::test]
async fn a_grant_names_an_iam_principal_arn_or_is_refused_and_writes_nothing() {
    let harness = Harness::new();

    for (index, case) in principal_cases().into_iter().enumerate() {
        let label = case.label;
        let group = format!("cgp-principal-{index}");
        create_group(&harness, &group).await;

        let answer = put_permission(
            &harness,
            &group,
            ACTION_GROUP,
            std::slice::from_ref(&case.value),
            None,
        )
        .await;

        if case.granted {
            assert_eq!(
                answer.status,
                200,
                "case {label:?} ({:?}): expected 200, got {} {}",
                case.value,
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            // Read back through GetPolicy: the stored grant is the ARN submitted, and the
            // revision the write minted is the one the group now carries.
            let revision = string_member(&answer, "revisionId");
            let read = get_policy(&harness, &group).await;
            assert_eq!(
                read.status,
                200,
                "case {label:?}: GetPolicy must answer 200, got {} {}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                string_member(&read, "revisionId"),
                revision,
                "case {label:?}: GetPolicy must report the revision the write minted, got {}",
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                granted_principal(&string_member(&read, "policy")),
                serde_json::Value::from(case.value.clone()),
                "case {label:?}: expected the submitted ARN as the statement's principal, got {}",
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                harness.stored_policy_revision(&group),
                Some(revision),
                "case {label:?}: the group must hold the policy the write left"
            );
        } else {
            assert_eq!(
                answer.status,
                400,
                "case {label:?} ({:?}): expected 400, got {} {}",
                case.value,
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            // Nothing was written: the group still holds no policy, by the store and by
            // the read API alike.
            assert_eq!(
                harness.stored_policy_revision(&group),
                None,
                "case {label:?}: a refused grant must leave the group without a policy"
            );
            let read = get_policy(&harness, &group).await;
            assert_eq!(
                read.status,
                200,
                "case {label:?}: GetPolicy must answer 200, got {} {}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                read.json(),
                serde_json::json!({}),
                "case {label:?}: a refused grant must leave nothing to report, got {}",
                String::from_utf8_lossy(&read.body)
            );
        }
    }
}

#[tokio::test]
async fn one_bad_principal_refuses_the_whole_grant_wherever_it_sits() {
    let harness = Harness::new();
    let good = format!("arn:aws:iam::{ACCOUNT}:role/ProfilerAgent");
    let other = format!("arn:aws:sts::{ACCOUNT}:assumed-role/ProfilerAgent/deploy-1");
    let bad = "profiler-agent";

    // (case, the submitted list, the 1-based position the refusal must name)
    let rows: Vec<(&str, Vec<String>, usize)> = vec![
        ("first of two", vec![bad.to_string(), good.clone()], 1),
        ("second of two", vec![good.clone(), bad.to_string()], 2),
        (
            "last of three",
            vec![good.clone(), other.clone(), bad.to_string()],
            3,
        ),
        (
            "middle of three",
            vec![good.clone(), bad.to_string(), other.clone()],
            2,
        ),
    ];

    for (index, (label, principals, position)) in rows.into_iter().enumerate() {
        let group = format!("cgp-principal-position-{index}");
        create_group(&harness, &group).await;

        // The group already holds a policy, so the case also asserts that a refused write
        // leaves the PREVIOUS revision standing rather than merely writing nothing.
        let seeded = put_permission(
            &harness,
            &group,
            ACTION_GROUP,
            std::slice::from_ref(&good),
            None,
        )
        .await;
        assert_eq!(
            seeded.status,
            200,
            "case {label:?}: the first grant must be written, got {} {}",
            seeded.status,
            String::from_utf8_lossy(&seeded.body)
        );
        let revision = string_member(&seeded, "revisionId");

        let answer = put_permission(
            &harness,
            &group,
            ACTION_GROUP,
            &principals,
            Some(&revision),
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {label:?}: expected 400, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
        let message = answer.error_message();
        assert!(
            message.contains(&format!("'principals.{position}.member'")),
            "case {label:?}: the refusal must name the element that failed, got {message:?}"
        );
        assert!(
            message.contains(bad),
            "case {label:?}: the refusal must state the value submitted, got {message:?}"
        );
        assert_eq!(
            harness.stored_policy_revision(&group),
            Some(revision.clone()),
            "case {label:?}: a refused grant must leave the stored revision alone"
        );
        let read = get_policy(&harness, &group).await;
        assert_eq!(
            granted_principal(&string_member(&read, "policy")),
            serde_json::Value::from(good.clone()),
            "case {label:?}: the earlier grant must still stand, got {}",
            String::from_utf8_lossy(&read.body)
        );
    }
}
