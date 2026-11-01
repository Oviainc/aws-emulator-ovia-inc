// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the instant `GetWorkflow` reports for the copy of a workflow's
//! code artifacts.
//!
//! RULE under test: `CodeSnapshottedAt` reports when the code artifacts of the version
//! being read were copied, and a copy only ever happens for a request that supplied a
//! `Code` location — "the service copies the code from this location at the time of the
//! request" (`smithy_query(query="doc", shape="CreateWorkflowRequest", member="Code")`).
//! So a read reports the creation instant of the version it resolves when that version
//! froze a location, and reports NOTHING when it froze none — exactly as the same
//! response reports no `Code`. The two members agree in every response: either both
//! describe that version's artifacts, or both are absent.
//!
//! Each row below is one version HISTORY, and the rows differ along what the rule
//! generalises over: whether the call that minted a version supplied a `Code`, which form
//! the supplied union took, how many versions the history holds, and whether a later
//! version adds or drops the location an earlier one froze. Every version of every row is
//! read back by name as well as through the default read, so an immutable version keeps
//! answering for its own artifacts.

use crate::workflow_lifecycle::{role_arn, Harness};

/// The `Code` forms a request can supply, from
/// `smithy_query(query="shape", shape="Code,S3Location")`: the union's single
/// `S3Location` member, whose `VersionId` is optional. Index 0 is "no `Code` member at
/// all", the absence the rule is defined against.
fn code_forms() -> Vec<(&'static str, Option<serde_json::Value>)> {
    vec![
        ("no Code supplied", None),
        (
            "an S3 location with no VersionId",
            Some(serde_json::json!({
                "S3Location": {"Bucket": "code", "ObjectKey": "src.zip"}
            })),
        ),
        (
            "an S3 location carrying a VersionId",
            Some(serde_json::json!({
                "S3Location": {
                    "Bucket": "artifacts-bucket",
                    "ObjectKey": "builds/deep/path/src-2.zip",
                    "VersionId": "s3objectversion2",
                }
            })),
        ),
    ]
}

/// One row: the `code_forms` index each call of the history supplies, the first being the
/// `CreateWorkflow` and the rest `UpdateWorkflow` calls.
struct Row {
    case: &'static str,
    steps: &'static [usize],
}

const ROWS: &[Row] = &[
    Row {
        case: "a create that supplied no code",
        steps: &[0],
    },
    Row {
        case: "a create that supplied a code location",
        steps: &[1],
    },
    Row {
        case: "a create with no code, then an update that adds one",
        steps: &[0, 1],
    },
    Row {
        case: "a create with code, then an update that drops it",
        steps: &[1, 0],
    },
    Row {
        case: "a create with code, then an update that replaces it with another form",
        steps: &[1, 2],
    },
    Row {
        case: "three versions, only the middle one carrying code",
        steps: &[0, 2, 0],
    },
    Row {
        case: "four versions alternating between carrying code and carrying none",
        steps: &[1, 0, 2, 0],
    },
];

/// What one call of a history minted: the version it reported, the `Code` member it
/// supplied (`Null` for a call that supplied none) and the instant the call itself
/// reported for its own write.
struct Minted {
    version: String,
    code: serde_json::Value,
    instant: serde_json::Value,
}

/// Assert the two members agree in `response`, whatever version it resolved: a copy
/// instant is reported exactly when the artifacts it describes are.
fn assert_members_agree(case: &str, what: &str, response: &serde_json::Value) {
    assert_eq!(
        response["CodeSnapshottedAt"].is_null(),
        response["Code"].is_null(),
        "case {case:?}, {what}: Code and CodeSnapshottedAt must either both describe this \
         version's artifacts or both be absent, got Code {:?} and CodeSnapshottedAt {:?}",
        response["Code"],
        response["CodeSnapshottedAt"]
    );
    if !response["CodeSnapshottedAt"].is_null() {
        assert!(
            response["CodeSnapshottedAt"].is_number(),
            "case {case:?}, {what}: expected a timestamp on the wire, got {:?}",
            response["CodeSnapshottedAt"]
        );
    }
}

/// Drive one row's history: the create, then one update per further step, answering the
/// workflow's ARN and what each call minted.
async fn drive_history(harness: &Harness, row: &Row) -> (String, Vec<Minted>) {
    let forms = code_forms();
    let case = row.case;
    let mut minted: Vec<Minted> = Vec::new();
    let mut arn = String::new();

    for (step, index) in row.steps.iter().copied().enumerate() {
        let (form, code) = &forms[index];
        let mut body = if step == 0 {
            serde_json::json!({
                "Name": "code.snapshot_instant-1",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v1.yaml"},
                "RoleArn": role_arn(),
            })
        } else {
            serde_json::json!({
                "WorkflowArn": arn,
                "DefinitionS3Location": {
                    "Bucket": "defs",
                    "ObjectKey": format!("dags/v{}.yaml", step + 1),
                },
                "RoleArn": role_arn(),
            })
        };
        if let Some(code) = code {
            body["Code"] = code.clone();
        }
        let operation = if step == 0 {
            "CreateWorkflow"
        } else {
            "UpdateWorkflow"
        };
        let (status, answered) = harness.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} supplying {form} to succeed, got {status} \
             {answered:?}"
        );
        if step == 0 {
            arn = answered["WorkflowArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
        // The create reports the instant it wrote as `CreatedAt` and an update reports
        // its own as `ModifiedAt`: either way, the instant of the request that supplied
        // the location this version froze.
        let instant = if step == 0 {
            answered["CreatedAt"].clone()
        } else {
            answered["ModifiedAt"].clone()
        };
        minted.push(Minted {
            version: answered["WorkflowVersion"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            code: code.clone().unwrap_or(serde_json::Value::Null),
            instant,
        });
    }

    (arn, minted)
}

#[tokio::test]
async fn the_copy_instant_is_reported_only_for_a_version_that_froze_a_code_location() {
    for row in ROWS {
        // A fresh emulator per row, so one row's versions cannot answer another's read.
        let harness = Harness::new("us-west-2");
        let case = row.case;
        let (arn, minted) = drive_history(&harness, row).await;

        // Every version, read by its own name, answers for the artifacts IT froze.
        for (step, record) in minted.iter().enumerate() {
            let what = format!("version {} of {}", step + 1, minted.len());
            let (status, fetched) = harness
                .call(
                    "GetWorkflow",
                    serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": record.version}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}, {what}: expected GetWorkflow to succeed, got {status} {fetched:?}"
            );
            assert_eq!(
                fetched["Code"], record.code,
                "case {case:?}, {what}: expected the Code this version froze, got {:?}",
                fetched["Code"]
            );
            if record.code.is_null() {
                assert!(
                    fetched["CodeSnapshottedAt"].is_null(),
                    "case {case:?}, {what}: nothing was copied for this version, so no copy \
                     instant can be reported, got {:?}",
                    fetched["CodeSnapshottedAt"]
                );
            } else {
                assert_eq!(
                    fetched["CodeSnapshottedAt"], record.instant,
                    "case {case:?}, {what}: expected the instant of the request that supplied \
                     the location ({:?}), got {:?}",
                    record.instant, fetched["CodeSnapshottedAt"]
                );
            }
            assert_members_agree(case, &what, &fetched);
        }

        // The default read resolves the newest version, so it answers for THAT version's
        // artifacts — a workflow whose latest version froze no location reports no copy
        // instant even when an earlier version froze one.
        let latest = minted.last().expect("every row mints at least one version");
        let (status, fetched) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the default GetWorkflow to succeed, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["WorkflowVersion"].as_str(),
            Some(latest.version.as_str()),
            "case {case:?}: expected the default read to resolve the newest version, got {:?}",
            fetched["WorkflowVersion"]
        );
        assert_eq!(
            fetched["Code"], latest.code,
            "case {case:?}: expected the Code the newest version froze, got {:?}",
            fetched["Code"]
        );
        let expected_instant = if latest.code.is_null() {
            serde_json::Value::Null
        } else {
            latest.instant.clone()
        };
        assert_eq!(
            fetched["CodeSnapshottedAt"], expected_instant,
            "case {case:?}: expected the newest version's own copy instant ({expected_instant:?}), \
             got {:?}",
            fetched["CodeSnapshottedAt"]
        );
        assert_members_agree(case, "the default read", &fetched);
    }
}
