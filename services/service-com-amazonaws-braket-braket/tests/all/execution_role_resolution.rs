// SPDX-License-Identifier: CC0-1.0

//! `CreateJob`: the execution role a hybrid job is created with is resolved against the
//! account's IAM records BEFORE the job is persisted.
//!
//! The rule under test: a create whose member names the IAM role the service will act AS
//! resolves that role at create time — the check is not deferred to the job's own run — and
//! a value IAM answers for as a role this account does not hold, or one whose trust policy
//! does not admit the Braket service principal, is refused with the modelled
//! `ValidationException` (400) with nothing persisted. A value IAM was not asked about,
//! because it names no role of THIS account, grounds no refusal: another account's IAM is
//! not this emulator's to answer for.
//!
//! `roleArn` on `CreateJob` is the only member of this service that names an IAM role
//! (`smithy_query(query="digest")` lists no other), so this is the whole surface the rule
//! covers.
//!
//! The rows span what the rule generalises over: WHAT the value names (a role of this
//! account trusted by Braket, the same role under a path, a role trusted among several
//! principals, a role trusted through a wildcard action, a role that exists but trusts
//! another service, a role whose trust policy denies Braket explicitly, a well-formed ARN
//! naming a role the account does not hold, and a role of another account).
//!
//! IAM is mounted beside this service on ONE engine and every role is created through IAM's
//! own operations, so a role exists only because IAM answered for it, and the resolution
//! under test runs over the same in-process callback as in production.

use serde_json::{json, Value};

use crate::quantum_task_loop::{encode, member, Braket};

/// The region the harness signs its Braket requests for, so a minted ARN carries it.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The principal a role Amazon Braket acts under has to trust.
const BRAKET_PRINCIPAL: &str = "braket.amazonaws.com";

/// A service principal that is NOT Braket, for the role that exists and trusts someone
/// else.
const OTHER_PRINCIPAL: &str = "sagemaker.amazonaws.com";

/// The error code a refused create answers with, and the status that carries it.
const VALIDATION: &str = "ValidationException";

/// A trust policy carrying `statements`.
fn trust(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// One trust statement: `effect` the `action` for the service principals `principals`.
fn statement(effect: &str, principals: Value, action: &str) -> Value {
    json!({
        "Effect": effect,
        "Principal": { "Service": principals },
        "Action": action,
    })
}

/// Create `role_name` under `path` with `trust_document`, through IAM itself, and answer
/// the ARN IAM minted for it.
async fn create_role(
    iam: &::aws_sdk_iam::Client,
    path: &str,
    role_name: &str,
    trust_document: &str,
) -> String {
    iam.create_role()
        .role_name(role_name)
        .path(path)
        .assume_role_policy_document(trust_document)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: IAM must create the role {role_name}: {error:?}"))
        .role()
        .map(|role| role.arn().to_string())
        .unwrap_or_else(|| panic!("setup: IAM must report an ARN for {role_name}"))
}

/// A `CreateJob` body with only the required members filled in, running as `role_arn`.
fn job_body(client_token: &str, job_name: &str, role_arn: &str, device_arn: &str) -> Value {
    json!({
        "clientToken": client_token,
        "jobName": job_name,
        "roleArn": role_arn,
        "algorithmSpecification": {
            "containerImage": {
                "uri": "000000000000.dkr.ecr.eu-west-1.amazonaws.com/braket:latest"
            }
        },
        "outputDataConfig": { "s3Path": "s3://amazon-braket-emulator/jobs" },
        "instanceConfig": { "instanceType": "ml.m5.large", "volumeSizeInGb": 30 },
        "deviceConfig": { "device": device_arn },
    })
}

/// The ARN `CreateJob` mints for `job_name` in this harness's credential scope — the ARN a
/// refused create must leave unresolvable.
fn job_arn(job_name: &str) -> String {
    format!("arn:aws:braket:{REGION}:{ACCOUNT}:job/{job_name}")
}

/// One role value the rule is asserted over.
struct Row {
    /// What this row varies.
    case: &'static str,
    /// The value `roleArn` carries.
    role_arn: String,
    /// Whether the create must be accepted.
    accepted: bool,
}

#[tokio::test]
async fn a_jobs_execution_role_is_resolved_before_the_job_is_persisted() {
    let (braket, iam) = Braket::with_iam();
    let device = braket.an_online_device().await;

    let trusted = create_role(
        &iam,
        "/",
        "braket-jobs-execution",
        &trust(json!([statement(
            "Allow",
            json!(BRAKET_PRINCIPAL),
            "sts:AssumeRole"
        )])),
    )
    .await;
    let trusted_under_path = create_role(
        &iam,
        "/service-role/",
        "braket-jobs-execution-pathed",
        &trust(json!(statement(
            "Allow",
            json!(BRAKET_PRINCIPAL),
            "sts:AssumeRole"
        ))),
    )
    .await;
    let trusted_among_several = create_role(
        &iam,
        "/",
        "braket-jobs-shared",
        &trust(json!([statement(
            "Allow",
            json!([OTHER_PRINCIPAL, BRAKET_PRINCIPAL]),
            "sts:AssumeRole"
        )])),
    )
    .await;
    let trusted_by_wildcard_action = create_role(
        &iam,
        "/",
        "braket-jobs-wildcard",
        &trust(json!([statement(
            "Allow",
            json!(BRAKET_PRINCIPAL),
            "sts:*"
        )])),
    )
    .await;
    let untrusted = create_role(
        &iam,
        "/",
        "braket-jobs-other-service",
        &trust(json!([statement(
            "Allow",
            json!(OTHER_PRINCIPAL),
            "sts:AssumeRole"
        )])),
    )
    .await;
    let denied = create_role(
        &iam,
        "/",
        "braket-jobs-denied",
        &trust(json!([
            statement("Allow", json!(BRAKET_PRINCIPAL), "sts:AssumeRole"),
            statement("Deny", json!(BRAKET_PRINCIPAL), "sts:AssumeRole"),
        ])),
    )
    .await;

    let rows = [
        Row {
            case: "a role of this account that trusts Braket",
            role_arn: trusted.clone(),
            accepted: true,
        },
        Row {
            case: "the same role under a path",
            role_arn: trusted_under_path,
            accepted: true,
        },
        Row {
            case: "a role that trusts Braket among several principals",
            role_arn: trusted_among_several,
            accepted: true,
        },
        Row {
            case: "a role that trusts Braket through a wildcard action",
            role_arn: trusted_by_wildcard_action,
            accepted: true,
        },
        Row {
            // IAM holds no role under that name: the class the recorded refusal belongs
            // to — a well-formed, account-correct ARN naming a role that is not there.
            case: "a well-formed ARN naming a role the account does not hold",
            role_arn: format!("{trusted}-absent"),
            accepted: false,
        },
        Row {
            case: "a role that exists but trusts another service",
            role_arn: untrusted,
            accepted: false,
        },
        Row {
            case: "a role whose trust policy denies Braket explicitly",
            role_arn: denied,
            accepted: false,
        },
        Row {
            // This account's IAM cannot answer for another account's roles, so nothing
            // grounds a refusal.
            case: "a role of another account",
            role_arn: "arn:aws:iam::123456789012:role/BraketCrossAccount".to_string(),
            accepted: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let job_name = format!("role-row-{index}");
        let reply = braket
            .call(
                "POST",
                "/job",
                Some(job_body(
                    &format!("tok-role-{index}"),
                    &job_name,
                    &row.role_arn,
                    &device,
                )),
            )
            .await;
        let case = row.case;
        if row.accepted {
            assert_eq!(
                reply.status, 201,
                "case {case:?}: expected the create to be accepted, got {} {}",
                reply.status, reply.body
            );
            let arn = reply.str_member("jobArn");
            assert_eq!(
                arn,
                job_arn(&job_name),
                "case {case:?}: expected the minted ARN to carry the job name, got {arn:?}"
            );
            // Read the job back through the implemented reader: an accepted create
            // persisted a job that runs as the role the request named.
            let read = braket
                .call("GET", &format!("/job/{}", encode(&arn)), None)
                .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected GetJob to report the created job, got {} {}",
                read.status, read.body
            );
            assert_eq!(
                member(&read.body, "roleArn"),
                row.role_arn,
                "case {case:?}: expected the stored job to run as {}, got {}",
                row.role_arn,
                read.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected the create to be refused, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some(VALIDATION),
                "case {case:?}: expected {VALIDATION}, got {:?} in {}",
                reply.error,
                reply.body
            );
            let expected = format!("Braket cannot assume the IAM role: '{}'", row.role_arn);
            assert_eq!(
                reply.message(),
                expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                reply.message()
            );
            // Nothing was written: the ARN the job would have carried resolves to no job.
            let read = braket
                .call("GET", &format!("/job/{}", encode(&job_arn(&job_name))), None)
                .await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: a refused create must persist nothing, got {} {}",
                read.status, read.body
            );
        }
    }
}

/// The resolution is grounded in what IAM ANSWERED: with no IAM to ask, the same values
/// are all accepted, so the refusal is never the emulator guessing about a role.
#[tokio::test]
async fn with_no_iam_mounted_no_role_grounds_a_refusal() {
    let braket = Braket::new();
    let device = braket.an_online_device().await;

    // (case, roleArn) — every shape the suite above refuses, plus one it accepts.
    let rows: &[(&str, String)] = &[
        (
            "a role of this account IAM was never asked about",
            format!("arn:aws:iam::{ACCOUNT}:role/braket-jobs-execution"),
        ),
        (
            "the same role under a path",
            format!("arn:aws:iam::{ACCOUNT}:role/service-role/braket-jobs-execution"),
        ),
        (
            "a role of another account",
            "arn:aws:iam::123456789012:role/BraketCrossAccount".to_string(),
        ),
    ];

    for (index, (case, role_arn)) in rows.iter().enumerate() {
        let job_name = format!("no-iam-row-{index}");
        let reply = braket
            .call(
                "POST",
                "/job",
                Some(job_body(
                    &format!("tok-no-iam-{index}"),
                    &job_name,
                    role_arn,
                    &device,
                )),
            )
            .await;
        assert_eq!(
            reply.status, 201,
            "case {case:?}: with no IAM to answer, nothing grounds a refusal, got {} {}",
            reply.status, reply.body
        );
    }
}
