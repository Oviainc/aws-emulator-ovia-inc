// SPDX-License-Identifier: CC0-1.0

//! `CountOpenWorkflowExecutions`, end to end over the service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`)
//! through the same `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: the operation answers with the NUMBER of runs
//! stored in the named domain that are still OPEN, whose start instant lies inside the
//! required `startTimeFilter` window (both bounds inclusive), and that match whichever
//! ONE of the mutually exclusive `executionFilter` / `typeFilter` / `tagFilter` the
//! request carries — an unknown domain is the modelled `UnknownResourceFault`, two of
//! those filters at once is a bad request, and the count is a pure read.
//!
//! The expected count of each row is computed by the TEST from the seeded table, never
//! hardcoded, and the rows vary along what the rule generalises over: which filter is
//! given, its value, whether a type filter pins a version, tag membership, run status
//! and the position of the time window relative to a run's start.

use std::sync::Arc;

use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::ExecutionTimeFilter;
use service_com_amazonaws_swf_simpleworkflowservice::state::within_time_window;
use service_com_amazonaws_swf_simpleworkflowservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "SimpleWorkflowService";

/// The region the `Authorization` credential scope below signs for.
const REGION: &str = "us-east-1";

/// The slack, in seconds, a time-window row leaves around a run's start instant. The
/// wire form of a timestamp is epoch seconds as a JSON number, so reading one back and
/// sending it again costs a rounding of well under a microsecond; 10 ms is far wider
/// than that and far narrower than any wait in this test.
const WINDOW_SLACK_SECONDS: f64 = 0.010;

/// One emulator: a fresh engine behind a router that claims `swf` requests.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and return `(status, body)`.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/swf/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "case {operation}: expected a JSON body, got {error}: {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// Call one operation, requiring a 200, and return the body.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} with {parsed:?}"
        );
        parsed
    }

    /// Call one operation, requiring a 400 with `expected_code`.
    async fn refused(&self, case: &str, operation: &str, body: &Value, expected_code: &str) {
        let (status, parsed) = self.call(operation, body).await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {case:?}: expected {operation} to answer 400 {expected_code}, \
             got {status} {code} with {parsed:?}"
        );
    }

    /// `CountOpenWorkflowExecutions` over `domain`, with `members` folded into the
    /// request beside the required `startTimeFilter`.
    async fn count(&self, case: &str, domain: &str, members: &Value) -> Value {
        let mut body = json!({
            "domain": domain,
            // Epoch 0: a window that opens before any run this test starts.
            "startTimeFilter": { "oldestDate": 0 },
        });
        merge(&mut body, members.clone());
        self.ok(case, "CountOpenWorkflowExecutions", &body).await
    }
}

/// The `__type` discriminator of an awsJson error body, reduced to the shape name.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Fold `extra`'s members into `body`.
fn merge(body: &mut Value, extra: Value) {
    let (Some(target), Some(source)) = (body.as_object_mut(), extra.as_object()) else {
        panic!("merge expects two JSON objects, got {body:?} and {extra:?}");
    };
    for (key, value) in source {
        target.insert(key.clone(), value.clone());
    }
}

fn number_at(body: &Value, pointer: &str) -> f64 {
    body.pointer(pointer)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a number at {pointer} of {body:?}"))
}

fn count_of(body: &Value) -> i64 {
    body.get("count")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected a count member in {body:?}"))
}

/// One run this test seeds: its id, its workflow type, its tags, and whether it is
/// terminated afterwards (so it is CLOSED and must never be counted).
#[derive(Debug)]
struct SeededRun {
    workflow_id: &'static str,
    workflow_type: (&'static str, &'static str),
    tags: &'static [&'static str],
    terminated: bool,
}

/// The seeded runs. Three workflow types, two of them sharing a name at different
/// versions; tag lists of size 0, 1 and 2, with one tag shared by two runs and one tag
/// held only by the closed run.
const SEEDED: [SeededRun; 4] = [
    SeededRun {
        workflow_id: "wf-open-alpha-1",
        workflow_type: ("alpha", "1.0"),
        tags: &["red", "shared"],
        terminated: false,
    },
    SeededRun {
        workflow_id: "wf-open-alpha-2",
        workflow_type: ("alpha", "2.0"),
        tags: &["shared"],
        terminated: false,
    },
    SeededRun {
        workflow_id: "wf-open-beta",
        workflow_type: ("beta", "1.0"),
        tags: &[],
        terminated: false,
    },
    SeededRun {
        workflow_id: "wf-closed-alpha-1",
        workflow_type: ("alpha", "1.0"),
        tags: &["red", "closed-only"],
        terminated: true,
    },
];

/// The one optional, mutually exclusive filter a row carries.
#[derive(Debug)]
enum Filter {
    /// No optional filter at all: every open run in the window counts.
    None,
    Execution(&'static str),
    /// A type name, and a version only when the row pins one.
    Type(&'static str, Option<&'static str>),
    Tag(&'static str),
}

impl Filter {
    /// The request members this filter contributes.
    fn members(&self) -> Value {
        match self {
            Filter::None => json!({}),
            Filter::Execution(workflow_id) => json!({
                "executionFilter": { "workflowId": workflow_id },
            }),
            Filter::Type(name, None) => json!({ "typeFilter": { "name": name } }),
            Filter::Type(name, Some(version)) => json!({
                "typeFilter": { "name": name, "version": version },
            }),
            Filter::Tag(tag) => json!({ "tagFilter": { "tag": tag } }),
        }
    }

    /// The rule, applied by the TEST to one seeded run: this is the oracle the
    /// handler's answer is checked against.
    fn matches(&self, run: &SeededRun) -> bool {
        match self {
            Filter::None => true,
            Filter::Execution(workflow_id) => run.workflow_id == *workflow_id,
            Filter::Type(name, version) => {
                run.workflow_type.0 == *name
                    && version.is_none_or(|version| run.workflow_type.1 == version)
            }
            Filter::Tag(tag) => run.tags.contains(tag),
        }
    }

    /// How many seeded runs this filter counts: the open ones it matches.
    fn expected_count(&self) -> i64 {
        SEEDED
            .iter()
            .filter(|run| !run.terminated && self.matches(run))
            .count() as i64
    }
}

/// Register the domain, its three workflow types, start every seeded run and terminate
/// the one that must end up CLOSED. Returns the `runId` the service minted for each
/// seeded `workflowId`, so no later call has to invent one.
async fn seed(emulator: &Emulator, domain: &str) -> Vec<(&'static str, String)> {
    emulator
        .ok(
            "seed",
            "RegisterDomain",
            &json!({ "name": domain, "workflowExecutionRetentionPeriodInDays": "7" }),
        )
        .await;
    for (name, version) in [("alpha", "1.0"), ("alpha", "2.0"), ("beta", "1.0")] {
        emulator
            .ok(
                "seed",
                "RegisterWorkflowType",
                &json!({
                    "domain": domain,
                    "name": name,
                    "version": version,
                    "defaultTaskList": { "name": "tasks" },
                    "defaultTaskStartToCloseTimeout": "60",
                    "defaultExecutionStartToCloseTimeout": "300",
                    "defaultChildPolicy": "TERMINATE",
                }),
            )
            .await;
    }
    let mut run_ids = Vec::new();
    for run in &SEEDED {
        let started = emulator
            .ok(
                "seed",
                "StartWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": run.workflow_id,
                    "workflowType": {
                        "name": run.workflow_type.0,
                        "version": run.workflow_type.1,
                    },
                    "tagList": run.tags,
                }),
            )
            .await;
        let run_id = started
            .get("runId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("the start of {:?} must mint a runId", run.workflow_id))
            .to_string();
        if run.terminated {
            emulator
                .ok(
                    "seed",
                    "TerminateWorkflowExecution",
                    &json!({ "domain": domain, "workflowId": run.workflow_id }),
                )
                .await;
        }
        run_ids.push((run.workflow_id, run_id));
    }
    run_ids
}

#[tokio::test]
async fn the_count_is_the_number_of_open_runs_matching_the_requested_filter() {
    let emulator = Emulator::new();
    let run_ids = seed(&emulator, "counts").await;

    let rows = [
        Filter::None,
        // Every workflowId in the table, open and closed, plus one that was never
        // started.
        Filter::Execution("wf-open-alpha-1"),
        Filter::Execution("wf-open-alpha-2"),
        Filter::Execution("wf-open-beta"),
        Filter::Execution("wf-closed-alpha-1"),
        Filter::Execution("wf-never-started"),
        // A name shared by two versions, with and without a version pinned, plus a
        // version and a name no run carries.
        Filter::Type("alpha", None),
        Filter::Type("alpha", Some("1.0")),
        Filter::Type("alpha", Some("2.0")),
        Filter::Type("alpha", Some("9.9")),
        Filter::Type("beta", None),
        Filter::Type("beta", Some("1.0")),
        Filter::Type("gamma", None),
        // A tag two runs share, a tag one open run has, a tag only the closed run has,
        // and a tag nothing carries.
        Filter::Tag("shared"),
        Filter::Tag("red"),
        Filter::Tag("closed-only"),
        Filter::Tag("absent"),
    ];

    for row in &rows {
        let case = format!("{row:?}");
        let body = emulator.count(&case, "counts", &row.members()).await;
        let actual = count_of(&body);
        assert_eq!(
            actual,
            row.expected_count(),
            "case {case:?}: expected the count of open seeded runs the filter matches, \
             got {actual} in {body:?}"
        );
        assert_eq!(
            body.get("truncated").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: the store holds far fewer runs than this API truncates at, \
             so truncated must be false, got {body:?}"
        );

        // A count is a pure read: asking twice answers the same, and the runs keep
        // their status.
        let again = emulator.count(&case, "counts", &row.members()).await;
        assert_eq!(
            count_of(&again),
            actual,
            "case {case:?}: counting twice must answer the same, got {again:?}"
        );
    }

    for (run, (_, run_id)) in SEEDED.iter().zip(&run_ids) {
        let described = emulator
            .ok(
                run.workflow_id,
                "DescribeWorkflowExecution",
                &json!({
                    "domain": "counts",
                    "execution": { "workflowId": run.workflow_id, "runId": run_id },
                }),
            )
            .await;
        let expected_status = if run.terminated { "CLOSED" } else { "OPEN" };
        let actual = described
            .pointer("/executionInfo/executionStatus")
            .and_then(Value::as_str)
            .unwrap_or("<missing>");
        assert_eq!(
            actual, expected_status,
            "case {:?}: counting must not move a run's status, got {described:?}",
            run.workflow_id
        );
    }
}

#[tokio::test]
async fn the_start_time_window_bounds_which_runs_are_counted() {
    let emulator = Emulator::new();
    let run_ids = seed(&emulator, "window").await;
    let run_id = run_ids
        .iter()
        .find(|(workflow_id, _)| *workflow_id == "wf-open-beta")
        .map(|(_, run_id)| run_id.clone())
        .expect("the seeded run");

    // The instant one seeded run started, as the service itself reports it.
    let described = emulator
        .ok(
            "window",
            "DescribeWorkflowExecution",
            &json!({
                "domain": "window",
                "execution": { "workflowId": "wf-open-beta", "runId": run_id },
            }),
        )
        .await;
    let started = number_at(&described, "/executionInfo/startTimestamp");

    // Rows: a window that opens after the run started, one that closes before it, and
    // the two that bracket it on either side — plus a window wholly in the past.
    let rows: [(&str, Value, i64); 5] = [
        (
            "opens after the run started",
            json!({ "oldestDate": started + WINDOW_SLACK_SECONDS }),
            0,
        ),
        (
            "opens before the run started",
            json!({ "oldestDate": started - WINDOW_SLACK_SECONDS }),
            1,
        ),
        (
            "closes before the run started",
            json!({
                "oldestDate": 0,
                "latestDate": started - WINDOW_SLACK_SECONDS,
            }),
            0,
        ),
        (
            "closes after the run started",
            json!({
                "oldestDate": 0,
                "latestDate": started + WINDOW_SLACK_SECONDS,
            }),
            1,
        ),
        (
            "wholly in the past",
            json!({ "oldestDate": 0, "latestDate": 1_000 }),
            0,
        ),
    ];

    for (case, start_time_filter, expected) in &rows {
        let body = emulator
            .ok(
                case,
                "CountOpenWorkflowExecutions",
                &json!({
                    "domain": "window",
                    "startTimeFilter": start_time_filter,
                    "executionFilter": { "workflowId": "wf-open-beta" },
                }),
            )
            .await;
        let actual = count_of(&body);
        assert_eq!(
            actual, *expected,
            "case {case:?}: expected {expected} for window {start_time_filter:?}, \
             got {actual} in {body:?}"
        );
    }
}

/// The window rule at EXACT instants, where a wire timestamp cannot be nudged: both
/// `oldestDate` ("the oldest start or close date and time to return") and the optional
/// `latestDate` are INCLUSIVE, so a run starting exactly on a bound is counted and one
/// a single nanosecond outside it is not.
#[test]
fn the_time_window_includes_both_of_its_bounds() {
    let oldest = DateTime::from_secs_and_nanos(1_700_000_000, 250);
    let latest = DateTime::from_secs_and_nanos(1_700_000_060, 750);

    let rows: [(&str, DateTime, Option<DateTime>, bool); 8] = [
        ("exactly on oldestDate", oldest, Some(latest), true),
        (
            "one nanosecond before oldestDate",
            DateTime::from_secs_and_nanos(1_700_000_000, 249),
            Some(latest),
            false,
        ),
        ("exactly on latestDate", latest, Some(latest), true),
        (
            "one nanosecond after latestDate",
            DateTime::from_secs_and_nanos(1_700_000_060, 751),
            Some(latest),
            false,
        ),
        (
            "inside the window",
            DateTime::from_secs_and_nanos(1_700_000_030, 0),
            Some(latest),
            true,
        ),
        (
            "far after an open-ended window",
            DateTime::from_secs_and_nanos(4_000_000_000, 0),
            None,
            true,
        ),
        ("exactly on an open-ended oldestDate", oldest, None, true),
        (
            "before an open-ended window",
            DateTime::from_secs_and_nanos(1_600_000_000, 0),
            None,
            false,
        ),
    ];

    for (case, at, latest_date, expected) in rows {
        let filter = ExecutionTimeFilter {
            oldest_date: oldest,
            latest_date,
        };
        let actual = within_time_window(at, &filter);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected} for {at:?} against {filter:?}, got {actual}"
        );
    }
}

#[tokio::test]
async fn a_domain_the_store_does_not_hold_counts_zero_without_creating_it() {
    let emulator = Emulator::new();
    let _ = seed(&emulator, "known").await;

    // Rows: a name never registered, one differing from a registered domain only in
    // case, one differing by a suffix, and a registered domain holding no runs. All of
    // them hold no matching open run, so all of them count zero.
    for domain in ["never-registered", "KNOWN", "known-2", "empty"] {
        if domain == "empty" {
            emulator
                .ok(
                    domain,
                    "RegisterDomain",
                    &json!({ "name": domain, "workflowExecutionRetentionPeriodInDays": "1" }),
                )
                .await;
        }
        let body = emulator.count(domain, domain, &json!({})).await;
        assert_eq!(
            count_of(&body),
            0,
            "case {domain:?}: a domain holding no open run must count zero, got {body:?}"
        );
        assert_eq!(
            body.get("truncated").and_then(Value::as_bool),
            Some(false),
            "case {domain:?}: truncated must be false, got {body:?}"
        );
    }

    // The count is a pure read: the unregistered names it answered for were NOT
    // created, so an operation that does address the resource still refuses them.
    for domain in ["never-registered", "KNOWN", "known-2"] {
        emulator
            .refused(
                domain,
                "DescribeDomain",
                &json!({ "name": domain }),
                "UnknownResourceFault",
            )
            .await;
        emulator
            .refused(
                domain,
                "StartWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": "wf-1",
                    "workflowType": { "name": "alpha", "version": "1.0" },
                }),
                "UnknownResourceFault",
            )
            .await;
    }

    // And the seeded domain's own count is unaffected by any of that.
    let body = emulator.count("known", "known", &json!({})).await;
    assert_eq!(
        count_of(&body),
        Filter::None.expected_count(),
        "case \"known\": the seeded domain still counts its open runs, got {body:?}"
    );
}

#[tokio::test]
async fn two_of_the_mutually_exclusive_filters_is_a_bad_request() {
    let emulator = Emulator::new();
    let _ = seed(&emulator, "exclusive").await;

    let execution = json!({ "executionFilter": { "workflowId": "wf-open-beta" } });
    let kind = json!({ "typeFilter": { "name": "alpha" } });
    let tag = json!({ "tagFilter": { "tag": "shared" } });

    // Every pair, and all three at once.
    let rows = [
        ("execution+type", vec![&execution, &kind]),
        ("execution+tag", vec![&execution, &tag]),
        ("type+tag", vec![&kind, &tag]),
        ("all three", vec![&execution, &kind, &tag]),
    ];
    for (case, filters) in &rows {
        let mut body = json!({
            "domain": "exclusive",
            "startTimeFilter": { "oldestDate": 0 },
        });
        for filter in filters {
            merge(&mut body, (*filter).clone());
        }
        emulator
            .refused(case, "CountOpenWorkflowExecutions", &body, "ValidationException")
            .await;
    }

    // Each of them ALONE is accepted, so the refusal is about the combination.
    for (case, filter) in [
        ("execution", &execution),
        ("type", &kind),
        ("tag", &tag),
    ] {
        emulator.count(case, "exclusive", filter).await;
    }
}
