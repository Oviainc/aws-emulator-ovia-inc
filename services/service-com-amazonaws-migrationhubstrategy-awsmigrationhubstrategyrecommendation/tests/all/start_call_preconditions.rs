// SPDX-License-Identifier: CC0-1.0

//! What a start call of this service requires before it mints anything, end to end over
//! HTTP.
//!
//! Two rules, both one level more general than any single request.
//!
//! 1. `StartAssessment` "starts the assessment of an on-premises environment", and what
//!    collects the data an assessment is made of is the data collector installed in that
//!    environment. An account that has registered none has no data source for an
//!    assessment, so the start is refused with `ValidationException` (HTTP 400) telling the
//!    caller to register a collector, and nothing is minted — whatever else the request
//!    carries, and however many resources an import brought into the account, because an
//!    import declares the resources to assess and does not collect from them. The same
//!    request against an account that HAS a data source is accepted.
//!
//! 2. A bucket member naming a bucket this account does not hold is refused as the MISSING
//!    bucket it is — the message names the bucket the request sent, under the
//!    `[S3BucketNotFound]` reason — and never as the permissions one, which asserts that
//!    the bucket is there and that only its name keeps the service out: a client told that
//!    renames a bucket that does not exist instead of creating it. The rule holds for every
//!    bucket member of both start calls, and nothing is minted either way. The emulator
//!    here mounts no object storage, so no name it does not provision for itself answers to
//!    a bucket; which refusal each case earns when object storage IS mounted is tabled in
//!    `start_call_bucket_reachability.rs`.
//!
//! The rows span what each rule generalises over: for the first, what the request carries
//! (every `AssessmentDataSourceType`, targets, a report bucket) across accounts with no
//! data source and with each kind of one; for the second, which member carries the name and
//! how near that name comes to the prefix.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState, ServerDiscovery,
    ServerRecord,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{
    at, call, get_latest_assessment_id, register_collector_in, start_assessment, start_import,
    Answer, TEST_REGION,
};

/// Every value `AssessmentDataSourceType` declares, from
/// `smithy_query(query="shape",shape="AssessmentDataSourceType")`, so the table spans the
/// whole catalog rather than one data-source type.
const ASSESSMENT_DATA_SOURCE_TYPES: &[&str] = &[
    "ApplicationDiscoveryService",
    "ManualImport",
    "StrategyRecommendationsApplicationDataCollector",
];

/// The failure reason a name no bucket of this account answers to has to be reported
/// under, and the one it must NOT be reported under.
const NOT_FOUND_REASON: &str = "[S3BucketNotFound]";
const PERMISSIONS_REASON: &str = "[S3PermissionsIsIncorrect]";

/// A bucket name that satisfies the prefix, for the members a row is not probing.
const CONFORMING_BUCKET: &str = "migrationhub-strategy-analysis";

/// What data source the account under test holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataSource {
    /// None at all: no collector registered, and no server the service holds collected
    /// data for.
    None,
    /// Servers in the inventory and nothing that collects from them — what a file import
    /// leaves behind.
    ImportedServersOnly,
    /// A data collector registered with the service.
    RegisteredCollector,
    /// A server a collector already fetched discovery data for.
    DiscoveredServer,
}

impl DataSource {
    /// Whether an assessment can be started at all in such an account.
    fn can_assess(self) -> bool {
        matches!(
            self,
            DataSource::RegisteredCollector | DataSource::DiscoveredServer
        )
    }
}

/// `ResourceId`s (`^[0-9a-b]+`, max 44) for the seeded servers.
const SEEDED_SERVERS: &[&str] = &["0a", "1b"];

/// One emulator whose account holds the data source `source` in the Region every request
/// here is signed for.
///
/// The records are seeded through the shared engine, into the same maps the paths that
/// create them write: an import's ingestion writes the servers, a collector's own
/// registration writes the collector and the discovery data it fetched. Every assertion
/// then goes through the router.
fn emulator_holding(source: DataSource) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let portfolio = store.region_entry(TEST_REGION);
        if matches!(
            source,
            DataSource::ImportedServersOnly | DataSource::DiscoveredServer
        ) {
            for id in SEEDED_SERVERS {
                let record = ServerRecord {
                    id: (*id).to_string(),
                    name: Some(format!("server-{id}")),
                    discovery: (source == DataSource::DiscoveredServer).then(|| ServerDiscovery {
                        hostname: Some(format!("host-{id}")),
                        ip_address: Some("10.0.0.9".to_string()),
                        source: Some("vCenter".to_string()),
                        vm_id: Some(format!("vm-{id}")),
                    }),
                    ..Default::default()
                };
                portfolio.servers.insert(record.id.clone(), record);
            }
        }
    }
    if source == DataSource::RegisteredCollector {
        register_collector_in(&engine, &[TEST_REGION]);
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The `StartAssessment` requests the table runs against every account, differing along
/// what the rule generalises over: which optional members are present, and which
/// `assessmentDataSourceType` the caller names.
fn assessment_requests() -> Vec<(String, Value)> {
    let mut requests = vec![
        (
            "the analysis bucket alone".to_string(),
            json!({"s3bucketForAnalysisData": CONFORMING_BUCKET}),
        ),
        (
            "an analysis bucket, a report bucket and targets".to_string(),
            json!({
                "s3bucketForAnalysisData": CONFORMING_BUCKET,
                "s3bucketForReportData": "migrationhub-strategy-reports",
                "assessmentTargets": [
                    {"condition": "EQUALS", "name": "SERVER_ID", "values": ["0a"]},
                ],
            }),
        ),
    ];
    for data_source_type in ASSESSMENT_DATA_SOURCE_TYPES {
        requests.push((
            format!("assessmentDataSourceType {data_source_type:?}"),
            json!({
                "s3bucketForAnalysisData": CONFORMING_BUCKET,
                "assessmentDataSourceType": data_source_type,
            }),
        ));
    }
    requests
}

/// An account with no data collector cannot start an assessment: the start is refused with
/// 400 `ValidationException` and the service's sentence telling the caller to register a
/// collector, nothing is minted, and the same request is accepted the moment the account
/// does hold a data source.
#[tokio::test]
async fn an_assessment_needs_a_data_source_to_collect_from() {
    for source in [
        DataSource::None,
        DataSource::ImportedServersOnly,
        DataSource::RegisteredCollector,
        DataSource::DiscoveredServer,
    ] {
        for (what, request) in assessment_requests() {
            let case = format!("{source:?} / {what}");
            // A fresh emulator per row, so "nothing was minted" is about THIS request.
            let router = emulator_holding(source);
            let answer = start_assessment(&router, request).await;

            if source.can_assess() {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: an account holding a data source can start an \
                     assessment, got {} with {:?}",
                    answer.status, answer.body
                );
                assert!(
                    !answer.string("/assessmentId").is_empty(),
                    "case {case:?}: an accepted start hands back an assessment id, got {:?}",
                    answer.body
                );
                continue;
            }

            assert_eq!(
                answer.status, 400,
                "case {case:?}: an account with no data source cannot assess, so the start \
                 must be refused, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
            let message = answer.string("/message");
            assert!(
                message.to_ascii_lowercase().contains("register")
                    && message.to_ascii_lowercase().contains("collector"),
                "case {case:?}: the refusal must tell the caller to register a collector, \
                 got {message:?}"
            );
            assert!(
                answer.string("/assessmentId").is_empty(),
                "case {case:?}: a refused start hands back no assessment id, got {:?}",
                answer.body
            );
            assert_eq!(
                get_latest_assessment_id(&router).await.status,
                404,
                "case {case:?}: a refused start must mint no assessment"
            );
        }
    }
}

/// One row of the bucket table: which operation and member carry the name, and the name.
struct BucketRow {
    operation: &'static str,
    member: &'static str,
    bucket: &'static str,
}

/// Every bucket member of both start calls, against names that come at the prefix from a
/// different side each time: a name with no prefix at all, one a character short of it, one
/// carrying it somewhere other than the start, and one differing from it only in case.
fn bucket_rows() -> Vec<BucketRow> {
    let names: &[&str] = &[
        "my-bucket",
        "migrationhub-strateg",
        "x-migrationhub-strategy-data",
        "migrationhub.strategy-data",
    ];
    let members: &[(&str, &str)] = &[
        ("StartAssessment", "s3bucketForAnalysisData"),
        ("StartAssessment", "s3bucketForReportData"),
        ("StartImportFileTask", "S3Bucket"),
        ("StartImportFileTask", "s3bucketForReportData"),
    ];
    members
        .iter()
        .flat_map(|(operation, member)| {
            names.iter().map(move |bucket| BucketRow {
                operation,
                member,
                bucket,
            })
        })
        .collect()
}

/// The request one row sends: its own member carries the non-conforming name, and every
/// other member the operation needs carries a conforming one.
fn request_for(row: &BucketRow) -> Value {
    match row.operation {
        "StartAssessment" => {
            let mut request = json!({"s3bucketForAnalysisData": CONFORMING_BUCKET});
            request[row.member] = json!(row.bucket);
            request
        }
        "StartImportFileTask" => {
            let mut request = json!({
                "name": "prefix-probe",
                "S3Bucket": "migrationhub-strategy-import",
                "s3key": "servers.json",
            });
            request[row.member] = json!(row.bucket);
            request
        }
        other => panic!("this table has no case for operation {other:?}"),
    }
}

async fn import_task_count(router: &MultiServiceRouter) -> usize {
    let listed: Answer = call(router, "GET", "/list-import-file-task", None).await;
    at(&listed.body, "/taskInfos")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// A bucket name this account holds no bucket for is refused as the missing bucket it is,
/// by a message naming the submitted name — not as a bucket the service may not reach — and
/// no task and no assessment is created against it.
#[tokio::test]
async fn a_name_no_bucket_answers_to_is_refused_as_a_missing_bucket() {
    for row in bucket_rows() {
        let case = format!("{}${} = {:?}", row.operation, row.member, row.bucket);
        // A fresh emulator per row, with a registered collector, so an assessment row is
        // refused for the bucket name and not for the account's data source.
        let router = emulator_holding(DataSource::RegisteredCollector);
        let request = request_for(&row);
        let answer = match row.operation {
            "StartAssessment" => start_assessment(&router, request).await,
            "StartImportFileTask" => start_import(&router, request).await,
            other => panic!("this table has no case for operation {other:?}"),
        };

        assert_eq!(
            answer.status, 400,
            "case {case:?}: a name no bucket answers to is refused, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        let message = answer.string("/message");
        assert!(
            message.contains(NOT_FOUND_REASON),
            "case {case:?}: the refusal must carry the missing-bucket reason \
             {NOT_FOUND_REASON:?}, got {message:?}"
        );
        assert!(
            !message.contains(PERMISSIONS_REASON),
            "case {case:?}: a bucket that is not there must not be reported under \
             {PERMISSIONS_REASON:?}, which says it is, got {message:?}"
        );
        assert!(
            message.contains(row.bucket),
            "case {case:?}: the refusal must name the submitted bucket {:?}, got {message:?}",
            row.bucket
        );

        // And nothing was created against the name the service refused.
        assert_eq!(
            import_task_count(&router).await,
            0,
            "case {case:?}: a refused start must leave no import file task behind"
        );
        assert_eq!(
            get_latest_assessment_id(&router).await.status,
            404,
            "case {case:?}: a refused start must leave no assessment behind"
        );
    }
}
