// SPDX-License-Identifier: CC0-1.0

//! What `SubmitFeedback` does with the feedback a caller submits, end to end over HTTP
//! through the router this service mounts in production.
//!
//! The rule every case here asserts: feedback is submitted ABOUT an anomaly instance of
//! the named profiling group's analysis. So the group must exist (a name the store cannot
//! resolve is the modelled `ResourceNotFoundException`, 404); the `anomalyInstanceId` must
//! name an instance of an anomaly that group holds, and any other well-formed id is the
//! modelled `ValidationException` (400) stating the lookup that found nothing, with
//! nothing filed; and a submission about an instance the group does hold is filed against
//! it — one feedback per instance, a further submission superseding the one stored, read
//! back on that instance's `userFeedback` through `GetRecommendations`, belonging to the
//! group it was submitted for and to no other, and answered with the modelled empty 204
//! while the group's own description stays as it was.
//!
//! The rows differ along what that rule generalises over: which `FeedbackType` was
//! submitted, whether a comment came with it and what it holds (absent, empty, multi-byte
//! text, a kilobyte), which anomaly instance the feedback is about, how many instances one
//! group has been given feedback about, which of two groups a call is addressed to,
//! whether the group holds any anomaly at all, whether the id belongs to another group's
//! anomaly or to one the catalogue has evicted, and whether the group is stored at all.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;

use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::{
    DetectedAnomaly, DetectedAnomalyInstance,
};

use crate::profiling_group_lifecycle::{Answer, Harness};

/// This service's state type, read through the shared engine the harness mounts: an
/// anomaly enters the store only through its one writer (`record_detected_anomaly` — this
/// emulator runs no anomaly detector, so there is no operation that mints one), which is
/// how a case here gives a group the anomaly instances a submission can be about. The
/// stored feedback is read back both from the store and, on the wire, from the instance
/// `GetRecommendations` reports it on.
type State =
    service_com_amazonaws_codeguruprofiler_codeguruprofiler::ComAmazonawsCodeguruprofilerCodeguruprofilerState;

/// Anomaly instance ids this suite submits feedback about. Each matches the model's
/// `AnomalyInstanceId` pattern `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`
/// (`smithy_query(query="shape", shape="com.amazonaws.codeguruprofiler#AnomalyInstanceId")`),
/// so the boundary validator admits them and the handler decides what they mean.
const INSTANCE_A: &str = "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d";
const INSTANCE_B: &str = "1b2c3d4e-5f6a-7b8c-9d0e-1f2a3b4c5d6e";

/// The `FeedbackType` values the model declares, in the order its enum lists them
/// (`smithy_query(query="shape", shape="com.amazonaws.codeguruprofiler#FeedbackType")`):
/// "Positive" and "Negative", the two values the operation's own doc names.
const FEEDBACK_TYPES: [&str; 2] = ["Positive", "Negative"];

/// The instant the anomaly periods this suite records start at (2020-06-01T00:00:00Z), and
/// how long each period runs. A recorded anomaly instance needs a period, and these cases
/// are about the id rather than the window, so every instance gets a one-hour one placed
/// in sequence from here — well inside the window [`get_recommendations`] asks about.
const ANOMALY_PERIOD_START: i64 = 1_590_969_600;
const ANOMALY_PERIOD_SECONDS: i64 = 3_600;

/// One anomaly instance id, distinct per `n` and matching the modelled pattern: used where
/// a case needs many instances rather than a particular one.
fn instance_id(n: usize) -> String {
    format!("{n:08x}-0000-4000-8000-{n:012x}")
}

/// One submission, as the stored record holds it: (anomaly instance, feedback type,
/// comment), least recently submitted first.
type Stored = Vec<(String, String, Option<String>)>;

/// What one group holds right now, in stored order.
fn stored(harness: &Harness, group: &str) -> Stored {
    let state = harness.engine().state::<State>();
    let guard = state.lock();
    let group = guard
        .groups
        .get(group)
        .unwrap_or_else(|| panic!("no stored group named {group}"));
    group
        .anomaly_feedback
        .iter()
        .map(|feedback| {
            (
                feedback.anomaly_instance_id.clone(),
                feedback.feedback_type.clone(),
                feedback.comment.clone(),
            )
        })
        .collect()
}

/// The instant one group's feedback about `anomaly_instance_id` was filed, or `None` when
/// it holds none about that instance.
fn stored_at(harness: &Harness, group: &str, anomaly_instance_id: &str) -> Option<f64> {
    let state = harness.engine().state::<State>();
    let guard = state.lock();
    guard
        .groups
        .get(group)
        .and_then(|group| group.feedback_about(anomaly_instance_id))
        .map(|feedback| feedback.submitted_at.as_secs_f64())
}

/// Record one detected anomaly against `group`, holding one instance per id in
/// `instances` — the anomalies a group's analysis found, which is what an
/// `anomalyInstanceId` is resolved against. Each instance is given the `n`th one-hour
/// period from [`ANOMALY_PERIOD_START`], so the periods are distinct and ordered.
///
/// `metricType` is the one value the model's `MetricType` enum declares, taken from the
/// service's own enum catalogue rather than spelled out here.
fn record_anomaly(harness: &Harness, group: &str, instances: &[String]) {
    let metric_type = service_com_amazonaws_codeguruprofiler_codeguruprofiler::ops::types::METRIC_TYPE_VALUES[0];
    let anomaly = DetectedAnomaly {
        frame_name: format!("com.example.{group}.hotFrame"),
        metric_type: metric_type.to_string(),
        thread_states: vec!["RUNNABLE".to_string()],
        reason: "ThreadStateDeviation".to_string(),
        instances: instances
            .iter()
            .enumerate()
            .map(|(index, id)| {
                let start = ANOMALY_PERIOD_START + (index as i64) * ANOMALY_PERIOD_SECONDS;
                DetectedAnomalyInstance {
                    id: id.clone(),
                    start_time: DateTime::from_secs(start),
                    end_time: Some(DateTime::from_secs(start + ANOMALY_PERIOD_SECONDS)),
                }
            })
            .collect(),
    };
    let state = harness.engine().state::<State>();
    let mut guard = state.lock();
    assert!(
        guard.record_detected_anomaly(group, anomaly),
        "case {group}: an anomaly must be recordable against a stored group"
    );
}

async fn create_group(harness: &Harness, name: &str) {
    let answer = harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken=token-{name}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "profilingGroupName": name }))
                .expect("a serializable body"),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case {name}: CreateProfilingGroup must succeed, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// `SubmitFeedback`: `POST /internal/profilingGroups/{profilingGroupName}/anomalies/{anomalyInstanceId}/feedback`
/// (`smithy_query(query="operation", operation="SubmitFeedback")`), answered 204.
async fn submit(
    harness: &Harness,
    group: &str,
    anomaly_instance_id: &str,
    body: serde_json::Value,
) -> Answer {
    harness
        .send(
            "POST",
            &format!(
                "/internal/profilingGroups/{group}/anomalies/{anomaly_instance_id}/feedback"
            ),
            "application/json",
            serde_json::to_vec(&body).expect("a serializable body"),
        )
        .await
}

/// `GetRecommendations` over a window covering every period this suite records
/// (`GET /internal/profilingGroups/{name}/recommendations?startTime=..&endTime=..`): the
/// read that reports the anomalies a group holds, and the submitted feedback on them.
async fn get_recommendations(harness: &Harness, group: &str) -> Answer {
    let start = DateTime::from_secs(ANOMALY_PERIOD_START - ANOMALY_PERIOD_SECONDS)
        .fmt(Format::DateTime)
        .expect("a renderable instant");
    let end = DateTime::from_secs(ANOMALY_PERIOD_START + 1_000 * ANOMALY_PERIOD_SECONDS)
        .fmt(Format::DateTime)
        .expect("a renderable instant");
    harness
        .send(
            "GET",
            &format!(
                "/internal/profilingGroups/{group}/recommendations?startTime={start}&endTime={end}"
            ),
            "application/json",
            Vec::new(),
        )
        .await
}

/// The `userFeedback.type` `GetRecommendations` reports on the anomaly instance
/// `anomaly_instance_id`, or `None` when it reports that instance without feedback.
/// Panics when the answer reports no such instance at all, so a case cannot pass by
/// comparing nothing with nothing.
fn reported_feedback(case: &str, answer: &Answer, anomaly_instance_id: &str) -> Option<String> {
    let body = answer.json();
    let anomalies = body
        .get("anomalies")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the analysis must report anomalies, got {body}"));
    let instance = anomalies
        .iter()
        .filter_map(|anomaly| anomaly.get("instances")?.as_array())
        .flatten()
        .find(|instance| {
            instance.get("id").and_then(serde_json::Value::as_str) == Some(anomaly_instance_id)
        })
        .unwrap_or_else(|| {
            panic!("case {case}: the analysis must report instance {anomaly_instance_id}, got {body}")
        });
    instance
        .get("userFeedback")
        .and_then(|feedback| feedback.get("type"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// The request body for a submission: the `@required` `type`, plus `comment` only when the
/// case sends one.
fn feedback_body(feedback_type: &str, comment: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    body.insert("type".to_string(), serde_json::Value::from(feedback_type));
    if let Some(comment) = comment {
        body.insert("comment".to_string(), serde_json::Value::from(comment));
    }
    serde_json::Value::Object(body)
}

/// A successful submission answers the modelled 204 and reports no members: the
/// `SubmitFeedbackResponse` has none, so an empty body and an empty object are both that
/// answer and anything else is not.
fn assert_accepted(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 204,
        "case {case}: SubmitFeedback must answer 204, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    if !answer.body.is_empty() {
        let body = answer.json();
        assert_eq!(
            body,
            serde_json::json!({}),
            "case {case}: SubmitFeedbackResponse declares no members, got {body}"
        );
    }
}

/// The refusal an `anomalyInstanceId` the named group holds no anomaly instance for draws:
/// the lookup that found nothing — the id asked about and the account the anomalies were
/// looked for in, which is this emulator's own.
fn expected_unknown_anomaly_message(anomaly_instance_id: &str) -> String {
    format!(
        "Anomaly with id: {anomaly_instance_id} not found for accountId: {}",
        engine::emulator_account_id()
    )
}

/// `case`'s answer is the operation's `ValidationException` (400) about an anomaly
/// instance the group does not hold, and nothing was filed.
fn assert_unknown_anomaly(case: &str, answer: &Answer, anomaly_instance_id: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: feedback about an anomaly instance the group does not hold must \
         answer 400, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case}: expected ValidationException, got {}",
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_message(),
        expected_unknown_anomaly_message(anomaly_instance_id),
        "case {case}: expected {:?}, got {:?}",
        expected_unknown_anomaly_message(anomaly_instance_id),
        answer.error_message()
    );
}

async fn delete_group(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "DELETE",
            &format!("/profilingGroups/{name}"),
            "application/json",
            Vec::new(),
        )
        .await
}

/// One submission the table below makes: which instance it is about, the feedback type and
/// the comment as sent.
struct FeedbackCase {
    label: &'static str,
    anomaly_instance_id: String,
    feedback_type: &'static str,
    comment: Option<String>,
}

/// The table: rows differ in feedback type, in whether a comment came with the submission
/// and what it holds, and in which anomaly instance the feedback is about. `comment` is a
/// plain `String` member with no modelled constraint, so an empty one, multi-byte text and
/// a kilobyte are all valid submissions.
fn feedback_cases() -> Vec<FeedbackCase> {
    vec![
        FeedbackCase {
            label: "positive, with a comment",
            anomaly_instance_id: INSTANCE_A.to_string(),
            feedback_type: FEEDBACK_TYPES[0],
            comment: Some("this one found a real hot path".to_string()),
        },
        FeedbackCase {
            label: "negative, with no comment at all",
            anomaly_instance_id: INSTANCE_B.to_string(),
            feedback_type: FEEDBACK_TYPES[1],
            comment: None,
        },
        FeedbackCase {
            label: "negative, with an empty comment",
            anomaly_instance_id: instance_id(3),
            feedback_type: FEEDBACK_TYPES[1],
            comment: Some(String::new()),
        },
        FeedbackCase {
            label: "positive, with a multi-byte multi-line comment",
            anomaly_instance_id: instance_id(4),
            feedback_type: FEEDBACK_TYPES[0],
            comment: Some("ünïcode ✓\nкомментарий 🙂".to_string()),
        },
        FeedbackCase {
            label: "positive, with a kilobyte of comment",
            anomaly_instance_id: instance_id(5),
            feedback_type: FEEDBACK_TYPES[0],
            comment: Some("x".repeat(1024)),
        },
    ]
}

/// Every instance id the table submits feedback about, in table order.
fn table_instances() -> Vec<String> {
    feedback_cases()
        .into_iter()
        .map(|case| case.anomaly_instance_id)
        .collect()
}

#[tokio::test]
async fn a_submission_is_filed_against_the_anomaly_instance_it_is_about() {
    let harness = Harness::new();
    let (submitted_to, other) = ("feedback-target", "feedback-bystander");
    create_group(&harness, submitted_to).await;
    create_group(&harness, other).await;
    // Both groups' analyses found the same anomaly instances, so a submission can be shown
    // to belong to the group it was addressed to rather than to the id.
    record_anomaly(&harness, submitted_to, &table_instances());
    record_anomaly(&harness, other, &table_instances());

    // The group's own description before any feedback: filing feedback is not a change to
    // the group's configuration, so the record a reader answers with must not move.
    let described_before = harness.describe(submitted_to).await.json();

    let mut expected: Stored = Vec::new();
    for case in feedback_cases() {
        let label = case.label;
        let answer = submit(
            &harness,
            submitted_to,
            &case.anomaly_instance_id,
            feedback_body(case.feedback_type, case.comment.as_deref()),
        )
        .await;
        assert_accepted(label, &answer);

        expected.push((
            case.anomaly_instance_id.clone(),
            case.feedback_type.to_string(),
            case.comment.clone(),
        ));
        let held = stored(&harness, submitted_to);
        assert_eq!(
            held, expected,
            "case {label}: the group must hold every submission made about it, in \
             submission order, got {held:?}"
        );

        // And the submission is READ BACK on the anomaly instance it is about, which is
        // where the modelled `UserFeedback` is reported.
        let reported = get_recommendations(&harness, submitted_to).await;
        let read_back = reported_feedback(label, &reported, &case.anomaly_instance_id);
        assert_eq!(
            read_back.as_deref(),
            Some(case.feedback_type),
            "case {label}: GetRecommendations must report the submitted feedback on the \
             instance it is about, got {read_back:?}"
        );

        // The feedback belongs to the group it was submitted for: the same anomaly
        // instance names nothing in another group.
        let bystander = stored(&harness, other);
        assert!(
            bystander.is_empty(),
            "case {label}: a submission against {submitted_to} must not reach {other}, \
             got {bystander:?}"
        );
        let bystander_reported = get_recommendations(&harness, other).await;
        let bystander_feedback =
            reported_feedback(label, &bystander_reported, &case.anomaly_instance_id);
        assert_eq!(
            bystander_feedback, None,
            "case {label}: {other} must report its own instance without feedback, got \
             {bystander_feedback:?}"
        );
    }

    let described_after = harness.describe(submitted_to).await.json();
    assert_eq!(
        described_after, described_before,
        "case the group record: submitting feedback must not change what \
         DescribeProfilingGroup answers, got {described_after}"
    );

    // And the same anomaly instance ids are usable independently in the other group.
    for case in feedback_cases() {
        let answer = submit(
            &harness,
            other,
            &case.anomaly_instance_id,
            feedback_body(FEEDBACK_TYPES[1], None),
        )
        .await;
        assert_accepted(case.label, &answer);
    }
    assert_eq!(
        stored(&harness, other).len(),
        feedback_cases().len(),
        "case the second group: each group holds its own submissions, got {:?}",
        stored(&harness, other)
    );
    assert_eq!(
        stored(&harness, submitted_to),
        expected,
        "case the first group: submissions against {other} must leave {submitted_to} as it was"
    );
}

#[tokio::test]
async fn a_further_submission_about_one_anomaly_instance_supersedes_the_stored_one() {
    let harness = Harness::new();
    let group = "feedback-supersede";
    create_group(&harness, group).await;
    record_anomaly(
        &harness,
        group,
        &[INSTANCE_A.to_string(), INSTANCE_B.to_string()],
    );

    // Each row is the submission that supersedes the one before it about INSTANCE_A:
    // the type flips, and the comment is added, replaced, emptied and dropped.
    let rows: [(&str, &str, Option<&str>); 5] = [
        ("first, positive with a comment", "Positive", Some("useful")),
        ("negative, comment replaced", "Negative", Some("not useful")),
        ("negative again, comment dropped", "Negative", None),
        ("positive, comment emptied", "Positive", Some("")),
        ("positive again, same submission repeated", "Positive", Some("")),
    ];

    // A second instance is given feedback between the supersessions, so the stored order
    // is observable: the superseding submission becomes the most recent one.
    assert_accepted(
        "the second instance, seeded first",
        &submit(
            &harness,
            group,
            INSTANCE_B,
            feedback_body("Negative", Some("the other instance")),
        )
        .await,
    );

    let mut previous_at = 0.0_f64;
    for (case, feedback_type, comment) in rows {
        let answer = submit(
            &harness,
            group,
            INSTANCE_A,
            feedback_body(feedback_type, comment),
        )
        .await;
        assert_accepted(case, &answer);

        let held = stored(&harness, group);
        assert_eq!(
            held,
            vec![
                (
                    INSTANCE_B.to_string(),
                    "Negative".to_string(),
                    Some("the other instance".to_string()),
                ),
                (
                    INSTANCE_A.to_string(),
                    feedback_type.to_string(),
                    comment.map(str::to_string),
                ),
            ],
            "case {case}: one anomaly instance carries one feedback, the latest submitted, \
             and the superseding submission is the most recent one, got {held:?}"
        );

        // The instance reports ONE feedback type, the one submitted last.
        let reported = get_recommendations(&harness, group).await;
        let read_back = reported_feedback(case, &reported, INSTANCE_A);
        assert_eq!(
            read_back.as_deref(),
            Some(feedback_type),
            "case {case}: the instance must report the latest feedback submitted about it, \
             got {read_back:?}"
        );

        let at = stored_at(&harness, group, INSTANCE_A)
            .unwrap_or_else(|| panic!("case {case}: the submission must be stored"));
        assert!(
            at >= previous_at,
            "case {case}: the stored submission instant must not move backwards, \
             got {at} after {previous_at}"
        );
        previous_at = at;
    }
}

#[tokio::test]
async fn feedback_about_an_anomaly_instance_the_group_does_not_hold_is_the_modelled_400() {
    let harness = Harness::new();
    let (analysed, bare, crowded) = (
        "feedback-unknown-analysed",
        "feedback-unknown-bare",
        "feedback-unknown-crowded",
    );
    for group in [analysed, bare, crowded] {
        create_group(&harness, group).await;
    }
    // One group whose analysis holds exactly INSTANCE_A, one whose analysis found nothing
    // at all, and one whose catalogue is pushed past the store's anomaly bound, so its
    // earliest recorded anomaly is evicted and its instance is no longer one the group
    // holds.
    record_anomaly(&harness, analysed, &[INSTANCE_A.to_string()]);
    let evicted = instance_id(900);
    let retained = instance_id(901);
    record_anomaly(&harness, crowded, std::slice::from_ref(&evicted));
    let anomaly_bound = service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::MAX_RETAINED_ANOMALIES_PER_GROUP;
    for n in 0..anomaly_bound {
        record_anomaly(&harness, crowded, &[instance_id(1_000 + n)]);
    }
    record_anomaly(&harness, crowded, std::slice::from_ref(&retained));

    // (case, the group named, the instance named): well-formed ids of every shape the
    // modelled pattern allows — all-zero, all-f, mixed — against a group whose analysis
    // holds another instance, against a group whose analysis holds none, against an id
    // that is an instance of ANOTHER group's anomaly, and against one the catalogue has
    // evicted.
    let rows: [(&str, &str, String); 7] = [
        (
            "a group whose analysis holds another instance",
            analysed,
            INSTANCE_B.to_string(),
        ),
        (
            "the lowest id the pattern allows",
            analysed,
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "the highest id the pattern allows",
            analysed,
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        ),
        (
            "a group whose analysis found nothing",
            bare,
            INSTANCE_A.to_string(),
        ),
        (
            "an instance of another group's anomaly",
            bare,
            retained.clone(),
        ),
        (
            "an instance the catalogue has evicted",
            crowded,
            evicted.clone(),
        ),
        (
            "an id one character apart from an instance the group holds",
            crowded,
            format!("{}e", &retained[..retained.len() - 1]),
        ),
    ];

    for (case, group, anomaly_instance_id) in rows {
        let answer = submit(
            &harness,
            group,
            &anomaly_instance_id,
            feedback_body(FEEDBACK_TYPES[0], Some("about an anomaly nobody detected")),
        )
        .await;
        assert_unknown_anomaly(case, &answer, &anomaly_instance_id);
        let held = stored(&harness, group);
        assert!(
            held.is_empty(),
            "case {case}: a refused submission must file nothing, got {held:?}"
        );
    }

    // The instances these groups DO hold are still submittable, so the refusal is about
    // the id and not about the operation.
    for (group, instance) in [(analysed, INSTANCE_A.to_string()), (crowded, retained)] {
        let answer = submit(
            &harness,
            group,
            &instance,
            feedback_body(FEEDBACK_TYPES[0], None),
        )
        .await;
        assert_accepted(&format!("{group} holds {instance}"), &answer);
    }
}

#[tokio::test]
async fn feedback_about_a_group_the_store_cannot_resolve_is_the_modelled_404() {
    let harness = Harness::new();
    let deleted = "feedback-deleted";
    create_group(&harness, deleted).await;
    record_anomaly(
        &harness,
        deleted,
        &[INSTANCE_A.to_string(), INSTANCE_B.to_string()],
    );
    assert_accepted(
        "a submission to seed the group",
        &submit(
            &harness,
            deleted,
            INSTANCE_A,
            feedback_body("Positive", Some("before the delete")),
        )
        .await,
    );
    let gone = delete_group(&harness, deleted).await;
    assert_eq!(
        gone.status, 204,
        "case the delete: DeleteProfilingGroup must answer 204, got {} / {}",
        gone.status,
        String::from_utf8_lossy(&gone.body)
    );

    // (case, the group named, the instance named): a name nobody created, a name of the
    // right shape at both ends of the modelled length range, and a group that existed and
    // was deleted — the anomalies and the feedback went with it. The group's absence is
    // decided BEFORE the anomaly id, so an id the deleted group used to hold is still a
    // 404 and not the 400 of an unknown instance.
    let long_name = "g".repeat(255);
    let rows: [(&str, &str, &str); 4] = [
        ("never created", "feedback-absent", INSTANCE_A),
        ("a one-character name", "g", INSTANCE_B),
        ("a 255-character name", long_name.as_str(), INSTANCE_A),
        ("created and then deleted", deleted, INSTANCE_A),
    ];

    for (case, group, anomaly_instance_id) in rows {
        let answer = submit(
            &harness,
            group,
            anomaly_instance_id,
            feedback_body("Positive", Some("about a group that is not there")),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case}: SubmitFeedback on a group that is not stored must answer 404, \
             got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_message().contains(group),
            "case {case}: the refusal must name the group it could not find, got {:?}",
            answer.error_message()
        );
    }

    // Re-created under the same name, the group holds none of the deleted group's
    // anomalies — so the id it accepted before is refused as unknown — and none of its
    // feedback.
    create_group(&harness, deleted).await;
    assert!(
        stored(&harness, deleted).is_empty(),
        "case the re-created group: a new group holds no feedback, got {:?}",
        stored(&harness, deleted)
    );
    assert_unknown_anomaly(
        "the re-created group",
        &submit(&harness, deleted, INSTANCE_A, feedback_body("Negative", None)).await,
        INSTANCE_A,
    );

    // And it is submittable again once its own analysis records that instance.
    record_anomaly(&harness, deleted, &[INSTANCE_A.to_string()]);
    assert_accepted(
        "the re-created group, analysed again",
        &submit(&harness, deleted, INSTANCE_A, feedback_body("Negative", None)).await,
    );
    assert_eq!(
        stored(&harness, deleted),
        vec![(INSTANCE_A.to_string(), "Negative".to_string(), None)],
        "case the re-created group: it must hold exactly the submission just made"
    );
}

#[tokio::test]
async fn a_request_the_model_refuses_stores_no_feedback() {
    let harness = Harness::new();
    let group = "feedback-refused";
    create_group(&harness, group).await;
    record_anomaly(&harness, group, &[INSTANCE_A.to_string()]);

    // (case, the instance named, the body sent): the modelled constraints the boundary
    // walk enforces — `type` is `@required` and a `FeedbackType`, and
    // `anomalyInstanceId` carries the UUID pattern.
    let rows: [(&str, &str, serde_json::Value); 4] = [
        (
            "a feedback type outside the modelled enum",
            INSTANCE_A,
            serde_json::json!({ "type": "Neutral" }),
        ),
        (
            "a feedback type of the wrong case",
            INSTANCE_A,
            serde_json::json!({ "type": "positive" }),
        ),
        ("no feedback type at all", INSTANCE_A, serde_json::json!({})),
        (
            "an anomaly instance id that is not a uuid",
            "not-a-uuid",
            serde_json::json!({ "type": "Positive" }),
        ),
    ];

    for (case, anomaly_instance_id, body) in rows {
        let answer = submit(&harness, group, anomaly_instance_id, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: a request the model refuses must answer 400, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case}: expected ValidationException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
        let held = stored(&harness, group);
        assert!(
            held.is_empty(),
            "case {case}: a refused request must store nothing, got {held:?}"
        );
    }
}

#[tokio::test]
async fn what_one_group_holds_is_bounded_by_the_stores_retention_limit() {
    let harness = Harness::new();
    let limit = service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::MAX_RETAINED_ANOMALY_FEEDBACK_PER_GROUP;
    // Each row is how far past the bound a group is pushed: at it, one over, and well
    // over — so the bound itself and the eviction past it are both exercised. Each row
    // gets its own group, so a row never starts from what the previous one left stored.
    for over in [0usize, 1, 8] {
        let group = &format!("feedback-retention-{over}");
        create_group(&harness, group).await;
        let submissions = limit + over;
        // ONE detected anomaly holding every instance the row submits about: the
        // submissions, not the anomalies, are what this bound is about.
        let instances: Vec<String> = (0..submissions).map(instance_id).collect();
        record_anomaly(&harness, group, &instances);
        for (n, instance) in instances.iter().enumerate() {
            let answer = submit(
                &harness,
                group,
                instance,
                feedback_body(FEEDBACK_TYPES[n % FEEDBACK_TYPES.len()], None),
            )
            .await;
            assert_accepted(&format!("{over} over the bound, submission {n}"), &answer);
        }

        let held = stored(&harness, group);
        assert_eq!(
            held.len(),
            limit.min(submissions),
            "case {over} over the bound: a group retains at most {limit} submissions, \
             got {}",
            held.len()
        );
        // The submissions retained are the most recent ones: the oldest are evicted
        // first, and the newest submission is never the one dropped.
        let expected: Vec<String> = instances[submissions.saturating_sub(limit)..].to_vec();
        let held_ids: Vec<String> = held.iter().map(|(id, _, _)| id.clone()).collect();
        assert_eq!(
            held_ids, expected,
            "case {over} over the bound: the retained submissions must be the most recent \
             ones, got {held_ids:?}"
        );
    }
}
