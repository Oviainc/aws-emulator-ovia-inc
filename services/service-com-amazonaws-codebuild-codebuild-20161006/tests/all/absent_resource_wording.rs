// SPDX-License-Identifier: CC0-1.0

//! Cover for the sentence each CodeBuild refusal states, driven over the wire the router
//! serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! An error's status, its `__type` and its `message` are all part of the wire contract, so
//! every refusal below is checked on the sentence CodeBuild states for THAT operation's
//! error class: its fixed text verbatim, and as variable parts only what the service
//! itself interpolates — an ARN computed from the request and this account, or an
//! identifier echoed exactly as sent. Where a family of operations shares one sentence,
//! the table drives every member of the family rather than the one operation an issue
//! happened to name.
//!
//! Each suite in `tests/all/` drives the wire itself, so the tables below reach for the
//! shared harness in `tests/all/project_build_lifecycle.rs` and nothing else.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::REPORT_GROUP_TREND_FIELD_TYPE_VALUES;
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{
    call, create_project, emulator, error_type, expected_arn, REGION,
};
use super::report_group_lifecycle::create_report_group_body;

/// Call `operation`, assert it was refused with HTTP 400 and `code`, and hand back the
/// sentence it stated.
async fn refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
    code: &str,
) -> String {
    let (status, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 from {operation}, got {status} with {answer}"
    );
    assert_eq!(
        error_type(&answer),
        code,
        "case {case:?}: expected {code} from {operation}, got {answer}"
    );
    answer["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {operation} must state a message, got {answer}"))
        .to_string()
}

/// Project names a request can address: the shortest the model's `ProjectName` pattern
/// allows, one of each permitted punctuation, a long one, and one differing from a held
/// project only in case — every form of "a name this account holds nothing under".
///
/// From `smithy_query(query="shape",shape="ProjectName")`:
/// `^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$`.
const ABSENT_PROJECTS: [&str; 5] = [
    "ab",
    "absent-project",
    "absent_project",
    "a0123456789012345678901234567890123456789012345678901234567890123456789",
    "HELD-PROJECT",
];

/// RULE: a listing of ONE project's records refuses a project this account does not hold
/// with 400 `ResourceNotFoundException`, stating `The provided project <ARN> does not
/// exist.` — fixed text whose only variable part is the ARN the requested name resolves to
/// in the region the call is signed for, with the trailing period.
///
/// The table is the whole family of per-project listings crossed with every form of an
/// absent project name, so it covers the operations the issue did not cite as well as the
/// ones it did; the controls at the end prove each refusal is about the name asked for and
/// not about the account holding nothing.
#[tokio::test]
async fn a_per_project_listing_quotes_the_absent_project_by_arn() {
    let router = emulator();
    create_project(&router, "held-project", json!({})).await;

    // Every operation whose input names ONE project and lists that project's records.
    let operations = [
        "ListBuildsForProject",
        "ListBuildBatchesForProject",
        "ListSandboxesForProject",
    ];

    for operation in operations {
        for project in ABSENT_PROJECTS {
            let case = format!("{operation} for {project:?}");
            let stated = refusal(
                &router,
                &case,
                operation,
                json!({"projectName": project}),
                "ResourceNotFoundException",
            )
            .await;
            let expected = format!(
                "The provided project {} does not exist.",
                expected_arn(&format!("project/{project}"))
            );
            assert_eq!(
                stated, expected,
                "case {case:?}: expected CodeBuild's provided-project sentence, got {stated:?}"
            );
        }

        // The control: the project this account DOES hold lists rather than refusing, so
        // every row above was decided by the name and not by the operation.
        let (status, answer) = call(&router, operation, json!({"projectName": "held-project"})).await;
        assert_eq!(
            status, 200,
            "{operation} for a held project: expected 200, got {status} with {answer}"
        );
    }
}

/// RULE: an operation that MANAGES a project refuses one this account does not hold with
/// 400 `ResourceNotFoundException` stating `Project does not exist` — wholly fixed text
/// that interpolates neither the name nor an ARN, so a client comparing messages across
/// calls sees the same sentence for every absent project.
///
/// The table spans the project-management family — the update of a project's configuration
/// and the three webhook operations that resolve a project — crossed with every form of an
/// absent name.
#[tokio::test]
async fn project_management_states_codebuilds_fixed_absent_project_sentence() {
    let router = emulator();

    for project in ABSENT_PROJECTS {
        // (operation, the request that addresses the project)
        let requests: [(&str, Value); 4] = [
            ("UpdateProject", json!({"name": project})),
            ("CreateWebhook", json!({"projectName": project})),
            ("UpdateWebhook", json!({"projectName": project})),
            ("DeleteWebhook", json!({"projectName": project})),
        ];
        for (operation, body) in requests {
            let case = format!("{operation} for {project:?}");
            let stated = refusal(
                &router,
                &case,
                operation,
                body,
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                stated,
                state::PROJECT_NOT_FOUND_MESSAGE,
                "case {case:?}: expected CodeBuild's fixed absent-project sentence, got \
                 {stated:?}"
            );
            assert!(
                !stated.contains("arn:") && !stated.contains(project),
                "case {case:?}: the sentence must name neither the project nor its ARN, got \
                 {stated:?}"
            );
        }
    }

    // The control: a project this account holds is updated rather than refused.
    create_project(&router, "held-project", json!({})).await;
    let (status, answer) = call(
        &router,
        "UpdateProject",
        json!({"name": "held-project", "description": "changed"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateProject on a held project: expected 200, got {status} with {answer}"
    );
}

/// RULE: a batch-build stop refuses an id this account holds no batch build under with 400
/// `ResourceNotFoundException` stating `BuildBatch <id> does not exist` — the resource
/// named by its MODELLED shape name, the id echoed exactly as the request sent it, and no
/// trailing period.
///
/// The rows span the forms such an id takes: the documented `<project>:<uuid>` shape, one
/// of a project that was never created, a bare project name, a batch build's ARN form, and
/// a held batch build's id with a suffix.
#[tokio::test]
async fn an_absent_batch_build_is_named_by_its_modelled_shape_name() {
    let router = emulator();
    create_project(&router, "batch-proj", json!({})).await;
    let (status, answer) = call(&router, "StartBuildBatch", json!({"projectName": "batch-proj"})).await;
    assert_eq!(
        status, 200,
        "StartBuildBatch: expected 200, got {status} with {answer}"
    );
    let held = answer["buildBatch"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("StartBuildBatch must report an id, got {answer}"))
        .to_string();

    let cases: [(&str, String); 5] = [
        (
            "the documented shape, never minted",
            "batch-proj:00000000-0000-8000-8000-000000000000".to_string(),
        ),
        (
            "a project that was never created",
            "no-such-proj:3f3ec2b3-57b1-4a16-9a6a-4a4d2b9c9a11".to_string(),
        ),
        ("a bare project name", "batch-proj".to_string()),
        (
            "a batch build's ARN form",
            expected_arn(&format!("build/{held}")),
        ),
        ("a held id with a suffix", format!("{held}-not-this-one")),
    ];

    for (case, id) in &cases {
        let stated = refusal(
            &router,
            case,
            "StopBuildBatch",
            json!({"id": id}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            stated,
            format!("BuildBatch {id} does not exist"),
            "case {case:?}: expected CodeBuild's absent-batch sentence, got {stated:?}"
        );
        assert!(
            !stated.ends_with('.'),
            "case {case:?}: the sentence carries no trailing period, got {stated:?}"
        );
    }

    // The control: the batch build this account holds stops rather than refusing.
    let (status, answer) = call(&router, "StopBuildBatch", json!({"id": held})).await;
    assert_eq!(
        status, 200,
        "StopBuildBatch on a held batch build: expected 200, got {status} with {answer}"
    );
}

/// RULE: a reporting read of a report group's trend refuses an ARN this account holds no
/// group under with 400 `ResourceNotFoundException` stating `The provided report group
/// <ARN> does not exist` — fixed text whose only variable part is the ARN the request sent,
/// with no trailing period — while the report-group MANAGEMENT operations state the wholly
/// fixed `Report group does not exist` for the same absence.
///
/// The rows span the ways a well-formed group ARN can address no held group (never created,
/// another partition, region or account, a different case, and a group that was deleted),
/// each read for every value of `ReportGroupTrendFieldType`
/// (`crate::ops::types::REPORT_GROUP_TREND_FIELD_TYPE_VALUES`), so the sentence is proved
/// to belong to the absent group rather than to the field asked for.
#[tokio::test]
async fn an_absent_report_group_trend_quotes_the_arn_asked_about() {
    let router = emulator();
    let account = engine::EMULATOR_ACCOUNT_ID;
    for name in ["held-group", "deleted-group"] {
        let (status, answer) = call(
            &router,
            "CreateReportGroup",
            create_report_group_body(name, "TEST", json!({"exportConfigType": "NO_EXPORT"}), None),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateReportGroup {name:?}: expected 200, got {status} with {answer}"
        );
    }
    let (status, answer) = call(
        &router,
        "DeleteReportGroup",
        json!({"arn": state::report_group_arn(REGION, "deleted-group")}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteReportGroup: expected 200, got {status} with {answer}"
    );

    let absent: [(&str, String); 6] = [
        (
            "a group never created",
            state::report_group_arn(REGION, "no-such-group"),
        ),
        (
            "another partition",
            format!("arn:aws-cn:codebuild:{REGION}:{account}:report-group/held-group"),
        ),
        (
            "another region",
            state::report_group_arn("eu-west-1", "held-group"),
        ),
        (
            "another account",
            format!("arn:aws:codebuild:{REGION}:111122223333:report-group/held-group"),
        ),
        (
            "the held group in another case",
            state::report_group_arn(REGION, "HELD-GROUP"),
        ),
        (
            "a group that was deleted",
            state::report_group_arn(REGION, "deleted-group"),
        ),
    ];

    for (case, arn) in &absent {
        for field in REPORT_GROUP_TREND_FIELD_TYPE_VALUES {
            let case = format!("{case}, trendField {field}");
            let stated = refusal(
                &router,
                &case,
                "GetReportGroupTrend",
                json!({"reportGroupArn": arn, "trendField": field}),
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                stated,
                format!("The provided report group {arn} does not exist"),
                "case {case:?}: expected CodeBuild's provided-report-group sentence, got \
                 {stated:?}"
            );
        }

        // The contrast: MANAGING the same absent group states the fixed sentence, with no
        // ARN in it — the two wordings are per-operation, not one shared template.
        let stated = refusal(
            &router,
            case,
            "UpdateReportGroup",
            json!({"arn": arn}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            stated,
            state::REPORT_GROUP_NOT_FOUND_MESSAGE,
            "case {case:?}: a management refusal states the fixed sentence, got {stated:?}"
        );
    }

    // The control: the group this account holds reports a trend.
    let (status, answer) = call(
        &router,
        "GetReportGroupTrend",
        json!({
            "reportGroupArn": state::report_group_arn(REGION, "held-group"),
            "trendField": "TOTAL",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetReportGroupTrend on a held group: expected 200, got {status} with {answer}"
    );
}

/// RULE: a resource-policy write refuses an ARN of the right shape that this account holds
/// no resource for with 400 `ResourceNotFoundException` stating `Resource ARN does not
/// exist` — the ARN MEMBER at fault named, and never one of the two resource kinds the
/// value could have denoted, because the operation stores a policy "for the ARN of a
/// Project or ReportGroup object" and so cannot tell the caller which kind is missing.
///
/// The rows cross both resource kinds a policy attaches to with every way an ARN of the
/// right shape can still name nothing here.
#[tokio::test]
async fn an_absent_policy_target_is_refused_on_the_arn_member() {
    let router = emulator();
    create_project(&router, "held-project", json!({})).await;
    let (status, answer) = call(
        &router,
        "CreateReportGroup",
        create_report_group_body(
            "held-group",
            "TEST",
            json!({"exportConfigType": "NO_EXPORT"}),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateReportGroup: expected 200, got {status} with {answer}"
    );
    let account = engine::EMULATOR_ACCOUNT_ID;

    let cases: [(&str, String); 6] = [
        (
            "a project never created",
            state::project_arn(REGION, "no-such-project"),
        ),
        (
            "a held project in another region",
            state::project_arn("eu-west-1", "held-project"),
        ),
        (
            "a held project in another account",
            format!("arn:aws:codebuild:{REGION}:111122223333:project/held-project"),
        ),
        (
            "a report group never created",
            state::report_group_arn(REGION, "no-such-group"),
        ),
        (
            "a held report group in another partition",
            format!("arn:aws-us-gov:codebuild:{REGION}:{account}:report-group/held-group"),
        ),
        (
            "a held report group in another case",
            state::report_group_arn(REGION, "HELD-GROUP"),
        ),
    ];

    for (case, arn) in &cases {
        // A document granting only the read-only consumer actions of the kind the ARN
        // names, so the row is decided by the absent resource and not by the policy's
        // contents.
        let action = if arn.contains(":project/") {
            "codebuild:BatchGetProjects"
        } else {
            "codebuild:BatchGetReportGroups"
        };
        let policy = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
                "Action": [action],
                "Resource": arn,
            }],
        })
        .to_string();
        let stated = refusal(
            &router,
            case,
            "PutResourcePolicy",
            json!({"resourceArn": arn, "policy": policy}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            stated,
            state::RESOURCE_ARN_NOT_FOUND_MESSAGE,
            "case {case:?}: expected CodeBuild's absent-resource-ARN sentence, got {stated:?}"
        );
        for kind in ["Project", "Report group", "report-group", "arn:"] {
            assert!(
                !stated.contains(kind),
                "case {case:?}: the sentence must not name {kind:?}, got {stated:?}"
            );
        }
    }
}

/// RULE: opening a connection into a sandbox classifies a `sandboxId` it cannot resolve as
/// BAD INPUT — 400 `InvalidInputException` stating `No sandbox found with specified ID` —
/// where the operations that act on the sandbox itself report the same absence as a missing
/// resource, 400 `ResourceNotFoundException` stating `Sandbox does not exist`. The error
/// CODE is what an SDK branches on, so each operation carries the class CodeBuild assigns
/// it.
///
/// The rows span the forms an unresolvable identifier takes: the documented
/// `<project>:<uuid>` shape, one of a project that was never created, a bare project name,
/// and a sandbox ARN — `sandboxId` is documented as "A sandboxId or sandboxArn", so both
/// forms are driven.
#[tokio::test]
async fn an_unresolvable_sandbox_id_is_bad_input_to_a_connection() {
    let router = emulator();
    create_project(&router, "sandbox-proj", json!({})).await;

    let absent = [
        (
            "the documented shape, never minted",
            "sandbox-proj:00000000-0000-8000-8000-000000000000".to_string(),
        ),
        (
            "a project that was never created",
            "no-such-proj:3f3ec2b3-57b1-4a16-9a6a-4a4d2b9c9a11".to_string(),
        ),
        ("a bare project name", "sandbox-proj".to_string()),
        (
            "an ARN of a sandbox never minted",
            state::sandbox_arn(REGION, "sandbox-proj:never-minted"),
        ),
    ];

    for (case, id) in &absent {
        let case_connection = format!("StartSandboxConnection for {case}");
        let stated = refusal(
            &router,
            &case_connection,
            "StartSandboxConnection",
            json!({"sandboxId": id}),
            "InvalidInputException",
        )
        .await;
        assert_eq!(
            stated,
            state::UNRESOLVABLE_SANDBOX_ID_MESSAGE,
            "case {case_connection:?}: expected CodeBuild's unresolvable-id sentence, got \
             {stated:?}"
        );
        assert!(
            !stated.contains("arn:") && !stated.contains(id.as_str()),
            "case {case_connection:?}: the sentence must name no identifier, got {stated:?}"
        );

        // The contrast, on the SAME identifier: acting on the sandbox reports the absence
        // as a missing resource.
        for operation in ["StopSandbox", "StartCommandExecution"] {
            let case_act = format!("{operation} for {case}");
            let body = if operation == "StopSandbox" {
                json!({"id": id})
            } else {
                json!({"sandboxId": id, "command": "echo hi"})
            };
            let stated = refusal(
                &router,
                &case_act,
                operation,
                body,
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                stated,
                state::SANDBOX_NOT_FOUND_MESSAGE,
                "case {case_act:?}: expected CodeBuild's absent-sandbox sentence, got {stated:?}"
            );
        }
    }
}

/// RULE: every paginated listing of this service refuses a continuation token that opens no
/// page of the listing asked for with 400 `InvalidInputException` stating `Invalid
/// continuation token specified.` — one sentence, the same on every operation, that never
/// quotes the token back (on `ListFleets` the member is even `@sensitive`).
///
/// The table is every operation of the service whose input models a `nextToken`, crossed
/// with the forms a token can fail in: text that was never a token, the empty-looking
/// shapes, one that names an entry of a DIFFERENT listing, and one that is a held entry's
/// id with a suffix.
#[tokio::test]
async fn every_paginated_listing_refuses_a_continuation_token_in_one_sentence() {
    let router = emulator();
    create_project(&router, "paged-proj", json!({})).await;

    // Every operation whose input models a `nextToken`, with the other members each one
    // needs. The per-project and per-sandbox listings name records this account holds, so
    // each row is decided by the token and not by an absent parent.
    let operations: [(&str, Value); 12] = [
        ("ListBuilds", json!({})),
        ("ListBuildBatches", json!({})),
        ("ListProjects", json!({})),
        ("ListReportGroups", json!({})),
        ("ListSharedProjects", json!({})),
        ("ListSharedReportGroups", json!({})),
        ("ListSandboxes", json!({})),
        ("ListFleets", json!({})),
        ("ListBuildsForProject", json!({"projectName": "paged-proj"})),
        (
            "ListBuildBatchesForProject",
            json!({"projectName": "paged-proj"}),
        ),
        (
            "ListSandboxesForProject",
            json!({"projectName": "paged-proj"}),
        ),
        (
            "ListCommandExecutionsForSandbox",
            json!({"sandboxId": "paged-proj:00000000-0000-8000-8000-000000000000"}),
        ),
    ];

    let tokens = [
        "not-a-token",
        " ",
        "0",
        "paged-proj:00000000-0000-8000-8000-000000000000",
        "arn:aws:codebuild:eu-west-1:111122223333:project/paged-proj",
    ];

    for (operation, base) in &operations {
        for token in tokens {
            let mut body = base.clone();
            if let Value::Object(members) = &mut body {
                members.insert("nextToken".to_string(), json!(token));
            }
            let case = format!("{operation} with nextToken {token:?}");
            let stated = refusal(
                &router,
                &case,
                operation,
                body,
                "InvalidInputException",
            )
            .await;
            assert_eq!(
                stated,
                state::INVALID_CONTINUATION_TOKEN_MESSAGE,
                "case {case:?}: expected CodeBuild's continuation-token sentence, got {stated:?}"
            );
            assert!(
                !stated.contains(token.trim()) || token.trim().is_empty(),
                "case {case:?}: the token must never be quoted back, got {stated:?}"
            );
        }

        // The control: the same request without a token lists rather than refusing, so
        // every row above was decided by the token.
        let (status, answer) = call(&router, operation, base.clone()).await;
        assert_eq!(
            status, 200,
            "{operation} with no nextToken: expected 200, got {status} with {answer}"
        );
    }
}
