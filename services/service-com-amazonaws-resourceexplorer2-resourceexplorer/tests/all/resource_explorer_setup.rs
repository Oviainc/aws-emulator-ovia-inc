// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the multi-Region Resource Explorer setup slice:
//! `CreateResourceExplorerSetup` and the `GetResourceExplorerSetup` report it is
//! read back through.
//!
//! Every assertion here goes over the wire: the suite mounts this service on a
//! [`MultiServiceRouter`] over ONE shared engine and POSTs restJson1 requests at it,
//! so nothing is proved about a handler that is not also proved about the serialized
//! request and response an SDK sends and receives.
//!
//! The rules under test, each over a TABLE of inputs rather than one request:
//!
//! * a setup call configures every Region it names — each gets its own user-owned
//!   index and the named view — and the task it answers with reports each Region's
//!   two steps, reaching its terminal status only once the resources those steps
//!   are about are themselves usable;
//! * what a Region already holds is not created twice: the step for it is `SKIPPED`
//!   and names the resource that was already there;
//! * every reported value comes from the store, so each index and view the task
//!   created round-trips through `GetIndex` / `GetView`; and
//! * the report is paginated like every other `@paginated` operation, and bound to
//!   the task it was issued for.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Comfortably past the two dependency levels of a setup task — the Region's index
/// coming up, and then the view behind that index — each one
/// `crate::state::INDEX_SETTLE_DELAY` long, so a task left in `IN_PROGRESS` is
/// certain to have reached its terminal status by the next call.
const PAST_SETUP: Duration = Duration::from_millis(1_400);

/// The published bounds of `GetResourceExplorerSetupInput$MaxResults`
/// (`smithy_query(query="operation",operation="GetResourceExplorerSetup")` reports
/// `range {min: 1, max: 100}`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// One emulator: the service mounted on a router over a single shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

/// What one call answered: the HTTP status, the restJson1 error code (the
/// `x-amzn-errortype` header), and the decoded JSON body.
struct Answer {
    status: u16,
    error: Option<String>,
    body: Value,
}

impl Answer {
    /// The body of a call that had to succeed.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({:?}) with body {}",
            self.status, self.error, self.body
        );
        &self.body
    }

    /// Assert this call failed with `status` and the error code `code`.
    fn failed(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error.as_deref()),
            (status, Some(code)),
            "case {case:?}: expected HTTP {status} {code}, got {} ({:?}) with body {}",
            self.status,
            self.error,
            self.body
        );
    }

    /// The error's `Message` member, however the envelope cases it.
    fn error_message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_resourceexplorer2_resourceexplorer::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// POST `/{operation}` as a client signed for `region` would.
    async fn call(&self, region: &str, operation: &str, body: Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://resource-explorer-2.{region}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}\
                     /resource-explorer-2/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error,
            body,
        }
    }

    /// `CreateResourceExplorerSetup`, asserting success, and the `TaskId` it answered
    /// with.
    async fn create_setup(&self, case: &str, body: Value) -> String {
        let answer = self
            .call("us-east-1", "CreateResourceExplorerSetup", body)
            .await;
        let task_id = answer.ok(case)["TaskId"]
            .as_str()
            .expect("CreateResourceExplorerSetup answers a TaskId")
            .to_string();
        assert!(
            is_modelled_task_id(&task_id),
            "case {case:?}: expected a TaskId of the modelled form, got {task_id:?}"
        );
        task_id
    }

    /// Every `RegionStatus` of `task_id`, read through as many pages as it takes.
    async fn setup_regions(&self, case: &str, task_id: &str, max_results: Option<i64>) -> Vec<Value> {
        let mut regions = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut body = json!({ "TaskId": task_id });
            if let Some(max) = max_results {
                body["MaxResults"] = json!(max);
            }
            if let Some(token) = &next_token {
                body["NextToken"] = json!(token);
            }
            let answer = self
                .call("us-east-1", "GetResourceExplorerSetup", body)
                .await;
            let page = answer.ok(case);
            let items = page["Regions"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Regions list in {page}"))
                .clone();
            if let Some(max) = max_results {
                assert!(
                    items.len() as i64 <= max,
                    "case {case:?}: expected at most {max} Regions in a page, got {}",
                    items.len()
                );
            }
            regions.extend(items);
            match page["NextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
            assert!(
                regions.len() < 64,
                "case {case:?}: pagination did not terminate after {} Regions",
                regions.len()
            );
        }
        regions
    }
}

/// Whether `task_id` has the form `CreateResourceExplorerSetupOutput$TaskId` is
/// modelled with: `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`
/// (from `smithy_query(query="operation",operation="CreateResourceExplorerSetup")`).
fn is_modelled_task_id(task_id: &str) -> bool {
    let groups: Vec<&str> = task_id.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|group| {
            group
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
}

/// The `Region` member of each reported `RegionStatus`, in the order reported.
fn reported_regions(regions: &[Value]) -> Vec<String> {
    regions
        .iter()
        .map(|status| {
            status["Region"]
                .as_str()
                .expect("every RegionStatus names its Region")
                .to_string()
        })
        .collect()
}

/// The `RegionStatus` for `region`.
fn status_of<'a>(case: &str, regions: &'a [Value], region: &str) -> &'a Value {
    regions
        .iter()
        .find(|status| status["Region"].as_str() == Some(region))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a RegionStatus for {region}, got {regions:?}")
        })
}

/// Assert one step's `Status`, and hand back the step so its resource can be read.
fn step<'a>(case: &str, status: &'a Value, member: &str, expected: &str) -> &'a Value {
    let step = &status[member];
    assert_eq!(
        step["Status"].as_str(),
        Some(expected),
        "case {case:?}: expected {member}.Status {expected}, got {step}"
    );
    step
}

/// One row of the setup table: the Regions to configure, the aggregator Region (the
/// modelled `AggregatorRegions` holds at most one), and the view name.
struct SetupCase {
    name: &'static str,
    region_list: &'static [&'static str],
    aggregator: Option<&'static str>,
    view_name: &'static str,
}

/// A setup call configures every Region it names, and the task it answers with
/// reaches `SUCCEEDED` for both of each Region's steps only once the index and the
/// view those steps are about are really there.
///
/// The rows differ along what the rule generalises over: how many Regions are named,
/// whether one of them is an aggregator Region, and whether the aggregator Region is
/// named inside `RegionList` or only in `AggregatorRegions`.
#[tokio::test]
async fn setup_configures_every_region_and_settles_to_succeeded() {
    let cases = [
        SetupCase {
            name: "one Region",
            region_list: &["us-east-1"],
            aggregator: None,
            view_name: "setup-single",
        },
        SetupCase {
            name: "two Regions",
            region_list: &["us-east-1", "eu-west-1"],
            aggregator: None,
            view_name: "setup-pair",
        },
        SetupCase {
            name: "three Regions with an aggregator among them",
            region_list: &["us-east-1", "eu-west-1", "ap-southeast-2"],
            aggregator: Some("us-east-1"),
            view_name: "setup-trio",
        },
        SetupCase {
            name: "an aggregator Region named outside RegionList",
            region_list: &["eu-west-2"],
            aggregator: Some("eu-central-1"),
            view_name: "setup-outside",
        },
    ];

    for case in cases {
        // One emulator per row: each row is a fresh account.
        let emulator = Emulator::new();
        let mut body = json!({
            "RegionList": case.region_list,
            "ViewName": case.view_name,
        });
        if let Some(aggregator) = case.aggregator {
            body["AggregatorRegions"] = json!([aggregator]);
        }
        let task_id = emulator.create_setup(case.name, body).await;

        // Every Region the call named, plus an aggregator Region named only in
        // `AggregatorRegions`: an aggregator Region holds an index of its own.
        let expected: Vec<String> = {
            let mut names: BTreeSet<String> = case
                .region_list
                .iter()
                .map(|region| region.to_string())
                .collect();
            names.extend(case.aggregator.map(str::to_string));
            names.into_iter().collect()
        };

        // Straight after the call the work is still owed: nothing is reported done.
        let fresh = emulator.setup_regions(case.name, &task_id, None).await;
        assert_eq!(
            reported_regions(&fresh),
            expected,
            "case {:?}: expected the Regions {expected:?} reported in order, got {:?}",
            case.name,
            reported_regions(&fresh)
        );
        for region in &expected {
            let status = status_of(case.name, &fresh, region);
            let case_name = format!("{} / {region} straight after the call", case.name);
            step(&case_name, status, "View", "IN_PROGRESS");
        }

        tokio::time::sleep(PAST_SETUP).await;

        let settled = emulator.setup_regions(case.name, &task_id, None).await;
        for region in &expected {
            let case_name = format!("{} / {region}", case.name);
            let status = status_of(&case_name, &settled, region);
            let index_step = step(&case_name, status, "Index", "SUCCEEDED");
            let view_step = step(&case_name, status, "View", "SUCCEEDED");

            // The aggregator Region's index is the AGGREGATOR one; every other
            // Region's is local.
            let expected_type = if case.aggregator == Some(region.as_str()) {
                "AGGREGATOR"
            } else {
                "LOCAL"
            };
            assert_eq!(
                index_step["Index"]["Region"].as_str(),
                Some(region.as_str()),
                "case {case_name:?}: expected the step's Index to be the Region's, got {index_step}"
            );
            assert_eq!(
                index_step["Index"]["Type"].as_str(),
                Some(expected_type),
                "case {case_name:?}: expected index type {expected_type}, got {index_step}"
            );
            assert_eq!(
                view_step["View"]["ViewName"].as_str(),
                Some(case.view_name),
                "case {case_name:?}: expected the view {}, got {view_step}",
                case.view_name
            );

            // Every reported value is the store's: both resources read back through
            // this service's own readers, in the Region the step named.
            let index_arn = index_step["Index"]["Arn"]
                .as_str()
                .expect("a succeeded index step names its index");
            let read = emulator.call(region, "GetIndex", json!({})).await;
            let read = read.ok(&case_name);
            assert_eq!(
                (read["Arn"].as_str(), read["Type"].as_str(), read["State"].as_str()),
                (Some(index_arn), Some(expected_type), Some("ACTIVE")),
                "case {case_name:?}: expected GetIndex to report the configured index, got {read}"
            );

            let view_arn = view_step["View"]["ViewArn"]
                .as_str()
                .expect("a succeeded view step names its view");
            let read = emulator
                .call(region, "GetView", json!({ "ViewArn": view_arn }))
                .await;
            let read = read.ok(&case_name);
            assert_eq!(
                read["View"]["ViewName"].as_str(),
                Some(case.view_name),
                "case {case_name:?}: expected GetView to report the configured view, got {read}"
            );
        }
    }
}

/// What a Region already holds is not created a second time: that step is `SKIPPED`
/// and names the resource that was there, which also makes the operation idempotent —
/// running the same setup again configures nothing new.
#[tokio::test]
async fn setup_skips_the_index_and_view_a_region_already_holds() {
    let emulator = Emulator::new();
    let view_name = "already-here";

    // `us-east-1` is set up by hand first: an index, and a view of the name the
    // setup call will ask for. `eu-west-1` holds nothing.
    let held_index = emulator
        .call("us-east-1", "CreateIndex", json!({ "ClientToken": "by-hand" }))
        .await;
    let held_index = held_index.ok("CreateIndex")["Arn"]
        .as_str()
        .expect("CreateIndex answers an Arn")
        .to_string();
    tokio::time::sleep(PAST_SETUP).await;
    let held_view = emulator
        .call("us-east-1", "CreateView", json!({ "ViewName": view_name }))
        .await;
    let held_view = held_view.ok("CreateView")["View"]["ViewArn"]
        .as_str()
        .expect("CreateView answers a ViewArn")
        .to_string();

    let task_id = emulator
        .create_setup(
            "setup over a configured Region",
            json!({
                "RegionList": ["us-east-1", "eu-west-1"],
                "ViewName": view_name,
            }),
        )
        .await;
    tokio::time::sleep(PAST_SETUP).await;
    let regions = emulator
        .setup_regions("setup over a configured Region", &task_id, None)
        .await;

    let held = status_of("held", &regions, "us-east-1");
    let index_step = step("held index", held, "Index", "SKIPPED");
    let view_step = step("held view", held, "View", "SKIPPED");
    assert_eq!(
        index_step["Index"]["Arn"].as_str(),
        Some(held_index.as_str()),
        "case \"held index\": expected the step to name the index already there, got {index_step}"
    );
    assert_eq!(
        view_step["View"]["ViewArn"].as_str(),
        Some(held_view.as_str()),
        "case \"held view\": expected the step to name the view already there, got {view_step}"
    );

    let fresh = status_of("fresh", &regions, "eu-west-1");
    step("fresh index", fresh, "Index", "SUCCEEDED");
    step("fresh view", fresh, "View", "SUCCEEDED");

    // Idempotent: the same setup again creates nothing, so every step is skipped and
    // each Region still holds exactly one view of that name.
    let replay = emulator
        .create_setup(
            "the same setup again",
            json!({
                "RegionList": ["us-east-1", "eu-west-1"],
                "ViewName": view_name,
            }),
        )
        .await;
    assert_ne!(
        replay, task_id,
        "case \"the same setup again\": expected a task id of its own, got {replay}"
    );
    tokio::time::sleep(PAST_SETUP).await;
    let replayed = emulator
        .setup_regions("the same setup again", &replay, None)
        .await;
    for region in ["us-east-1", "eu-west-1"] {
        let status = status_of("the same setup again", &replayed, region);
        step(&format!("replayed index in {region}"), status, "Index", "SKIPPED");
        step(&format!("replayed view in {region}"), status, "View", "SKIPPED");

        let views = emulator.call(region, "ListViews", json!({})).await;
        let views = views.ok(region)["Views"]
            .as_array()
            .expect("ListViews answers a Views list")
            .len();
        assert_eq!(
            views, 2,
            "case {region:?}: expected the view the Region was turned on with and the \
             one view the setup asked for, got {views}"
        );
        let indexes = emulator
            .call(region, "ListIndexes", json!({ "Regions": [region] }))
            .await;
        let indexes = indexes.ok(region)["Indexes"]
            .as_array()
            .expect("ListIndexes answers an Indexes list")
            .len();
        assert_eq!(
            indexes, 1,
            "case {region:?}: expected the Region's one index, got {indexes}"
        );
    }
}

/// The progress report is paginated like every other `@paginated` operation of this
/// service — lossless at every page size, and the cursor is bound to its own task —
/// and an id no task was issued for is the modelled 404.
#[tokio::test]
async fn setup_progress_is_paginated_and_bound_to_its_task() {
    let emulator = Emulator::new();
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2", "eu-west-2"];
    let task_id = emulator
        .create_setup(
            "four Regions",
            json!({ "RegionList": regions, "ViewName": "paged-setup" }),
        )
        .await;
    let other_task = emulator
        .create_setup(
            "another task",
            json!({ "RegionList": ["us-west-2"], "ViewName": "other-setup" }),
        )
        .await;
    tokio::time::sleep(PAST_SETUP).await;

    let whole = emulator.setup_regions("whole", &task_id, None).await;
    let expected = reported_regions(&whole);
    assert_eq!(
        expected.len(),
        regions.len(),
        "case \"whole\": expected {} Regions, got {expected:?}",
        regions.len()
    );
    // Rows: each published bound of `MaxResults`, and the page sizes around the
    // result count.
    for max_results in [MAX_RESULTS_MIN, 2, 3, 4, 5, MAX_RESULTS_MAX] {
        let case = format!("MaxResults {max_results}");
        let paged = emulator
            .setup_regions(&case, &task_id, Some(max_results))
            .await;
        assert_eq!(
            reported_regions(&paged),
            expected,
            "case {case:?}: expected paging to be lossless and ordered, got {:?}",
            reported_regions(&paged)
        );
    }

    // One past each published bound is refused by the modelled range.
    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        emulator
            .call(
                "us-east-1",
                "GetResourceExplorerSetup",
                json!({ "TaskId": task_id, "MaxResults": max_results }),
            )
            .await
            .failed(&format!("MaxResults {max_results}"), 400, "ValidationException");
    }

    // A token this task issued cannot resume another task's report.
    let first_page = emulator
        .call(
            "us-east-1",
            "GetResourceExplorerSetup",
            json!({ "TaskId": task_id, "MaxResults": 1 }),
        )
        .await;
    let token = first_page.ok("first page")["NextToken"]
        .as_str()
        .expect("a page of four Regions with MaxResults 1 has a NextToken")
        .to_string();
    emulator
        .call(
            "us-east-1",
            "GetResourceExplorerSetup",
            json!({ "TaskId": other_task, "NextToken": token, "MaxResults": 1 }),
        )
        .await
        .failed("token from another task", 400, "ValidationException");

    // An id of the modelled form that no task was issued for resolves to nothing.
    let unknown = "00000000-0000-0000-0000-000000000000";
    let answer = emulator
        .call(
            "us-east-1",
            "GetResourceExplorerSetup",
            json!({ "TaskId": unknown }),
        )
        .await;
    answer.failed("unknown task", 404, "ResourceNotFoundException");
    assert!(
        answer.error_message().contains(unknown),
        "case \"unknown task\": expected the message to name the id, got {:?}",
        answer.error_message()
    );
}

/// A setup is refused where it cannot be carried out: a Region this emulator does not
/// serve, and a second aggregator Region for an account that already has one.
#[tokio::test]
async fn setup_refuses_an_unservable_region_and_a_second_aggregator() {
    let emulator = Emulator::new();

    // Rows: grammar-shaped values the modelled `RegionList` pattern admits but that
    // name no Region, in `RegionList` and in `AggregatorRegions`.
    for (member, body) in [
        (
            "RegionList",
            json!({ "RegionList": ["us-east-1", "xx-nowhere-1"], "ViewName": "bad-region" }),
        ),
        (
            "AggregatorRegions",
            json!({
                "RegionList": ["us-east-1"],
                "AggregatorRegions": ["zz-elsewhere-9"],
                "ViewName": "bad-aggregator",
            }),
        ),
    ] {
        let answer = emulator
            .call("us-east-1", "CreateResourceExplorerSetup", body)
            .await;
        answer.failed(member, 400, "ValidationException");
        // Nothing was configured for the refused request.
        emulator
            .call("us-east-1", "GetIndex", json!({}))
            .await
            .failed(member, 404, "ResourceNotFoundException");
    }

    // A value the modelled pattern itself refuses never reaches the handler, and is
    // the same 400.
    emulator
        .call(
            "us-east-1",
            "CreateResourceExplorerSetup",
            json!({ "RegionList": ["not a region"], "ViewName": "bad-shape" }),
        )
        .await
        .failed("RegionList off the modelled pattern", 400, "ValidationException");
    emulator
        .call(
            "us-east-1",
            "CreateResourceExplorerSetup",
            json!({ "RegionList": ["us-east-1"], "ViewName": "not a view name" }),
        )
        .await
        .failed("ViewName off the modelled pattern", 400, "ValidationException");

    // One aggregator index per account: a second setup naming a different aggregator
    // Region is the modelled conflict, and the first aggregator is untouched.
    let task_id = emulator
        .create_setup(
            "first aggregator",
            json!({
                "RegionList": ["us-east-1", "eu-west-1"],
                "AggregatorRegions": ["us-east-1"],
                "ViewName": "aggregated",
            }),
        )
        .await;
    tokio::time::sleep(PAST_SETUP).await;
    let regions = emulator
        .setup_regions("first aggregator", &task_id, None)
        .await;
    step(
        "first aggregator",
        status_of("first aggregator", &regions, "us-east-1"),
        "Index",
        "SUCCEEDED",
    );

    let answer = emulator
        .call(
            "us-east-1",
            "CreateResourceExplorerSetup",
            json!({
                "RegionList": ["eu-west-1"],
                "AggregatorRegions": ["eu-west-1"],
                "ViewName": "second-aggregator",
            }),
        )
        .await;
    answer.failed("second aggregator Region", 409, "ConflictException");
    // The refusal is worded the way the service documents this case for a promotion,
    // which is the same wording `UpdateIndexType` answers with.
    assert!(
        answer
            .error_message()
            .contains("You already have an AGGREGATOR index in a different Amazon Web Services Region."),
        "case \"second aggregator Region\": expected the documented one-aggregator \
         wording, got {:?}",
        answer.error_message()
    );
    let read = emulator.call("eu-west-1", "GetIndex", json!({})).await;
    assert_eq!(
        read.ok("eu-west-1 after the refusal")["Type"].as_str(),
        Some("LOCAL"),
        "case \"second aggregator Region\": expected the refused promotion to leave the \
         index local, got {}",
        read.body
    );

    // Naming the aggregator Region it already has is not a conflict: there is one
    // aggregator index and it is where the account put it.
    let again = emulator
        .create_setup(
            "the same aggregator again",
            json!({
                "RegionList": ["us-east-1"],
                "AggregatorRegions": ["us-east-1"],
                "ViewName": "aggregated",
            }),
        )
        .await;
    tokio::time::sleep(PAST_SETUP).await;
    let regions = emulator
        .setup_regions("the same aggregator again", &again, None)
        .await;
    let status = status_of("the same aggregator again", &regions, "us-east-1");
    step("the same aggregator again", status, "Index", "SKIPPED");
}

/// An aggregator Region whose index is already local is PROMOTED by the setup rather
/// than left behind: the step is in progress until the clock owes the promotion, and
/// the Region's index then reports `AGGREGATOR`.
#[tokio::test]
async fn setup_promotes_an_existing_local_index_to_aggregator() {
    let emulator = Emulator::new();
    let region = "eu-west-1";
    emulator
        .call(region, "CreateIndex", json!({ "ClientToken": "local-first" }))
        .await
        .ok("CreateIndex");
    tokio::time::sleep(PAST_SETUP).await;
    let read = emulator.call(region, "GetIndex", json!({})).await;
    assert_eq!(
        read.ok("before the promotion")["Type"].as_str(),
        Some("LOCAL"),
        "case \"before the promotion\": expected a local index, got {}",
        read.body
    );

    let task_id = emulator
        .create_setup(
            "promote",
            json!({
                "RegionList": [region],
                "AggregatorRegions": [region],
                "ViewName": "promoted",
            }),
        )
        .await;
    let in_flight = emulator.setup_regions("promote", &task_id, None).await;
    step(
        "promote, in flight",
        status_of("promote", &in_flight, region),
        "Index",
        "IN_PROGRESS",
    );
    // The promotion is recorded the moment it is asked for, as `UpdateIndexType`
    // records one: the index reports the new type while it is still switching, so a
    // second setup cannot promote another Region's index behind this one's back.
    let read = emulator.call(region, "GetIndex", json!({})).await;
    let read = read.ok("during the promotion");
    assert_eq!(
        (read["Type"].as_str(), read["State"].as_str()),
        (Some("AGGREGATOR"), Some("UPDATING")),
        "case \"during the promotion\": expected the switching index, got {read}"
    );
    emulator
        .call(
            "us-east-1",
            "CreateResourceExplorerSetup",
            json!({
                "RegionList": ["us-east-1"],
                "AggregatorRegions": ["us-east-1"],
                "ViewName": "second-aggregator",
            }),
        )
        .await
        .failed("another aggregator mid-promotion", 409, "ConflictException");

    tokio::time::sleep(PAST_SETUP).await;
    let settled = emulator.setup_regions("promote", &task_id, None).await;
    let status = status_of("promote", &settled, region);
    let index_step = step("promote, settled", status, "Index", "SUCCEEDED");
    assert_eq!(
        index_step["Index"]["Type"].as_str(),
        Some("AGGREGATOR"),
        "case \"promote, settled\": expected the promoted type, got {index_step}"
    );
    let read = emulator.call(region, "GetIndex", json!({})).await;
    assert_eq!(
        read.ok("after the promotion")["Type"].as_str(),
        Some("AGGREGATOR"),
        "case \"after the promotion\": expected GetIndex to report the promotion, got {}",
        read.body
    );
}

/// A Region whose index is being torn down cannot be set up: both of its steps `FAIL`
/// with `ErrorDetails` saying why, and nothing is created there — while a Region in
/// the same call that holds nothing is configured normally.
#[tokio::test]
async fn setup_fails_the_steps_of_a_region_whose_index_is_being_deleted() {
    let emulator = Emulator::new();
    let doomed = "ap-southeast-2";
    let fresh = "eu-west-2";
    let index_arn = emulator
        .call(doomed, "CreateIndex", json!({ "ClientToken": "to-be-deleted" }))
        .await
        .ok("CreateIndex")["Arn"]
        .as_str()
        .expect("CreateIndex answers an Arn")
        .to_string();
    tokio::time::sleep(PAST_SETUP).await;
    let deleted = emulator
        .call(doomed, "DeleteIndex", json!({ "Arn": index_arn }))
        .await;
    assert_eq!(
        deleted.ok("DeleteIndex")["State"].as_str(),
        Some("DELETING"),
        "case \"DeleteIndex\": expected the index to be on its way out, got {}",
        deleted.body
    );

    // Within the deletion's span, so the Region still holds an index that is going
    // away: there is nothing the setup can build on there.
    let task_id = emulator
        .create_setup(
            "a Region mid-deletion",
            json!({
                "RegionList": [doomed, fresh],
                "ViewName": "mid-deletion",
            }),
        )
        .await;
    let regions = emulator
        .setup_regions("a Region mid-deletion", &task_id, None)
        .await;
    let status = status_of("a Region mid-deletion", &regions, doomed);
    for member in ["Index", "View"] {
        let case = format!("{member} of a Region mid-deletion");
        let failed = step(&case, status, member, "FAILED");
        assert_eq!(
            failed["ErrorDetails"]["Code"].as_str(),
            Some("ConflictException"),
            "case {case:?}: expected the refusal's code, got {failed}"
        );
        let message = failed["ErrorDetails"]["Message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(doomed) && message.contains("deleted"),
            "case {case:?}: expected a message naming the Region and the deletion, got \
             {message:?}"
        );
        assert!(
            failed.get(member).is_none(),
            "case {case:?}: expected no resource on a failed step, got {failed}"
        );
    }

    // The other Region in the same call is configured as usual.
    tokio::time::sleep(PAST_SETUP).await;
    let regions = emulator
        .setup_regions("a Region mid-deletion", &task_id, None)
        .await;
    let status = status_of("the healthy Region", &regions, fresh);
    step("the healthy Region's index", status, "Index", "SUCCEEDED");
    step("the healthy Region's view", status, "View", "SUCCEEDED");
    // And the deleted Region holds nothing: no index, and so no view.
    emulator
        .call(doomed, "GetIndex", json!({}))
        .await
        .failed("the deleted Region", 404, "ResourceNotFoundException");
    let views = emulator.call(doomed, "ListViews", json!({})).await;
    assert_eq!(
        views.ok("the deleted Region")["Views"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "case \"the deleted Region\": expected no views, got {}",
        views.body
    );
}
