// SPDX-License-Identifier: CC0-1.0

//! Regional scoping of this service's state, end to end over HTTP.
//!
//! The rule under test is the one a regional AWS service states for ALL of its
//! control-plane resources, not for one of them: a resource belongs to the Region the
//! request that created it was signed for, so every read signed for that Region reports
//! it and a read signed for any other Region answers from an account that holds none of
//! it — an empty list, the modelled refusal for an id, and nothing saved for a
//! per-Region singleton. A per-Region quota or singleton is therefore held once per
//! Region, not once for the whole account.
//!
//! So nothing below asserts one recorded pair of Regions: the table spans the Regions
//! the rule generalises over (four, in three partitions, including the one an unsigned
//! request defaults to) and every resource kind this service holds — an import file
//! task, an assessment and its latest-id singleton, the preferences singleton, a
//! recommendation report generation task and its one-at-a-time coalescing, and the
//! server inventory an import ingests. Each resource is created through ONE Region's
//! endpoint and then looked for through every Region's, so a read that answers from
//! another Region's portfolio fails here.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState, ServerRecord,
    MAX_REGIONS,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{at, emulator, Answer};

/// The Regions every table below spans: the one this crate serves an unsigned request
/// in, two more commercial Regions in different geographies, and one outside the `aws`
/// partition — a Region is a string the caller signs for, so the set is deliberately
/// wider than the pair the issue cited.
const REGIONS: &[&str] = &["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-1"];

/// Every value `ApplicationMode` admits, from
/// `smithy_query(query="closure",operation="PutPortfolioPreferences")` -> `ApplicationMode`.
/// One per Region below, so a preferences read that answers from the wrong Region reports
/// a mode that Region never saved.
const APPLICATION_MODES: &[&str] = &["ALL", "KNOWN", "UNKNOWN"];

/// One emulator over one shared engine, with `servers` seeded into the portfolio of
/// `region` — the same map an import's ingestion writes and `ListServers` reads.
fn emulator_with_servers_per_region(
    seeded: &[(&str, Vec<ServerRecord>)],
) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        for (region, servers) in seeded {
            let portfolio = store.region_entry(region);
            for record in servers.clone() {
                portfolio.servers.insert(record.id.clone(), record);
            }
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One request to this service's endpoint in `region`, signed for that same Region: the
/// host and the SigV4 credential scope are what select a Region, exactly as a client
/// configured for it sends them.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("migrationhub-strategy.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{region}/migrationhub-strategy/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        );
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} in {region} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// The ids `ListImportFileTask` reported in `region`, in the order it reported them.
async fn listed_import_ids(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let answer = call_in(router, region, "GET", "/list-import-file-task", None).await;
    assert_eq!(
        answer.status, 200,
        "case {region:?}: the listing must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    let entries = at(&answer.body, "/taskInfos")
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {region:?}: taskInfos is always reported as a list, got {:?}",
                answer.body
            )
        });
    assert_eq!(
        at(&answer.body, "/nextToken"),
        Value::Null,
        "case {region:?}: a page holding the whole Region carries no cursor, got {:?}",
        answer.body
    );
    entries
        .iter()
        .map(|entry| {
            at(entry, "/id")
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {region:?}: every listed task reports its id, got {entry:?}")
                })
                .to_string()
        })
        .collect()
}

/// What one Region's endpoint created, so a read through another Region's can be checked
/// to report none of it.
struct Portfolio {
    region: &'static str,
    import_id: String,
    assessment_id: String,
    report_id: String,
    application_mode: &'static str,
}

/// Create one of every resource through `region`'s endpoint and answer what was minted.
///
/// The requests differ from Region to Region along what the rule generalises over — the
/// import name and key, the assessment's targets, the report's output format and the
/// preferences mode — so a read answering from the wrong Region reports a value that
/// Region never created rather than a coincidentally equal one.
///
/// The preferences are saved FIRST, because a save is refused while a task of that
/// Region's portfolio is still running. Called for one Region after another inside one
/// task's span, that ordering also shows the refusal is scoped to its own Region: this
/// Region's save succeeds while the previous Region's tasks are still in progress.
async fn create_everything_in(router: &MultiServiceRouter, index: usize) -> Portfolio {
    let region = REGIONS[index];
    let application_mode = APPLICATION_MODES[index % APPLICATION_MODES.len()];
    let saved = call_in(
        router,
        region,
        "POST",
        "/put-portfolio-preferences",
        Some(json!({"applicationMode": application_mode})),
    )
    .await;
    assert_eq!(
        saved.status, 200,
        "case {region:?}: the preferences must save, got {} with {:?}",
        saved.status, saved.body
    );

    let started = call_in(
        router,
        region,
        "POST",
        "/start-import-file-task",
        Some(json!({
            "name": format!("import-from-{region}"),
            "S3Bucket": "migrationhub-strategy-import",
            "s3key": format!("{region}/servers.csv"),
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {region:?}: the import must start, got {} with {:?}",
        started.status, started.body
    );

    let assessment = call_in(
        router,
        region,
        "POST",
        "/start-assessment",
        Some(json!({
            "s3bucketForAnalysisData": "migrationhub-strategy-analysis",
            "assessmentTargets": [
                {"condition": "EQUALS", "name": "SERVER_ID", "values": [format!("s-{region}")]},
            ],
        })),
    )
    .await;
    assert_eq!(
        assessment.status, 200,
        "case {region:?}: the assessment must start, got {} with {:?}",
        assessment.status, assessment.body
    );

    let report = call_in(
        router,
        region,
        "POST",
        "/start-recommendation-report-generation",
        Some(json!({"outputFormat": if index.is_multiple_of(2) { "Excel" } else { "Json" }})),
    )
    .await;
    assert_eq!(
        report.status, 200,
        "case {region:?}: the report generation must start, got {} with {:?}",
        report.status, report.body
    );

    Portfolio {
        region,
        import_id: started.string("/id"),
        assessment_id: assessment.string("/assessmentId"),
        report_id: report.string("/id"),
        application_mode,
    }
}

/// Every resource is reported only through the endpoint of the Region it was created in:
/// each Region's listing holds exactly its own import task, each id resolves only there,
/// and the two per-Region singletons (the latest assessment id, the preferences record)
/// are that Region's own.
#[tokio::test]
async fn a_resource_is_reported_only_through_the_region_it_was_created_in() {
    let router = emulator();
    let mut created: Vec<Portfolio> = Vec::new();
    for index in 0..REGIONS.len() {
        created.push(create_everything_in(&router, index).await);
    }

    // No two Regions were handed the same id, so an id that resolves in the wrong Region
    // is a leak and not a collision.
    let mut minted: Vec<&String> = created
        .iter()
        .flat_map(|portfolio| {
            [
                &portfolio.import_id,
                &portfolio.assessment_id,
                &portfolio.report_id,
            ]
        })
        .collect();
    let total = minted.len();
    minted.sort();
    minted.dedup();
    assert_eq!(
        minted.len(),
        total,
        "case \"minted ids\": every resource of every Region gets its own id, got {minted:?}"
    );

    for owner in &created {
        let case = owner.region;

        // The listing of the owning Region holds exactly the one task started there.
        assert_eq!(
            listed_import_ids(&router, owner.region).await,
            vec![owner.import_id.clone()],
            "case {case:?}: the listing is the task started through this Region's endpoint"
        );

        // Its own ids resolve, and report the record this Region created.
        let import = call_in(
            &router,
            owner.region,
            "GET",
            &format!("/get-import-file-task/{}", owner.import_id),
            None,
        )
        .await;
        assert_eq!(
            import.status, 200,
            "case {case:?}: the Region that started the import reports it, got {} with {:?}",
            import.status, import.body
        );
        assert_eq!(
            import.string("/importName"),
            format!("import-from-{case}"),
            "case {case:?}: the reported import is the one started here, got {:?}",
            import.body
        );

        let latest = call_in(&router, owner.region, "GET", "/get-latest-assessment-id", None).await;
        assert_eq!(
            (latest.status, latest.string("/id")),
            (200, owner.assessment_id.clone()),
            "case {case:?}: the latest assessment id is this Region's own, got {:?}",
            latest.body
        );

        let preferences =
            call_in(&router, owner.region, "GET", "/get-portfolio-preferences", None).await;
        assert_eq!(
            (preferences.status, preferences.string("/applicationMode")),
            (200, owner.application_mode.to_string()),
            "case {case:?}: the preferences record is the one saved here, got {:?}",
            preferences.body
        );

        let details = call_in(
            &router,
            owner.region,
            "GET",
            &format!("/get-recommendation-report-details/{}", owner.report_id),
            None,
        )
        .await;
        assert_eq!(
            (details.status, details.string("/id")),
            (200, owner.report_id.clone()),
            "case {case:?}: the Region that started the report reports it, got {:?}",
            details.body
        );

        // And every OTHER Region holds none of it.
        for other in created.iter().filter(|other| other.region != owner.region) {
            let pair = format!("{} read through {}", owner.region, other.region);

            let import = call_in(
                &router,
                other.region,
                "GET",
                &format!("/get-import-file-task/{}", owner.import_id),
                None,
            )
            .await;
            assert_eq!(
                (import.status, import.error_code()),
                (404, "ResourceNotFoundException"),
                "case {pair:?}: another Region's import task id names nothing here, got {:?}",
                import.body
            );

            // `GetAssessment` refuses an id it was never issued as an invalid parameter
            // (400 `ValidationException`), which is this service's refusal for that id
            // whether it was never minted or minted in another Region.
            let assessment = call_in(
                &router,
                other.region,
                "GET",
                &format!("/get-assessment/{}", owner.assessment_id),
                None,
            )
            .await;
            assert_eq!(
                (assessment.status, assessment.error_code()),
                (400, "ValidationException"),
                "case {pair:?}: another Region's assessment id names nothing here, got {:?}",
                assessment.body
            );

            let stopped = call_in(
                &router,
                other.region,
                "POST",
                "/stop-assessment",
                Some(json!({"assessmentId": owner.assessment_id})),
            )
            .await;
            assert_eq!(
                (stopped.status, stopped.error_code()),
                (404, "ResourceNotFoundException"),
                "case {pair:?}: another Region's assessment cannot be stopped here, got {:?}",
                stopped.body
            );

            let details = call_in(
                &router,
                other.region,
                "GET",
                &format!("/get-recommendation-report-details/{}", owner.report_id),
                None,
            )
            .await;
            assert_eq!(
                (details.status, details.error_code()),
                (404, "ResourceNotFoundException"),
                "case {pair:?}: another Region's report task id names nothing here, got {:?}",
                details.body
            );

            // The other Region reports its OWN singletons, never this one's.
            let latest =
                call_in(&router, other.region, "GET", "/get-latest-assessment-id", None).await;
            assert_ne!(
                latest.string("/id"),
                owner.assessment_id,
                "case {pair:?}: the latest assessment id must not be another Region's, got {:?}",
                latest.body
            );
            let preferences =
                call_in(&router, other.region, "GET", "/get-portfolio-preferences", None).await;
            assert_eq!(
                preferences.string("/applicationMode"),
                other.application_mode.to_string(),
                "case {pair:?}: the preferences read must be this Region's own, got {:?}",
                preferences.body
            );
        }
    }
}

/// A Region nothing was created in is an EMPTY account, not a window onto another
/// Region's: the listing is empty, every id is refused, and the two singletons report
/// that nothing was saved.
#[tokio::test]
async fn a_region_nothing_was_created_in_reads_as_an_empty_account() {
    // Everything is created through ONE Region, and every OTHER Region is then read.
    for (index, owner) in REGIONS.iter().enumerate() {
        let router = emulator();
        let created = create_everything_in(&router, index).await;

        for region in REGIONS.iter().filter(|region| *region != owner) {
            let case = format!("{owner} created, {region} read");

            assert_eq!(
                listed_import_ids(&router, region).await,
                Vec::<String>::new(),
                "case {case:?}: a Region nothing was started in lists nothing"
            );

            let latest = call_in(&router, region, "GET", "/get-latest-assessment-id", None).await;
            assert_eq!(
                (latest.status, latest.error_code()),
                (400, "ValidationException"),
                "case {case:?}: no assessment was started in this Region, got {:?}",
                latest.body
            );

            let preferences =
                call_in(&router, region, "GET", "/get-portfolio-preferences", None).await;
            assert_eq!(
                (preferences.status, preferences.error_code()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: no preferences were saved in this Region, got {:?}",
                preferences.body
            );

            let servers = call_in(&router, region, "POST", "/list-servers", Some(json!({}))).await;
            assert_eq!(
                (servers.status, at(&servers.body, "/serverInfos")),
                (200, json!([])),
                "case {case:?}: a Region nothing was imported in holds no servers, got {:?}",
                servers.body
            );

            // And reading another Region left the owning Region exactly as it was.
            assert_eq!(
                listed_import_ids(&router, owner).await,
                vec![created.import_id.clone()],
                "case {case:?}: the owning Region still reports its own task"
            );
        }
    }
}

/// The one-report-at-a-time rule is per Region: a start call made in another Region while
/// a task is running does NOT coalesce onto it, it mints that Region's own task.
#[tokio::test]
async fn the_running_report_task_is_per_region() {
    let router = emulator();
    let mut ids: Vec<(&str, String)> = Vec::new();
    for region in REGIONS {
        let started = call_in(
            &router,
            region,
            "POST",
            "/start-recommendation-report-generation",
            Some(json!({})),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {region:?}: the report generation must start, got {} with {:?}",
            started.status, started.body
        );
        ids.push((region, started.string("/id")));
    }

    for (region, id) in &ids {
        // Inside the same Region a second start coalesces onto the running task.
        let again = call_in(
            &router,
            region,
            "POST",
            "/start-recommendation-report-generation",
            Some(json!({"outputFormat": "Json"})),
        )
        .await;
        assert_eq!(
            again.string("/id"),
            *id,
            "case {region:?}: a start inside one Region's running task coalesces onto it, got {:?}",
            again.body
        );
        // Across Regions the tasks are distinct records.
        for (other, other_id) in ids.iter().filter(|(other, _)| other != region) {
            assert_ne!(
                other_id, id,
                "case {region:?}/{other:?}: each Region runs its own report task"
            );
        }
    }
}

/// The Region comes from the caller's own credential scope, so the number of portfolios
/// the store keeps is bounded: a write that would be the first thing stored in a further
/// Region is refused, while every Region already holding something keeps working.
#[tokio::test]
async fn the_number_of_region_portfolios_is_bounded() {
    let router = emulator();
    for index in 0..MAX_REGIONS {
        let region = format!("us-test-{index}");
        let started = call_in(
            &router,
            &region,
            "POST",
            "/start-import-file-task",
            Some(json!({
                "name": format!("import-{index}"),
                "S3Bucket": "migrationhub-strategy-import",
                "s3key": "servers.csv",
            })),
        )
        .await;
        assert_eq!(
            started.status,
            200,
            "case {region:?} (portfolio {}/{MAX_REGIONS}): the import must start, got {} with {:?}",
            index + 1,
            started.status,
            started.body
        );
    }

    // One Region past the ceiling: refused, through the operation's own modelled quota
    // error, rather than minting a further portfolio.
    let over = format!("us-test-{MAX_REGIONS}");
    let refused = call_in(
        &router,
        &over,
        "POST",
        "/start-import-file-task",
        Some(json!({
            "name": "import-over",
            "S3Bucket": "migrationhub-strategy-import",
            "s3key": "servers.csv",
        })),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (402, "ServiceQuotaExceededException"),
        "case {over:?} (one past {MAX_REGIONS}): expected the quota refusal, got {:?}",
        refused.body
    );

    // And a Region that already holds a portfolio is not refused by that ceiling.
    let again = call_in(
        &router,
        "us-test-0",
        "POST",
        "/start-import-file-task",
        Some(json!({
            "name": "import-again",
            "S3Bucket": "migrationhub-strategy-import",
            "s3key": "servers.csv",
        })),
    )
    .await;
    assert_eq!(
        again.status, 200,
        "case \"us-test-0 again\": a Region the store already holds still accepts a start, got {} \
         with {:?}",
        again.status, again.body
    );
    assert_eq!(
        listed_import_ids(&router, "us-test-0").await.len(),
        2,
        "case \"us-test-0 again\": both of that Region's tasks are listed there"
    );
    assert_eq!(
        listed_import_ids(&router, &over).await,
        Vec::<String>::new(),
        "case {over:?}: the refused start stored nothing"
    );
}

/// The server inventory, and the operations that read and write one server of it, are
/// scoped the same way: a server of one Region's portfolio is not listed, read, updated
/// or reported through another Region's endpoint.
#[tokio::test]
async fn the_server_inventory_is_scoped_to_one_region() {
    // One server per Region, each with the id its Region's reads must report.
    let seeded: Vec<(&str, Vec<ServerRecord>)> = REGIONS
        .iter()
        .enumerate()
        .map(|(index, region)| {
            (
                *region,
                vec![ServerRecord {
                    id: format!("{index}a"),
                    name: Some(format!("server-in-{region}")),
                    ..ServerRecord::default()
                }],
            )
        })
        .collect();
    let router = emulator_with_servers_per_region(&seeded);

    for (region, servers) in &seeded {
        let own = servers[0].id.clone();
        let listed = call_in(&router, region, "POST", "/list-servers", Some(json!({}))).await;
        assert_eq!(
            (listed.status, at(&listed.body, "/serverInfos/0/id")),
            (200, json!(own)),
            "case {region:?}: the listing is this Region's own inventory, got {:?}",
            listed.body
        );
        assert_eq!(
            at(&listed.body, "/serverInfos")
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1,
            "case {region:?}: no other Region's server is listed, got {:?}",
            listed.body
        );

        for (other, other_servers) in seeded.iter().filter(|(other, _)| other != region) {
            let foreign = other_servers[0].id.clone();
            let pair = format!("{other}'s server read through {region}");

            let details = call_in(
                &router,
                region,
                "GET",
                &format!("/get-server-details/{foreign}"),
                None,
            )
            .await;
            assert_eq!(
                (details.status, details.error_code()),
                (404, "ResourceNotFoundException"),
                "case {pair:?}: another Region's server id names nothing here, got {:?}",
                details.body
            );

            let strategies = call_in(
                &router,
                region,
                "GET",
                &format!("/get-server-strategies/{foreign}"),
                None,
            )
            .await;
            assert_eq!(
                (strategies.status, strategies.error_code()),
                (404, "ResourceNotFoundException"),
                "case {pair:?}: another Region's server has no strategies here, got {:?}",
                strategies.body
            );

            let updated = call_in(
                &router,
                region,
                "POST",
                "/update-server-config/",
                Some(json!({
                    "serverId": foreign,
                    "strategyOption": {"strategy": "Rehost", "isPreferred": true},
                })),
            )
            .await;
            assert_eq!(
                (updated.status, updated.error_code()),
                (404, "ResourceNotFoundException"),
                "case {pair:?}: another Region's server cannot be configured here, got {:?}",
                updated.body
            );
        }
    }
}
