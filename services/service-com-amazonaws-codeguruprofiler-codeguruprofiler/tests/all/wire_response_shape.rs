// SPDX-License-Identifier: CC0-1.0

//! The WIRE SHAPE of this service's successful response bodies, end to end over HTTP
//! through the router this service mounts in production.
//!
//! The two rules every case here asserts:
//!
//! 1. a member whose model gives it `@timestampFormat("date-time")` goes onto the
//!    restJson1 wire as an ISO-8601 / RFC 3339 date-time STRING naming the stored instant
//!    — never as restJson1's default epoch-seconds NUMBER. Every `Timestamp` of this
//!    service is `com.amazonaws.codeguruprofiler#Timestamp`, which carries that trait
//!    (`smithy_query(query="shape", shape="Timestamp")`), so the rule covers every
//!    timestamp member of every response, not one operation's pair;
//! 2. a member the service has nothing to report for is ABSENT from the body, never
//!    present as an empty container: `profilingStatus` is left out of a CREATE's payload
//!    while nothing has been reported about the group (a read of the stored group carries
//!    the container either way), `recommendations`/`anomalies` are left out until an
//!    analysis has findings, and `profilingGroupNames`/`profilingGroups` follow the
//!    `includeDescription` toggle the model documents.
//!
//! Rule 1 is asserted both end to end (every date-time member of every response an
//! implemented operation can produce) and directly on the member schemas the JSON codec
//! reads the wire form off, which is the only reach the shapes whose operations are still
//! scaffolded 501s (`TimestampStructure`, `BatchGetFrameMetricDataResponse`,
//! `Recommendation`, `AnomalyInstance`, `AggregatedProfileTime`) have.
//!
//! The harness here is deliberately this suite's own; nothing in `tests/` is shared
//! between suites except where a suite names another's explicitly.

use std::sync::Arc;

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::Schema;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::Value;
use service_com_amazonaws_codeguruprofiler_codeguruprofiler::ops::types::{
    AggregatedProfileTime, AnomalyInstance, BatchGetFrameMetricDataOutput, FindingsReportSummary,
    GetRecommendationsOutput, ListProfilingGroupsOutput, ProfileTime, ProfilingGroupDescription,
    ProfilingStatus, Recommendation, TimestampStructure,
};
use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::FindingsReport;
use service_com_amazonaws_codeguruprofiler_codeguruprofiler::{
    ComAmazonawsCodeguruprofilerCodeguruprofilerState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// A signed-looking request header, so the router's `can_handle` resolves this service by
/// its SigV4 credential scope the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/codeguru-profiler/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// Every member name this service puts a `@timestampFormat("date-time")` timestamp under,
/// as the wire spells it. Source:
/// `smithy_query(query="shape", shape="ProfilingGroupDescription,ProfilingStatus,AggregatedProfileTime,FindingsReportSummary,GetRecommendationsResponse,Recommendation,AnomalyInstance,ProfileTime,TimestampStructure,BatchGetFrameMetricDataResponse")`
/// — every member of this service targeting `com.amazonaws.codeguruprofiler#Timestamp`.
/// The body walk below holds every value under one of these names to rule 1, whichever
/// operation produced it and however deeply nested it is.
const DATE_TIME_MEMBERS: &[&str] = &[
    "createdAt",
    "updatedAt",
    "latestAgentProfileReportedAt",
    "latestAgentOrchestratedAt",
    "start",
    "profileStartTime",
    "profileEndTime",
    "startTime",
    "endTime",
    "value",
];

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service. The engine is kept so a test can place stored instants the clock
/// cannot be asked for, and record analysis through the store's own writer.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn send(&self, method: &str, uri: &str, body: Vec<u8>) -> Answer {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "codeguru-profiler.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        Answer {
            status: response.status().as_u16(),
            body: response.into_body().to_vec(),
        }
    }

    async fn create(&self, name: &str) -> Answer {
        self.send(
            "POST",
            &format!("/profilingGroups?clientToken=tok-{name}"),
            serde_json::to_vec(&serde_json::json!({"profilingGroupName": name}))
                .expect("a serializable body"),
        )
        .await
    }

    async fn describe(&self, name: &str) -> Answer {
        self.send("GET", &format!("/profilingGroups/{name}"), Vec::new())
            .await
    }

    async fn update(&self, name: &str, profiling_enabled: bool) -> Answer {
        self.send(
            "PUT",
            &format!("/profilingGroups/{name}"),
            serde_json::to_vec(&serde_json::json!({
                "agentOrchestrationConfig": {"profilingEnabled": profiling_enabled}
            }))
            .expect("a serializable body"),
        )
        .await
    }

    async fn list(&self, query: &str) -> Answer {
        self.send("GET", &format!("/profilingGroups{query}"), Vec::new())
            .await
    }

    async fn configure_agent(&self, name: &str) -> Answer {
        self.send(
            "POST",
            &format!("/profilingGroups/{name}/configureAgent"),
            b"{}".to_vec(),
        )
        .await
    }

    async fn post_agent_profile(&self, name: &str, body: Vec<u8>) -> Answer {
        self.send("POST", &format!("/profilingGroups/{name}/agentProfile"), body)
            .await
    }

    async fn get_recommendations(&self, name: &str, start: i64, end: i64) -> Answer {
        self.send(
            "GET",
            &format!(
                "/internal/profilingGroups/{name}/recommendations?startTime={}&endTime={}",
                iso(start),
                iso(end)
            ),
            Vec::new(),
        )
        .await
    }

    async fn list_profile_times(&self, name: &str, start: i64, end: i64, period: &str) -> Answer {
        self.send(
            "GET",
            &format!(
                "/profilingGroups/{name}/profileTimes?startTime={}&endTime={}&period={period}",
                iso(start),
                iso(end)
            ),
            Vec::new(),
        )
        .await
    }

    async fn list_findings_reports(&self, name: &str, start: i64, end: i64) -> Answer {
        self.send(
            "GET",
            &format!(
                "/internal/profilingGroups/{name}/findingsReports?startTime={}&endTime={}",
                iso(start),
                iso(end)
            ),
            Vec::new(),
        )
        .await
    }

    fn state(&self) -> engine::ServiceState<ComAmazonawsCodeguruprofilerCodeguruprofilerState> {
        self.engine
            .state::<ComAmazonawsCodeguruprofilerCodeguruprofilerState>()
    }

    /// Place a group's stored `createdAt`/`updatedAt` at chosen instants. The clock cannot
    /// be asked for a past instant or for a chosen sub-second part, so this writes the
    /// same stored fields a create and an update write, and nothing else.
    fn stamp_group_times(&self, name: &str, created_at: DateTime, updated_at: DateTime) {
        let state = self.state();
        let mut guard = state.lock();
        let group = guard
            .groups
            .get_mut(name)
            .unwrap_or_else(|| panic!("no stored group named {name}"));
        group.created_at = created_at;
        group.updated_at = updated_at;
    }

    /// Place the receipt time of the one profile a group retains, and the group's
    /// `latestAgentProfileReportedAt`, at a chosen instant — the stored fields
    /// `PostAgentProfile` itself writes.
    fn stamp_receipt_time(&self, name: &str, at: DateTime) {
        let state = self.state();
        let mut guard = state.lock();
        let group = guard
            .groups
            .get_mut(name)
            .unwrap_or_else(|| panic!("no stored group named {name}"));
        for profile in group.profiles.iter_mut() {
            profile.received_at = at;
        }
        group.latest_agent_profile_reported_at = Some(at);
    }

    /// Record one analysis report against a group, through the store's own writer.
    fn record_report(&self, name: &str, report: FindingsReport) {
        let state = self.state();
        let mut guard = state.lock();
        assert!(
            guard.record_findings_report(name, report),
            "recording analysis against {name} must find the group"
        );
    }
}

/// An instant in the ISO-8601 form the modelled `date-time` query members carry.
fn iso(secs: i64) -> String {
    DateTime::from_secs(secs)
        .fmt(Format::DateTime)
        .expect("a formattable instant")
}

/// Hold one JSON value to rule 1: it is a STRING an ISO-8601 / RFC 3339 date-time parser
/// accepts. Returns the instant it names so a caller can compare it with what is stored.
fn assert_date_time_string(case: &str, member: &str, value: &Value) -> DateTime {
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {member} is modelled @timestampFormat(\"date-time\"), so it must be \
             an ISO-8601 string, got {value}"
        )
    });
    DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} {text:?} must parse as a date-time: {error}")
    })
}

/// Walk a whole response body and hold EVERY value under a [`DATE_TIME_MEMBERS`] name to
/// rule 1, at any nesting depth. Returns how many such members the body carried, so a
/// case can refuse to pass vacuously.
fn assert_every_date_time_member(case: &str, body: &Value) -> usize {
    fn walk(case: &str, path: &str, value: &Value, found: &mut usize) {
        match value {
            Value::Object(members) => {
                for (key, member) in members {
                    let child = format!("{path}.{key}");
                    if DATE_TIME_MEMBERS.contains(&key.as_str()) {
                        assert_date_time_string(case, &child, member);
                        *found += 1;
                    } else {
                        walk(case, &child, member, found);
                    }
                }
            }
            Value::Array(elements) => {
                for (index, element) in elements.iter().enumerate() {
                    walk(case, &format!("{path}[{index}]"), element, found);
                }
            }
            _ => {}
        }
    }

    let mut found = 0usize;
    walk(case, "$", body, &mut found);
    found
}

/// The instants a stamped group is driven through: rows differing in sub-second part
/// (none, the documented one-millisecond example, a fraction of a second, nearly a whole
/// second), in era (before the epoch, the epoch itself, now-ish, far future) and in
/// whether `createdAt` and `updatedAt` are the same instant.
///
/// `(case, created_at, updated_at)`.
fn stamped_instants() -> Vec<(&'static str, DateTime, DateTime)> {
    vec![
        (
            "the epoch itself, created and updated together",
            DateTime::from_secs(0),
            DateTime::from_secs(0),
        ),
        (
            "a whole second, updated an hour later",
            DateTime::from_secs(1_590_969_600),
            DateTime::from_secs(1_590_973_200),
        ),
        (
            // The model's own example: "2020-06-01T13:15:02.001Z represents 1 millisecond
            // past June 1, 2020 1:15:02 PM UTC" — 1_591_017_302 epoch seconds.
            "one millisecond past the second",
            DateTime::from_secs_and_nanos(1_591_017_302, 1_000_000),
            DateTime::from_secs_and_nanos(1_591_017_302, 1_000_000),
        ),
        (
            "a fraction of a second apart",
            DateTime::from_secs_and_nanos(1_792_704_389, 34_000_000),
            DateTime::from_secs_and_nanos(1_792_704_389, 567_000_000),
        ),
        (
            "an instant before the epoch",
            DateTime::from_secs(-86_400),
            DateTime::from_secs(-1),
        ),
        (
            "a far-future instant, nearly a whole second in",
            DateTime::from_secs_and_nanos(4_102_444_800, 999_000_000),
            DateTime::from_secs_and_nanos(4_102_444_800, 999_000_000),
        ),
    ]
}

/// Two instants are the same instant, to the nanosecond the stored `DateTime` carries.
fn same_instant(left: DateTime, right: DateTime) -> bool {
    left.secs() == right.secs() && left.subsec_nanos() == right.subsec_nanos()
}

#[tokio::test]
async fn every_profiling_group_timestamp_is_an_iso8601_string_naming_the_stored_instant() {
    for (case, created_at, updated_at) in stamped_instants() {
        let harness = Harness::new();
        let name = "stamped-group";
        assert_eq!(
            harness.create(name).await.status,
            201,
            "case {case:?}: create must answer 201"
        );
        harness.stamp_group_times(name, created_at, updated_at);

        // Every read path of a profiling-group description: the create's own 201 payload is
        // asserted separately below (it reports the clock, not a stamped instant), so these
        // are the three that report the stamped record.
        let described = harness.describe(name).await;
        assert_eq!(described.status, 200, "case {case:?}: describe must answer 200");
        let listed = harness.list("?includeDescription=true").await;
        assert_eq!(listed.status, 200, "case {case:?}: list must answer 200");
        let listed_description = listed
            .json()
            .get("profilingGroups")
            .and_then(|groups| groups.get(0))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no described group in {}", listed.json()));

        for (label, description) in [
            ("describe", described.json()),
            ("list(includeDescription=true)", listed_description),
        ] {
            let found = assert_every_date_time_member(&format!("{case} via {label}"), &description);
            assert_eq!(
                found, 2,
                "case {case:?} via {label}: the description must carry createdAt and updatedAt, \
                 got {description}"
            );
            for (member, expected) in [("createdAt", created_at), ("updatedAt", updated_at)] {
                let value = description
                    .get(member)
                    .unwrap_or_else(|| panic!("case {case:?} via {label}: no {member}"));
                let parsed = assert_date_time_string(case, member, value);
                assert!(
                    same_instant(parsed, expected),
                    "case {case:?} via {label}: {member} must name the stored instant \
                     ({}s +{}ns), got {value}",
                    expected.secs(),
                    expected.subsec_nanos()
                );
            }
        }
    }
}

#[tokio::test]
async fn the_create_and_update_payloads_report_their_timestamps_as_date_time_strings() {
    let harness = Harness::new();

    // Rows: the operations that answer with a freshly written description, so the
    // timestamps come from the clock rather than from a stamped record. Each must still be
    // a date-time string, and the pair must stay ordered.
    let name = "clock-stamped";
    let created = harness.create(name).await;
    assert_eq!(created.status, 201, "create must answer 201");
    let updated = harness.update(name, false).await;
    assert_eq!(
        updated.status, 200,
        "update must answer 200, got {}",
        String::from_utf8_lossy(&updated.body)
    );

    for (case, body) in [
        ("CreateProfilingGroup", created.json()),
        ("UpdateProfilingGroup", updated.json()),
    ] {
        let found = assert_every_date_time_member(case, &body);
        assert_eq!(
            found, 2,
            "case {case:?}: the payload must carry createdAt and updatedAt, got {body}"
        );
        let created_at = assert_date_time_string(
            case,
            "createdAt",
            body.get("createdAt")
                .unwrap_or_else(|| panic!("case {case:?}: no createdAt in {body}")),
        );
        let updated_at = assert_date_time_string(
            case,
            "updatedAt",
            body.get("updatedAt")
                .unwrap_or_else(|| panic!("case {case:?}: no updatedAt in {body}")),
        );
        assert!(
            updated_at.as_secs_f64() >= created_at.as_secs_f64(),
            "case {case:?}: updatedAt must not precede createdAt, got {body}"
        );
    }
}

#[tokio::test]
async fn every_timestamp_of_the_analysis_and_listing_responses_is_a_date_time_string() {
    let harness = Harness::new();
    let name = "analysed-group";
    assert_eq!(harness.create(name).await.status, 201, "create must succeed");
    assert_eq!(
        harness.configure_agent(name).await.status,
        200,
        "ConfigureAgent must succeed"
    );
    assert_eq!(
        harness
            .post_agent_profile(name, br#"{"frames":["A"]}"#.to_vec())
            .await
            .status,
        204,
        "PostAgentProfile must succeed"
    );
    // A stamped receipt time is what puts the retained profile, and so every instant the
    // listings report, at a known sub-second instant.
    let received_at = DateTime::from_secs_and_nanos(1_590_969_605, 25_000_000);
    harness.stamp_receipt_time(name, received_at);
    harness.record_report(
        name,
        FindingsReport {
            id: "11111111-1111-4111-8111-111111111111".to_string(),
            profile_start_time: DateTime::from_secs(1_590_969_600),
            profile_end_time: DateTime::from_secs_and_nanos(1_590_973_200, 1_000_000),
            total_number_of_findings: 3,
            aggregation_period: "PT1H".to_string(),
        },
    );

    // Rows: every OTHER implemented operation whose response body carries a date-time
    // member, with how many such members the body must hold. The aggregation periods are
    // the three the model's `AggregationPeriod` enum names, so the listing is exercised on
    // each of them rather than on one.
    let window = (1_590_969_600_i64, 1_590_973_200_i64);
    let mut cases: Vec<(String, Value, usize)> = Vec::new();

    let recommendations = harness.get_recommendations(name, window.0, window.1).await;
    assert_eq!(
        recommendations.status,
        200,
        "GetRecommendations must answer 200, got {}",
        String::from_utf8_lossy(&recommendations.body)
    );
    cases.push((
        "GetRecommendations".to_string(),
        recommendations.json(),
        2, // profileStartTime, profileEndTime
    ));

    for period in ["PT5M", "PT1H", "P1D"] {
        let times = harness
            .list_profile_times(name, window.0 - 86_400, window.1 + 86_400, period)
            .await;
        assert_eq!(
            times.status,
            200,
            "ListProfileTimes({period}) must answer 200, got {}",
            String::from_utf8_lossy(&times.body)
        );
        cases.push((
            format!("ListProfileTimes({period})"),
            times.json(),
            1, // the one window the one retained profile falls in
        ));
    }

    let reports = harness.list_findings_reports(name, window.0, window.1).await;
    assert_eq!(
        reports.status,
        200,
        "ListFindingsReports must answer 200, got {}",
        String::from_utf8_lossy(&reports.body)
    );
    cases.push((
        "ListFindingsReports".to_string(),
        reports.json(),
        2, // profileStartTime, profileEndTime of the one recorded report
    ));

    let described = harness.describe(name).await;
    assert_eq!(described.status, 200, "describe must answer 200");
    cases.push((
        "DescribeProfilingGroup with agent activity".to_string(),
        described.json(),
        4, // createdAt, updatedAt, and the two profilingStatus instants
    ));

    for (case, body, expected_members) in cases {
        let found = assert_every_date_time_member(&case, &body);
        assert_eq!(
            found, expected_members,
            "case {case:?}: expected {expected_members} date-time members, found {found} in \
             {body}"
        );
    }

    // The analysed span the listings report is the stamped receipt instant, to the
    // nanosecond — so the string carries the stored instant and not a rounded one.
    let recommendations = harness
        .get_recommendations(name, window.0, window.1)
        .await
        .json();
    for member in ["profileStartTime", "profileEndTime"] {
        let value = recommendations
            .get(member)
            .unwrap_or_else(|| panic!("no {member} in {recommendations}"));
        let parsed = assert_date_time_string("the analysed span", member, value);
        assert!(
            same_instant(parsed, received_at),
            "{member} must name the stored receipt instant ({}s +{}ns), got {value}",
            received_at.secs(),
            received_at.subsec_nanos()
        );
    }
}

/// The member schema the JSON codec reads the wire form off: every member this service
/// models `@timestampFormat("date-time")` reports that format, and a member the model
/// leaves alone reports none.
///
/// This is the only reach the shapes whose operations are still scaffolded 501s
/// (`TimestampStructure` and `BatchGetFrameMetricDataResponse`, reached only by
/// `BatchGetFrameMetricData`) and the analysis shapes no implemented path fills
/// (`Recommendation`, `AnomalyInstance`, `AggregatedProfileTime`) have, and it is what
/// would fail if any of these schemas went back to the vendored codegen's output.
#[test]
fn every_modelled_date_time_member_schema_carries_the_date_time_format() {
    // Source: every member of this service targeting
    // `com.amazonaws.codeguruprofiler#Timestamp`, from
    // `smithy_query(query="shape", shape="...")` on each shape below.
    let date_time_members: Vec<(&str, &Schema)> = vec![
        (
            "ProfilingGroupDescription$createdAt",
            &ProfilingGroupDescription::M_CREATED_AT,
        ),
        (
            "ProfilingGroupDescription$updatedAt",
            &ProfilingGroupDescription::M_UPDATED_AT,
        ),
        (
            "ProfilingStatus$latestAgentProfileReportedAt",
            &ProfilingStatus::M_LATEST_AGENT_PROFILE_REPORTED_AT,
        ),
        (
            "ProfilingStatus$latestAgentOrchestratedAt",
            &ProfilingStatus::M_LATEST_AGENT_ORCHESTRATED_AT,
        ),
        ("AggregatedProfileTime$start", &AggregatedProfileTime::M_START),
        (
            "FindingsReportSummary$profileStartTime",
            &FindingsReportSummary::M_PROFILE_START_TIME,
        ),
        (
            "FindingsReportSummary$profileEndTime",
            &FindingsReportSummary::M_PROFILE_END_TIME,
        ),
        (
            "GetRecommendationsResponse$profileStartTime",
            &GetRecommendationsOutput::M_PROFILE_START_TIME,
        ),
        (
            "GetRecommendationsResponse$profileEndTime",
            &GetRecommendationsOutput::M_PROFILE_END_TIME,
        ),
        ("Recommendation$startTime", &Recommendation::M_START_TIME),
        ("Recommendation$endTime", &Recommendation::M_END_TIME),
        ("AnomalyInstance$startTime", &AnomalyInstance::M_START_TIME),
        ("AnomalyInstance$endTime", &AnomalyInstance::M_END_TIME),
        ("ProfileTime$start", &ProfileTime::M_START),
        ("TimestampStructure$value", &TimestampStructure::M_VALUE),
        (
            "BatchGetFrameMetricDataResponse$startTime",
            &BatchGetFrameMetricDataOutput::M_START_TIME,
        ),
        (
            "BatchGetFrameMetricDataResponse$endTime",
            &BatchGetFrameMetricDataOutput::M_END_TIME,
        ),
    ];

    for (case, schema) in date_time_members {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case:?}: the member schema must carry @timestampFormat(date-time), got \
             {format:?}"
        );
    }

    // A member the model does not annotate keeps the protocol default, so the fix put no
    // format where the model names none.
    for (case, schema) in [
        ("ProfilingGroupDescription$name", &ProfilingGroupDescription::M_NAME),
        ("ProfilingGroupDescription$tags", &ProfilingGroupDescription::M_TAGS),
        ("AggregatedProfileTime$period", &AggregatedProfileTime::M_PERIOD),
        (
            "ListProfilingGroupsResponse$profilingGroupNames",
            &ListProfilingGroupsOutput::M_PROFILING_GROUP_NAMES,
        ),
    ] {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            format.is_none(),
            "case {case:?}: the model annotates no format for this member, got {format:?}"
        );
    }
}

#[tokio::test]
async fn a_create_omits_profiling_status_until_something_has_been_reported() {
    // Rows: what has reached the group, and which `profilingStatus` instants that activity
    // owes. The CREATE payload carries the member only once there is activity to report in
    // it (a create replayed on its `clientToken` is the path that can see any); a READ of
    // the stored group carries the container either way, holding exactly the instants the
    // activity owes.
    #[derive(Debug)]
    struct Case {
        label: &'static str,
        configure_agent: bool,
        post_profile: bool,
        expected_instants: &'static [&'static str],
    }

    let cases = [
        Case {
            label: "a freshly created group",
            configure_agent: false,
            post_profile: false,
            expected_instants: &[],
        },
        Case {
            label: "an agent has pinged back",
            configure_agent: true,
            post_profile: false,
            expected_instants: &["latestAgentOrchestratedAt"],
        },
        Case {
            label: "a profile has been posted",
            configure_agent: false,
            post_profile: true,
            expected_instants: &["latestAgentProfileReportedAt"],
        },
        Case {
            label: "both an agent ping and a posted profile",
            configure_agent: true,
            post_profile: true,
            expected_instants: &["latestAgentOrchestratedAt", "latestAgentProfileReportedAt"],
        },
    ];

    for case in cases {
        let harness = Harness::new();
        let name = "status-group";
        let created = harness.create(name).await;
        assert_eq!(created.status, 201, "case {case:?}: create must answer 201");
        assert_eq!(
            created.json().get("profilingStatus"),
            None,
            "case {case:?}: a create reports no profiling activity, so the member must be \
             absent, got {}",
            created.json()
        );

        if case.configure_agent {
            assert_eq!(
                harness.configure_agent(name).await.status,
                200,
                "case {case:?}: ConfigureAgent must succeed"
            );
        }
        if case.post_profile {
            assert_eq!(
                harness
                    .post_agent_profile(name, br#"{"frames":["A"]}"#.to_vec())
                    .await
                    .status,
                204,
                "case {case:?}: PostAgentProfile must succeed"
            );
        }

        // The read path reports the container whatever has happened, holding exactly the
        // instants the activity owes and no others.
        let description = harness.describe(name).await.json();
        let status = description
            .get("profilingStatus")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: no profilingStatus in {description}"));
        assert_eq!(
            status.len(),
            case.expected_instants.len(),
            "case {case:?}: the status must report exactly what happened, got {status:?}"
        );
        for member in case.expected_instants {
            let value = status
                .get(*member)
                .unwrap_or_else(|| panic!("case {case:?}: no {member} in {status:?}"));
            assert_date_time_string(case.label, member, value);
        }

        // Replaying the create on its own `clientToken` answers with the group that token
        // created: the member is there only once there is activity to report.
        let replayed = harness.create(name).await;
        assert_eq!(
            replayed.status, 201,
            "case {case:?}: replaying the clientToken must answer 201, got {}",
            String::from_utf8_lossy(&replayed.body)
        );
        let replayed_status = replayed.json().get("profilingStatus").cloned();
        if case.expected_instants.is_empty() {
            assert_eq!(
                replayed_status,
                None,
                "case {case:?}: with nothing reported the create payload must omit \
                 profilingStatus, not send an empty object, got {}",
                replayed.json()
            );
        } else {
            assert_eq!(
                replayed_status.as_ref().and_then(Value::as_object).map(|o| o.len()),
                Some(case.expected_instants.len()),
                "case {case:?}: with activity reported the create payload must carry it, got {}",
                replayed.json()
            );
        }
    }
}

#[tokio::test]
async fn include_description_toggles_which_list_the_body_carries() {
    // Rows: how many groups are stored, what `includeDescription` says, and how the page
    // is sized — the key present must be `profilingGroups` when descriptions were asked
    // for and `profilingGroupNames` when they were not, never both and never neither.
    for group_count in [0usize, 1, 3] {
        let harness = Harness::new();
        for index in 0..group_count {
            let name = format!("group-{index}");
            assert_eq!(
                harness.create(&name).await.status,
                201,
                "creating {name} must answer 201"
            );
        }

        for (case, flag_query, wants_descriptions) in [
            ("includeDescription absent", "", false),
            ("includeDescription=false", "?includeDescription=false", false),
            ("includeDescription=true", "?includeDescription=true", true),
        ] {
            for page in ["", "&maxResults=1", "&maxResults=1000"] {
                let query = if flag_query.is_empty() {
                    page.replacen('&', "?", 1)
                } else {
                    format!("{flag_query}{page}")
                };
                let answer = harness.list(&query).await;
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?} ({group_count} groups, {query:?}): must answer 200, got {}",
                    String::from_utf8_lossy(&answer.body)
                );
                let body = answer.json();
                let (present, absent) = if wants_descriptions {
                    ("profilingGroups", "profilingGroupNames")
                } else {
                    ("profilingGroupNames", "profilingGroups")
                };
                assert!(
                    body.get(present).and_then(Value::as_array).is_some(),
                    "case {case:?} ({group_count} groups, {query:?}): {present} must be the key \
                     the body carries, got {body}"
                );
                assert_eq!(
                    body.get(absent),
                    None,
                    "case {case:?} ({group_count} groups, {query:?}): {absent} must be absent, \
                     got {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_analysis_with_nothing_to_report_omits_both_analysis_lists() {
    let harness = Harness::new();
    // Rows: a group that has never been profiled and one that retains a profile, over
    // windows of different widths and positions. No analyser runs over the stored profiles
    // either way, so neither list is ever reported — present-and-empty is the answer a
    // client reads as "analysis ran and found nothing".
    let bare = "bare-group";
    let profiled = "profiled-group";
    for name in [bare, profiled] {
        assert_eq!(
            harness.create(name).await.status,
            201,
            "creating {name} must answer 201"
        );
    }
    assert_eq!(
        harness
            .post_agent_profile(profiled, br#"{"frames":["A"]}"#.to_vec())
            .await
            .status,
        204,
        "PostAgentProfile must succeed"
    );
    harness.stamp_receipt_time(profiled, DateTime::from_secs(1_590_969_900));

    let cases: Vec<(&str, &str, i64, i64)> = vec![
        ("a group with no profiles, hour-long window", bare, 1_590_969_600, 1_590_973_200),
        ("a group with no profiles, one-second window", bare, 1_590_969_600, 1_590_969_601),
        ("a profiled group, window around the profile", profiled, 1_590_969_600, 1_590_973_200),
        ("a profiled group, window before the profile", profiled, 1_590_000_000, 1_590_000_600),
        ("a profiled group, decade-long window", profiled, 1_300_000_000, 1_600_000_000),
    ];

    for (case, name, start, end) in cases {
        let answer = harness.get_recommendations(name, start, end).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: must answer 200, got {}",
            String::from_utf8_lossy(&answer.body)
        );
        let body = answer.json();
        for member in ["recommendations", "anomalies"] {
            assert_eq!(
                body.get(member),
                None,
                "case {case:?}: {member} must be absent when the analysis has nothing to \
                 report, got {body}"
            );
        }
        assert_eq!(
            body.get("profilingGroupName").and_then(Value::as_str),
            Some(name),
            "case {case:?}: the reported group must be the stored one, got {body}"
        );
        assert_eq!(
            assert_every_date_time_member(case, &body),
            2,
            "case {case:?}: the body must still carry both analysed-span instants, got {body}"
        );
    }
}
