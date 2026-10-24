// SPDX-License-Identifier: CC0-1.0

//! Enum membership on the request members of the two start operations, end to end over
//! HTTP.
//!
//! The rule under test is a property of the SERVICE, not of any one operation, member or
//! value: a request member whose target shape is a Smithy enumeration accepts only the
//! values that enumeration declares — including a member nested inside an element of a
//! list — and a value it does not declare is refused with `ValidationException`
//! (HTTP 400), the service's one `Invalid request body` sentence, leaving nothing behind.
//! The Smithy `enum` trait's value list is not a constraint the boundary validator
//! enforces, so each handler is what holds a value to the catalog.
//!
//! So nothing here is asserted over one value: every enum-targeting member of the two
//! implemented start operations is a row, each is probed with a family of undeclared
//! values (an unrelated word, the empty string, a declared value in the wrong case, with
//! whitespace, truncated or extended, and a value a SIBLING enumeration of this service
//! declares), and the complementary table accepts EVERY value each enumeration declares —
//! the whole published catalog, not the one value the issue cited.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{
    at, call, emulator, get_assessment, start_assessment, start_import, Answer,
};

/// This service's one sentence for a value that fails a rule it enforces, spelled out
/// here rather than imported from the crate so a wrong message fails this suite instead
/// of agreeing with itself.
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// Every value `AssessmentDataSourceType` declares, from
/// `smithy_query(query="shape",shape="AssessmentDataSourceType")`.
const ASSESSMENT_DATA_SOURCE_TYPE: &[&str] = &[
    "StrategyRecommendationsApplicationDataCollector",
    "ManualImport",
    "ApplicationDiscoveryService",
];

/// Every value `Condition` declares, from `smithy_query(query="shape",shape="Condition")`.
const CONDITION: &[&str] = &["EQUALS", "NOT_EQUALS", "CONTAINS", "NOT_CONTAINS"];

/// Every value `DataSourceType` declares, from
/// `smithy_query(query="shape",shape="DataSourceType")`.
const DATA_SOURCE_TYPE: &[&str] = &[
    "ApplicationDiscoveryService",
    "Import",
    "MPA",
    "StrategyRecommendationsApplicationDataCollector",
];

/// Every value `GroupName` declares, from `smithy_query(query="shape",shape="GroupName")`.
const GROUP_NAME: &[&str] = &["ExternalId", "ExternalSourceType"];

/// The enumerations above, so a row can be probed with a value a SIBLING enumeration of
/// this service declares — the hardest kind of undeclared value, since it is a legal
/// value of the service, just not of this member.
const CATALOGS: &[&[&str]] = &[
    ASSESSMENT_DATA_SOURCE_TYPE,
    CONDITION,
    DATA_SOURCE_TYPE,
    GROUP_NAME,
];

/// A `StartAssessment` request carrying `value` in `assessmentDataSourceType`, with the
/// analysis bucket the operation cannot run without.
fn assessment_with_source_type(value: &str) -> Value {
    json!({
        "s3bucketForAnalysisData": "migrationhub-strategy-analysis",
        "assessmentDataSourceType": value,
    })
}

/// A `StartAssessment` request carrying `value` in one `assessmentTargets` entry's
/// `condition`. `name` and `values` are `@required` on `AssessmentTarget`, so both are
/// filled in and only the condition varies.
fn assessment_with_condition(value: &str) -> Value {
    json!({
        "s3bucketForAnalysisData": "migrationhub-strategy-analysis",
        "assessmentTargets": [{"condition": value, "name": "OS_NAME", "values": ["Linux"]}],
    })
}

/// A `StartAssessment` request whose SECOND target carries `value`: the check has to look
/// at every element of the list, not only the first.
fn assessment_with_second_condition(value: &str) -> Value {
    json!({
        "s3bucketForAnalysisData": "migrationhub-strategy-analysis",
        "assessmentTargets": [
            {"condition": CONDITION[0], "name": "SERVER_ID", "values": ["server-1"]},
            {"condition": value, "name": "OS_NAME", "values": ["Linux"]},
        ],
    })
}

/// A `StartImportFileTask` request carrying `value` in `dataSourceType`, with the three
/// members the model marks `@required`.
fn import_with_source_type(value: &str) -> Value {
    json!({
        "name": "import-1",
        "S3Bucket": "migrationhub-strategy-import",
        "s3key": "servers.csv",
        "dataSourceType": value,
    })
}

/// A `StartImportFileTask` request carrying `value` in one `groupId` entry's `name`.
fn import_with_group_name(value: &str) -> Value {
    json!({
        "name": "import-1",
        "S3Bucket": "migrationhub-strategy-import",
        "s3key": "servers.csv",
        "groupId": [{"name": value, "value": "wave-1"}],
    })
}

/// One enum-targeting request member of an implemented start operation.
struct EnumMember {
    /// The operation whose request carries the member.
    operation: &'static str,
    /// The member under probe, named in every assertion.
    member: &'static str,
    /// Every value the member's target enumeration declares.
    declared: &'static [&'static str],
    /// A request carrying the given value in that member, every other member legal.
    request: fn(&str) -> Value,
}

/// Every request member of the two implemented start operations whose target shape is an
/// enumeration, from
/// `smithy_query(query="operation",operation="StartAssessment,StartImportFileTask")` read
/// against `smithy_query(query="shape",shape="AssessmentTarget,Group")`: the two
/// top-level members and the two that sit one level down, inside a list element.
///
/// (The service's other enum-targeting members belong to operations with handlers of
/// their own: `ListServers`' `serverCriteria`/`sort`/`groupIdFilter`,
/// `StartRecommendationReportGeneration`'s `outputFormat`/`groupIdFilter`,
/// `UpdateServerConfig`'s and `PutPortfolioPreferences`' preference enums, each already
/// held to its catalog and covered by its own suite; the remaining ones belong to
/// operations that are still 501 stubs.)
fn enum_members() -> Vec<EnumMember> {
    vec![
        EnumMember {
            operation: "StartAssessment",
            member: "assessmentDataSourceType",
            declared: ASSESSMENT_DATA_SOURCE_TYPE,
            request: assessment_with_source_type,
        },
        EnumMember {
            operation: "StartAssessment",
            member: "assessmentTargets[0].condition",
            declared: CONDITION,
            request: assessment_with_condition,
        },
        EnumMember {
            operation: "StartAssessment",
            member: "assessmentTargets[1].condition",
            declared: CONDITION,
            request: assessment_with_second_condition,
        },
        EnumMember {
            operation: "StartImportFileTask",
            member: "dataSourceType",
            declared: DATA_SOURCE_TYPE,
            request: import_with_source_type,
        },
        EnumMember {
            operation: "StartImportFileTask",
            member: "groupId[0].name",
            declared: GROUP_NAME,
            request: import_with_group_name,
        },
    ]
}

/// Values the enumeration `declared` does not declare, differing along what the rule
/// generalises over: an unrelated word, the empty string, how near the value comes to a
/// declared one (case, whitespace, one character short, one character long) and whether
/// it is a value a sibling enumeration of this service declares.
///
/// Derived from the catalog itself, so each row is a value for THAT member rather than a
/// string that happens to work for one of them, and anything that is in fact declared is
/// dropped.
fn undeclared_values(declared: &[&str]) -> Vec<(String, String)> {
    let near = declared[0];
    let mut rows = vec![
        // The two values the issue recorded.
        ("an unrelated word".to_string(), "Bogus".to_string()),
        ("an SQL-looking condition".to_string(), "LIKE".to_string()),
        ("the empty string".to_string(), String::new()),
        (
            "a declared value in the wrong case".to_string(),
            near.to_lowercase(),
        ),
        (
            "a declared value with trailing whitespace".to_string(),
            format!("{near} "),
        ),
        (
            "a declared value one character short".to_string(),
            near[..near.len() - 1].to_string(),
        ),
        (
            "a declared value with a suffix".to_string(),
            format!("{near}X"),
        ),
    ];
    for catalog in CATALOGS {
        if let Some(foreign) = catalog.iter().find(|value| !declared.contains(value)) {
            rows.push((
                "a value a sibling enumeration of this service declares".to_string(),
                (*foreign).to_string(),
            ));
        }
    }
    rows.retain(|(_, value)| !declared.contains(&value.as_str()));
    rows
}

/// Send `request` to the named operation.
async fn start(router: &MultiServiceRouter, operation: &str, request: Value) -> Answer {
    match operation {
        "StartAssessment" => start_assessment(router, request).await,
        "StartImportFileTask" => start_import(router, request).await,
        other => panic!("this suite has no case for operation {other:?}"),
    }
}

/// Where the id a successful start hands back lives in that operation's response.
fn minted_id_pointer(operation: &str) -> &'static str {
    match operation {
        "StartAssessment" => "/assessmentId",
        "StartImportFileTask" => "/id",
        other => panic!("this suite has no case for operation {other:?}"),
    }
}

/// How many assessments this account holds, as the service reports them: an account with
/// none has no latest assessment id at all (404), so this counts 0 or "at least one".
async fn has_an_assessment(router: &MultiServiceRouter) -> bool {
    let answer = call(router, "GET", "/get-latest-assessment-id", None).await;
    match answer.status {
        200 => true,
        404 => false,
        other => panic!(
            "GetLatestAssessmentId answered neither 200 nor 404 but {other} with {:?}",
            answer.body
        ),
    }
}

/// How many import file tasks this account holds, read back through the listing.
async fn import_task_count(router: &MultiServiceRouter) -> usize {
    let answer = call(router, "GET", "/list-import-file-task", None).await;
    assert_eq!(
        answer.status, 200,
        "the import listing must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    at(&answer.body, "/taskInfos")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// Whether the operation created anything on this (fresh) emulator.
async fn created_something(router: &MultiServiceRouter, operation: &str) -> bool {
    match operation {
        "StartAssessment" => has_an_assessment(router).await,
        "StartImportFileTask" => import_task_count(router).await > 0,
        other => panic!("this suite has no case for operation {other:?}"),
    }
}

/// A value an enumeration does not declare is refused with 400 `ValidationException` in
/// the service's one wording, hands back no id, and leaves nothing behind.
#[tokio::test]
async fn an_undeclared_enum_value_is_refused_and_creates_nothing() {
    for member in enum_members() {
        for (shape, value) in undeclared_values(member.declared) {
            let case = format!(
                "{}${} = {value:?} ({shape})",
                member.operation, member.member
            );
            // A fresh emulator per row, so "nothing was created" is about THIS request.
            let router = emulator();
            let answer = start(&router, member.operation, (member.request)(&value)).await;

            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.string("/message"),
                INVALID_REQUEST_BODY,
                "case {case:?}: expected the service's {INVALID_REQUEST_BODY:?} sentence, got {:?}",
                answer.body
            );
            assert_eq!(
                at(&answer.body, minted_id_pointer(member.operation)),
                Value::Null,
                "case {case:?}: a refused start hands back no id, got {:?}",
                answer.body
            );
            assert!(
                !created_something(&router, member.operation).await,
                "case {case:?}: a refused start must leave no resource behind"
            );
        }
    }
}

/// EVERY value each enumeration declares is accepted on the same path, and the start
/// creates exactly one resource — so the refusal above is scoped to values the catalog
/// really does not hold, not to the member being present.
#[tokio::test]
async fn every_value_the_enumeration_declares_is_accepted() {
    for member in enum_members() {
        for declared in member.declared {
            let case = format!("{}${} = {declared:?}", member.operation, member.member);
            let router = emulator();
            let answer = start(&router, member.operation, (member.request)(declared)).await;

            assert_eq!(
                answer.status, 200,
                "case {case:?}: a declared value is accepted, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                !answer.string(minted_id_pointer(member.operation)).is_empty(),
                "case {case:?}: an accepted start hands back an id, got {:?}",
                answer.body
            );
            assert!(
                created_something(&router, member.operation).await,
                "case {case:?}: an accepted start creates the resource"
            );
        }
    }
}

/// A declared condition round-trips: the targets `GetAssessment` reports are the ones the
/// accepted start stored, for every value `Condition` declares — the reporting read is
/// what would have echoed an undeclared condition back.
#[tokio::test]
async fn a_declared_condition_round_trips_through_get_assessment() {
    let router = emulator();

    for condition in CONDITION {
        let request = assessment_with_condition(condition);
        let started = start_assessment(&router, request.clone()).await;
        assert_eq!(
            started.status, 200,
            "condition {condition:?}: the start must be accepted, got {} with {:?}",
            started.status, started.body
        );
        let id = started.string("/assessmentId");

        let reported = get_assessment(&router, &id).await;
        assert_eq!(
            reported.status, 200,
            "condition {condition:?}: the assessment must be reported, got {} with {:?}",
            reported.status, reported.body
        );
        assert_eq!(
            at(&reported.body, "/assessmentTargets"),
            request["assessmentTargets"],
            "condition {condition:?}: the stored targets must come back as supplied, got {:?}",
            at(&reported.body, "/assessmentTargets")
        );
    }
}
