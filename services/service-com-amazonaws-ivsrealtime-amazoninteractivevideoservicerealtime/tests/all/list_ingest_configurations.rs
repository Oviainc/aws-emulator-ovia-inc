// SPDX-License-Identifier: CC0-1.0

//! `ListIngestConfigurations` over the wire: the page of ingest configurations this
//! account holds.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. LISTING REPORTS PERSISTED STATE. The listing answers a page of the ingest
//!    configurations this account holds, in creation order, bounded by `maxResults`
//!    (default: 50), with a `nextToken` exactly when configurations are left over — and
//!    that token resumes where the page ended.
//! 2. A SUMMARY AND A READ AGREE. Every member `IngestConfigurationSummary` carries is
//!    the member of the same name `GetIngestConfiguration` reports for that same
//!    configuration, read at the same instant; the members the summary shape does not
//!    carry (the ingest keys, `attributes`, `tags`) are absent from a page.
//! 3. A FILTER NARROWS BY THE MEMBER'S OWN DOC: `filterByStageArn` keeps the
//!    configurations associated with that stage, `filterByState` the ones in that state
//!    right now — and "only one filter (by stage ARN or by state) can be used at a time",
//!    so a request carrying both is refused rather than served.
//! 4. A READ CHANGES NOTHING: the listing is `@readonly`, so repeated pages over an
//!    unchanged store are identical and a read in between answers the same.
//! 5. REFUSALS: a value outside a member's modelled `@range`/`@pattern`/`@length` is the
//!    modelled `ValidationException` (HTTP 400), and a pagination token this listing did
//!    not issue is refused rather than read as "start again".
//!
//! The `state` filter needs a configuration a publisher is broadcasting with, and no API
//! of this service makes a participant join a stage — that is the stage's data plane,
//! which is `state::join_stage_session`. So the publishing case brings a publisher into a
//! session through it, reaching the store through this suite's own engine handle, exactly
//! as the `DeleteIngestConfiguration` suite beside this one does.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The account id every ARN this emulator mints carries (`engine::emulator_account_id`),
/// and so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The region this suite signs for.
const REGION: &str = "us-east-1";

/// `PaginationToken`'s `@pattern`, from
/// `smithy_query(query="operation",operation="ListIngestConfigurations")` — member
/// `nextToken`.
const PAGINATION_TOKEN_PATTERN: &str = "^[a-zA-Z0-9+/=_-]*$";

/// The published bounds of `MaxIngestConfigurationResults`' `@range`, from
/// `smithy_query(query="operation",operation="ListIngestConfigurations")`: `1..100`.
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// The page size the listing uses when the request omits `maxResults`, from
/// `ListIngestConfigurationsRequest$maxResults`: "Maximum number of results to return.
/// Default: 50."
const DEFAULT_PAGE_SIZE: usize = 50;

/// The members `IngestConfigurationSummary` carries, from
/// `smithy_query(query="shape",shape="IngestConfigurationSummary")`.
const SUMMARY_MEMBERS: [&str; 8] = [
    "name",
    "arn",
    "ingestProtocol",
    "stageArn",
    "participantId",
    "state",
    "userId",
    "redundantIngest",
];

/// The members the full `IngestConfiguration` carries that the summary shape does NOT
/// (`smithy_query(query="shape",shape="IngestConfiguration,IngestConfigurationSummary")`),
/// so a page reports fewer members than a read rather than different ones.
const MEMBERS_NOT_IN_A_SUMMARY: [&str; 4] = [
    "streamKey",
    "redundantIngestCredentials",
    "attributes",
    "tags",
];

/// The two values the modelled `IngestConfigurationState` carries, from
/// `smithy_query(query="shape",shape="IngestConfigurationState")`: a configuration is
/// `ACTIVE` while a publisher is publishing to its stage, and `INACTIVE` otherwise.
const PUBLISHING_STATE: &str = "ACTIVE";
const NO_PUBLISHER_STATE: &str = "INACTIVE";

/// The values `IngestProtocol` accepts, from
/// `smithy_query(query="shape",shape="IngestProtocol")`.
const INGEST_PROTOCOLS: [&str; 2] = ["RTMP", "RTMPS"];

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it, plus the engine handle the data-plane seeding writes through.
struct RealTime {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.{REGION}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create an ingest configuration and return the `ingestConfiguration` the service
    /// reported for it.
    async fn create(&self, case: &str, body: &Value) -> Value {
        let (status, response) = self.call("CreateIngestConfiguration", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateIngestConfiguration {body} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// Create a stage and return the ARN the service minted for it.
    async fn create_stage(&self, case: &str, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {name} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// Read one configuration back, asserting the read succeeded.
    async fn get(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self
            .call("GetIngestConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetIngestConfiguration on {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// One `ListIngestConfigurations` page, asserting the call succeeded.
    async fn list(&self, case: &str, request: &Value) -> Value {
        let (status, response) = self.call("ListIngestConfigurations", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListIngestConfigurations {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response
    }

    /// The ARNs one page reports, in the order it reported them.
    async fn listed_arns(&self, case: &str, request: &Value) -> Vec<String> {
        let response = self.list(case, request).await;
        summaries(&response)
            .iter()
            .map(|summary| member(summary, "arn"))
            .collect()
    }

    /// Record a publisher joining `stage_arn`, the way the stage's data plane does.
    fn join(&self, case: &str, stage_arn: &str, joined: state::JoinedParticipant) {
        let participant_id = joined.participant_id.clone();
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::join_stage_session(&mut store, stage_arn, joined).is_some(),
            "case {case:?}: the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// The text a refusal states.
fn error_message(response: &Value) -> &str {
    response["message"].as_str().unwrap_or("<no message>")
}

/// A string member, read as the empty string when absent — what a client sees for a
/// member the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The summaries one page reported, as an empty list when the member is absent — so a
/// failing assertion names what came back rather than panicking inside the reader.
fn summaries(response: &Value) -> Vec<Value> {
    response["ingestConfigurations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `value` restricted to the members [`SUMMARY_MEMBERS`] names, so a summary and a full
/// read can be compared member for member on exactly the members both carry.
fn summary_members(value: &Value) -> Value {
    let mut projected = Map::new();
    for name in SUMMARY_MEMBERS {
        if let Some(found) = value.get(name) {
            projected.insert(name.to_string(), found.clone());
        }
    }
    Value::Object(projected)
}

/// Does `value` satisfy the one model `@pattern` this suite checks?
///
/// The pattern is quoted on [`PAGINATION_TOKEN_PATTERN`], straight from `smithy_query`,
/// and is simple enough to check directly — which keeps this crate's dependency set
/// exactly what the service itself needs. A pattern with no arm panics rather than
/// passing silently.
fn matches(pattern: &str, value: &str) -> bool {
    match pattern {
        PAGINATION_TOKEN_PATTERN => value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'_' | b'-')
        }),
        other => panic!("this suite checks no pattern {other:?}"),
    }
}

/// The create requests this suite spans the class of configurations with: each row
/// differs from the others in what a summary reports — the protocol, whether the request
/// names the configuration, whether it carries a user id, and whether redundant ingest is
/// on. `stage` names which of the suite's stages the row is associated with, by index into
/// the stages the caller created, or `None` for no stage at all.
fn create_rows() -> Vec<(&'static str, Option<usize>, Value)> {
    vec![
        (
            "only the required member",
            None,
            json!({ "ingestProtocol": "RTMPS" }),
        ),
        (
            "named, with a user id, on the first stage",
            Some(0),
            json!({
                "ingestProtocol": "RTMPS",
                "name": "studio-feed",
                "userId": "user-7",
            }),
        ),
        (
            "redundant ingest on the first stage",
            Some(0),
            json!({ "ingestProtocol": "RTMPS", "redundantIngest": true }),
        ),
        (
            "cleartext RTMP with insecure ingest allowed, on the second stage",
            Some(1),
            json!({
                "ingestProtocol": "RTMP",
                "insecureIngest": true,
                "name": "legacy",
            }),
        ),
        (
            "attributes and tags, which no summary carries",
            Some(1),
            json!({
                "ingestProtocol": "RTMPS",
                "attributes": { "seat": "left" },
                "tags": { "env": "test" },
            }),
        ),
        (
            "an explicitly empty stage association",
            None,
            json!({ "ingestProtocol": "RTMPS", "stageArn": "" }),
        ),
        (
            "a second configuration with no stage",
            None,
            json!({ "ingestProtocol": "RTMP", "insecureIngest": true }),
        ),
    ]
}

/// The suite's fixture: two stages and the configurations [`create_rows`] describes, in
/// creation order.
struct Fixture {
    emulator: RealTime,
    stages: Vec<String>,
    /// The `ingestConfiguration` each create reported, in creation order.
    created: Vec<Value>,
    /// Which stage each created configuration is associated with, as the ARN it reports.
    stage_of: Vec<String>,
}

impl Fixture {
    async fn new() -> Self {
        let emulator = RealTime::new();
        let mut stages = Vec::new();
        for index in 0..2 {
            stages.push(
                emulator
                    .create_stage("the suite's fixture", &format!("stage-{index}"))
                    .await,
            );
        }

        let mut created = Vec::new();
        let mut stage_of = Vec::new();
        for (case, stage, request) in create_rows() {
            let mut request = request.clone();
            if let Some(index) = stage {
                request["stageArn"] = json!(stages[index]);
            }
            created.push(emulator.create(case, &request).await);
            stage_of.push(stage.map(|index| stages[index].clone()).unwrap_or_default());
        }
        Self {
            emulator,
            stages,
            created,
            stage_of,
        }
    }

    /// The ARNs of the created configurations, in creation order.
    fn arns(&self) -> Vec<String> {
        self.created
            .iter()
            .map(|value| member(value, "arn"))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Rule 1: the listing reports persisted state, a page at a time
// ---------------------------------------------------------------------------

/// A listing over an empty store reports the required member as an empty list and no
/// `nextToken` — it never answers "not implemented" and never invents a configuration.
#[tokio::test]
async fn listing_an_empty_account_reports_no_ingest_configurations() {
    let emulator = RealTime::new();

    // Rows differ in the page bound and the filter, none of which changes anything when
    // there is nothing stored.
    let requests = [
        json!({}),
        json!({ "maxResults": 1 }),
        json!({ "maxResults": 100 }),
        json!({ "filterByState": NO_PUBLISHER_STATE }),
        json!({ "filterByStageArn": format!("arn:aws:ivs:{REGION}:{ACCOUNT}:stage/absent") }),
    ];
    for request in requests {
        let case = format!("request {request}");
        let response = emulator.list(&case, &request).await;
        assert_eq!(
            response["ingestConfigurations"],
            json!([]),
            "case {case:?}: expected an empty ingestConfigurations list, got {response}"
        );
        assert_eq!(
            response.get("nextToken"),
            None,
            "case {case:?}: expected no nextToken on the only page, got {response}"
        );
    }
}

/// Whatever the page bound, paging through the listing yields every stored configuration
/// exactly once, in creation order, and only the last page omits `nextToken`.
#[tokio::test]
async fn the_listing_pages_over_every_configuration_in_creation_order() {
    let fixture = Fixture::new().await;
    let expected = fixture.arns();
    let stored = expected.len();

    // Rows differ in the page bound, which is what the rule generalises over: every bound
    // below, at and above the stored count, both published `@range` bounds, and the absent
    // member (the documented default).
    for max_results in [
        Some(MAX_RESULTS_RANGE.0),
        Some(2),
        Some(3),
        Some(stored as i64 - 1),
        Some(stored as i64),
        Some(stored as i64 + 1),
        Some(MAX_RESULTS_RANGE.1),
        None,
    ] {
        let case = format!("maxResults={max_results:?}");
        let bound = max_results.map_or(DEFAULT_PAGE_SIZE, |max| max as usize);
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({});
            if let Some(max) = max_results {
                request["maxResults"] = json!(max);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let response = fixture.emulator.list(&case, &request).await;
            let page = summaries(&response);
            assert!(
                page.len() <= bound,
                "case {case:?}: expected at most {bound} configurations per page, got {}",
                page.len()
            );
            for summary in &page {
                seen.push(member(summary, "arn"));
            }

            pages += 1;
            assert!(pages <= 20, "case {case:?}: pagination did not terminate");
            match response["nextToken"].as_str() {
                Some(token) => {
                    assert!(
                        matches(PAGINATION_TOKEN_PATTERN, token),
                        "case {case:?}: expected a nextToken matching \
                         {PAGINATION_TOKEN_PATTERN}, got {token:?}"
                    );
                    assert_eq!(
                        page.len(),
                        bound,
                        "case {case:?}: expected a full page before a nextToken, got {response}"
                    );
                    next_token = Some(token.to_string());
                }
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: expected every created configuration once, in creation order, \
             got {seen:?}"
        );
    }
}

/// A configuration that goes away leaves the listing, and one created later joins it at
/// the end — the page is read off the store rather than remembered.
#[tokio::test]
async fn the_listing_follows_creates_and_deletes() {
    let fixture = Fixture::new().await;
    let mut expected = fixture.arns();

    // Rows differ in WHERE the configuration that goes away sat: the first, the middle and
    // the last of what the listing reported.
    for index in [0usize, expected.len() / 2, expected.len() - 3] {
        let doomed = expected[index].clone();
        let case = format!("deleting {doomed}");
        let (status, response) = fixture
            .emulator
            .call("DeleteIngestConfiguration", &json!({ "arn": doomed }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteIngestConfiguration must answer HTTP 200, \
             got {status} {response}"
        );
        expected.remove(index);
        let listed = fixture
            .emulator
            .listed_arns(&case, &json!({ "maxResults": 2 }))
            .await;
        assert_eq!(
            listed,
            expected.iter().take(2).cloned().collect::<Vec<String>>(),
            "case {case:?}: expected the first page of what is left, got {listed:?}"
        );
        let listed = fixture.emulator.listed_arns(&case, &json!({})).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the deleted configuration to leave the listing, \
             got {listed:?}"
        );
    }

    // And a configuration created now joins the listing at the end, since the order is
    // creation order.
    let created = fixture
        .emulator
        .create("a late configuration", &json!({ "ingestProtocol": "RTMPS" }))
        .await;
    expected.push(member(&created, "arn"));
    let listed = fixture.emulator.listed_arns("a late configuration", &json!({})).await;
    assert_eq!(
        listed, expected,
        "case \"a late configuration\": expected it listed last, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: a summary and a read agree
// ---------------------------------------------------------------------------

/// Every member a summary carries is the member of the same name
/// `GetIngestConfiguration` reports for that same configuration, and the members the
/// summary shape does not carry are absent from a page.
#[tokio::test]
async fn every_summary_reports_what_a_read_reports() {
    let fixture = Fixture::new().await;

    let listed = fixture.emulator.list("the whole listing", &json!({})).await;
    let page = summaries(&listed);
    assert_eq!(
        page.len(),
        fixture.created.len(),
        "expected every created configuration on one page, got {listed}"
    );

    for (position, summary) in page.iter().enumerate() {
        let arn = member(summary, "arn");
        let case = format!("summary {position} ({arn})");
        let read = fixture.emulator.get(&case, &arn).await;
        assert_eq!(
            summary_members(summary),
            summary_members(&read),
            "case {case:?}: expected the summary to report what the read reports, \
             got {summary} against {read}"
        );
        // The create's own answer is the same configuration too, so a page, a read and a
        // create cannot report three different things.
        assert_eq!(
            summary_members(summary),
            summary_members(&fixture.created[position]),
            "case {case:?}: expected the summary to report what the create reported, \
             got {summary}"
        );
        for absent in MEMBERS_NOT_IN_A_SUMMARY {
            assert!(
                summary.get(absent).is_none(),
                "case {case:?}: {absent} is not a member of IngestConfigurationSummary, \
                 got {summary}"
            );
        }
        // The protocol a summary reports is one of the modelled values, and the state one
        // of the two the model lists.
        let protocol = member(summary, "ingestProtocol");
        assert!(
            INGEST_PROTOCOLS.contains(&protocol.as_str()),
            "case {case:?}: expected one of {INGEST_PROTOCOLS:?}, got {protocol:?}"
        );
        let reported_state = member(summary, "state");
        assert!(
            [PUBLISHING_STATE, NO_PUBLISHER_STATE].contains(&reported_state.as_str()),
            "case {case:?}: expected a modelled IngestConfigurationState, \
             got {reported_state:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the filters
// ---------------------------------------------------------------------------

/// `filterByStageArn` "filters the response list to match the specified stage ARN": the
/// page holds exactly the configurations associated with that stage, in creation order,
/// and a stage no configuration names selects nothing.
#[tokio::test]
async fn filter_by_stage_arn_keeps_the_configurations_of_that_stage() {
    let fixture = Fixture::new().await;
    let arns = fixture.arns();

    // One more stage this account holds but no configuration names, so "nothing matches"
    // is a real case rather than a constructed ARN.
    let unused_stage = fixture
        .emulator
        .create_stage("an unused stage", "unused")
        .await;

    // Rows differ in which stage is filtered for and how many configurations name it: the
    // two stages the fixture associates configurations with, a stage this account holds
    // that none name, and a well-formed ARN no stage answers for at all.
    let mut rows: Vec<(String, String)> = fixture
        .stages
        .iter()
        .map(|stage| (format!("stage {stage}"), stage.clone()))
        .collect();
    rows.push(("a stage no configuration names".to_string(), unused_stage));
    rows.push((
        "a well-formed ARN naming no stage".to_string(),
        format!("arn:aws:ivs:{REGION}:{ACCOUNT}:stage/never-created"),
    ));

    for (case, stage_arn) in rows {
        let expected: Vec<String> = arns
            .iter()
            .enumerate()
            .filter(|(index, _)| fixture.stage_of[*index] == stage_arn)
            .map(|(_, arn)| arn.clone())
            .collect();
        let listed = fixture
            .emulator
            .listed_arns(&case, &json!({ "filterByStageArn": stage_arn }))
            .await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the configurations of that stage, got {listed:?}"
        );

        // The filter narrows the listing; it does not change how it pages. A bound of one
        // walks exactly the same configurations.
        let mut paged: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({ "filterByStageArn": stage_arn, "maxResults": 1 });
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let response = fixture.emulator.list(&case, &request).await;
            paged.extend(summaries(&response).iter().map(|value| member(value, "arn")));
            match response["nextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
            assert!(
                paged.len() <= expected.len(),
                "case {case:?}: pagination did not terminate, got {paged:?}"
            );
        }
        assert_eq!(
            paged, expected,
            "case {case:?}: expected a bound of one to walk the same configurations, \
             got {paged:?}"
        );
    }
}

/// `filterByState` "filters the response list to match the specified state": with nobody
/// publishing every configuration is `INACTIVE`, and the one a publisher is broadcasting
/// with moves to `ACTIVE` — so each value of the modelled state selects exactly the
/// configurations a read reports in that state.
#[tokio::test]
async fn filter_by_state_keeps_the_configurations_in_that_state() {
    let fixture = Fixture::new().await;

    // Rows differ in whether a publisher is broadcasting: before the join every
    // configuration is INACTIVE, after it exactly one is ACTIVE.
    for stage in ["nobody publishing", "one publisher broadcasting"] {
        if stage == "one publisher broadcasting" {
            // The publisher reaches the stage with the configuration's own ARN, which is
            // one of the identities a publisher can carry.
            let publishing = member(&fixture.created[1], "arn");
            fixture.emulator.join(
                stage,
                &fixture.stages[0],
                state::JoinedParticipant {
                    participant_id: "broadcaster-1".to_string(),
                    published: true,
                    ingest_configuration_arn: Some(publishing),
                    ..Default::default()
                },
            );
        }

        // What a READ reports for each configuration is the expectation the filter is
        // held to: the two must agree at the same instant.
        let mut expected_by_state: Vec<(String, String)> = Vec::new();
        for value in &fixture.created {
            let arn = member(value, "arn");
            let read = fixture.emulator.get(stage, &arn).await;
            expected_by_state.push((arn, member(&read, "state")));
        }
        if stage == "one publisher broadcasting" {
            let publishing_arn = member(&fixture.created[1], "arn");
            let reported = expected_by_state
                .iter()
                .find(|(arn, _)| *arn == publishing_arn)
                .map(|(_, reported)| reported.clone())
                .unwrap_or_default();
            assert_eq!(
                reported, PUBLISHING_STATE,
                "case {stage:?}: the premise of the case is that {publishing_arn} reports \
                 {PUBLISHING_STATE:?}, got {reported:?}"
            );
        }

        for wanted in [PUBLISHING_STATE, NO_PUBLISHER_STATE] {
            let case = format!("{stage} / filterByState={wanted}");
            let expected: Vec<String> = expected_by_state
                .iter()
                .filter(|(_, reported)| reported == wanted)
                .map(|(arn, _)| arn.clone())
                .collect();
            let listed = fixture
                .emulator
                .listed_arns(&case, &json!({ "filterByState": wanted }))
                .await;
            assert_eq!(
                listed, expected,
                "case {case:?}: expected the configurations a read reports as {wanted}, \
                 got {listed:?}"
            );
        }
    }
}

/// "Only one filter (by stage ARN or by state) can be used at a time": a request carrying
/// both is refused with the modelled `ValidationException` (HTTP 400) naming the members,
/// and reports no page at all — while either filter alone is served.
#[tokio::test]
async fn a_request_carrying_both_filters_is_refused() {
    let fixture = Fixture::new().await;
    let expected_message = "ListIngestConfigurations is required to have at most one of \
         filterByStageArn, filterByState, found filterByStageArn, filterByState";

    // Rows differ in which stage and which state the two filters name, including a pair
    // that would have matched something and a pair that would have matched nothing.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a stage and a state that both match configurations",
            json!({
                "filterByStageArn": fixture.stages[0],
                "filterByState": NO_PUBLISHER_STATE,
            }),
        ),
        (
            "a stage that matches and a state nothing is in",
            json!({
                "filterByStageArn": fixture.stages[1],
                "filterByState": PUBLISHING_STATE,
            }),
        ),
        (
            "both filters beside a page bound",
            json!({
                "filterByStageArn": fixture.stages[0],
                "filterByState": NO_PUBLISHER_STATE,
                "maxResults": 1,
            }),
        ),
    ];
    for (case, request) in rows {
        let (status, response) = fixture
            .emulator
            .call("ListIngestConfigurations", &request)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {request}, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            expected_message,
            "case {case:?}: expected the refusal to name both members, got {}",
            error_message(&response)
        );
        assert!(
            response.get("ingestConfigurations").is_none(),
            "case {case:?}: a refused listing reports no page, got {response}"
        );
    }

    // Either filter ALONE is served, which is what makes the refusal about the pair.
    for request in [
        json!({ "filterByStageArn": fixture.stages[0] }),
        json!({ "filterByState": NO_PUBLISHER_STATE }),
    ] {
        let case = format!("one filter only: {request}");
        let response = fixture.emulator.list(&case, &request).await;
        assert!(
            !summaries(&response).is_empty(),
            "case {case:?}: expected the fixture's configurations, got {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the listing is a read
// ---------------------------------------------------------------------------

/// Listing is a read: it changes nothing a later caller sees. Repeated pages over an
/// unchanged store are identical, whatever the bound and the filter, and a read of a
/// configuration in between answers the same too.
#[tokio::test]
async fn listing_changes_nothing() {
    let fixture = Fixture::new().await;
    let probe_arn = member(&fixture.created[1], "arn");

    // Rows differ in the request repeated: the whole listing, a bounded page, and each
    // filter on its own.
    let requests = [
        json!({}),
        json!({ "maxResults": 2 }),
        json!({ "filterByStageArn": fixture.stages[0] }),
        json!({ "filterByState": NO_PUBLISHER_STATE }),
    ];
    for request in requests {
        let case = format!("request {request}");
        let first = fixture.emulator.list(&case, &request).await;
        let before = fixture.emulator.get(&case, &probe_arn).await;
        let second = fixture.emulator.list(&case, &request).await;
        let after = fixture.emulator.get(&case, &probe_arn).await;
        assert_eq!(
            first, second,
            "case {case:?}: expected the second page to be identical, got {second}"
        );
        assert_eq!(
            before, after,
            "case {case:?}: expected listing to leave the configuration where it was, \
             got {after}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: refusals
// ---------------------------------------------------------------------------

/// `maxResults` is held to the modelled `@range(1..100)`: both published bounds are
/// accepted and a value just past either is refused with the modelled
/// `ValidationException`.
#[tokio::test]
async fn the_listing_holds_the_modeled_max_results_range() {
    let fixture = Fixture::new().await;

    let (minimum, maximum) = MAX_RESULTS_RANGE;
    let rows: Vec<(&str, i64, u16)> = vec![
        ("the published minimum", minimum, 200),
        ("the published maximum", maximum, 200),
        ("one below the minimum", minimum - 1, 400),
        ("one above the maximum", maximum + 1, 400),
        ("far below the minimum", -1, 400),
        ("far above the maximum", 1_000, 400),
    ];
    for (case, max_results, expected_status) in rows {
        let (status, response) = fixture
            .emulator
            .call(
                "ListIngestConfigurations",
                &json!({ "maxResults": max_results }),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?} (maxResults={max_results}): expected HTTP {expected_status}, \
             got {status} {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {response}"
            );
        }
    }
}

/// A pagination token this listing did not issue names no position, so it is refused
/// rather than read as "start again" — a token another listing of this same service
/// issued included — while the empty token is the first page and its own token resumes.
#[tokio::test]
async fn the_listing_refuses_a_pagination_token_it_did_not_issue() {
    let fixture = Fixture::new().await;
    let stored = fixture.created.len();

    // A token from the stage listing, which pages over a different store: it names no
    // position here.
    let (status, stage_page) = fixture
        .emulator
        .call("ListStages", &json!({ "maxResults": 1 }))
        .await;
    assert_eq!(
        status, 200,
        "fixture ListStages must answer HTTP 200, got {status} {stage_page}"
    );
    let foreign_token = stage_page["nextToken"]
        .as_str()
        .expect("a nextToken while stages remain")
        .to_string();

    // Rows differ in how the token is wrong: not base64, base64 of foreign text, base64
    // of a marker with a non-numeric position, a token issued by another listing of this
    // service — and the empty token, which means the first page.
    let rows: Vec<(&str, String, u16)> = vec![
        ("not base64 at all", "++++".to_string(), 400),
        (
            "base64 of foreign text",
            "c29tZXRoaW5nLWVsc2U=".to_string(),
            400,
        ),
        (
            "base64 of a marker with a non-numeric position",
            "aXZzLXJlYWx0aW1lLXN0b3JhZ2UtY29uZmlndXJhdGlvbi1hZnRlcjpub3Bl".to_string(),
            400,
        ),
        ("a token issued by ListStages", foreign_token, 400),
        (
            "the empty token, which means the first page",
            String::new(),
            200,
        ),
    ];
    for (case, token, expected_status) in rows {
        let (status, response) = fixture
            .emulator
            .call("ListIngestConfigurations", &json!({ "nextToken": token }))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {response}"
            );
        } else {
            assert_eq!(
                summaries(&response).len(),
                stored,
                "case {case:?}: expected the first page of every configuration, \
                 got {response}"
            );
        }
    }

    // Its OWN token resumes where the page ended: the configurations it reports are the
    // ones the first page did not.
    let first = fixture
        .emulator
        .list("its own token", &json!({ "maxResults": 1 }))
        .await;
    let resumed = fixture
        .emulator
        .listed_arns(
            "its own token",
            &json!({ "nextToken": first["nextToken"].as_str().unwrap_or_default() }),
        )
        .await;
    assert_eq!(
        resumed,
        fixture.arns()[1..].to_vec(),
        "case \"its own token\": expected the configurations the first page left, \
         got {resumed:?}"
    );
}
