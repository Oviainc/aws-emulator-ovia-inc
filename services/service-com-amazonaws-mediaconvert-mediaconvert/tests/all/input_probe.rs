// SPDX-License-Identifier: CC0-1.0

//! A job is never classified from a probe this emulator could not perform.
//!
//! The rule under test is the second half of the one `Job$ErrorCode` carries: a terminal
//! failure reports the code that classifies WHAT actually failed, so a code is reported
//! ONLY for a failure the service really observed — a failure it could not observe is
//! answered with NO code rather than an invented one, and the job still says why it
//! failed.
//!
//! This suite is the one that pins the "could not observe" side. The emulator it builds
//! carries MediaConvert ALONE, so the in-process `HeadObject` the input probe sends
//! reaches no S3 at all and the router answers it 404 with no S3 error code. A bare 404
//! is exactly what an absent OBJECT would also answer with, so this is the case where
//! reading the status alone would classify a job from the absence of a SERVICE rather
//! than from the absence of a file: every row below must therefore come back with
//! `ErrorCode` absent, whatever shape its inputs have.
//!
//! The rows span what the rule generalises over — how many inputs the job states, whether
//! their URIs are S3 locations the probe would address, locations it cannot address, or
//! no inputs at all. The 1404 side of the same rule, where S3 IS mounted and really
//! answers for the address, is `core/router/tests/all/mediaconvert_input_probe.rs`, which
//! needs two services on one engine.

use serde_json::{json, Value};

use super::job_lifecycle::{Emulator, Reply, ROLE, SHORT_SETTLE};

/// Comfortably past [`SHORT_SETTLE`], so every job below has reached its terminal status.
const PAST_THE_SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// A `CreateJob` body whose `Settings.Inputs` are exactly `inputs`.
fn job_body(inputs: &[&str]) -> Value {
    let inputs: Vec<Value> = inputs
        .iter()
        .map(|uri| json!({ "fileInput": uri }))
        .collect();
    json!({
        "role": ROLE,
        "settings": {
            "inputs": inputs,
            "outputGroups": [{
                "name": "File Group",
                "outputs": [{ "nameModifier": "_out" }],
                "outputGroupSettings": {
                    "type": "FILE_GROUP_SETTINGS",
                    "fileGroupSettings": { "destination": "s3://a-bucket/out/" }
                }
            }]
        }
    })
}

/// One row: a label and the `Settings.Inputs[].FileInput` URIs the job submits.
struct Case {
    case: &'static str,
    inputs: &'static [&'static str],
}

/// The table spans the input class along what the rule generalises over: the NUMBER of
/// inputs and whether each one is an address the probe would even send a request for.
const CASES: &[Case] = &[
    Case {
        case: "one S3 input, with no S3 in this emulator to answer for it",
        inputs: &["s3://a-bucket/in/one.mp4"],
    },
    Case {
        case: "several S3 inputs in different buckets",
        inputs: &[
            "s3://a-bucket/in/one.mp4",
            "s3://another-bucket/in/two.mov",
            "s3://a-bucket/deeper/in/three.mxf",
        ],
    },
    Case {
        case: "an S3 input whose key has no prefix",
        inputs: &["s3://a-bucket/one.mp4"],
    },
    Case {
        case: "an input that is not an S3 location",
        inputs: &["https://example.com/in/one.mp4"],
    },
    Case {
        case: "an s3 URI with an empty key, which is not an object address",
        inputs: &["s3://a-bucket/"],
    },
    Case {
        case: "an S3 input beside one that is not an S3 location",
        inputs: &["https://example.com/in/one.mp4", "s3://a-bucket/in/two.mp4"],
    },
];

/// The `errorCode` member of a `GetJob` reply, as an integer, or `None` when it is absent.
fn error_code(reply: &Reply) -> Option<i64> {
    reply.at("/job/errorCode").as_i64()
}

#[tokio::test]
async fn a_probe_that_could_not_run_classifies_no_job() {
    let emulator = Emulator::with_settle_span(SHORT_SETTLE);
    let queue = "probe-unavailable-queue";
    let created = emulator.create_queue(json!({ "name": queue })).await;
    assert_eq!(
        created.status, 201,
        "setup: CreateQueue must succeed, got {} {:?}",
        created.status, created.body
    );

    // Every job is submitted first and read once the clock has had its span, so the suite
    // waits once rather than per row.
    let mut ids = Vec::new();
    for case in CASES {
        let mut body = job_body(case.inputs);
        body["queue"] = Value::String(queue.to_string());
        let created = emulator.create_job(body).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: CreateJob must succeed, got {} {:?}",
            case.case, created.status, created.body
        );
        let id = created.string("/job/id");
        assert!(
            !id.is_empty(),
            "case {:?}: CreateJob must mint an Id, got {:?}",
            case.case,
            created.body
        );
        // The probe is part of the create, so a reply that came back at all is proof the
        // handler did not fail on an unanswerable callback.
        assert_eq!(
            error_code(&created),
            None,
            "case {:?}: a job that has not failed yet carries no ErrorCode, got {:?}",
            case.case,
            created.body
        );
        ids.push(id);
    }

    // The due instant is stored, never slept for by a handler; the suite waits past it.
    tokio::time::sleep(PAST_THE_SETTLE).await;

    for (case, id) in CASES.iter().zip(&ids) {
        let read = emulator.get_job(id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: GetJob must succeed, got {} {:?}",
            case.case, read.status, read.body
        );
        let status = read.string("/job/status");
        assert_eq!(
            status, "ERROR",
            "case {:?}: the job must have reached ERROR, got {status:?} in {:?}",
            case.case, read.body
        );
        let actual = error_code(&read);
        assert_eq!(
            actual, None,
            "case {:?}: a failure this emulator could not observe must carry no \
             ErrorCode, got {actual:?} in {:?}",
            case.case, read.body
        );
        let message = read.string("/job/errorMessage");
        assert!(
            !message.is_empty(),
            "case {:?}: an ERROR job must still say why it failed, got {message:?}",
            case.case
        );
        // The failure that was NOT observed must not be reported as one: the message is
        // the unclassified one, so it names no input of this job.
        for input in case.inputs {
            assert!(
                !message.contains(input),
                "case {:?}: an unclassified failure must not name {input:?} as the input \
                 that failed, got {message:?}",
                case.case
            );
        }
    }
}
