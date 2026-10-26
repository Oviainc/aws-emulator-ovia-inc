// SPDX-License-Identifier: CC0-1.0

//! The one `maxResults` page-bound rule this service enforces, driven end to end over HTTP
//! through every operation that takes a page size.
//!
//! The rule under test is the one the service states for ANY of its lists, not for one
//! request: an absent `maxResults` takes the documented maximum of 100, a value inside
//! 1..=100 is the number of records the page holds, and anything else is refused with 400
//! `ValidationException` carrying this service's sentence for a value it cannot act on —
//! the same decision whichever list is asked, rather than one decision per list.
//!
//! The table below therefore spans the class the rule generalises over in two directions
//! at once: every operation of this service that takes a `maxResults` — the servers, the
//! application components, the import file tasks, the collectors, one server's associated
//! applications and the analyzable servers — crossed with the page sizes the rule
//! distinguishes (absent, the minimum, values inside the range, the documented maximum) and
//! with the values outside it (zero, negative, one past the maximum, far past it, and each
//! end of the request member's own `integer` range).
//!
//! Every list of the fixture holds one record MORE than the documented maximum, so a page
//! that takes the default leaves a record behind and the default cannot pass by being large
//! enough to hold the whole list. A server, a component, a collector and an import file
//! task each enter this service through paths filed separately (an import file task's
//! ingestion, a collector's own registration), so these cases seed the records directly
//! through the shared engine — the same maps those paths write and these operations read,
//! through the harness the suites beside this one use — and then drive every assertion
//! through the router, so each handler, its refusal and its serialized page are exercised
//! exactly as a client reaches them.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::ops::types::AssociatedApplication;
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ApplicationComponentRecord, CollectorRecord,
    ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState, ImportFileTask,
    ServerDiscovery, ServerRecord,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{at, call, Answer, TEST_REGION};

/// The documented maximum of `maxResults`, which is also what an absent `maxResults` takes:
/// "The maximum number of items to include in the response. The maximum value is 100."
/// (`smithy_query(query="operation",operation="ListServers,ListApplicationComponents,ListCollectors,ListImportFileTask,ListAnalyzableServers,GetServerDetails")`
/// -> `maxResults`). Spelled out independently of the crate's constant, so a wrong bound in
/// `src/` fails here rather than agreeing with itself.
const MAX_RESULTS_MAX: i64 = 100;

/// This service's sentence for a value it cannot act on, spelled out for the same reason.
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// The terminal `ImportFileTaskStatus` the seeded import file tasks carry
/// (`smithy_query(query="shape",shape="ImportFileTaskStatus")`).
const IMPORT_SUCCESS: &str = "ImportSuccess";

/// One value of `DataSourceType`, which `AnalyzableServerSummary$source` reports as stored
/// (`smithy_query(query="shape",shape="DataSourceType")`).
const DATA_SOURCE_TYPE: &str = "StrategyRecommendationsApplicationDataCollector";

/// How many records every list of the fixture holds: one MORE than the documented maximum,
/// so the default page leaves a record behind.
const RECORDS: usize = MAX_RESULTS_MAX as usize + 1;

/// The index of the server whose associated applications `GetServerDetails` pages over.
const DETAIL_SERVER: usize = 0;

/// The key the fixture's `index`-th record is seeded under, zero padded so the store's
/// ascending order over the keys is the order the indexes count in.
///
/// The digits are inside `ResourceId`'s `^[0-9a-b]+` (max 44), which both a `serverId` and
/// an application component id target.
fn key(index: usize) -> String {
    format!("{index:04}")
}

/// One server of the fixture, carrying collector discovery data so the same record is also
/// an analyzable server — one seeded inventory feeds both lists.
fn fixture_server(index: usize) -> ServerRecord {
    ServerRecord {
        id: key(index),
        name: Some(format!("srv-{index}")),
        os_type: Some(if index.is_multiple_of(2) { "LINUX" } else { "WINDOWS" }.to_string()),
        discovery: Some(ServerDiscovery {
            hostname: Some(format!("host-{index}")),
            ip_address: Some(format!("10.0.{}.{}", index / 256, index % 256)),
            source: Some(DATA_SOURCE_TYPE.to_string()),
            vm_id: Some(format!("vm-{index}")),
        }),
        // Only the server `GetServerDetails` is asked about holds associated applications:
        // that list is what its page is taken from.
        associated_applications: if index == DETAIL_SERVER {
            (0..RECORDS)
                .map(|app| AssociatedApplication {
                    name: Some(format!("app-{app}")),
                    id: Some(format!("d-application-{app:04}")),
                })
                .collect()
        } else {
            Vec::new()
        },
        ..Default::default()
    }
}

/// One application component of the fixture.
fn fixture_component(index: usize) -> ApplicationComponentRecord {
    ApplicationComponentRecord {
        id: key(index),
        name: Some(format!("component-{index}")),
        ..Default::default()
    }
}

/// One collector of the fixture; `collectorId` is a `.*\S.*` string, so it carries a prefix
/// of its own.
fn fixture_collector(index: usize) -> CollectorRecord {
    CollectorRecord {
        id: format!("c{index:04}"),
        host_name: Some(format!("collector-{index}")),
        ..Default::default()
    }
}

/// One completed import file task of the fixture; its id is an `AsyncTaskId`
/// (`[0-9a-z-:]+`, max 52).
fn fixture_import_task(index: usize) -> ImportFileTask {
    let started = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + index as u64);
    ImportFileTask {
        id: format!("t{index:04}"),
        import_name: format!("import-{index}"),
        input_s3_bucket: "migrationhub-strategy-import".to_string(),
        input_s3_key: format!("servers-{index}.csv"),
        data_source_type: None,
        group_id: Vec::new(),
        status_report_s3_bucket: None,
        status_report_s3_key: None,
        status: IMPORT_SUCCESS.to_string(),
        start_time: started,
        completion_time: Some(started + Duration::from_secs(60)),
        number_of_records_success: 1,
        number_of_records_failed: 0,
        completion_due: None,
    }
}

/// One emulator over one shared engine, holding [`RECORDS`] servers (each of them an
/// analyzable server, the first of them carrying that many associated applications), that
/// many application components, collectors and import file tasks — seeded into the same maps
/// the paths that create them write and these operations read.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let portfolio = store.region_entry(TEST_REGION);
        for index in 0..RECORDS {
            let server = fixture_server(index);
            portfolio.servers.insert(server.id.clone(), server);
            let component = fixture_component(index);
            portfolio
                .application_components
                .insert(component.id.clone(), component);
            let collector = fixture_collector(index);
            portfolio.collectors.insert(collector.id.clone(), collector);
            let task = fixture_import_task(index);
            portfolio.import_tasks.insert(task.id.clone(), task);
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Every list of this service a `maxResults` is asked of.
#[derive(Clone, Copy, Debug)]
enum Listing {
    Servers,
    ApplicationComponents,
    ImportFileTasks,
    Collectors,
    ServerApplications,
    AnalyzableServers,
}

/// Each list, named for the operation that serves it.
const LISTINGS: &[Listing] = &[
    Listing::Servers,
    Listing::ApplicationComponents,
    Listing::ImportFileTasks,
    Listing::Collectors,
    Listing::ServerApplications,
    Listing::AnalyzableServers,
];

impl Listing {
    /// The paginated output member this list reports its page in, as a JSON pointer
    /// (the `@paginated(items=...)` member of each operation, from `OPERATIONS.md`).
    fn items(self) -> &'static str {
        match self {
            Self::Servers => "/serverInfos",
            Self::ApplicationComponents => "/applicationComponentInfos",
            Self::ImportFileTasks => "/taskInfos",
            Self::Collectors => "/Collectors",
            Self::ServerApplications => "/associatedApplications",
            Self::AnalyzableServers => "/analyzableServers",
        }
    }
}

/// One page of `listing`, asking for `max_results` (absent when `None`) and resuming from
/// `next_token`.
async fn page(
    router: &MultiServiceRouter,
    listing: Listing,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Answer {
    match listing {
        Listing::Servers => {
            call(router, "POST", "/list-servers", Some(body(max_results, next_token))).await
        }
        Listing::ApplicationComponents => {
            call(
                router,
                "POST",
                "/list-applicationcomponents",
                Some(body(max_results, next_token)),
            )
            .await
        }
        Listing::AnalyzableServers => {
            call(
                router,
                "POST",
                "/list-analyzable-servers",
                Some(body(max_results, next_token)),
            )
            .await
        }
        Listing::ImportFileTasks => {
            let uri = with_query("/list-import-file-task", max_results, next_token);
            call(router, "GET", &uri, None).await
        }
        Listing::Collectors => {
            let uri = with_query("/list-collectors", max_results, next_token);
            call(router, "GET", &uri, None).await
        }
        Listing::ServerApplications => {
            let uri = with_query(
                &format!("/get-server-details/{}", key(DETAIL_SERVER)),
                max_results,
                next_token,
            );
            call(router, "GET", &uri, None).await
        }
    }
}

/// The request body a list taking its members in the payload carries.
fn body(max_results: Option<i64>, next_token: Option<&str>) -> Value {
    let mut request = json!({});
    if let Some(max_results) = max_results {
        request["maxResults"] = json!(max_results);
    }
    if let Some(token) = next_token {
        request["nextToken"] = json!(token);
    }
    request
}

/// `path` with the query members a list taking them in the URI carries.
fn with_query(path: &str, max_results: Option<i64>, next_token: Option<&str>) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", query.join("&"))
    }
}

/// How many records the page reported.
fn reported(answer: &Answer, listing: Listing) -> usize {
    at(&answer.body, listing.items())
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// A `maxResults` inside the documented range is how many records the page holds, on every
/// list of this service: the one bound, asked of each operation that takes a page size.
#[tokio::test]
async fn an_accepted_max_results_is_the_size_of_the_page() {
    let router = emulator();
    for listing in LISTINGS {
        // The minimum a page can hold, values inside the range, and the documented maximum.
        for size in [1i64, 2, 7, 99, MAX_RESULTS_MAX] {
            let what = format!("{listing:?} asked for a page of {size}");
            let answer = page(&router, *listing, Some(size), None).await;
            assert_eq!(
                answer.status,
                200,
                "case {what:?}: expected HTTP 200, got {} with {:?}",
                answer.status,
                answer.body
            );
            let held = reported(&answer, *listing);
            assert_eq!(
                held,
                size as usize,
                "case {what:?}: expected the page to hold {size} of the {RECORDS} records, got \
                 {held}"
            );
            // The fixture holds more records than any accepted page takes, so another record
            // always follows the page and a cursor must come with it.
            let token = at(&answer.body, "/nextToken");
            assert!(
                token.is_string(),
                "case {what:?}: expected a nextToken while records remain, got {token:?}"
            );
        }
    }
}

/// An absent `maxResults` takes the documented maximum, on every list of this service — not
/// the whole list and not a size of the list's own.
#[tokio::test]
async fn an_absent_max_results_takes_the_documented_maximum() {
    let router = emulator();
    for listing in LISTINGS {
        let what = format!("{listing:?} asked with no maxResults at all");
        let answer = page(&router, *listing, None, None).await;
        assert_eq!(
            answer.status,
            200,
            "case {what:?}: expected HTTP 200, got {} with {:?}",
            answer.status,
            answer.body
        );
        let held = reported(&answer, *listing);
        assert_eq!(
            held,
            MAX_RESULTS_MAX as usize,
            "case {what:?}: expected the documented maximum of {MAX_RESULTS_MAX} records out of \
             the {RECORDS} held, got {held}"
        );
        let token = at(&answer.body, "/nextToken");
        assert!(
            token.is_string(),
            "case {what:?}: expected a nextToken for the record the default page left behind, \
             got {token:?}"
        );
    }
}

/// The accepted size applies to a page RESUMED from a cursor exactly as it does to the
/// first page, on every list of this service.
#[tokio::test]
async fn a_resumed_page_takes_the_same_accepted_size() {
    let router = emulator();
    for listing in LISTINGS {
        let size = 7i64;
        let what = format!("{listing:?} resumed with a page of {size}");
        let first = page(&router, *listing, Some(size), None).await;
        let token = first.string("/nextToken");
        assert!(
            !token.is_empty(),
            "case {what:?}: the first page must hand out a cursor, got {:?}",
            first.body
        );

        let second = page(&router, *listing, Some(size), Some(&token)).await;
        assert_eq!(
            second.status,
            200,
            "case {what:?}: expected HTTP 200 on the resumed page, got {} with {:?}",
            second.status,
            second.body
        );
        let held = reported(&second, *listing);
        assert_eq!(
            held, size as usize,
            "case {what:?}: expected the resumed page to hold {size} records, got {held}"
        );
    }
}

/// A `maxResults` outside the documented range is refused — never clamped to the maximum
/// and never taken as "no limit" — with the same status, error and sentence on every list of
/// this service.
#[tokio::test]
async fn a_max_results_outside_the_documented_range_is_refused() {
    let router = emulator();
    for listing in LISTINGS {
        for (edge, max_results) in [
            ("zero, which asks for a page of nothing", 0i64),
            ("one below zero", -1),
            ("one past the documented maximum", MAX_RESULTS_MAX + 1),
            ("far past the documented maximum", 1_000),
            ("the largest value the member admits", i64::from(i32::MAX)),
            ("the smallest value the member admits", i64::from(i32::MIN)),
        ] {
            let what = format!("{listing:?} asked for a maxResults of {max_results} ({edge})");
            let answer = page(&router, *listing, Some(max_results), None).await;
            assert_eq!(
                answer.status,
                400,
                "case {what:?}: expected HTTP 400, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {what:?}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.string("/message"),
                INVALID_REQUEST_BODY,
                "case {what:?}: expected the service's {INVALID_REQUEST_BODY:?}, got {:?}",
                answer.body
            );
            assert_eq!(
                at(&answer.body, listing.items()),
                Value::Null,
                "case {what:?}: expected a refusal to report no records at all, got {:?}",
                answer.body
            );
        }
    }
}
