// SPDX-License-Identifier: CC0-1.0

//! The span this emulator owes every asynchronous Firehose transition, read over the
//! `awsJson1_1` wire.
//!
//! The rule under test: a transition this service leaves to the clock is due a FIXED span
//! after the call that starts it, and that span is long enough to be observed — a caller
//! that reads the resource back a second or more after the starting call still sees the
//! transient status, and the settled status appears only once the span has elapsed. The
//! rule holds for every such transition the service has, so the rows below span them:
//! the creation of a stream, the enabling of SSE a create asks for, the enabling and the
//! disabling an encryption call starts, and the removal a delete starts.
//!
//! A span too short to outlast the work a caller does between the starting call and its
//! read makes the transient status unobservable — a status the emulator then never shows,
//! however carefully a client looks for it — which is what [`WITHIN_THE_SPAN`] pins: it is
//! an absolute wait, deliberately NOT derived from the emulator's own constant, so
//! shortening that constant fails this suite instead of silently making the transient
//! status a race.
//!
//! The harness (`Emulator`, `at`, `error_code`, `PAST_THE_TRANSITION`) is the lifecycle
//! suite's, and the CMK fixture the start-encryption suite's: one copy of each.

use std::time::Duration;

use serde_json::{json, Value};
// The modelled status catalogs, generated from the service's own Smithy model, so a status
// this suite reads is checked against the model rather than against a hand-copied list.
use service_com_amazonaws_firehose_firehose_20150804::ops::types::shared_types::{
    DELIVERY_STREAM_ENCRYPTION_STATUS_VALUES, DELIVERY_STREAM_STATUS_VALUES,
};

use crate::delivery_stream_encryption::key_arn;
use crate::delivery_stream_lifecycle::{
    at, error_code, s3_configuration, Emulator, PAST_THE_TRANSITION,
};

/// How long after the starting call the transient status must STILL be reported.
///
/// An absolute duration, not a fraction of the emulator's span: the point of the span is
/// that a client doing ordinary work between the starting call and its read observes the
/// transient status, and a client's work is not measured in the emulator's constants. A
/// whole second is far longer than the few hundred milliseconds a control-plane call costs
/// here, and leaves the rest of the span as headroom for a loaded machine.
const WITHIN_THE_SPAN: Duration = Duration::from_millis(1_000);

/// The description member a row is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum About {
    /// `DeliveryStreamDescription$DeliveryStreamStatus`.
    StreamStatus,
    /// `DeliveryStreamEncryptionConfiguration$Status`.
    EncryptionStatus,
}

impl About {
    /// Where the member sits in a `DescribeDeliveryStream` body.
    fn path(self) -> &'static str {
        match self {
            Self::StreamStatus => "DeliveryStreamDescription/DeliveryStreamStatus",
            Self::EncryptionStatus => {
                "DeliveryStreamDescription/DeliveryStreamEncryptionConfiguration/Status"
            }
        }
    }

    /// The modelled enum the member's value has to come from.
    fn catalog(self) -> &'static [&'static str] {
        match self {
            Self::StreamStatus => DELIVERY_STREAM_STATUS_VALUES,
            Self::EncryptionStatus => DELIVERY_STREAM_ENCRYPTION_STATUS_VALUES,
        }
    }
}

/// What has to exist, settled, before the call that starts the transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Before {
    /// Nothing: the starting call is the create itself.
    NoStream,
    /// A settled stream with SSE never asked for.
    Active,
    /// A settled stream with SSE in force, so there is something to disable.
    Encrypted,
}

/// One clock-owed transition: how it is started, and what a read reports while it is in
/// flight and once its span has elapsed.
struct TransitionCase {
    case: &'static str,
    stream: &'static str,
    before: Before,
    /// The operation that starts the transition, and the request body it takes beside the
    /// stream's name.
    operation: &'static str,
    request: Value,
    about: About,
    /// The status reported while the transition is in flight.
    in_flight: &'static str,
    /// The status reported once the span has elapsed — `None` where the transition is the
    /// stream leaving the register, which a describe then reports as a refusal.
    settled: Option<&'static str>,
}

/// Every transition this service leaves to the clock, one row each. The rows differ in the
/// operation that starts the transition, the member it moves and the direction it moves
/// it, which is what the rule generalises over.
fn transition_cases() -> Vec<TransitionCase> {
    vec![
        TransitionCase {
            case: "a created stream reaches ACTIVE",
            stream: "span-created",
            before: Before::NoStream,
            operation: "CreateDeliveryStream",
            request: json!({ "S3DestinationConfiguration": s3_configuration() }),
            about: About::StreamStatus,
            in_flight: "CREATING",
            settled: Some("ACTIVE"),
        },
        TransitionCase {
            case: "SSE under a service-owned CMK asked for at create",
            stream: "span-created-encrypted",
            before: Before::NoStream,
            operation: "CreateDeliveryStream",
            request: json!({
                "S3DestinationConfiguration": s3_configuration(),
                "DeliveryStreamEncryptionConfigurationInput": { "KeyType": "AWS_OWNED_CMK" },
            }),
            about: About::EncryptionStatus,
            in_flight: "ENABLING",
            settled: Some("ENABLED"),
        },
        TransitionCase {
            case: "SSE under a customer managed CMK asked for at create",
            stream: "span-created-customer-key",
            before: Before::NoStream,
            operation: "CreateDeliveryStream",
            request: json!({
                "S3DestinationConfiguration": s3_configuration(),
                "DeliveryStreamEncryptionConfigurationInput": {
                    "KeyType": "CUSTOMER_MANAGED_CMK",
                    "KeyARN": key_arn("span-key"),
                },
            }),
            about: About::EncryptionStatus,
            in_flight: "ENABLING",
            settled: Some("ENABLED"),
        },
        TransitionCase {
            case: "SSE started after the create",
            stream: "span-start-encryption",
            before: Before::Active,
            operation: "StartDeliveryStreamEncryption",
            request: json!({}),
            about: About::EncryptionStatus,
            in_flight: "ENABLING",
            settled: Some("ENABLED"),
        },
        TransitionCase {
            case: "SSE stopped on an encrypted stream",
            stream: "span-stop-encryption",
            before: Before::Encrypted,
            operation: "StopDeliveryStreamEncryption",
            request: json!({}),
            about: About::EncryptionStatus,
            in_flight: "DISABLING",
            settled: Some("DISABLED"),
        },
        TransitionCase {
            case: "a deleted stream leaves the register",
            stream: "span-deleted",
            before: Before::Active,
            operation: "DeleteDeliveryStream",
            request: json!({}),
            about: About::StreamStatus,
            in_flight: "DELETING",
            settled: None,
        },
    ]
}

/// The status `DescribeDeliveryStream` reports for `about`, or `None` once the stream has
/// left the register — which the control plane reports as `ResourceNotFoundException`.
async fn reported_status(
    emulator: &Emulator,
    name: &str,
    about: About,
    case: &str,
) -> Option<String> {
    let (status, described) = emulator
        .call("DescribeDeliveryStream", json!({ "DeliveryStreamName": name }))
        .await;
    if status != 200 {
        assert_eq!(
            error_code(&described),
            "ResourceNotFoundException",
            "case {case:?}: describe got {status} {described:?}"
        );
        return None;
    }
    let reported = at(&described, about.path())
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| panic!("case {case:?}: no {} in {described:?}", about.path()));
    assert!(
        about.catalog().contains(&reported.as_str()),
        "case {case:?}: {} is one of {:?}, got {reported:?}",
        about.path(),
        about.catalog()
    );
    Some(reported)
}

/// Put `case`'s stream in the state the starting call needs, settled.
async fn settled_stream(emulator: &Emulator, case: &TransitionCase) {
    match case.before {
        Before::NoStream => return,
        Before::Active => emulator.create_stream(case.stream, None).await,
        Before::Encrypted => {
            let (status, created) = emulator
                .call(
                    "CreateDeliveryStream",
                    json!({
                        "DeliveryStreamName": case.stream,
                        "S3DestinationConfiguration": s3_configuration(),
                        "DeliveryStreamEncryptionConfigurationInput": {
                            "KeyType": "AWS_OWNED_CMK",
                        },
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {:?}: create with SSE got {created:?}",
                case.case
            );
        }
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    if case.before == Before::Encrypted {
        let settled =
            reported_status(emulator, case.stream, About::EncryptionStatus, case.case).await;
        assert_eq!(
            settled.as_deref(),
            Some("ENABLED"),
            "case {:?}: SSE is in force before the stop, got {settled:?}",
            case.case
        );
    }
}

/// Drive one row: start its transition, read the transient status back after
/// [`WITHIN_THE_SPAN`], and read the settled one back once the span has elapsed.
async fn drive(case: TransitionCase) {
    let emulator = Emulator::new();
    settled_stream(&emulator, &case).await;

    let mut request = case.request.clone();
    request["DeliveryStreamName"] = json!(case.stream);
    let (status, answered) = emulator.call(case.operation, request).await;
    assert_eq!(
        status, 200,
        "case {:?}: {} got {answered:?}",
        case.case, case.operation
    );

    let in_flight = reported_status(&emulator, case.stream, case.about, case.case).await;
    assert_eq!(
        in_flight.as_deref(),
        Some(case.in_flight),
        "case {:?}: the transient status is reported right after {}, got {in_flight:?}",
        case.case,
        case.operation
    );

    tokio::time::sleep(WITHIN_THE_SPAN).await;
    let still = reported_status(&emulator, case.stream, case.about, case.case).await;
    assert_eq!(
        still.as_deref(),
        Some(case.in_flight),
        "case {:?}: the transition is still in flight {WITHIN_THE_SPAN:?} after {}, got \
         {still:?}",
        case.case,
        case.operation
    );

    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let settled = reported_status(&emulator, case.stream, case.about, case.case).await;
    assert_eq!(
        settled.as_deref(),
        case.settled,
        "case {:?}: the clock settles the transition {} started, got {settled:?}",
        case.case,
        case.operation
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn every_clock_owed_transition_is_observable_for_its_span() {
    // One emulator per row, each driven on its own task: the rows' waits overlap instead
    // of adding up, and no row's clock is spent by another row's calls.
    let mut running = Vec::new();
    for case in transition_cases() {
        running.push(tokio::spawn(drive(case)));
    }
    for handle in running {
        match handle.await {
            Ok(()) => {}
            Err(joined) if joined.is_panic() => std::panic::resume_unwind(joined.into_panic()),
            Err(joined) => panic!("a case did not run to completion: {joined}"),
        }
    }
}
