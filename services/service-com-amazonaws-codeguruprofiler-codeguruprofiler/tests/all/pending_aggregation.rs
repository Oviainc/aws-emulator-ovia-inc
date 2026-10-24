// SPDX-License-Identifier: CC0-1.0

//! What a group's profile surface reports while the aggregation a posted agent profile
//! is owed has not fallen due yet, end to end over HTTP through the router this service
//! mounts in production.
//!
//! The rule every case here asserts: `GetProfile` and `ListProfileTimes` report
//! AGGREGATED profiles, and a posted agent profile is not one at the instant it is
//! received — the service collects posted profiles into aggregated profiles at periods
//! aligned to UTC, so the window a profile lands in is still open when the post returns
//! and its aggregate falls due a span after that window closes. Until then the window is
//! covered by no aggregated profile, so `GetProfile` answers its 404
//! `ResourceNotFoundException` with the fixed no-data text and `ListProfileTimes` reports
//! no profile time for it — while the group still records that the profile was RECEIVED
//! (`profilingStatus.latestAgentProfileReportedAt`). Once the aggregation is due, the
//! same requests serve the stored bytes.
//!
//! Rows vary what the rule generalises over: how the requested window is expressed (no
//! parameters, each of the three modelled aggregation periods alone, both bounds, each
//! bound alone), the payload's size and bytes, the media type it was posted under, and
//! how many profiles the group holds.
//!
//! The harness is the one [`crate::profiling_group_lifecycle`] already builds, and the
//! aggregation is let fall due through the one shared helper every suite uses.

use crate::profiling_group_lifecycle::{Answer, Harness};

/// The fixed text a 404 about absent profile DATA carries
/// (`state::NO_PROFILES_IN_TIME_RANGE`), restated here so this suite pins the wire text
/// rather than importing the value it compares against.
const NO_PROFILES_IN_TIME_RANGE: &str = "No profiles found for the requested time range!";

/// A window wide enough to contain any instant these cases place a receipt at, in the
/// ISO 8601 form the modelled `date-time` query members carry.
const WIDE_START: &str = "2000-01-01T00:00:00Z";
const WIDE_END: &str = "2099-01-01T00:00:00Z";

async fn create_group(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken=tok-{name}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({
                "profilingGroupName": name,
                "agentOrchestrationConfig": { "profilingEnabled": true }
            }))
            .expect("a serializable body"),
        )
        .await
}

async fn list_profile_times(harness: &Harness, name: &str, period: &str) -> Answer {
    harness
        .send(
            "GET",
            &format!(
                "/profilingGroups/{name}/profileTimes?startTime={WIDE_START}&endTime={WIDE_END}\
                 &period={period}"
            ),
            "application/json",
            Vec::new(),
        )
        .await
}

/// How many profile times one `ListProfileTimes` answer reports.
fn reported_times(answer: &Answer) -> usize {
    answer
        .json()
        .get("profileTimes")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or_else(|| panic!("no profileTimes array in {}", answer.json()))
}

/// One row: how the `GetProfile` window is expressed, and the profile posted under it.
struct Row {
    label: &'static str,
    /// The `GetProfile` query string, which must select the stored profile once its
    /// aggregation is due.
    query: String,
    content_type: &'static str,
    bytes: Vec<u8>,
    /// How many profiles the group is posted, each with its own `profileToken`.
    posts: usize,
}

fn rows() -> Vec<Row> {
    // The three period values are the ones the model's `AggregationPeriod` enum declares
    // (PT5M, PT1H, P1D); the media types are the two the model's `GetProfile$contentType`
    // doc names ("either application/json or the default application/x-amzn-ion").
    vec![
        Row {
            label: "no parameters",
            query: String::new(),
            content_type: "application/json",
            bytes: br#"{"frames":["A"]}"#.to_vec(),
            posts: 1,
        },
        Row {
            label: "period PT5M alone",
            query: "?period=PT5M".to_string(),
            content_type: "application/json",
            bytes: vec![b'{'],
            posts: 1,
        },
        Row {
            label: "period PT1H alone",
            query: "?period=PT1H".to_string(),
            content_type: "application/x-amzn-ion",
            bytes: (0..=255u8).collect(),
            posts: 2,
        },
        Row {
            label: "period P1D alone",
            query: "?period=P1D".to_string(),
            content_type: "application/x-amzn-ion",
            bytes: (0..4_096u32).map(|i| (i % 251) as u8).collect(),
            posts: 1,
        },
        Row {
            label: "both bounds",
            query: format!("?startTime={WIDE_START}&endTime={WIDE_END}"),
            content_type: "application/json",
            bytes: br#"{"frames":["A","B","C"]}"#.to_vec(),
            posts: 3,
        },
        Row {
            label: "a start bound alone",
            query: format!("?startTime={WIDE_START}"),
            content_type: "application/json",
            bytes: br#"{"frames":[]}"#.to_vec(),
            posts: 1,
        },
        Row {
            label: "an end bound alone",
            query: format!("?endTime={WIDE_END}"),
            content_type: "application/x-amzn-ion",
            bytes: vec![0xe0, 0x01, 0x00, 0xea],
            posts: 2,
        },
    ]
}

#[tokio::test]
async fn a_posted_profile_is_served_only_once_its_aggregation_is_due() {
    let harness = Harness::new();

    for (index, row) in rows().iter().enumerate() {
        let name = format!("pending-{index}");
        let case = row.label;
        assert_eq!(
            create_group(&harness, &name).await.status,
            201,
            "case {case:?}: create must succeed"
        );

        // Before any post there is nothing to aggregate, so the same 404 answers.
        let before = harness.get_profile(&name, &row.query).await;
        assert_eq!(
            before.status, 404,
            "case {case:?}: GetProfile before any post must answer 404, got {} {}",
            before.status,
            String::from_utf8_lossy(&before.body)
        );

        let mut last = Vec::new();
        for post in 0..row.posts {
            let mut bytes = row.bytes.clone();
            bytes.push(u8::try_from(post % 256).unwrap_or(0));
            assert_eq!(
                harness
                    .post_agent_profile(
                        &name,
                        row.content_type,
                        Some(&format!("ptok-{index}-{post}")),
                        bytes.clone(),
                    )
                    .await
                    .status,
                204,
                "case {case:?}: post {post} must answer 204"
            );
            last = bytes;
        }

        // The post was RECEIVED — the group says so — and is still not aggregated.
        let described = harness.describe(&name).await;
        assert!(
            described
                .json()
                .pointer("/profilingStatus/latestAgentProfileReportedAt")
                .is_some(),
            "case {case:?}: the post must be recorded as received, got {}",
            String::from_utf8_lossy(&described.body)
        );

        let pending = harness.get_profile(&name, &row.query).await;
        assert_eq!(
            pending.status, 404,
            "case {case:?}: a profile whose aggregation is not due must not be served, got \
             {} {}",
            pending.status,
            String::from_utf8_lossy(&pending.body)
        );
        assert_eq!(
            pending.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&pending.body)
        );
        assert_eq!(
            pending.error_message(),
            NO_PROFILES_IN_TIME_RANGE,
            "case {case:?}: expected the fixed no-data text, got {:?}",
            pending.error_message()
        );
        // The absence is about the DATA, not about the group: nothing of the request is
        // interpolated into the sentence.
        assert!(
            !pending.error_message().contains(&name),
            "case {case:?}: the no-data text must not name the group, got {:?}",
            pending.error_message()
        );

        // No aggregated profile exists yet, so no profile time is reported for any period.
        for period in ["PT5M", "PT1H", "P1D"] {
            let times = list_profile_times(&harness, &name, period).await;
            assert_eq!(
                times.status, 200,
                "case {case:?} ({period}): ListProfileTimes must answer 200, got {} {}",
                times.status,
                String::from_utf8_lossy(&times.body)
            );
            assert_eq!(
                reported_times(&times),
                0,
                "case {case:?} ({period}): a pending aggregation must report no profile \
                 time, got {}",
                String::from_utf8_lossy(&times.body)
            );
        }

        // Once the aggregation its window owes has fallen due, the SAME request serves
        // the stored bytes — so the 404 above is about the pending aggregation and not
        // about a profile the store never kept.
        harness.complete_aggregation(&name);
        let served = harness.get_profile(&name, &row.query).await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: an aggregated profile must be served, got {} {}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );
        assert_eq!(
            served.body, last,
            "case {case:?}: GetProfile must return the bytes posted last ({} bytes)",
            last.len()
        );
        for period in ["PT5M", "PT1H", "P1D"] {
            let times = list_profile_times(&harness, &name, period).await;
            assert!(
                reported_times(&times) >= 1,
                "case {case:?} ({period}): an aggregated profile must be reported, got {}",
                String::from_utf8_lossy(&times.body)
            );
        }
    }
}

#[tokio::test]
async fn reading_a_pending_aggregation_changes_nothing_a_later_caller_sees() {
    let harness = Harness::new();
    let name = "pending-read";
    assert_eq!(
        create_group(&harness, name).await.status,
        201,
        "create must succeed"
    );
    let bytes = br#"{"frames":["A"]}"#.to_vec();
    assert_eq!(
        harness
            .post_agent_profile(name, "application/json", None, bytes.clone())
            .await
            .status,
        204,
        "the post must succeed"
    );
    let reported_at = harness
        .describe(name)
        .await
        .json()
        .pointer("/profilingStatus/latestAgentProfileReportedAt")
        .cloned()
        .expect("the post must be recorded");

    // Polling the pending window is what a client written against the real service does;
    // it must neither serve the profile early nor consume it.
    for attempt in 0..3 {
        let answer = harness.get_profile(name, "?period=PT5M").await;
        assert_eq!(
            answer.status, 404,
            "attempt {attempt}: a pending aggregation must stay pending, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
    assert_eq!(
        harness
            .describe(name)
            .await
            .json()
            .pointer("/profilingStatus/latestAgentProfileReportedAt")
            .cloned(),
        Some(reported_at),
        "polling GetProfile must not move the recorded profile-report time"
    );

    harness.complete_aggregation(name);
    // And once it is due, two reads of the same window agree: the read is reporting.
    let first = harness.get_profile(name, "?period=PT5M").await;
    let second = harness.get_profile(name, "?period=PT5M").await;
    assert_eq!(
        first.status, 200,
        "the aggregated profile must be served, got {} {}",
        first.status,
        String::from_utf8_lossy(&first.body)
    );
    assert_eq!(
        first.body, second.body,
        "two reads of one aggregated profile must agree"
    );
    assert_eq!(
        first.body, bytes,
        "GetProfile must return the posted bytes unchanged"
    );
}
