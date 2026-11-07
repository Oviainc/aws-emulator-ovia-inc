// SPDX-License-Identifier: CC0-1.0

//! What a client polling from OUTSIDE this process sees of Comprehend's asynchronous
//! lifecycles, and what a cancellation issued from there can still reach.
//!
//! RULE: every non-terminal phase of an asynchronous lifecycle this service runs
//! lasts one whole phase span, so a client polling the matching `Describe*` on a
//! cadence of a second or more OBSERVES each intermediate status of the documented
//! walk, and a call that acts on a transient phase — a stop — issued once that phase
//! has been observed still lands on it rather than finding the resource already
//! settled.
//!
//! The first suite is a TABLE over the lifecycles this service runs (two analysis-job
//! families that complete, one whose run reports a failure, a document classifier's
//! training and an endpoint's provisioning), polled at a one-second cadence, and
//! asserts every intermediate status of each walk is observed, in the documented
//! order. The second is a TABLE over the cancellations, differing along the resource
//! kind and the phase the stop arrives in, and asserts the stop reaches `STOPPED`
//! instead of being dropped on a resource whose lifecycle had already settled.
//!
//! The sibling suites poll as fast as an in-process caller can, which is what hid
//! both defects: a phase a few hundred milliseconds wide is observable at a 20 ms
//! cadence and invisible at a realistic one.

// `std::thread::sleep`, not `tokio::time::sleep`: the workspace's shared `tokio`
// carries no `time` feature, so there is no timer driver. It is safe here only
// because every call below is awaited sequentially on a current-thread runtime with
// nothing in the background.
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::comprehend_client::{create_bucket_payload, Comprehend};

/// The bucket every row's input documents live in.
const BUCKET: &str = "lifecycle-observability-bucket";

/// The role ARN every submission names; the modelled `IamRoleArn` form.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/ComprehendRole";

/// The cadence a client in ANOTHER PROCESS polls a `Describe*` on.
///
/// One second is the fastest cadence this suite is allowed to assume — the rule has
/// to hold for a cadence of a second or more — so a phase narrower than this is
/// unobservable to the clients the rule is about.
const POLL_CADENCE: Duration = Duration::from_secs(1);

/// Longest a suite waits for a lifecycle to reach a terminal status while polling at
/// [`POLL_CADENCE`]. A walk is at most two phase spans long
/// (`crate::state::LIFECYCLE_PHASE_SPAN` each) plus the stop's own; this leaves ample
/// head-room on a loaded machine.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(60);

/// The key the analysed document is written under, inside the prefix a submission
/// names.
const DOCUMENT_KEY: &str = "input/doc.txt";

/// A document an analysis run has something to report about.
const DOCUMENT: &str =
    "The new laptop screen is broken. Send a replacement screen for the laptop, quickly.";

/// The key the labelled training data is written under.
const TRAINING_KEY: &str = "train.csv";

/// A `COMPREHEND_CSV` training corpus: three labels whose documents share their own
/// vocabulary, and enough rows for the documented held-out test share.
const TRAINING_CSV: &str = "BILLING,invoice payment refund\n\
                            BILLING,refund invoice payment\n\
                            BILLING,payment invoice refund\n\
                            BILLING,invoice refund payment\n\
                            SUPPORT,laptop screen repair\n\
                            SUPPORT,screen repair laptop\n\
                            SUPPORT,repair laptop screen\n\
                            SUPPORT,laptop repair screen\n\
                            SALES,quote discount contract\n\
                            SALES,discount contract quote\n\
                            SALES,contract quote discount\n\
                            SALES,quote contract discount\n";

/// One asynchronous plane this service runs a lifecycle for.
#[derive(Debug, Clone, Copy)]
enum Plane {
    /// One `Start*DetectionJob` family: its start and describe operations, the member
    /// its describe reports the job's properties under, the stop operation that
    /// cancels it (`None` while that stop is still a scaffold) and whether the
    /// submission names a `LanguageCode` — the dominant-language family detects it
    /// instead.
    Job {
        start: &'static str,
        describe: &'static str,
        properties: &'static str,
        stop: Option<&'static str>,
        language: bool,
    },
    /// A custom document classifier's training.
    Classifier,
    /// A model-specific inference endpoint's provisioning.
    Endpoint,
}

/// The dominant-language family, whose submission names no `LanguageCode`.
const DOMINANT_LANGUAGE: Plane = Plane::Job {
    start: "StartDominantLanguageDetectionJob",
    describe: "DescribeDominantLanguageDetectionJob",
    properties: "DominantLanguageDetectionJobProperties",
    stop: Some("StopDominantLanguageDetectionJob"),
    language: false,
};

/// The key-phrases family. Its stop is still a scaffold, so only the walk is asserted
/// for it.
const KEY_PHRASES: Plane = Plane::Job {
    start: "StartKeyPhrasesDetectionJob",
    describe: "DescribeKeyPhrasesDetectionJob",
    properties: "KeyPhrasesDetectionJobProperties",
    stop: None,
    language: true,
};

/// The sentiment family.
const SENTIMENT: Plane = Plane::Job {
    start: "StartSentimentDetectionJob",
    describe: "DescribeSentimentDetectionJob",
    properties: "SentimentDetectionJobProperties",
    stop: Some("StopSentimentDetectionJob"),
    language: true,
};

/// The member a plane's `Describe*` reports its status under: `JobStatus` for an
/// analysis job, `Status` for a resource.
fn status_member(plane: Plane) -> &'static str {
    match plane {
        Plane::Job { .. } => "JobStatus",
        Plane::Classifier | Plane::Endpoint => "Status",
    }
}

/// Create `BUCKET`, so a run's reads and writes have somewhere to go.
async fn create_bucket(comprehend: &Comprehend) {
    let (status, body) = comprehend
        .s3("PUT", &format!("/{BUCKET}"), create_bucket_payload().as_bytes())
        .await;
    assert_eq!(
        status,
        200,
        "CreateBucket {BUCKET}: expected 200, got {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// Write one object.
async fn put_object(comprehend: &Comprehend, key: &str, body: &[u8]) {
    let (status, response) = comprehend
        .s3("PUT", &format!("/{BUCKET}/{key}"), body)
        .await;
    assert_eq!(
        status,
        200,
        "PutObject {key}: expected 200, got {status}: {:?}",
        String::from_utf8_lossy(&response)
    );
}

/// The properties `plane`'s `Describe*` reports for `id`.
async fn properties(comprehend: &Comprehend, plane: Plane, id: &str) -> Value {
    match plane {
        Plane::Job {
            describe,
            properties,
            ..
        } => {
            let output = comprehend.ok(describe, &json!({ "JobId": id })).await;
            output[properties].clone()
        }
        Plane::Classifier => {
            let output = comprehend
                .ok(
                    "DescribeDocumentClassifier",
                    &json!({ "DocumentClassifierArn": id }),
                )
                .await;
            output["DocumentClassifierProperties"].clone()
        }
        Plane::Endpoint => {
            let output = comprehend
                .ok("DescribeEndpoint", &json!({ "EndpointArn": id }))
                .await;
            output["EndpointProperties"].clone()
        }
    }
}

/// The status those properties report, as a plain string.
fn reported_status(plane: Plane, properties: &Value) -> String {
    properties[status_member(plane)]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Start an analysis job of `plane` over the prefix the suite writes its document
/// under, and return its `JobId`.
async fn start_job(comprehend: &Comprehend, plane: Plane) -> String {
    let Plane::Job {
        start, language, ..
    } = plane
    else {
        panic!("start_job is for an analysis-job family, got {plane:?}");
    };
    let mut body = json!({
        "InputDataConfig": { "S3Uri": format!("s3://{BUCKET}/input/") },
        "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/output/") },
        "DataAccessRoleArn": ROLE_ARN,
    });
    if language {
        body["LanguageCode"] = json!("en");
    }
    let submitted = comprehend.ok(start, &body).await;
    submitted["JobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {start:?}: expected a JobId, got {submitted}"))
        .to_string()
}

/// Create a document classifier over the training data the suite wrote, and return
/// its ARN.
async fn create_classifier(comprehend: &Comprehend, name: &str) -> String {
    let output = comprehend
        .ok(
            "CreateDocumentClassifier",
            &json!({
                "DocumentClassifierName": name,
                "DataAccessRoleArn": ROLE_ARN,
                "LanguageCode": "en",
                "InputDataConfig": { "S3Uri": format!("s3://{BUCKET}/{TRAINING_KEY}") },
            }),
        )
        .await;
    output["DocumentClassifierArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: expected an ARN, got {output}"))
        .to_string()
}

/// Start the lifecycle `plane` and return the identifier its `Describe*` names: a
/// `JobId` for an analysis job, the resource's ARN otherwise.
///
/// An endpoint is created over a model this suite trained first, because an endpoint
/// only comes into service behind a trained model; that training is setup, so it is
/// waited out as fast as the store will answer rather than at [`POLL_CADENCE`].
async fn begin(comprehend: &Comprehend, plane: Plane) -> String {
    match plane {
        Plane::Job { .. } => start_job(comprehend, plane).await,
        Plane::Classifier => create_classifier(comprehend, "observable-classifier").await,
        Plane::Endpoint => {
            let model = create_classifier(comprehend, "endpoint-model").await;
            let started = Instant::now();
            loop {
                let reported =
                    reported_status(Plane::Classifier, &properties(comprehend, Plane::Classifier, &model).await);
                if matches!(reported.as_str(), "TRAINED" | "TRAINED_WITH_WARNING") {
                    break;
                }
                assert!(
                    started.elapsed() < SETTLE_TIMEOUT,
                    "case \"an endpoint's provisioning\": the model never trained, last status \
                     {reported:?}"
                );
                sleep(Duration::from_millis(20));
            }
            let output = comprehend
                .ok(
                    "CreateEndpoint",
                    &json!({
                        "EndpointName": "observable-endpoint",
                        "ModelArn": model,
                        "DesiredInferenceUnits": 1,
                    }),
                )
                .await;
            output["EndpointArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case \"an endpoint\": expected an ARN, got {output}"))
                .to_string()
        }
    }
}

/// Poll `plane`'s `Describe*` for `id` at [`POLL_CADENCE`] until it reports a status
/// that is not in `transient`, returning every distinct status seen in order and the
/// properties the walk ended with.
async fn walk_at_poll_cadence(
    comprehend: &Comprehend,
    case: &str,
    plane: Plane,
    id: &str,
    transient: &[&str],
) -> (Vec<String>, Value) {
    let started = Instant::now();
    let mut seen: Vec<String> = Vec::new();
    loop {
        let reported = properties(comprehend, plane, id).await;
        let status = reported_status(plane, &reported);
        if seen.last() != Some(&status) {
            seen.push(status.clone());
        }
        if !transient.contains(&status.as_str()) {
            return (seen, reported);
        }
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case {case:?}: never left the transient phases {transient:?}; statuses seen: \
             {seen:?}"
        );
        sleep(POLL_CADENCE);
    }
}

/// One row of the walk table.
struct Walk {
    case: &'static str,
    plane: Plane,
    /// Whether the input documents are written before the lifecycle starts: a run
    /// whose input location holds nothing reports a failure, which is the other
    /// terminal status of the same walk.
    input_present: bool,
    /// Every non-terminal status the walk passes through, in the documented order.
    transient: &'static [&'static str],
    /// The statuses the walk may end in.
    terminal: &'static [&'static str],
}

/// The documented `JobStatus` walk of an analysis job up to its terminal status:
/// "SUBMITTED - The job has been received and is queued for processing. IN_PROGRESS -
/// Amazon Comprehend is processing the job."
const JOB_TRANSIENT: &[&str] = &["SUBMITTED", "IN_PROGRESS"];

/// The `ModelStatus` phases a classifier's training passes through before it ends.
const CLASSIFIER_TRANSIENT: &[&str] = &["SUBMITTED", "TRAINING"];

/// Every lifecycle the first suite walks.
const WALKS: &[Walk] = &[
    Walk {
        case: "a dominant-language detection job that completes",
        plane: DOMINANT_LANGUAGE,
        input_present: true,
        transient: JOB_TRANSIENT,
        terminal: &["COMPLETED"],
    },
    Walk {
        case: "a key-phrases detection job that completes",
        plane: KEY_PHRASES,
        input_present: true,
        transient: JOB_TRANSIENT,
        terminal: &["COMPLETED"],
    },
    Walk {
        case: "a sentiment detection job whose input location holds nothing",
        plane: SENTIMENT,
        input_present: false,
        transient: JOB_TRANSIENT,
        terminal: &["FAILED"],
    },
    Walk {
        case: "a document classifier's training",
        plane: Plane::Classifier,
        input_present: true,
        transient: CLASSIFIER_TRANSIENT,
        terminal: &["TRAINED", "TRAINED_WITH_WARNING"],
    },
    Walk {
        case: "an endpoint's provisioning",
        plane: Plane::Endpoint,
        input_present: true,
        transient: &["CREATING"],
        terminal: &["IN_SERVICE"],
    },
];

#[tokio::test]
async fn every_non_terminal_phase_is_observable_at_a_one_second_poll_cadence() {
    for walk in WALKS {
        let case = walk.case;
        let comprehend = Comprehend::with_object_storage();
        if walk.input_present {
            create_bucket(&comprehend).await;
            put_object(&comprehend, DOCUMENT_KEY, DOCUMENT.as_bytes()).await;
            put_object(&comprehend, TRAINING_KEY, TRAINING_CSV.as_bytes()).await;
        }

        let id = begin(&comprehend, walk.plane).await;
        let (seen, ended) =
            walk_at_poll_cadence(&comprehend, case, walk.plane, &id, walk.transient).await;

        // EVERY intermediate phase was seen by a poller on a realistic cadence —
        // the rule the issue reported broken, where a job went straight from
        // SUBMITTED to COMPLETED.
        for phase in walk.transient {
            assert!(
                seen.iter().any(|status| status == phase),
                "case {case:?}: expected the phase {phase:?} to be observable at a \
                 {POLL_CADENCE:?} poll cadence, statuses seen: {seen:?}"
            );
        }
        // The statuses only ever move FORWARD along the documented order.
        let order: Vec<&str> = walk
            .transient
            .iter()
            .copied()
            .chain(walk.terminal.iter().copied())
            .collect();
        let ranks: Vec<usize> = seen
            .iter()
            .map(|status| {
                order
                    .iter()
                    .position(|known| known == status)
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: unexpected status {status:?}, expected one of \
                             {order:?}, statuses seen: {seen:?}"
                        )
                    })
            })
            .collect();
        assert!(
            ranks.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case:?}: expected the statuses to move forward along {order:?}, statuses \
             seen: {seen:?}"
        );
        let last = seen.last().cloned().unwrap_or_default();
        assert!(
            walk.terminal.contains(&last.as_str()),
            "case {case:?}: expected to end in one of {:?}, got {last:?}: {ended}",
            walk.terminal
        );
    }
}

/// One row of the cancellation table.
struct Cancellation {
    case: &'static str,
    plane: Plane,
    /// The status the caller polls for, at [`POLL_CADENCE`], before issuing the stop
    /// — the phase the stop has to still find the resource in.
    stop_after: &'static str,
    /// The status the stop marks the resource with and leaves readable, or `None`
    /// when the resource has nothing to wind down and stops outright.
    marked: Option<&'static str>,
}

/// The terminal status a stop that lands owes the resource.
const STOPPED: &str = "STOPPED";

/// Every cancellation the second suite issues: two resource kinds, and both the
/// queued phase and the running phase a stop can arrive in.
const CANCELLATIONS: &[Cancellation] = &[
    Cancellation {
        case: "a document classifier stopped while TRAINING",
        plane: Plane::Classifier,
        stop_after: "TRAINING",
        marked: Some("STOP_REQUESTED"),
    },
    Cancellation {
        case: "a document classifier stopped while still SUBMITTED",
        plane: Plane::Classifier,
        stop_after: "SUBMITTED",
        marked: None,
    },
    Cancellation {
        case: "a dominant-language detection job stopped while IN_PROGRESS",
        plane: DOMINANT_LANGUAGE,
        stop_after: "IN_PROGRESS",
        marked: Some("STOP_REQUESTED"),
    },
    Cancellation {
        case: "a sentiment detection job stopped while still SUBMITTED",
        plane: SENTIMENT,
        stop_after: "SUBMITTED",
        marked: Some("STOP_REQUESTED"),
    },
];

/// Poll at [`POLL_CADENCE`] until `plane` reports `wanted` for `id`, failing if the
/// lifecycle settles into something a stop can no longer reach.
async fn poll_until(
    comprehend: &Comprehend,
    case: &str,
    plane: Plane,
    id: &str,
    wanted: &str,
    before: &[&str],
) {
    let started = Instant::now();
    let mut seen: Vec<String> = Vec::new();
    loop {
        let reported = properties(comprehend, plane, id).await;
        let status = reported_status(plane, &reported);
        if seen.last() != Some(&status) {
            seen.push(status.clone());
        }
        if status == wanted {
            return;
        }
        assert!(
            before.contains(&status.as_str()),
            "case {case:?}: the lifecycle reached {status:?} before {wanted:?} could be \
             observed, statuses seen: {seen:?}"
        );
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case {case:?}: never reported {wanted:?}, statuses seen: {seen:?}"
        );
        sleep(POLL_CADENCE);
    }
}

/// Issue `plane`'s stop for `id`, asserting the response the operation documents, and
/// return the status it reported (an analysis job's stop reports one; the classifier's
/// answers an empty body).
async fn request_stop(comprehend: &Comprehend, case: &str, plane: Plane, id: &str) -> Option<String> {
    match plane {
        Plane::Job { stop, .. } => {
            let stop = stop.unwrap_or_else(|| panic!("case {case:?}: this family models no stop"));
            let (status, body) = comprehend.call(stop, &json!({ "JobId": id })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {stop} to answer 200, got {status}: {body}"
            );
            Some(
                body["JobStatus"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a JobStatus, got {body}"))
                    .to_string(),
            )
        }
        Plane::Classifier => {
            let (status, body) = comprehend
                .call(
                    "StopTrainingDocumentClassifier",
                    &json!({ "DocumentClassifierArn": id }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected StopTrainingDocumentClassifier to answer 200, got \
                 {status}: {body}"
            );
            // "the service sends back an HTTP 200 response with an empty HTTP body":
            // the output shape has no members, so the serialized body carries none.
            assert!(
                body.as_object().is_some_and(serde_json::Map::is_empty) || body == Value::Null,
                "case {case:?}: expected a body carrying no members, got {body}"
            );
            None
        }
        Plane::Endpoint => panic!("case {case:?}: an endpoint models no stop"),
    }
}

#[tokio::test]
async fn a_stop_issued_once_the_phase_is_observed_reaches_stopped() {
    for cancellation in CANCELLATIONS {
        let case = cancellation.case;
        let plane = cancellation.plane;
        let comprehend = Comprehend::with_object_storage();
        create_bucket(&comprehend).await;
        put_object(&comprehend, DOCUMENT_KEY, DOCUMENT.as_bytes()).await;
        put_object(&comprehend, TRAINING_KEY, TRAINING_CSV.as_bytes()).await;

        let id = begin(&comprehend, plane).await;
        // The phase the stop is issued from has to be reachable by a poller on a
        // realistic cadence: a classifier that has already settled to TRAINED, or a
        // job already COMPLETED, is what made cancellation inoperative.
        let before: &[&str] = match plane {
            Plane::Classifier => CLASSIFIER_TRANSIENT,
            _ => JOB_TRANSIENT,
        };
        poll_until(&comprehend, case, plane, &id, cancellation.stop_after, before).await;

        let reported = request_stop(&comprehend, case, plane, &id).await;
        let expected_report = cancellation.marked.unwrap_or(STOPPED);
        if let Some(reported) = &reported {
            assert_eq!(
                reported, expected_report,
                "case {case:?}: expected the stop to report {expected_report:?}, got \
                 {reported:?}"
            );
        }
        // The status the stop left is READABLE, so a client that waits on the
        // intermediate state sees it.
        let after = reported_status(plane, &properties(&comprehend, plane, &id).await);
        assert_eq!(
            after, expected_report,
            "case {case:?}: expected {expected_report:?} to be readable straight after the \
             stop, got {after:?}"
        );

        // And the clock then owes the cancelled resource STOPPED — never the status
        // it would have reached had nobody stopped it.
        let transient: Vec<&str> = cancellation.marked.into_iter().collect();
        let (seen, ended) =
            walk_at_poll_cadence(&comprehend, case, plane, &id, &transient).await;
        assert_eq!(
            seen.last().map(String::as_str),
            Some(STOPPED),
            "case {case:?}: expected the stop to settle into {STOPPED:?}, statuses seen \
             {seen:?}: {ended}"
        );

        // STOPPED is terminal: reading it again never moves it on.
        let once = properties(&comprehend, plane, &id).await;
        let twice = properties(&comprehend, plane, &id).await;
        assert_eq!(
            once, twice,
            "case {case:?}: reading a stopped resource changed it: {once} then {twice}"
        );
    }
}
