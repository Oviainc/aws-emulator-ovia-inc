// SPDX-License-Identifier: CC0-1.0

//! `GetRecommendations`, end to end over HTTP through the router this service mounts in
//! production.
//!
//! The rules these cases assert:
//!
//! 1. the analysis is reported for the window `[startTime, endTime]`, and that window
//!    must be a window — a request whose start is not strictly before its end is refused
//!    with `ValidationException` (HTTP 400), whatever the group it names;
//! 2. `locale`, when given, must be one of the BCP 47 codes the model's `Locale` shape
//!    lists (case-insensitively, as BCP 47 tags are); anything else is a 400;
//! 3. the analysis is about a STORED group: an unknown name is a `ResourceNotFoundException`
//!    (HTTP 404), the reported `profilingGroupName` comes from the stored record, and
//!    `profileStartTime`/`profileEndTime` are the earliest and latest receipt times of the
//!    agent profiles that group retains inside the requested window — the requested bounds
//!    only when it retains none there;
//! 4. the read changes nothing a later caller sees.
//!
//! Each table row varies what those rules generalise over (window position and width,
//! how many retained profiles it covers, locale tag, group existence); no row is
//! privileged, and the recorded "start after end" case is one row of rule 1.
//!
//! The harness here is deliberately this suite's own: `profiling_group_lifecycle.rs`
//! carries a much larger one built around profiling-group bodies and retention bounds,
//! and nothing in `tests/` is shared between suites.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use smithy_server_runtime::MultiServiceRouter;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/codeguru-profiler/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// Every BCP 47 code the model's `Locale` shape doc offers: "BCP47 language code.
/// Supported locales: de-DE, en-GB, en-US, es-ES, fr-FR, it-IT, ja-JP, ko-KR, pt-BR,
/// zh-CN, zh-TW".
const SUPPORTED_LOCALES: &[&str] = &[
    "de-DE", "en-GB", "en-US", "es-ES", "fr-FR", "it-IT", "ja-JP", "ko-KR", "pt-BR",
    "zh-CN", "zh-TW",
];

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The modelled error code, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The modelled error's `message`, which every error shape of this service carries.
    fn error_message(&self) -> String {
        self.json()
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    }

    /// One timestamp member of the response, as epoch seconds, so an assertion compares
    /// INSTANTS rather than renderings. Either JSON timestamp form is read: the
    /// epoch-seconds number the framework's restJson1 body serializer writes, or the ISO
    /// 8601 string, so these cases assert the instant this operation computes and not the
    /// service-wide rendering of it.
    fn timestamp(&self, member: &str, case: &str) -> f64 {
        let body = self.json();
        let value = body
            .get(member)
            .unwrap_or_else(|| panic!("case {case:?}: {member} is required, got {body}"));
        if let Some(seconds) = value.as_f64() {
            return seconds;
        }
        let text = value
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: {member} must be a timestamp, got {value}")
            })
            .to_string();
        DateTime::from_str(&text, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {member} {text:?} must parse as a date-time: {error}")
            })
            .as_secs_f64()
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service. The engine is kept so a test can place retained profiles at
/// chosen instants, which the clock cannot be asked for.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codeguruprofiler_codeguruprofiler::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
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

    async fn post_agent_profile(&self, name: &str, body: Vec<u8>) -> Answer {
        self.send(
            "POST",
            &format!("/profilingGroups/{name}/agentProfile"),
            body,
        )
        .await
    }

    /// `GetRecommendations` over `[start, end]` (epoch seconds, sub-second precision
    /// included), optionally in `locale`.
    async fn get_recommendations(
        &self,
        name: &str,
        start: f64,
        end: f64,
        locale: Option<&str>,
    ) -> Answer {
        let mut query = format!("?startTime={}&endTime={}", iso8601(start), iso8601(end));
        if let Some(locale) = locale {
            query.push_str(&format!("&locale={locale}"));
        }
        self.send(
            "GET",
            &format!("/internal/profilingGroups/{name}/recommendations{query}"),
            Vec::new(),
        )
        .await
    }

    /// Re-stamp, oldest first, the receipt times of the profiles one group retains. The
    /// clock cannot be asked for a past instant, so this is how a test places retained
    /// profiles at chosen instants; it writes the same stored field `PostAgentProfile`
    /// writes and nothing else.
    fn stamp_receipt_times(&self, name: &str, at: &[i64]) {
        let state = self
            .engine
            .state::<service_com_amazonaws_codeguruprofiler_codeguruprofiler::ComAmazonawsCodeguruprofilerCodeguruprofilerState>();
        let mut guard = state.lock();
        let group = guard
            .groups
            .get_mut(name)
            .unwrap_or_else(|| panic!("no stored group named {name}"));
        assert_eq!(
            group.profiles.len(),
            at.len(),
            "the group must retain exactly the profiles this test posted"
        );
        for (profile, secs) in group.profiles.iter_mut().zip(at) {
            profile.received_at = DateTime::from_secs(*secs);
        }
        group.latest_agent_profile_reported_at = at.iter().copied().max().map(DateTime::from_secs);
    }

    /// How many profiles one group retains right now — so a read can be shown not to
    /// have touched them.
    fn retained_profiles(&self, name: &str) -> usize {
        self.engine
            .state::<service_com_amazonaws_codeguruprofiler_codeguruprofiler::ComAmazonawsCodeguruprofilerCodeguruprofilerState>()
            .lock()
            .groups
            .get(name)
            .map(|group| group.profiles.len())
            .unwrap_or_else(|| panic!("no stored group named {name}"))
    }
}

/// An epoch-second instant in the ISO 8601 form the modelled `date-time` query members
/// carry.
fn iso8601(secs: f64) -> String {
    DateTime::from_secs_f64(secs)
        .fmt(Format::DateTime)
        .expect("a renderable instant")
}

/// The refusal this operation answers a window that is not a window with, built from
/// that request's own bounds: "Interval start time (<startTime>) must be before end time
/// (<endTime>)".
fn expected_window_refusal(start: f64, end: f64) -> String {
    format!(
        "Interval start time ({}) must be before end time ({})",
        iso8601(start),
        iso8601(end)
    )
}

/// The two analysis lists, which must always be present and — with no profile analyser in
/// this emulator — empty, never absent and never invented.
fn assert_empty_analysis(case: &str, body: &serde_json::Value) {
    for member in ["recommendations", "anomalies"] {
        let list = body
            .get(member)
            .unwrap_or_else(|| panic!("case {case:?}: {member} is required, got {body}"));
        assert_eq!(
            list.as_array().map(Vec::len),
            Some(0),
            "case {case:?}: {member} must be an empty list, got {list}"
        );
    }
}

#[tokio::test]
async fn the_reported_profile_span_is_the_retained_profiles_inside_the_window() {
    let harness = Harness::new();
    let name = "analysed-group";
    assert_eq!(harness.create(name).await.status, 201);

    // Three retained profiles, placed at chosen instants: 2020-06-01T00:00:00Z and
    // 10 minutes and 1 hour after it.
    let base: i64 = 1_590_969_600;
    let received_at = [base, base + 600, base + 3_600];
    for index in 0..received_at.len() {
        let body = format!("{{\"profile\":{index}}}").into_bytes();
        assert_eq!(
            harness.post_agent_profile(name, body).await.status,
            204,
            "profile {index} must be stored"
        );
    }
    harness.stamp_receipt_times(name, &received_at);

    let reported_at = harness
        .describe(name)
        .await
        .json()
        .pointer("/profilingStatus/latestAgentProfileReportedAt")
        .cloned()
        .expect("the posts must be recorded");

    // Rows: (case, window start, window end, the span the analysis must report). The
    // window bounds are inclusive, so a bound landing exactly on a receipt time includes
    // that profile; a window covering no profile covers nothing, so the requested bounds
    // stand.
    let cases: Vec<(&str, i64, i64, (i64, i64))> = vec![
        (
            "a window around every profile",
            base - 3_600,
            base + 7_200,
            (received_at[0], received_at[2]),
        ),
        (
            "a window holding only the middle profile",
            base + 100,
            base + 700,
            (received_at[1], received_at[1]),
        ),
        (
            "a window closed exactly on the first two receipt times",
            received_at[0],
            received_at[1],
            (received_at[0], received_at[1]),
        ),
        (
            "a window closed exactly on the last two receipt times",
            received_at[1],
            received_at[2],
            (received_at[1], received_at[2]),
        ),
        (
            "a one-second window on the earliest receipt time",
            received_at[0],
            received_at[0] + 1,
            (received_at[0], received_at[0]),
        ),
        (
            "a window entirely after every profile",
            base + 7_200,
            base + 10_800,
            (base + 7_200, base + 10_800),
        ),
        (
            "a window entirely before every profile",
            base - 10_800,
            base - 7_200,
            (base - 10_800, base - 7_200),
        ),
        (
            "a window strictly between two receipt times",
            received_at[0] + 1,
            received_at[1] - 1,
            (received_at[0] + 1, received_at[1] - 1),
        ),
    ];

    for (case, start, end, (expected_start, expected_end)) in cases {
        let answer = harness
            .get_recommendations(name, start as f64, end as f64, None)
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let body = answer.json();
        assert_eq!(
            body.get("profilingGroupName")
                .and_then(serde_json::Value::as_str),
            Some(name),
            "case {case:?}: profilingGroupName must come from the stored group, got {body}"
        );
        assert_eq!(
            answer.timestamp("profileStartTime", case),
            expected_start as f64,
            "case {case:?}: expected profileStartTime {}, got {body}",
            iso8601(expected_start as f64)
        );
        assert_eq!(
            answer.timestamp("profileEndTime", case),
            expected_end as f64,
            "case {case:?}: expected profileEndTime {}, got {body}",
            iso8601(expected_end as f64)
        );
        assert_empty_analysis(case, &body);
    }

    // The read changed nothing a later caller sees.
    assert_eq!(
        harness.retained_profiles(name),
        received_at.len(),
        "GetRecommendations must not touch the retained profiles"
    );
    assert_eq!(
        harness
            .describe(name)
            .await
            .json()
            .pointer("/profilingStatus/latestAgentProfileReportedAt")
            .cloned(),
        Some(reported_at),
        "GetRecommendations must not change the recorded profile-report time"
    );
}

#[tokio::test]
async fn a_group_with_no_profiles_reports_an_empty_analysis_of_the_requested_window() {
    let harness = Harness::new();
    let name = "never-profiled";
    assert_eq!(harness.create(name).await.status, 201);

    // Rows: windows of different widths over a group that retains nothing. Each one
    // covers no profile, so the analysed span is the requested window and both lists are
    // empty.
    for (case, start, end) in [
        ("a one-second window", 1_600_000_000_i64, 1_600_000_001_i64),
        ("an hour-long window", 1_600_000_000, 1_600_003_600),
        ("a decade-long window", 1_300_000_000, 1_600_000_000),
    ] {
        let answer = harness
            .get_recommendations(name, start as f64, end as f64, None)
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.timestamp("profileStartTime", case),
            start as f64,
            "case {case:?}: an empty window must report its own start"
        );
        assert_eq!(
            answer.timestamp("profileEndTime", case),
            end as f64,
            "case {case:?}: an empty window must report its own end"
        );
        assert_empty_analysis(case, &answer.json());
    }
}

#[tokio::test]
async fn a_window_whose_start_is_not_before_its_end_is_refused() {
    let harness = Harness::new();
    let name = "windowed";
    assert_eq!(harness.create(name).await.status, 201);

    let base: f64 = 1_590_969_600.0;
    // Rows: (case, start, end, expected status). The window must be a window; its width
    // and position are otherwise free, and sub-second bounds are named as readily as
    // whole seconds.
    let cases: Vec<(&str, f64, f64, u16)> = vec![
        ("a start one second after the end", base + 1.0, base, 400),
        ("a start a day after the end", base + 86_400.0, base, 400),
        ("a start a millisecond after the end", base + 0.001, base, 400),
        ("a start equal to the end", base, base, 400),
        ("a start one second before the end", base, base + 1.0, 200),
        ("a start a millisecond before the end", base, base + 0.001, 200),
        (
            "a start a year before the end",
            base,
            base + 31_536_000.0,
            200,
        ),
        ("a window over the epoch itself", 0.0, 1.0, 200),
    ];

    for (case, start, end, expected_status) in cases {
        let answer = harness.get_recommendations(name, start, end, None).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if expected_status == 400 {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            // The refusal names the window the REQUEST asked for, so every row carries
            // its own bounds rather than one recorded pair of instants.
            assert_eq!(
                answer.error_message(),
                expected_window_refusal(start, end),
                "case {case:?}: expected {:?}, got {:?}",
                expected_window_refusal(start, end),
                answer.error_message()
            );
        }
    }
}

#[tokio::test]
async fn an_unknown_profiling_group_is_a_404_and_a_bad_window_is_refused_before_the_lookup() {
    let harness = Harness::new();
    let stored = "stored-group";
    assert_eq!(harness.create(stored).await.status, 201);

    let base: f64 = 1_590_969_600.0;
    // Rows: (case, group name, start, end, expected status, expected error code).
    let cases: Vec<(&str, &str, f64, f64, u16, &str)> = vec![
        ("the stored group", stored, base, base + 3_600.0, 200, ""),
        (
            "a group that was never created",
            "no-such-group",
            base,
            base + 3_600.0,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a group name differing from the stored one by one character",
            "stored-grouq",
            base,
            base + 3_600.0,
            404,
            "ResourceNotFoundException",
        ),
        // A malformed window is refused before any resource is resolved, the way
        // request validation precedes resource lookup.
        (
            "an unknown group asked for over an inverted window",
            "no-such-group",
            base + 3_600.0,
            base,
            400,
            "ValidationException",
        ),
    ];

    for (case, group, start, end, expected_status, expected_code) in cases {
        let answer = harness.get_recommendations(group, start, end, None).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if !expected_code.is_empty() {
            assert_eq!(
                answer.error_code(),
                expected_code,
                "case {case:?}: expected {expected_code}, got {}",
                String::from_utf8_lossy(&answer.body)
            );
        }
    }
}

#[tokio::test]
async fn only_the_documented_locales_are_accepted() {
    let harness = Harness::new();
    let name = "localised";
    assert_eq!(harness.create(name).await.status, 201);

    let base: f64 = 1_590_969_600.0;

    // Every documented code is accepted — including the ones no recorded request cites.
    for locale in SUPPORTED_LOCALES {
        let answer = harness
            .get_recommendations(name, base, base + 3_600.0, Some(locale))
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {locale:?}: a documented locale must be accepted, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_empty_analysis(locale, &answer.json());
    }

    // Rows: (case, locale, expected status). BCP 47 tags are case-insensitive, so a
    // documented code in any casing is accepted; a tag outside the documented list is
    // refused however close it looks.
    let cases: Vec<(&str, &str, u16)> = vec![
        ("a documented code upper-cased", "EN-GB", 200),
        ("a documented code lower-cased", "pt-br", 200),
        ("an underscore instead of a hyphen", "en_US", 400),
        ("a language with no region", "en", 400),
        ("a region this service does not offer", "en-AU", 400),
        ("a language this service does not offer", "nl-NL", 400),
        ("an empty locale", "", 400),
    ];

    for (case, locale, expected_status) in cases {
        let answer = harness
            .get_recommendations(name, base, base + 3_600.0, Some(locale))
            .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?} ({locale:?}): expected HTTP {expected_status}, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if expected_status == 400 {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?} ({locale:?}): expected ValidationException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
        }
    }
}
