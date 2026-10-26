// SPDX-License-Identifier: CC0-1.0

//! Which CLASS `StartSearchResultExportJob` refuses a request in, and the wording of the
//! two refusals whose text is the whole explanation a caller gets. Driven over HTTP
//! exactly as a client drives it.
//!
//! Two rules, each asserted over a TABLE whose rows differ in what the rule generalises
//! over rather than over the one request the issue cited:
//!
//! * a `RoleArn` is judged by the ROLE it names, not by its text: a value that is a
//!   well-formed ARN is a role reference the service resolves, so one it cannot pass is
//!   the modelled `AccessDeniedException` (403) whatever the ARN names, while a value that
//!   is not an ARN at all, or is outside the modelled `IamRoleArn` length, is a
//!   member-value fault and stays a `ValidationException` (400). The rows differ in
//!   partition, service, region and account field, in what the resource field names (a
//!   role, a role under a path, a user, a table, a bucket), and in length — including both
//!   published bounds and one value past each.
//! * an export refused for the source job's STATUS carries the service's
//!   status-parameterised template, whose only variable part is that status; the rows are
//!   every status the service reports a search job in.

use serde_json::{json, Value};

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::export_listing::{listed_exports, passable_role, seed_job};
use crate::search_jobs::{call, emulator_with_engine};

/// The region every call in this suite is signed for.
const REGION: &str = "us-west-2";

/// What a row expects the service to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The export is started: the value names a role this caller can pass.
    Accepted,
    /// The pass-role decision refused the reference: 403 `AccessDeniedException`.
    PassRoleDenied,
    /// A member-value fault against the modelled `@pattern`: 400 `ValidationException`.
    InvalidPattern,
    /// A member-value fault against the modelled `@length`: 400 `ValidationException`.
    InvalidLength,
    /// A member-value fault against BOTH modelled constraints, reported together exactly
    /// as the request boundary reports two violations of one member.
    InvalidLengthAndPattern,
}

/// A `StartSearchResultExportJob` body naming `identifier`, with an S3 destination and an
/// optional `RoleArn`.
fn export_body(identifier: &str, role_arn: Option<&str>) -> Value {
    let mut body = json!({
        "SearchJobIdentifier": identifier,
        "ExportSpecification": {"s3ExportSpecification": {"DestinationBucket": "exports"}}
    });
    if let Some(role_arn) = role_arn {
        body["RoleArn"] = json!(role_arn);
    }
    body
}

/// The `IamRoleArn` `@length` bound the model publishes, min and max
/// (`smithy_query(query="shape",shape="IamRoleArn")`).
const ROLE_ARN_LENGTH: (usize, usize) = (20, 2048);

/// A role ARN of the caller's own account whose whole value is exactly `length`
/// characters long, so a row can sit ON a published length bound.
fn passable_role_of_length(length: usize) -> String {
    let prefix = passable_role("");
    let name = "x".repeat(length - prefix.chars().count());
    format!("{prefix}{name}")
}

#[tokio::test]
async fn a_role_arn_is_judged_by_the_role_it_names_not_by_its_text() {
    let account = engine::emulator_account_id();
    let rows: Vec<(&str, Option<String>, Expect)> = vec![
        // Values that name a role this caller can pass.
        (
            "an IAM role of the caller's account",
            Some(passable_role("backup-search-export")),
            Expect::Accepted,
        ),
        (
            "an IAM role of the caller's account under a path",
            Some(passable_role("service-role/backup-search-export")),
            Expect::Accepted,
        ),
        (
            "an IAM role of the caller's account in another partition",
            Some(format!("arn:aws-us-gov:iam::{account}:role/export")),
            Expect::Accepted,
        ),
        (
            "a role ARN exactly at the published maximum length",
            Some(passable_role_of_length(ROLE_ARN_LENGTH.1)),
            Expect::Accepted,
        ),
        (
            // The member is optional, so a request needing no role passed is accepted.
            "no RoleArn at all",
            None,
            Expect::Accepted,
        ),
        // Well-formed ARNs this caller cannot pass as a role: the authorization class,
        // whatever the ARN names.
        (
            "an IAM role of another account",
            Some("arn:aws:iam::123456789012:role/export".to_string()),
            Expect::PassRoleDenied,
        ),
        (
            "a well-formed ARN of another service carrying no account",
            Some("arn:aws:s3:::not-an-iam-role-arn-xxxxxxxx".to_string()),
            Expect::PassRoleDenied,
        ),
        (
            "a well-formed ARN of another service in the caller's own account",
            Some(format!("arn:aws:dynamodb:{REGION}:{account}:table/exports")),
            Expect::PassRoleDenied,
        ),
        (
            "an IAM ARN of the caller's account naming a user rather than a role",
            Some(format!("arn:aws:iam::{account}:user/engineer")),
            Expect::PassRoleDenied,
        ),
        (
            "an IAM role ARN carrying a region, which IAM ARNs do not",
            Some(format!("arn:aws:iam:{REGION}:{account}:role/export")),
            Expect::PassRoleDenied,
        ),
        (
            "an IAM role ARN in a partition the published role syntax does not list",
            Some(format!("arn:aws-iso:iam::{account}:role/export")),
            Expect::PassRoleDenied,
        ),
        (
            "a well-formed ARN exactly at the published minimum length",
            // 20 characters, so the length holds and only the pass-role decision is left.
            Some("arn:aws:s3:::bucket0".to_string()),
            Expect::PassRoleDenied,
        ),
        // Values that are not ARNs at all: the member-value class.
        (
            "a string that is no ARN at all",
            Some("not-a-valid-role-arn".to_string()),
            Expect::InvalidPattern,
        ),
        (
            "a role name alone, with no ARN around it",
            Some("AWSBackupSearchExportRole".to_string()),
            Expect::InvalidPattern,
        ),
        (
            "an arn-prefixed string with fewer fields than an ARN has",
            Some("arn:aws:iam:role/export".to_string()),
            Expect::InvalidPattern,
        ),
        // Values outside the published length bound.
        (
            "a well-formed ARN one character under the published minimum",
            Some("arn:aws:s3:::bucket".to_string()),
            Expect::InvalidLength,
        ),
        (
            "a role ARN one character over the published maximum",
            Some(passable_role_of_length(ROLE_ARN_LENGTH.1 + 1)),
            Expect::InvalidLength,
        ),
        (
            "a value that is neither an ARN nor long enough",
            Some("short".to_string()),
            Expect::InvalidLengthAndPattern,
        ),
    ];

    for (case, role_arn, expected) in rows {
        let (emulator, engine) = emulator_with_engine();
        let exportable = seed_job(&engine, state::STOPPED, Vec::new());
        let answer = call(
            &emulator,
            REGION,
            "PUT",
            "/export-search-jobs",
            Some(export_body(&exportable, role_arn.as_deref())),
        )
        .await;
        let message = answer.string("message");

        let expected_status = match expected {
            Expect::Accepted => 200,
            Expect::PassRoleDenied => 403,
            _ => 400,
        };
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected {expected_status} for RoleArn {role_arn:?}, got {} ({:?})",
            answer.status, answer.body
        );

        match expected {
            Expect::Accepted => {}
            Expect::PassRoleDenied => {
                assert_eq!(
                    answer.error_code.as_deref(),
                    Some("AccessDeniedException"),
                    "case {case:?}: a role this caller cannot pass is an authorization refusal, got {:?}",
                    answer.error_code
                );
                assert_eq!(
                    message,
                    state::CROSS_ACCOUNT_PASS_ROLE,
                    "case {case:?}: the pass-role refusal carries the service's fixed sentence, got {message:?}"
                );
            }
            Expect::InvalidPattern | Expect::InvalidLength | Expect::InvalidLengthAndPattern => {
                assert_eq!(
                    answer.error_code.as_deref(),
                    Some("ValidationException"),
                    "case {case:?}: a member-value fault is an input refusal, got {:?}",
                    answer.error_code
                );
                assert!(
                    message.contains("at 'roleArn'"),
                    "case {case:?}: the refusal must name the member it is about, got {message:?}"
                );
                let names_pattern = message.contains("regular expression pattern");
                let names_length = message.contains("length greater than or equal to 20")
                    || message.contains("length less than or equal to 2048");
                assert_eq!(
                    (names_pattern, names_length),
                    match expected {
                        Expect::InvalidPattern => (true, false),
                        Expect::InvalidLength => (false, true),
                        _ => (true, true),
                    },
                    "case {case:?}: expected {expected:?}, got {message:?}"
                );
            }
        }

        // Every refusal creates nothing; an accepted start creates exactly one record.
        let listed = listed_exports(&emulator, "").await;
        let created = listed
            .get("ExportJobs")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        let expected_created = usize::from(expected == Expect::Accepted);
        assert_eq!(
            created, expected_created,
            "case {case:?}: expected {expected_created} export job(s) afterwards, got {:?}",
            listed.body
        );
    }
}

#[tokio::test]
async fn an_export_refused_for_the_jobs_status_carries_the_services_template() {
    // Every status this service reports a search job in is a row, so the refusal's wording
    // is asserted over the whole status set rather than over the one status the issue
    // cited. The expected text is written out here rather than taken from the handler's
    // own helper: the template, with the job's status interpolated, is the requirement.
    let statuses: Vec<&str> = types::SEARCH_JOB_STATE_VALUES
        .iter()
        .copied()
        .chain(std::iter::once(state::CREATING))
        .collect();

    for status in statuses {
        let (emulator, engine) = emulator_with_engine();
        let identifier = seed_job(&engine, status, Vec::new());
        let answer = call(
            &emulator,
            REGION,
            "PUT",
            "/export-search-jobs",
            Some(export_body(&identifier, Some(&passable_role("export")))),
        )
        .await;

        if state::EXPORTABLE_SEARCH_JOB_STATES.contains(&status) {
            assert_eq!(
                answer.status, 200,
                "case {status:?}: an export of a settled job must be accepted, got {} ({:?})",
                answer.status, answer.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {status:?}: an export of a job whose results have not settled is refused 400, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {status:?}: the refusal must carry ValidationException, got {:?}",
            answer.error_code
        );
        let message = answer.string("message");
        assert_eq!(
            message,
            format!(
                "Export is only supported for search job with COMPLETED,STOPPED statuses. \
                 SearchJob has status {status}. Please try again later"
            ),
            "case {status:?}: the refusal must carry the service's template, got {message:?}"
        );
        assert!(
            !message.contains(&identifier),
            "case {status:?}: the template names no identifier, got {message:?}"
        );
    }
}
