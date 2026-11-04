// SPDX-License-Identifier: CC0-1.0

//! The wire form of the instants a LISTING reports INSIDE its items.
//!
//! The rule under test: a member targeting `com.amazonaws.appconfig#Iso8601DateTime`
//! (`@timestampFormat("date-time")`, `smithy_query(query="shape", shape="Iso8601DateTime")`)
//! is an RFC 3339 / ISO-8601 date-time STRING on the restJson1 wire, never the epoch-seconds
//! JSON number the protocol falls back to for a member schema carrying no format — in every
//! document that reports one, so in every ITEM of every PAGE of every listing as much as in
//! the single-resource document. A client that decodes a list page against the model errors
//! or reads a nonsense instant otherwise, and a list page is the normal discovery path.
//!
//! The existing suites pin the first page of one listing under one filter
//! (`experiment_definition_wire_fidelity.rs`, `experiment_run_wire_fidelity.rs`,
//! `list_deployments.rs`); what this one pins is the rest of the class the rule covers, as a
//! table spanning
//!
//! * WHICH listing, and so which item shape and which of its members carry an instant:
//!   `ExperimentDefinitionSummary` (`CreatedAt`, `UpdatedAt`), `ExperimentRunSummary`
//!   (`StartedAt`, `UpdatedAt`, `EndedAt`) and `ExperimentRunEvent` (`OccurredAt`);
//! * WHICH narrowing the request asks for, in each addressing form a filter accepts (an id,
//!   a name) and for each status the stored items actually hold;
//! * WHICH page size the request asks for — unasked-for, the published bounds of
//!   `MaxResults` and one in between — so an item served on a later page is asserted too;
//! * WHETHER the item's optional instant is there at all: a run still in flight reports no
//!   `EndedAt`, a finished one does.
//!
//! A page is compared member-for-member against the unpaged listing of the same filter, so
//! paging cannot change an item or the form of its instants, and every instant a listing
//! reports is compared against the single-resource reader's — which is what makes these
//! reports over stored state rather than values rendered at listing time.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::MAX_LIST_PAGE_SIZE;

use crate::appconfig_workflow::{router, Answer};
use crate::experiment_definitions::{
    body as definition_body, create_definition, flag, get_definition, scope, treatment, Scope,
};
use crate::list_experiment_definitions::list as list_definitions;
use crate::list_experiment_run_events::list_events;
use crate::list_experiment_runs::list as list_runs;
use crate::resource_tags::pct;
use crate::start_experiment_run::{get_run, start_run};
use crate::stop_experiment_run::stop_run;
use crate::update_experiment_run::update_run;

/// The label every resource of this suite's world is named from.
const LABEL: &str = "instant-wire";

/// How many experiment definitions the world holds: more than one page at the smaller page
/// sizes below, so a walk has several pages to compare.
const DEFINITIONS: usize = 5;

/// The published maximum of `MaxResults` on each of these listings (`range 1..50`,
/// `smithy_query(query="operation", operation="ListExperimentDefinitions")`), read off the
/// service's own page-size constant so a case cannot drift from it.
const MAX_RESULTS_MAX: i64 = MAX_LIST_PAGE_SIZE as i64;

/// The page sizes every walk is repeated at: unasked-for (the service's own default), the
/// published minimum, one in between, and the published maximum.
const PAGE_SIZES: [Option<i64>; 4] = [None, Some(1), Some(2), Some(MAX_RESULTS_MAX)];

/// The run of the world's definition whose timeline the event listing reports.
const TIMELINE_RUN: i64 = 1;

/// The listings of this service whose item shape carries a member targeting
/// `Iso8601DateTime`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Listing {
    Definitions,
    Runs,
    RunEvents,
}

impl Listing {
    /// The operation a failing assertion names.
    fn operation(self) -> &'static str {
        match self {
            Listing::Definitions => "ListExperimentDefinitions",
            Listing::Runs => "ListExperimentRuns",
            Listing::RunEvents => "ListExperimentRunEvents",
        }
    }

    /// The members of this listing's item shape that target `Iso8601DateTime`, from
    /// `smithy_query(query="shape", shape="ExperimentDefinitionSummary")`,
    /// `shape="ExperimentRunSummary"` and `shape="ExperimentRunEvent"`.
    fn instant_members(self) -> &'static [&'static str] {
        match self {
            Listing::Definitions => &["CreatedAt", "UpdatedAt"],
            Listing::Runs => &["StartedAt", "UpdatedAt", "EndedAt"],
            Listing::RunEvents => &["OccurredAt"],
        }
    }

    /// Whether the request can narrow this listing by `Status`: the two listings whose item
    /// shape carries one do, and the event feed — whose items carry an `EventType` instead —
    /// does not.
    fn filters_on_status(self) -> bool {
        match self {
            Listing::Definitions | Listing::Runs => true,
            Listing::RunEvents => false,
        }
    }
}

/// The resources every case below reports over, and the two names a filter can address them
/// by as well as by their ids.
struct World {
    scope: Scope,
    application_name: String,
    environment_name: String,
    /// The definition holding the run history and the timeline the run listings report.
    run_definition: String,
}

/// One application holding one environment, one configuration profile and
/// [`DEFINITIONS`] experiment definitions, the first of which has a run history: two runs
/// taken to their end (so each reports an `EndedAt`) and one left in flight (so it reports
/// none), the first of them also updated, which puts several events on its timeline.
async fn world(router: &MultiServiceRouter) -> World {
    let scope = scope(router, LABEL).await;

    let mut definitions: Vec<String> = Vec::new();
    for index in 0..DEFINITIONS {
        let name = format!("definition-{index}");
        let created = create_definition(
            router,
            &scope.application,
            &definition_body(
                &scope,
                &name,
                vec![treatment(50.0, Some("the new flow"), flag(true))],
                treatment(50.0, Some("today's experience"), flag(false)),
            ),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "seeding {name:?}: CreateExperimentDefinition must answer 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        definitions.push(created.string("Id").unwrap_or_else(|| {
            panic!("seeding {name:?}: the create answered no Id: {}", created.json())
        }));
    }

    let run_definition = definitions
        .first()
        .cloned()
        .expect("the world holds at least one definition");

    // Runs 1 and 2 are taken to their end; run 3 is left in flight. A run is started only
    // once the one before it has ended, which is the order a definition serves runs in.
    for run in [1_i64, 2, 3] {
        let started = start_run(
            router,
            &scope.application,
            &run_definition,
            &json!({ "ExposurePercentage": 10.0 }),
        )
        .await;
        assert_eq!(
            started.status,
            201,
            "seeding run {run}: StartExperimentRun must answer 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        assert_eq!(
            started.json().get("Run").and_then(Value::as_i64),
            Some(run),
            "seeding run {run}: expected run {run}, got {}",
            started.json()
        );

        if run == TIMELINE_RUN {
            let updated = update_run(
                router,
                &scope.application,
                &run_definition,
                run,
                &json!({ "ExposurePercentage": 25.0 }),
            )
            .await;
            assert_eq!(
                updated.status,
                200,
                "seeding run {run}: UpdateExperimentRun must answer 200, got {} / {}",
                updated.status,
                String::from_utf8_lossy(&updated.body)
            );
        }

        if run == 3 {
            break;
        }

        let stopped = stop_run(
            router,
            &scope.application,
            &run_definition,
            run,
            &json!({ "Result": { "ExecutiveSummary": format!("run {run} concluded") } }),
        )
        .await;
        assert_eq!(
            stopped.status,
            200,
            "seeding run {run}: StopExperimentRun must answer 200, got {} / {}",
            stopped.status,
            String::from_utf8_lossy(&stopped.body)
        );
    }

    World {
        application_name: format!("app-{LABEL}"),
        environment_name: format!("env-{LABEL}"),
        scope,
        run_definition,
    }
}

/// One page of `listing`, under `filter`, at `page_size`, resuming from `token`.
///
/// `max_results` and `next_token` are `httpQuery` members of each of these operations, so
/// the page a case asks for is asked for the way a client asks for it.
async fn fetch(
    router: &MultiServiceRouter,
    listing: Listing,
    world: &World,
    filter: &[(&'static str, String)],
    page_size: Option<i64>,
    token: Option<&str>,
) -> Answer {
    let mut query: Vec<(&str, String)> = filter.to_vec();
    if let Some(size) = page_size {
        query.push(("max_results", size.to_string()));
    }
    if let Some(token) = token {
        query.push(("next_token", token.to_string()));
    }
    match listing {
        Listing::Definitions => list_definitions(router, &query).await,
        Listing::Runs => {
            list_runs(
                router,
                &world.scope.application,
                &world.run_definition,
                &query,
            )
            .await
        }
        Listing::RunEvents => {
            let rendered: Vec<String> = query
                .iter()
                .map(|(name, value)| format!("{name}={}", pct(value)))
                .collect();
            list_events(
                router,
                &world.scope.application,
                &world.run_definition,
                TIMELINE_RUN,
                &rendered.join("&"),
            )
            .await
        }
    }
}

/// Every item `listing` reports under `filter` at `page_size`, in the order it served them:
/// the walk follows each `NextToken` until the listing hands none back.
async fn walk(
    router: &MultiServiceRouter,
    listing: Listing,
    world: &World,
    case: &str,
    filter: &[(&'static str, String)],
    page_size: Option<i64>,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let answer = fetch(router, listing, world, filter, page_size, token.as_deref()).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: {} must answer 200 for page {pages}, got {} / {}",
            listing.operation(),
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let body = answer.json();
        let items = body
            .get("Items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {} must report an Items array on page {pages}, got {body}",
                    listing.operation()
                )
            });
        collected.extend(items);
        token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        pages += 1;
        if token.is_none() {
            break;
        }
        assert!(
            pages <= 64,
            "case {case:?}: {} handed out a NextToken on every one of {pages} pages",
            listing.operation()
        );
    }
    collected
}

/// Everything the rule requires of ONE item's instants: each member of the item shape that
/// targets `Iso8601DateTime` is either absent — the instant the record does not hold — or a
/// UTC RFC 3339 date-time STRING, and an item of these shapes always holds at least one.
fn assert_item_instants(case: &str, listing: Listing, index: usize, item: &Value) {
    let mut reported = 0usize;
    for name in listing.instant_members() {
        let value = match item.get(*name) {
            None | Some(Value::Null) => continue,
            Some(value) => value,
        };
        let text = value.as_str().unwrap_or_else(|| {
            panic!(
                "case {case:?}: {}'s Items[{index}].{name} targets Iso8601DateTime, so it is an \
                 ISO-8601 date-time string on the wire, got {value}",
                listing.operation()
            )
        });
        DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: {}'s Items[{index}].{name} must parse as an RFC 3339 date-time, \
                 got {text:?}: {error}",
                listing.operation()
            )
        });
        assert!(
            text.ends_with('Z'),
            "case {case:?}: {}'s Items[{index}].{name} must be a UTC instant, got {text:?}",
            listing.operation()
        );
        reported += 1;
    }
    assert!(
        reported > 0,
        "case {case:?}: {}'s Items[{index}] must report at least one instant, got {item}",
        listing.operation()
    );
}

/// The distinct `Status` values the stored items actually hold, so a status row cannot name
/// one nothing is in.
fn distinct_statuses(items: &[Value]) -> Vec<String> {
    let mut statuses: Vec<String> = items
        .iter()
        .filter_map(|item| item.get("Status").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    statuses.sort();
    statuses.dedup();
    statuses
}

/// The narrowings each listing is walked under: the listing that reports the whole set,
/// then one row per addressing form each identifier filter accepts, and one per status the
/// items hold.
fn filter_rows(
    listing: Listing,
    world: &World,
    statuses: &[String],
) -> Vec<(String, Vec<(&'static str, String)>)> {
    let mut rows: Vec<(String, Vec<(&'static str, String)>)> = match listing {
        Listing::Definitions => vec![
            ("no filter — every definition the account holds".to_string(), Vec::new()),
            (
                "the application by id".to_string(),
                vec![("application_identifier", world.scope.application.clone())],
            ),
            (
                "the application by name".to_string(),
                vec![("application_identifier", world.application_name.clone())],
            ),
            (
                "the configuration profile by id, under the application".to_string(),
                vec![
                    ("application_identifier", world.scope.application.clone()),
                    ("configuration_profile_identifier", world.scope.profile.clone()),
                ],
            ),
            (
                "the environment by name".to_string(),
                vec![("environment_identifier", world.environment_name.clone())],
            ),
        ],
        Listing::Runs => vec![("every run of the definition".to_string(), Vec::new())],
        Listing::RunEvents => vec![("every event of the run".to_string(), Vec::new())],
    };
    for status in statuses {
        rows.push((
            format!("the items in status {status}"),
            vec![("status", status.clone())],
        ));
    }
    rows
}

/// Every item of every page of every listing reports its instants as ISO-8601 strings, and
/// paging changes neither the items nor their wire form.
#[tokio::test]
async fn every_page_of_every_listing_reports_item_instants_as_iso_8601_strings() {
    let router = router();
    let world = world(&router).await;

    for listing in [Listing::Definitions, Listing::Runs, Listing::RunEvents] {
        let unfiltered_case = format!("{}, unfiltered", listing.operation());
        let unfiltered = walk(&router, listing, &world, &unfiltered_case, &[], None).await;
        assert!(
            !unfiltered.is_empty(),
            "case {unfiltered_case:?}: the world holds items for this listing, got none"
        );
        let statuses = if listing.filters_on_status() {
            distinct_statuses(&unfiltered)
        } else {
            Vec::new()
        };

        for (narrowing, filter) in filter_rows(listing, &world, &statuses) {
            let case = format!("{}, {narrowing}", listing.operation());
            let whole = walk(&router, listing, &world, &case, &filter, None).await;
            assert!(
                !whole.is_empty(),
                "case {case:?}: this narrowing must still report items, got none"
            );
            for (index, item) in whole.iter().enumerate() {
                assert_item_instants(&case, listing, index, item);
            }

            for page_size in PAGE_SIZES {
                let paged_case = format!("{case}, at page size {page_size:?}");
                let paged = walk(&router, listing, &world, &paged_case, &filter, page_size).await;
                for (index, item) in paged.iter().enumerate() {
                    assert_item_instants(&paged_case, listing, index, item);
                }
                assert_eq!(
                    paged,
                    whole,
                    "case {paged_case:?}: paging must report the same items in the same form as \
                     the unpaged listing, got {paged:?} for {whole:?}"
                );
            }
        }
    }
}

/// Each instant a listing reports is the one the single-resource reader reports for that
/// same record, in the same wire form — so the summary is a report over stored state rather
/// than a value rendered at listing time, and an absent instant is absent on both paths.
#[tokio::test]
async fn a_listing_reports_the_instants_the_single_resource_reader_reports() {
    let router = router();
    let world = world(&router).await;

    let definitions = walk(
        &router,
        Listing::Definitions,
        &world,
        "every definition",
        &[],
        None,
    )
    .await;
    assert_eq!(
        definitions.len(),
        DEFINITIONS,
        "case \"every definition\": expected {DEFINITIONS} summaries, got {}",
        definitions.len()
    );
    for summary in &definitions {
        let id = summary
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("a summary carries no Id: {summary}"));
        let case = format!("definition {id}");
        let read = get_definition(&router, &world.scope.application, id).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetExperimentDefinition must answer 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let read = read.json();
        for name in Listing::Definitions.instant_members() {
            assert_eq!(
                summary.get(*name),
                read.get(*name),
                "case {case:?}: the summary must report the {name} the reader does, got {:?} for \
                 {:?}",
                summary.get(*name),
                read.get(*name)
            );
        }
    }

    let runs = walk(&router, Listing::Runs, &world, "every run", &[], None).await;
    assert!(
        runs.len() > 1,
        "case \"every run\": the definition holds a run history, got {} run(s)",
        runs.len()
    );
    for summary in &runs {
        let number = summary
            .get("Run")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("a run summary carries no Run: {summary}"));
        let case = format!("run {number}");
        let read = get_run(
            &router,
            &world.scope.application,
            &world.run_definition,
            number,
        )
        .await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetExperimentRun must answer 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let read = read.json();
        for name in Listing::Runs.instant_members() {
            assert_eq!(
                summary.get(*name),
                read.get(*name),
                "case {case:?}: the summary must report the {name} the reader does, got {:?} for \
                 {:?}",
                summary.get(*name),
                read.get(*name)
            );
        }
    }
}
