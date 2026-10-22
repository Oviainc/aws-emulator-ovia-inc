// SPDX-License-Identifier: CC0-1.0

//! What `BatchGetReportGroups` answers for the ARNs a caller asks about, driven over
//! the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.BatchGetReportGroups` on awsJson1_1.
//!
//! `tests/all/report_group_lifecycle.rs` beside this one drives the slice from the
//! WRITE end — what `CreateReportGroup` persists is what a read reports — and already
//! covers the held/not-found split in request order. This suite adds what that one
//! does not reach:
//!
//! * the reader reports whatever the store holds, whoever wrote it — a record seeded
//!   straight into the service's state is handed back verbatim, so the answer is not
//!   recomputed from the request;
//! * an ARN resolves only when it is EXACTLY a stored record's ARN, so a value
//!   differing in its partition, resource segment or the case of the name addresses
//!   nothing and is reported in `reportGroupsNotFound`;
//! * the answer stays positional for a REPEATED ARN, and a read leaves what a later
//!   caller sees unchanged.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::ReportGroup;
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState, StoredReportGroup,
};
use smithy_server_runtime::MultiServiceRouter;

// The harness every suite in this test crate drives the service through: one router
// over an engine this test holds, and one awsJson1_1 call signed for [`REGION`].
use super::project_build_lifecycle::{call, emulator_on, REGION};

/// The instants the seeded record reports, as epoch millis: fixed values, so the
/// assertion is on what the store holds rather than on a clock.
const SEEDED_CREATED_MILLIS: i64 = 1_700_000_000_000;
const SEEDED_LAST_MODIFIED_MILLIS: i64 = 1_700_000_123_000;

/// Read the report groups `arns` address, asserting the call succeeded.
async fn batch_get(router: &MultiServiceRouter, arns: &[String]) -> Value {
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
    body
}

/// The ARN this emulator mints for a report group of `name` in [`REGION`] — through
/// the service's own minter, so the test cannot drift from the ARN form the handler
/// stores.
fn report_group_arn(name: &str) -> String {
    state::report_group_arn(REGION, name)
}

/// An engine holding one report group named `name`, written straight into the
/// service's state rather than through `CreateReportGroup` — so what a read answers
/// can only have come from the store.
fn engine_holding(name: &str) -> Arc<engine::InMemoryEngine> {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let handle = engine.state::<CodeBuildState>();
    handle.lock().report_groups.insert(
        name.to_string(),
        StoredReportGroup {
            report_group: ReportGroup {
                arn: Some(report_group_arn(name)),
                name: Some(name.to_string()),
                r#type: Some("CODE_COVERAGE".to_string()),
                export_config: None,
                created: Some(state::datetime(SEEDED_CREATED_MILLIS)),
                last_modified: Some(state::datetime(SEEDED_LAST_MODIFIED_MILLIS)),
                tags: None,
                status: Some(state::REPORT_GROUP_ACTIVE_STATUS.to_string()),
            },
            created_millis: SEEDED_CREATED_MILLIS,
            last_modified_millis: SEEDED_LAST_MODIFIED_MILLIS,
        },
    );
    engine
}

/// RULE: the read reports the record the store holds, whoever wrote it — every member
/// as persisted, nothing recomputed from the request — and a repeated read answers the
/// same thing, so reading changes nothing a later caller sees.
#[tokio::test]
async fn a_read_reports_the_stored_record_verbatim() {
    let engine = engine_holding("seeded");
    let router = emulator_on(Arc::clone(&engine));
    let arn = report_group_arn("seeded");

    let read = batch_get(&router, std::slice::from_ref(&arn)).await;
    assert_eq!(
        read["reportGroups"],
        json!([{
            "arn": arn,
            "name": "seeded",
            "type": "CODE_COVERAGE",
            // awsJson1_1 renders a timestamp as epoch SECONDS; the absent
            // `exportConfig` and `tags` of the stored record stay absent.
            "created": SEEDED_CREATED_MILLIS / 1_000,
            "lastModified": SEEDED_LAST_MODIFIED_MILLIS / 1_000,
            "status": "ACTIVE",
        }]),
        "the read must report the stored record verbatim, got {read}"
    );
    assert!(
        read.get("reportGroupsNotFound").is_none(),
        "a held ARN must not be reported missing, got {read}"
    );

    let again = batch_get(&router, std::slice::from_ref(&arn)).await;
    assert_eq!(
        again, read,
        "a second read must answer the same record, got {again}"
    );
    let held = engine.state::<CodeBuildState>().lock().report_groups.len();
    assert_eq!(
        held, 1,
        "a read must not add to or drop from the store, got {held} record(s)"
    );
}

/// RULE: an ARN names a report group only when it is EXACTLY the one a stored record
/// carries, so a value differing in any ARN field — partition, region, account,
/// resource type — or in the case of the name resolves to nothing and is reported in
/// `reportGroupsNotFound`, never refused and never resolved to a near neighbour.
#[tokio::test]
async fn only_the_exact_stored_arn_resolves() {
    let engine = engine_holding("held-group");
    let router = emulator_on(engine);
    let account = engine::EMULATOR_ACCOUNT_ID;

    // (case, the ARN asked about, whether this account holds it)
    let rows: Vec<(&str, String, bool)> = vec![
        ("the stored ARN itself", report_group_arn("held-group"), true),
        (
            "another partition",
            format!("arn:aws-cn:codebuild:{REGION}:{account}:report-group/held-group"),
            false,
        ),
        (
            "another region",
            format!("arn:aws:codebuild:eu-west-1:{account}:report-group/held-group"),
            false,
        ),
        (
            "another account",
            format!("arn:aws:codebuild:{REGION}:111122223333:report-group/held-group"),
            false,
        ),
        (
            "another resource type of this service",
            format!("arn:aws:codebuild:{REGION}:{account}:project/held-group"),
            false,
        ),
        (
            "the name in another case",
            report_group_arn("Held-Group"),
            false,
        ),
        (
            "a name nothing is stored under",
            report_group_arn("absent-group"),
            false,
        ),
        (
            "the resource type with no name after it",
            format!("arn:aws:codebuild:{REGION}:{account}:report-group/"),
            false,
        ),
        (
            "a nested resource path",
            format!("arn:aws:codebuild:{REGION}:{account}:report-group/held-group/extra"),
            false,
        ),
        ("a bare name rather than an ARN", "held-group".to_string(), false),
    ];

    for (case, arn, held) in &rows {
        let read = batch_get(&router, std::slice::from_ref(arn)).await;
        if *held {
            assert_eq!(
                read["reportGroups"][0]["arn"],
                json!(arn),
                "case {case:?}: expected the stored record, got {read}"
            );
            assert!(
                read.get("reportGroupsNotFound").is_none(),
                "case {case:?}: a held ARN must not be reported missing, got {read}"
            );
        } else {
            assert_eq!(
                read["reportGroupsNotFound"],
                json!([arn]),
                "case {case:?}: expected the ARN reported missing, got {read}"
            );
            // `ReportGroups` is bounded at 1..100 entries, so an empty one is absent
            // rather than serialized as `[]`.
            assert!(
                read.get("reportGroups").is_none(),
                "case {case:?}: expected no reportGroups at all, got {read}"
            );
        }
    }

    // One batch over the whole table: each ARN lands in exactly one list, in request
    // order, and a REPEATED ARN is answered once per position rather than collapsed —
    // so a client can zip the answer against what it asked for.
    let mut asked: Vec<String> = rows.iter().map(|(_, arn, _)| arn.clone()).collect();
    asked.push(report_group_arn("held-group"));
    let read = batch_get(&router, &asked).await;
    let reported: Vec<String> = read["reportGroups"]
        .as_array()
        .expect("reportGroups is a list")
        .iter()
        .map(|group| group["arn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported,
        vec![report_group_arn("held-group"), report_group_arn("held-group")],
        "a repeated held ARN must be answered once per position, got {read}"
    );
    let missing: Vec<String> = read["reportGroupsNotFound"]
        .as_array()
        .expect("reportGroupsNotFound is a list")
        .iter()
        .map(|arn| arn.as_str().unwrap_or_default().to_string())
        .collect();
    let expected_missing: Vec<String> = rows
        .iter()
        .filter(|(_, _, held)| !held)
        .map(|(_, arn, _)| arn.clone())
        .collect();
    assert_eq!(
        missing, expected_missing,
        "every ARN the store does not hold must be answered in request order, got {read}"
    );
    assert_eq!(
        reported.len() + missing.len(),
        asked.len(),
        "the two lists must cover every ARN of the request, got {read}"
    );
}
