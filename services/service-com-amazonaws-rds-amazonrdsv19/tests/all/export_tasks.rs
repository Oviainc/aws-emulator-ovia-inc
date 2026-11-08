// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's snapshot- and cluster-export operations:
//! `DescribeExportTasks` and `CancelExportTask`.
//!
//! The rules these pin:
//!
//! * `DescribeExportTasks` reports the export tasks STORED in the region the request was
//!   signed for, projecting every member of the modelled `ExportTask` shape off the stored
//!   record; narrowing members (`ExportTaskIdentifier`, `SourceArn`, `SourceType`) and the
//!   four filter names the operation publishes intersect, pagination is by `MaxRecords` and
//!   a `Marker` the service itself issued, and an `ExportTaskIdentifier` that names no
//!   stored task selects nothing — an empty report, since every member here narrows.
//! * `CancelExportTask` moves a task that is still running to `CANCELING` and the clock
//!   then makes it `CANCELED` with the `TaskEndTime` it stopped at; a task that has already
//!   stopped — or whose cancellation was already accepted — is the modelled
//!   `InvalidExportTaskStateFault`, and an identifier naming nothing in the request's
//!   region is the not-found fault.
//!
//! Both are asserted over TABLES spanning what the rules generalise over: each of the six
//! statuses `ExportTask$Status` publishes, both values of `ExportSourceType`, a bucket
//! prefix given and omitted, an `ExportOnly` list of zero, one and two entries, every
//! published filter name, each published bound of `MaxRecords` plus one value past it, and
//! both regions.
//!
//! Every call goes over the wire, through the same `MultiServiceRouter` the production
//! router uses (`crate::db_instances`). The records the reads run against are SEEDED
//! through this service's own state API, which is how one suite can hold a task in each of
//! the six statuses at once; the create that writes such a record — `StartExportTask`, and
//! the objects it leaves in Amazon S3 — is driven end to end by
//! [`crate::rds_s3_integration`], so the two implemented reads here run against real
//! stored state rather than an empty map.

use std::sync::Arc;

use service_com_amazonaws_rds_amazonrdsv19::state::{
    self, ExportTaskRecord, EXPORT_TASK_STATUS_CANCELED, EXPORT_TASK_STATUS_CANCELING,
    EXPORT_TASK_STATUS_COMPLETE, EXPORT_TASK_STATUS_FAILED, EXPORT_TASK_STATUS_IN_PROGRESS,
    EXPORT_TASK_STATUS_STARTING, EXPORT_TASK_STATUS_VALUES,
};
use service_com_amazonaws_rds_amazonrdsv19::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, expect_error, expect_ok, optional_text, subtrees, text, texts, wait_for_settle,
    REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// A second region, for the rows that pin that an export task is reported and cancelled
/// only in the region it was started in.
const OTHER_REGION: &str = "eu-west-1";

/// The IAM role every seeded export writes to Amazon S3 under
/// (`StartExportTaskMessage$IamRoleArn`).
const EXPORT_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/rds-s3-export";

/// The KMS key every seeded export encrypts its objects with
/// (`StartExportTaskMessage$KmsKeyId`).
const EXPORT_KMS_KEY: &str =
    "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// The ARN of an RDS resource in `region`, as this service mints one.
fn rds_arn(region: &str, resource: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:{resource}")
}

/// One seeded export task — what `StartExportTask` will write.
///
/// `pub(crate)` so a sibling suite of this one test target can seed the same records
/// through [`stored_task`] rather than writing a second copy of them.
pub(crate) struct SeedSpec {
    /// `ExportTaskIdentifier`.
    identifier: &'static str,
    /// The resource part of `SourceArn`: a DB snapshot, a DB cluster snapshot or a DB
    /// cluster.
    source: &'static str,
    /// `SourceType`, one of the values `ExportSourceType` publishes.
    source_type: &'static str,
    /// `S3Bucket`.
    bucket: &'static str,
    /// `S3Prefix`, absent where the export writes at the root of the bucket.
    prefix: Option<&'static str>,
    /// `ExportOnly`, empty where the whole source is exported.
    export_only: &'static [&'static str],
    /// `Status`.
    status: &'static str,
    /// `PercentProgress`.
    percent_progress: i32,
    /// `TotalExtractedDataInGB`.
    extracted_gb: i32,
    /// `FailureCause`.
    failure_cause: Option<&'static str>,
    /// `WarningMessage`.
    warning_message: Option<&'static str>,
}

/// The seeded table: one task per status `ExportTask$Status` publishes, spanning both
/// `ExportSourceType` values, all three source kinds an export reads, three buckets, a
/// prefix given and omitted, and `ExportOnly` lists of zero, one and two entries.
///
/// Declared out of identifier order on purpose: a describe reports them in identifier
/// order, which is what the ordered store gives and what pagination resumes along.
const SEEDED: [SeedSpec; 6] = [
    SeedSpec {
        identifier: "exp-in-progress",
        source: "cluster:aurora-alpha",
        source_type: "CLUSTER",
        bucket: "exports-one",
        prefix: Some("nightly"),
        export_only: &["appdb"],
        status: EXPORT_TASK_STATUS_IN_PROGRESS,
        percent_progress: 42,
        extracted_gb: 3,
        failure_cause: None,
        warning_message: None,
    },
    SeedSpec {
        identifier: "exp-starting",
        source: "snapshot:db-snap-alpha",
        source_type: "SNAPSHOT",
        bucket: "exports-one",
        prefix: None,
        export_only: &[],
        status: EXPORT_TASK_STATUS_STARTING,
        percent_progress: 0,
        extracted_gb: 0,
        failure_cause: None,
        warning_message: None,
    },
    SeedSpec {
        identifier: "exp-canceling",
        source: "snapshot:db-snap-beta",
        source_type: "SNAPSHOT",
        bucket: "exports-two",
        prefix: None,
        export_only: &["appdb.orders", "appdb.schema.users"],
        status: EXPORT_TASK_STATUS_CANCELING,
        percent_progress: 10,
        extracted_gb: 1,
        failure_cause: None,
        warning_message: None,
    },
    SeedSpec {
        identifier: "exp-canceled",
        source: "cluster-snapshot:cluster-snap-alpha",
        source_type: "SNAPSHOT",
        bucket: "exports-two",
        prefix: Some("archive/2026"),
        export_only: &[],
        status: EXPORT_TASK_STATUS_CANCELED,
        percent_progress: 17,
        extracted_gb: 2,
        failure_cause: None,
        warning_message: Some("the export was canceled before it finished"),
    },
    SeedSpec {
        identifier: "exp-complete",
        source: "snapshot:db-snap-gamma",
        source_type: "SNAPSHOT",
        bucket: "exports-three",
        prefix: Some("done"),
        export_only: &["appdb"],
        status: EXPORT_TASK_STATUS_COMPLETE,
        percent_progress: 100,
        extracted_gb: 25,
        failure_cause: None,
        warning_message: None,
    },
    SeedSpec {
        identifier: "exp-failed",
        source: "cluster:aurora-beta",
        source_type: "CLUSTER",
        bucket: "exports-three",
        prefix: None,
        export_only: &[],
        status: EXPORT_TASK_STATUS_FAILED,
        percent_progress: 60,
        extracted_gb: 7,
        failure_cause: Some("the export role lost access to the bucket"),
        warning_message: None,
    },
];

/// The statuses an export task does not leave on its own, so a seeded task in one of them
/// carries the `TaskEndTime` it stopped at.
const TERMINAL_STATUSES: [&str; 3] = [
    EXPORT_TASK_STATUS_CANCELED,
    EXPORT_TASK_STATUS_COMPLETE,
    EXPORT_TASK_STATUS_FAILED,
];

/// The seeded spec of `identifier`.
pub(crate) fn spec(identifier: &str) -> &'static SeedSpec {
    SEEDED
        .iter()
        .find(|spec| spec.identifier == identifier)
        .expect("the table above declares this identifier")
}

/// A complete export-task record of `spec` in `region`.
pub(crate) fn stored_task(region: &str, spec: &SeedSpec) -> ExportTaskRecord {
    ExportTaskRecord {
        identifier: spec.identifier.to_string(),
        region: region.to_string(),
        source_arn: rds_arn(region, spec.source),
        source_type: spec.source_type.to_string(),
        export_only: spec.export_only.iter().map(|only| only.to_string()).collect(),
        snapshot_time: state::now_as_wire_timestamp(),
        task_start_time: state::now_as_wire_timestamp(),
        task_end_time: TERMINAL_STATUSES
            .contains(&spec.status)
            .then(state::now_as_wire_timestamp),
        s3_bucket: spec.bucket.to_string(),
        s3_prefix: spec.prefix.map(str::to_string),
        iam_role_arn: EXPORT_ROLE_ARN.to_string(),
        kms_key_id: EXPORT_KMS_KEY.to_string(),
        status: spec.status.to_string(),
        pending_status: None,
        percent_progress: spec.percent_progress,
        total_extracted_data_in_gb: spec.extracted_gb,
        failure_cause: spec.failure_cause.map(str::to_string),
        warning_message: spec.warning_message.map(str::to_string),
    }
}

/// A router carrying this service, holding `tasks` — `(region, record)` — in its store.
pub(crate) fn emulator_holding(tasks: Vec<(String, ExportTaskRecord)>) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let store = state::service_state(&*engine).expect("this service's store");
        let mut store = store.lock();
        for (region, record) in tasks {
            store
                .export_tasks
                .insert((region, record.identifier.clone()), record);
        }
    }
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; these tests sign
    // nothing, so the check is off and the scope header is read for routing only.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A router holding every row of [`SEEDED`] in [`REGION`], plus one task in
/// [`OTHER_REGION`] so no assertion below can pass by reading another region's records.
fn emulator_with_seeded_tasks() -> MultiServiceRouter {
    let mut tasks: Vec<(String, ExportTaskRecord)> = SEEDED
        .iter()
        .map(|spec| (REGION.to_string(), stored_task(REGION, spec)))
        .collect();
    let mut elsewhere = stored_task(OTHER_REGION, spec("exp-in-progress"));
    elsewhere.identifier = "exp-elsewhere".to_string();
    tasks.push((OTHER_REGION.to_string(), elsewhere));
    emulator_holding(tasks)
}

/// The `ExportTaskIdentifier`s `DescribeExportTasks` reports for `params`, in the order it
/// reports them, with the `Marker` it issued.
async fn describe_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    request: &[(String, String)],
) -> (Vec<String>, Option<String>) {
    let body = expect_ok(case, call_in(router, region, "DescribeExportTasks", request).await);
    let identifiers = subtrees(&body, "ExportTask")
        .iter()
        .map(|task| text(task, "ExportTaskIdentifier"))
        .collect();
    (identifiers, optional_text(&body, "Marker"))
}

/// The identifiers `DescribeExportTasks` reports in [`REGION`] for `params`.
async fn describe(
    router: &MultiServiceRouter,
    case: &str,
    request: &[(String, String)],
) -> Vec<String> {
    describe_in(router, REGION, case, request).await.0
}

/// The query parameters of one call, owned so a table row can hold them.
type Request = Vec<(String, String)>;

/// One row of the narrowing table: the case, the request it makes, and the
/// `ExportTaskIdentifier`s the report must carry.
type NarrowingRow = (String, Request, Vec<&'static str>);

/// One row of the refusal table: the case, the request, the HTTP status and the wire error
/// code the service must answer with.
type RefusalRow = (&'static str, Request, u16, &'static str);

/// `[(name, value)]` with owned values, which is what the shared wire helper takes.
fn params(pairs: &[(&str, &str)]) -> Request {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// The seeded tasks are reported out of state, in identifier order, with every member of
/// the modelled `ExportTask` shape projected off the stored record — and only in the region
/// they were started in.
#[tokio::test]
async fn describe_export_tasks_reports_the_stored_tasks_of_the_requests_region() {
    let router = emulator_with_seeded_tasks();

    let mut expected: Vec<String> = SEEDED
        .iter()
        .map(|spec| spec.identifier.to_string())
        .collect();
    expected.sort();
    let (reported, marker) = describe_in(&router, REGION, "describe all", &params(&[])).await;
    assert_eq!(
        reported, expected,
        "case \"describe all\": expected every seeded task in identifier order, got \
         {reported:?}"
    );
    assert_eq!(
        marker, None,
        "case \"describe all\": a page that holds every record issues no Marker, got \
         {marker:?}"
    );

    // Every member of every seeded row, read back off the response.
    for spec in &SEEDED {
        let case = format!("project {}", spec.identifier);
        let body = expect_ok(
            &case,
            call(
                &router,
                "DescribeExportTasks",
                &params(&[("ExportTaskIdentifier", spec.identifier)]),
            )
            .await,
        );
        let tasks = subtrees(&body, "ExportTask");
        assert_eq!(
            tasks.len(),
            1,
            "case {case:?}: expected exactly one ExportTask, got {body}"
        );
        let task = &tasks[0];
        for (member, expected) in [
            ("ExportTaskIdentifier", spec.identifier.to_string()),
            ("SourceArn", rds_arn(REGION, spec.source)),
            ("SourceType", spec.source_type.to_string()),
            ("S3Bucket", spec.bucket.to_string()),
            ("IamRoleArn", EXPORT_ROLE_ARN.to_string()),
            ("KmsKeyId", EXPORT_KMS_KEY.to_string()),
            ("Status", spec.status.to_string()),
            ("PercentProgress", spec.percent_progress.to_string()),
            ("TotalExtractedDataInGB", spec.extracted_gb.to_string()),
        ] {
            let actual = text(task, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected:?}, got {actual:?} in {task}"
            );
        }
        for (member, expected) in [
            ("S3Prefix", spec.prefix),
            ("FailureCause", spec.failure_cause),
            ("WarningMessage", spec.warning_message),
        ] {
            let actual = optional_text(task, member);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {member} {expected:?}, got {actual:?} in {task}"
            );
        }
        // `ExportOnly` carries the stored list, so a task that exports its whole source
        // reports no member at all.
        let export_only: Vec<String> = subtrees(task, "ExportOnly")
            .iter()
            .flat_map(|list| texts(list, "member"))
            .collect();
        let export_only: Vec<&str> = export_only.iter().map(String::as_str).collect();
        assert_eq!(
            export_only,
            spec.export_only.to_vec(),
            "case {case:?}: expected ExportOnly {:?}, got {export_only:?} in {task}",
            spec.export_only
        );
        // A task that has stopped reports when it stopped; one still running does not.
        let ended = optional_text(task, "TaskEndTime");
        assert_eq!(
            ended.is_some(),
            TERMINAL_STATUSES.contains(&spec.status),
            "case {case:?}: TaskEndTime is reported exactly for a task that has stopped, \
             got {ended:?} for status {}",
            spec.status
        );
        assert!(
            optional_text(task, "TaskStartTime").is_some(),
            "case {case:?}: every export task reports when it started, got {task}"
        );
        assert!(
            optional_text(task, "SnapshotTime").is_some(),
            "case {case:?}: every export task reports its source's snapshot time, got {task}"
        );
    }

    // RDS is regional: the other region's store holds its own task and nothing else.
    let (elsewhere, _) =
        describe_in(&router, OTHER_REGION, "describe in the other region", &params(&[])).await;
    assert_eq!(
        elsewhere,
        vec!["exp-elsewhere".to_string()],
        "case \"describe in the other region\": expected only that region's task, got \
         {elsewhere:?}"
    );
}

/// The narrowing members and every published filter name select out of the stored tasks,
/// and they intersect.
#[tokio::test]
async fn describe_export_tasks_narrows_by_the_members_and_filters_it_publishes() {
    let router = emulator_with_seeded_tasks();

    // (case, parameters, the identifiers the request must report)
    let mut rows: Vec<NarrowingRow> = vec![
        (
            "SourceArn of one task".to_string(),
            params(&[("SourceArn", &rds_arn(REGION, spec("exp-complete").source))]),
            vec!["exp-complete"],
        ),
        (
            "SourceArn of a source nothing exported".to_string(),
            params(&[("SourceArn", &rds_arn(REGION, "snapshot:db-snap-nobody"))]),
            vec![],
        ),
        (
            "SourceType CLUSTER".to_string(),
            params(&[("SourceType", "CLUSTER")]),
            vec!["exp-failed", "exp-in-progress"],
        ),
        (
            "SourceType SNAPSHOT".to_string(),
            params(&[("SourceType", "SNAPSHOT")]),
            vec!["exp-canceled", "exp-canceling", "exp-complete", "exp-starting"],
        ),
        (
            "the s3-bucket filter".to_string(),
            params(&[
                ("Filters.Filter.1.Name", "s3-bucket"),
                ("Filters.Filter.1.Values.Value.1", "exports-two"),
            ]),
            vec!["exp-canceled", "exp-canceling"],
        ),
        (
            "the source-arn filter".to_string(),
            params(&[
                ("Filters.Filter.1.Name", "source-arn"),
                (
                    "Filters.Filter.1.Values.Value.1",
                    &rds_arn(REGION, spec("exp-failed").source),
                ),
            ]),
            vec!["exp-failed"],
        ),
        (
            "the export-task-identifier filter".to_string(),
            params(&[
                ("Filters.Filter.1.Name", "export-task-identifier"),
                ("Filters.Filter.1.Values.Value.1", "exp-starting"),
            ]),
            vec!["exp-starting"],
        ),
        (
            "a filter carrying two values".to_string(),
            params(&[
                ("Filters.Filter.1.Name", "status"),
                ("Filters.Filter.1.Values.Value.1", "complete"),
                ("Filters.Filter.1.Values.Value.2", "failed"),
            ]),
            vec!["exp-complete", "exp-failed"],
        ),
        (
            "two filters that intersect".to_string(),
            params(&[
                ("Filters.Filter.1.Name", "status"),
                ("Filters.Filter.1.Values.Value.1", "complete"),
                ("Filters.Filter.2.Name", "s3-bucket"),
                ("Filters.Filter.2.Values.Value.1", "exports-three"),
            ]),
            vec!["exp-complete"],
        ),
        (
            "two filters that exclude each other".to_string(),
            params(&[
                ("Filters.Filter.1.Name", "status"),
                ("Filters.Filter.1.Values.Value.1", "complete"),
                ("Filters.Filter.2.Name", "s3-bucket"),
                ("Filters.Filter.2.Values.Value.1", "exports-one"),
            ]),
            vec![],
        ),
        (
            "a status filter value that is not lowercase".to_string(),
            params(&[
                ("Filters.Filter.1.Name", "status"),
                ("Filters.Filter.1.Values.Value.1", "COMPLETE"),
            ]),
            vec![],
        ),
        (
            "SourceType and a filter that intersect".to_string(),
            params(&[
                ("SourceType", "CLUSTER"),
                ("Filters.Filter.1.Name", "status"),
                ("Filters.Filter.1.Values.Value.1", "failed"),
            ]),
            vec!["exp-failed"],
        ),
        (
            "an identifier and a SourceArn that exclude each other".to_string(),
            params(&[
                ("ExportTaskIdentifier", "exp-complete"),
                ("SourceArn", &rds_arn(REGION, spec("exp-failed").source)),
            ]),
            vec![],
        ),
    ];
    // The `status` filter is documented over the statuses `ExportTask$Status` publishes,
    // lowercased: every one of them selects exactly the seeded task in that status.
    for status in EXPORT_TASK_STATUS_VALUES {
        let expected = SEEDED
            .iter()
            .find(|spec| spec.status == status)
            .expect("the seeded table covers every published status")
            .identifier;
        rows.push((
            format!("the status filter on {status}"),
            params(&[
                ("Filters.Filter.1.Name", "status"),
                (
                    "Filters.Filter.1.Values.Value.1",
                    &status.to_ascii_lowercase(),
                ),
            ]),
            vec![expected],
        ));
    }
    // Naming one task reports that task, whatever else is stored.
    for spec in &SEEDED {
        rows.push((
            format!("ExportTaskIdentifier {}", spec.identifier),
            params(&[("ExportTaskIdentifier", spec.identifier)]),
            vec![spec.identifier],
        ));
    }
    // `ExportTaskIdentifier` is a narrowing member like the others, so an identifier that
    // names no task of this region selects none of them — an empty report, not a fault.
    rows.push((
        "an identifier that names no stored task".to_string(),
        params(&[("ExportTaskIdentifier", "exp-no-such-task")]),
        vec![],
    ));
    rows.push((
        "an identifier of a task another region holds".to_string(),
        params(&[("ExportTaskIdentifier", "exp-elsewhere")]),
        vec![],
    ));

    for (case, request, expected) in rows {
        let reported = describe(&router, &case, &request).await;
        let reported: Vec<&str> = reported.iter().map(String::as_str).collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// `MaxRecords` pages the report, and the `Marker` the service issued resumes it exactly
/// where the page ended.
#[tokio::test]
async fn describe_export_tasks_pages_along_the_marker_it_issued() {
    // Enough tasks for more than one page at the smallest `MaxRecords` the member admits.
    let identifiers: Vec<String> = (1..=25).map(|n| format!("exp-paged-{n:02}")).collect();
    let template = spec("exp-in-progress");
    let tasks: Vec<(String, ExportTaskRecord)> = identifiers
        .iter()
        .map(|identifier| {
            let mut record = stored_task(REGION, template);
            record.identifier = identifier.clone();
            (REGION.to_string(), record)
        })
        .collect();
    let router = emulator_holding(tasks);

    let (first, marker) = describe_in(
        &router,
        REGION,
        "the first page of 20",
        &params(&[("MaxRecords", "20")]),
    )
    .await;
    assert_eq!(
        first,
        identifiers[..20].to_vec(),
        "case \"the first page of 20\": expected the first 20 identifiers in order, got \
         {first:?}"
    );
    let marker = marker.expect("a page that leaves records behind issues a Marker");
    assert_eq!(
        marker,
        state::encode_marker(&identifiers[19]),
        "case \"the first page of 20\": the Marker is the opaque token for the last record \
         the page served, got {marker:?}"
    );

    let (second, next) = describe_in(
        &router,
        REGION,
        "the page after the Marker",
        &params(&[("MaxRecords", "20"), ("Marker", &marker)]),
    )
    .await;
    assert_eq!(
        second,
        identifiers[20..].to_vec(),
        "case \"the page after the Marker\": expected the remaining identifiers, got \
         {second:?}"
    );
    assert_eq!(
        next, None,
        "case \"the page after the Marker\": the last page issues no Marker, got {next:?}"
    );

    // Each published bound of `MaxRecords` ("Minimum 20, maximum 100") is served.
    for bound in ["20", "100"] {
        let case = format!("MaxRecords at the published bound {bound}");
        let reported = describe(&router, &case, &params(&[("MaxRecords", bound)])).await;
        let expected = usize::min(bound.parse::<usize>().expect("a number"), identifiers.len());
        assert_eq!(
            reported.len(),
            expected,
            "case {case:?}: expected {expected} records, got {reported:?}"
        );
    }
}

/// Every request this operation refuses, and the status and error code it refuses it with.
#[tokio::test]
async fn describe_export_tasks_refuses_a_request_it_cannot_serve() {
    let router = emulator_with_seeded_tasks();

    // (case, parameters, HTTP status, wire error code)
    let rows: [RefusalRow; 5] = [
        (
            "a filter name the operation does not publish",
            params(&[
                ("Filters.Filter.1.Name", "s3-prefix"),
                ("Filters.Filter.1.Values.Value.1", "nightly"),
            ]),
            400,
            "InvalidParameterValue",
        ),
        (
            // `SourceType` targets the `ExportSourceType` enum, so the boundary refuses a
            // value outside the set the model publishes before the handler runs — under
            // the code this service answers an unacceptable parameter value with.
            "a SourceType outside the published values",
            params(&[("SourceType", "snapshot")]),
            400,
            "InvalidParameterValue",
        ),
        (
            // `MaxRecords` publishes its range in the model ("Minimum 20, maximum 100"), so
            // the boundary refuses a value outside it before the handler runs.
            "MaxRecords one below the published minimum",
            params(&[("MaxRecords", "19")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "MaxRecords one above the published maximum",
            params(&[("MaxRecords", "101")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a Marker no prior page issued",
            params(&[("Marker", "exp-not-a-marker")]),
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, request, status, code) in rows {
        expect_error(
            case,
            call(&router, "DescribeExportTasks", &request).await,
            status,
            code,
        );
    }
}

/// Cancelling a running export moves it to `CANCELING`, the clock then makes it `CANCELED`
/// with the time it stopped, and the report reads the moved record back.
#[tokio::test]
async fn cancel_export_task_moves_a_running_task_and_the_clock_ends_it() {
    let router = emulator_with_seeded_tasks();

    let canceled = expect_ok(
        "cancel a running export",
        call(
            &router,
            "CancelExportTask",
            &params(&[("ExportTaskIdentifier", "exp-in-progress")]),
        )
        .await,
    );
    assert_eq!(
        text(&canceled, "Status"),
        EXPORT_TASK_STATUS_CANCELING,
        "case \"cancel a running export\": a cancellation just accepted is CANCELING, got \
         {canceled}"
    );
    assert_eq!(
        optional_text(&canceled, "TaskEndTime"),
        None,
        "case \"cancel a running export\": a task that is still winding down has not ended, \
         got {canceled}"
    );
    // The members the cancel did not touch still come off the stored record.
    assert_eq!(
        text(&canceled, "PercentProgress"),
        spec("exp-in-progress").percent_progress.to_string(),
        "case \"cancel a running export\": the progress so far is the stored one, got \
         {canceled}"
    );

    // Inside the span the clock owes, the report shows the transient status — and the
    // `status` filter finds it under that status.
    let canceling = describe(
        &router,
        "the canceling task is reported as canceling",
        &params(&[
            ("Filters.Filter.1.Name", "status"),
            ("Filters.Filter.1.Values.Value.1", "canceling"),
        ]),
    )
    .await;
    assert_eq!(
        canceling,
        vec!["exp-canceling".to_string(), "exp-in-progress".to_string()],
        "case \"the canceling task is reported as canceling\": expected the seeded one and \
         the one just cancelled, got {canceling:?}"
    );

    // A second cancellation of the same task is refused: it is already on its way out.
    expect_error(
        "cancel the same task twice",
        call(
            &router,
            "CancelExportTask",
            &params(&[("ExportTaskIdentifier", "exp-in-progress")]),
        )
        .await,
        400,
        "InvalidExportTaskStateFault",
    );

    wait_for_settle();

    let body = expect_ok(
        "the cancelled task after the clock",
        call(
            &router,
            "DescribeExportTasks",
            &params(&[("ExportTaskIdentifier", "exp-in-progress")]),
        )
        .await,
    );
    assert_eq!(
        text(&body, "Status"),
        EXPORT_TASK_STATUS_CANCELED,
        "case \"the cancelled task after the clock\": the clock owed CANCELED, got {body}"
    );
    assert!(
        optional_text(&body, "TaskEndTime").is_some(),
        "case \"the cancelled task after the clock\": a task that has stopped reports when \
         it stopped, got {body}"
    );
}

/// Which statuses `CancelExportTask` acts on, and which it refuses, over every status
/// `ExportTask$Status` publishes.
#[tokio::test]
async fn cancel_export_task_acts_only_on_a_task_that_is_still_running() {
    for spec in &SEEDED {
        // A fresh store per row, so one row's cancellation cannot change the next's.
        let router = emulator_holding(vec![(REGION.to_string(), stored_task(REGION, spec))]);
        let case = format!("cancel a task in status {}", spec.status);
        let request = params(&[("ExportTaskIdentifier", spec.identifier)]);
        let response = call(&router, "CancelExportTask", &request).await;
        // Only a task that has neither stopped nor already been cancelled can be
        // cancelled; every other status is the modelled state fault.
        let cancelable = [EXPORT_TASK_STATUS_STARTING, EXPORT_TASK_STATUS_IN_PROGRESS]
            .contains(&spec.status);
        if cancelable {
            let body = expect_ok(&case, response);
            assert_eq!(
                text(&body, "Status"),
                EXPORT_TASK_STATUS_CANCELING,
                "case {case:?}: expected CANCELING, got {body}"
            );
        } else {
            let body = expect_error(&case, response, 400, "InvalidExportTaskStateFault");
            assert!(
                text(&body, "Message").contains(spec.status),
                "case {case:?}: the refusal names the status that makes it refusable, got \
                 {body}"
            );
            // The refused call left the record where it was.
            let after = expect_ok(
                &case,
                call(
                    &router,
                    "DescribeExportTasks",
                    &params(&[("ExportTaskIdentifier", spec.identifier)]),
                )
                .await,
            );
            assert_eq!(
                text(&after, "Status"),
                spec.status,
                "case {case:?}: a refused cancellation changes nothing, got {after}"
            );
        }
    }
}

/// An identifier that names no task in the request's region is the modelled not-found
/// fault, whether the store is empty or holds other tasks.
#[tokio::test]
async fn cancel_export_task_refuses_an_identifier_that_names_nothing() {
    for (case, router) in [
        ("an empty store", emulator_holding(Vec::new())),
        ("a store holding other tasks", emulator_with_seeded_tasks()),
    ] {
        for identifier in ["exp-no-such-task", "exp-elsewhere", "EXP-IN-PROGRESS"] {
            let case = format!("cancel {identifier} against {case}");
            expect_error(
                &case,
                call(
                    &router,
                    "CancelExportTask",
                    &params(&[("ExportTaskIdentifier", identifier)]),
                )
                .await,
                404,
                "ExportTaskNotFound",
            );
        }
    }
}

/// A report changes nothing a later caller sees: reading a task that owes the clock no
/// transition leaves its status where it was, however often it is read.
#[tokio::test]
async fn describing_an_export_task_does_not_move_it() {
    let router = emulator_with_seeded_tasks();
    for spec in &SEEDED {
        let case = format!("read {} twice", spec.identifier);
        let request = params(&[("ExportTaskIdentifier", spec.identifier)]);
        let first = expect_ok(
            &case,
            call(&router, "DescribeExportTasks", &request).await,
        );
        wait_for_settle();
        let second = expect_ok(
            &case,
            call(&router, "DescribeExportTasks", &request).await,
        );
        for member in ["Status", "PercentProgress", "TotalExtractedDataInGB"] {
            assert_eq!(
                text(&first, member),
                text(&second, member),
                "case {case:?}: {member} must not change because the task was read, got \
                 {second}"
            );
        }
    }
}
