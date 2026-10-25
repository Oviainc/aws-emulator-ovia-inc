// SPDX-License-Identifier: CC0-1.0

//! What `DeleteReportGroup` removes, driven over the wire the router serves: `POST /`
//! with `X-Amz-Target: CodeBuild_20161006.DeleteReportGroup` on awsJson1_1.
//!
//! The rule under test: an ARN addresses exactly the report group whose STORED ARN it
//! is, the call drops that record (and the resource-based policy attached to it) and
//! nothing else, and because the operation models no not-found error a well-formed
//! report-group ARN this account does not hold is a success that changes nothing — while
//! a value that is not a report-group ARN of this service is refused with the
//! operation's modelled `InvalidInputException` (HTTP 400).
//!
//! Every deletion is read back through `BatchGetReportGroups`, so what is asserted is
//! the store rather than the deleting call's own answer.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState, StoredResourcePolicy,
};
use smithy_server_runtime::MultiServiceRouter;

// The harness every suite in this test crate drives the service through: one router over
// an engine this test holds, and one awsJson1_1 call signed for [`REGION`].
use super::project_build_lifecycle::{call, emulator, emulator_on, error_type, REGION};
// The same `CreateReportGroup` body builder the write-end suite uses, so a group under
// test here is seeded exactly as a client would seed it.
use super::report_group_lifecycle::create_report_group_body;

/// The ARN this emulator mints for a report group of `name` in [`REGION`] — through the
/// service's own minter, so the test cannot drift from the ARN form the handler stores.
fn report_group_arn(name: &str) -> String {
    state::report_group_arn(REGION, name)
}

/// Create one report group, asserting the call succeeded, and hand back its ARN.
async fn create_report_group(
    router: &MultiServiceRouter,
    name: &str,
    report_type: &str,
    export_config: Value,
    tags: Option<Value>,
) -> String {
    let (status, body) = call(
        router,
        "CreateReportGroup",
        create_report_group_body(name, report_type, export_config, tags),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateReportGroup {name:?}: expected 200, got {status} with {body}"
    );
    body["reportGroup"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateReportGroup {name:?}: no arn in {body}"))
        .to_string()
}

/// Delete the report group `arn` names, with `delete_reports` supplied only when the
/// case asks for it, and hand back the HTTP status and body.
async fn delete_report_group(
    router: &MultiServiceRouter,
    arn: &str,
    delete_reports: Option<bool>,
) -> (u16, Value) {
    let mut body = json!({ "arn": arn });
    if let (Value::Object(target), Some(delete_reports)) = (&mut body, delete_reports) {
        target.insert("deleteReports".to_string(), json!(delete_reports));
    }
    call(router, "DeleteReportGroup", body).await
}

/// The ARNs `BatchGetReportGroups` reports as HELD, for the ARNs asked about.
async fn held_arns(router: &MultiServiceRouter, arns: &[String]) -> Vec<String> {
    let (status, body) = call(
        router,
        "BatchGetReportGroups",
        json!({ "reportGroupArns": arns }),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetReportGroups {arns:?}: expected 200, got {status} with {body}"
    );
    body["reportGroups"]
        .as_array()
        .unwrap_or_else(|| panic!("BatchGetReportGroups {arns:?}: no reportGroups in {body}"))
        .iter()
        .map(|group| group["arn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// An `S3` export config, which is what `exportConfigType` `S3` requires a bucket for.
fn s3_export(packaging: &str) -> Value {
    json!({
        "exportConfigType": "S3",
        "s3Destination": {"bucket": "reports-bucket", "packaging": packaging},
    })
}

/// One row of the deletion table.
#[derive(Debug)]
struct DeleteCase {
    /// The report group created and then deleted.
    name: &'static str,
    /// Its `type`. The catalog is `smithy_query(shape="ReportType")`: TEST |
    /// CODE_COVERAGE.
    report_type: &'static str,
    /// Whether the request carries `deleteReports`, and with which value. `None` leaves
    /// the member out, so the model's `false` default applies.
    delete_reports: Option<bool>,
    /// How many tags the group was created with.
    tags: usize,
}

/// RULE: `DeleteReportGroup` drops the one record its ARN names, whatever that group
/// holds and whichever way `deleteReports` is given, and leaves every other group where
/// it was; a repeated delete of the same ARN is a success that changes nothing, because
/// the operation models no not-found error.
///
/// The table spans the classes the rule generalises over: both `ReportType` values, each
/// `ReportExportConfigType` (S3 | NO_EXPORT) with each `ReportPackagingType` (ZIP |
/// NONE), tag lists of 0, 1 and the 50 entries `TagList`'s `@length(min: 0, max: 50)`
/// allows, names at both `ReportGroupName` bounds (`@length(min: 2, max: 128)`), and
/// `deleteReports` absent, `false` and `true`.
#[tokio::test]
async fn delete_report_group_drops_the_group_its_arn_names_and_nothing_else() {
    let long_name: &'static str = Box::leak("n".repeat(128).into_boxed_str());
    let cases = [
        DeleteCase {
            name: "rg-test-s3-zip",
            report_type: "TEST",
            delete_reports: None,
            tags: 0,
        },
        DeleteCase {
            name: "rg-coverage-no-export",
            report_type: "CODE_COVERAGE",
            delete_reports: Some(false),
            tags: 1,
        },
        DeleteCase {
            name: "rg-test-s3-none",
            report_type: "TEST",
            delete_reports: Some(true),
            tags: 50,
        },
        DeleteCase {
            name: "rg",
            report_type: "CODE_COVERAGE",
            delete_reports: Some(true),
            tags: 0,
        },
        DeleteCase {
            name: long_name,
            report_type: "TEST",
            delete_reports: Some(false),
            tags: 1,
        },
    ];

    for case in &cases {
        let router = emulator();
        let export_config = match (case.report_type, case.tags) {
            ("CODE_COVERAGE", _) => json!({"exportConfigType": "NO_EXPORT"}),
            (_, 50) => s3_export("NONE"),
            _ => s3_export("ZIP"),
        };
        let tags = Value::Array(
            (0..case.tags)
                .map(|index| json!({"key": format!("k{index}"), "value": format!("v{index}")}))
                .collect(),
        );
        let target = create_report_group(
            &router,
            case.name,
            case.report_type,
            export_config,
            Some(tags),
        )
        .await;
        // A second group, untouched by the deletion, so the assertion below is about
        // the ONE record the ARN names rather than about the store being emptied.
        let sibling = create_report_group(
            &router,
            "rg-sibling",
            "TEST",
            json!({"exportConfigType": "NO_EXPORT"}),
            None,
        )
        .await;
        let both = [target.clone(), sibling.clone()];
        assert_eq!(
            held_arns(&router, &both).await,
            both.to_vec(),
            "case {case:?}: both groups must be held before the delete"
        );

        let (status, body) = delete_report_group(&router, &target, case.delete_reports).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DeleteReportGroup, got {status} with {body}"
        );
        assert!(
            body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty),
            "case {case:?}: DeleteReportGroup models no output member, got {body}"
        );

        let held = held_arns(&router, &both).await;
        assert_eq!(
            held,
            vec![sibling.clone()],
            "case {case:?}: only the sibling may survive the delete, got {held:?}"
        );

        // Deleting it again is a success and changes nothing: the operation models no
        // not-found error.
        let (status, body) = delete_report_group(&router, &target, case.delete_reports).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated delete must answer 200, got {status} with {body}"
        );
        let held = held_arns(&router, &both).await;
        assert_eq!(
            held,
            vec![sibling],
            "case {case:?}: a repeated delete must leave the store as it was, got {held:?}"
        );
    }
}

/// RULE: a well-formed report-group ARN of a scope this account does not hold addresses
/// no local record, so the call succeeds and the store is untouched — an ARN resolves
/// only when it is EXACTLY a stored record's ARN.
///
/// The table spans the fields an ARN differs in: its partition, region, account,
/// resource name (including its case) and a name held nowhere.
#[tokio::test]
async fn a_report_group_arn_this_account_does_not_hold_deletes_nothing() {
    let router = emulator();
    let held = create_report_group(
        &router,
        "rg-held",
        "TEST",
        json!({"exportConfigType": "NO_EXPORT"}),
        None,
    )
    .await;

    let elsewhere = [
        // Another partition.
        held.replacen("arn:aws:", "arn:aws-us-gov:", 1),
        // Another region.
        state::report_group_arn("eu-central-1", "rg-held"),
        // Another account.
        format!("arn:aws:codebuild:{REGION}:111122223333:report-group/rg-held"),
        // The name in another case — a store keyed by name is case-sensitive.
        report_group_arn("RG-HELD"),
        // A name no group of this account was ever created under.
        report_group_arn("rg-never-created"),
    ];

    for case in &elsewhere {
        let (status, body) = delete_report_group(&router, case, Some(true)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a report-group ARN naming no held record must answer 200, \
             got {status} with {body}"
        );
        let still_held = held_arns(&router, std::slice::from_ref(&held)).await;
        assert_eq!(
            still_held,
            vec![held.clone()],
            "case {case:?}: the held group must survive, got {still_held:?}"
        );
    }
}

/// RULE: `arn` is typed `NonEmptyString`, so a value that is not a report-group ARN of
/// this service reaches the handler and is refused with the operation's only modelled
/// error, HTTP 400 `InvalidInputException`, persisting nothing.
///
/// The table spans the ways a value fails to be one: not an ARN at all, too few ARN
/// fields, another service's ARN, another resource type of this service, and a
/// report-group segment carrying no name.
#[tokio::test]
async fn an_arn_that_is_not_a_report_group_arn_is_refused() {
    let router = emulator();
    let held = create_report_group(
        &router,
        "rg-held",
        "CODE_COVERAGE",
        json!({"exportConfigType": "NO_EXPORT"}),
        None,
    )
    .await;

    let account = engine::EMULATOR_ACCOUNT_ID;
    let cases = [
        // A bare report group name, which is not an ARN.
        "rg-held".to_string(),
        // The resource segment alone.
        "report-group/rg-held".to_string(),
        // Too few ARN fields.
        format!("arn:aws:codebuild:{REGION}:{account}"),
        // Another service's ARN.
        "arn:aws:s3:::reports-bucket/rg-held".to_string(),
        // Another resource type of this service.
        format!("arn:aws:codebuild:{REGION}:{account}:project/rg-held"),
        // A report-group ARN carrying no name.
        format!("arn:aws:codebuild:{REGION}:{account}:report-group/"),
        format!("arn:aws:codebuild:{REGION}:{account}:report-group"),
    ];

    for case in &cases {
        let (status, body) = delete_report_group(&router, case, Some(true)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {case:?}: CodeBuild states a bad input as InvalidInputException, got {body}"
        );
        let still_held = held_arns(&router, std::slice::from_ref(&held)).await;
        assert_eq!(
            still_held,
            vec![held.clone()],
            "case {case:?}: a refused delete must persist nothing, got {still_held:?}"
        );
    }
}

/// RULE: the resource-based policy attached to a report group cannot outlive the group,
/// so deleting the group drops the policy stored against its ARN — and only that one.
#[tokio::test]
async fn deleting_a_report_group_drops_the_resource_policy_attached_to_it() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let deleted = create_report_group(
        &router,
        "rg-shared",
        "TEST",
        json!({"exportConfigType": "NO_EXPORT"}),
        None,
    )
    .await;
    let kept = create_report_group(
        &router,
        "rg-shared-sibling",
        "TEST",
        json!({"exportConfigType": "NO_EXPORT"}),
        None,
    )
    .await;

    // Seeded straight into the store: `PutResourcePolicy`, the operation that writes
    // one, is still scaffolded.
    let handle = engine.state::<CodeBuildState>();
    for arn in [&deleted, &kept] {
        handle.lock().resource_policies.insert(
            arn.clone(),
            StoredResourcePolicy {
                policy: json!({"Version": "2012-10-17", "Statement": []}).to_string(),
                last_modified_millis: 1_700_000_000_000,
            },
        );
    }

    let (status, body) = delete_report_group(&router, &deleted, None).await;
    assert_eq!(
        status, 200,
        "DeleteReportGroup {deleted:?}: expected 200, got {status} with {body}"
    );

    let attached: Vec<String> = handle.lock().resource_policies.keys().cloned().collect();
    assert_eq!(
        attached,
        vec![kept.clone()],
        "the deleted group's policy must go with it and the sibling's must stay, \
         got {attached:?}"
    );
}
