// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CancelSpotFleetRequests`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. a cancellation MOVES THE FLEET TO THE CANCELLED STATE `TerminateInstances` CHOOSES —
//!    `cancelled_terminating` when the instances the fleet launched are terminated with it,
//!    `cancelled_running` when they are left to run — and the call reports the state the fleet
//!    was in beside the one a describe now reports;
//! 2. "after you cancel a Spot Fleet request, the Spot Fleet launches no new instances": every
//!    Spot Instance request the fleet placed is cancelled, carrying the Spot request status
//!    code for the way it ended, so the clock launches nothing more for it;
//! 3. a cancelled fleet reaches `cancelled` once it has NO RUNNING SPOT INSTANCE left — owed to
//!    the clock and to those instances, so a fleet whose instances keep running stays
//!    `cancelled_running` until something terminates them;
//! 4. an id that names no cancellable fleet is ONE ENTRY of `UnsuccessfulFleetRequests` with
//!    its `CancelBatchErrorCode`, never a refusal of the call: the other ids are still
//!    cancelled;
//! 5. a request that cannot be served at all is refused and cancels nothing — `DryRun`
//!    performs nothing, and a batch above the documented bound of 100 ids deletes no fleet.
//!
//! Each table row is a distinct input along what the rule generalises over — which
//! `TerminateInstances`, whether the fleet's capacity has been fulfilled yet, how much target
//! capacity, how many pools, which unservable id — so no row is the specification.

use std::time::Duration;

use ec2::state::{
    CANCEL_BATCH_FLEET_REQUEST_ID_DOES_NOT_EXIST, CANCEL_BATCH_FLEET_REQUEST_ID_MALFORMED,
    CANCEL_BATCH_FLEET_REQUEST_NOT_IN_CANCELLABLE_STATE, ComAmazonawsEc2Amazonec2State as Ec2State,
    MAX_SPOT_FLEET_REQUESTS_PER_CANCEL, SPOT_FLEET_ACTIVITY_FULFILLED,
    SPOT_FLEET_ACTIVITY_PENDING_FULFILLMENT, SPOT_FLEET_ACTIVITY_PENDING_TERMINATION,
    SPOT_FLEET_STATE_ACTIVE, SPOT_FLEET_STATE_CANCELLED, SPOT_FLEET_STATE_CANCELLED_RUNNING,
    SPOT_FLEET_STATE_CANCELLED_TERMINATING, SPOT_FLEET_STATE_SUBMITTED,
    SPOT_REQUEST_STATE_CANCELLED, SPOT_STATUS_CANCELED_BEFORE_FULFILLMENT,
    SPOT_STATUS_INSTANCE_TERMINATED_BY_USER, SPOT_STATUS_REQUEST_CANCELED_AND_INSTANCE_RUNNING,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, described_instance_state, error_code, error_message, indexed};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The ARN of the IAM role a Spot Fleet request grants the fleet; the account is the caller's
/// rather than this emulator's, which is what `RequestSpotFleet` allows.
const FLEET_ROLE: &str = "arn:aws:iam::111122223333:role/aws-ec2-spot-fleet-tagging-role";

/// A well-formed Spot Fleet request id no fleet of the account is stored under: the mint counts
/// from one, so a high counter value names nothing.
const UNKNOWN_FLEET_ID: &str = "sfr-0000ffff-0000-0000-0000-00000000ffff";

/// Something that is not a Spot Fleet request id at all.
const MALFORMED_FLEET_ID: &str = "sfr-not-an-id";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `SpotFleetRequestConfig.<member>` parameter.
fn c(member: &str, value: impl Into<String>) -> (String, String) {
    p(&format!("SpotFleetRequestConfig.{member}"), value)
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// An AMI of the published catalog a fresh account can already see, read out of the service's
/// OWN state: a launch specification must name an image the account can resolve, so the test
/// cannot invent an id.
fn image_id() -> String {
    Ec2State::default()
        .images
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must publish at least one image")
}

/// Wait until every clock-owed transition started before now has certainly fallen due. Derived
/// from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// How many instances the store holds.
fn stored_instances(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().instances.len()
}

/// Request one Spot Fleet of `target_capacity` units over `instance_types` (one pool each) and
/// answer the fleet id it minted.
async fn requested_fleet(
    case: &str,
    router: &MultiServiceRouter,
    target_capacity: i64,
    instance_types: &[&str],
) -> String {
    let mut params: Params = vec![
        c("IamFleetRole", FLEET_ROLE),
        c("TargetCapacity", target_capacity.to_string()),
    ];
    for (index, instance_type) in instance_types.iter().enumerate() {
        let position = index + 1;
        params.push(c(&format!("LaunchSpecifications.{position}.ImageId"), image_id()));
        params.push(c(
            &format!("LaunchSpecifications.{position}.InstanceType"),
            *instance_type,
        ));
    }
    let (status, body) = call(router, REGION, "RequestSpotFleet", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from RequestSpotFleet, got {status}: {body}"
    );
    let parsed = document(case, &body);
    child_text(parsed.root_element(), "spotFleetRequestId")
        .unwrap_or_else(|| panic!("case {case:?}: no spotFleetRequestId in {body}"))
}

/// The `SpotFleetRequestId.N` and `TerminateInstances` fields of one cancellation.
fn cancel_params(ids: &[&str], terminate_instances: bool) -> Params {
    let mut params = indexed("SpotFleetRequestId", ids);
    params.push(p(
        "TerminateInstances",
        if terminate_instances { "true" } else { "false" },
    ));
    params
}

async fn cancel(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CancelSpotFleetRequests", params).await
}

/// One entry of `SuccessfulFleetRequests`.
#[derive(Debug)]
struct Cancelled {
    id: String,
    previous: String,
    current: String,
}

/// Every fleet a cancellation reports as cancelled, in document order.
fn cancelled(case: &str, body: &str) -> Vec<Cancelled> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "successfulFleetRequestSet")
        .into_iter()
        .map(|item| Cancelled {
            id: child_text(item, "spotFleetRequestId").unwrap_or_default(),
            previous: child_text(item, "previousSpotFleetRequestState").unwrap_or_default(),
            current: child_text(item, "currentSpotFleetRequestState").unwrap_or_default(),
        })
        .collect()
}

/// One entry of `UnsuccessfulFleetRequests`.
#[derive(Debug)]
struct NotCancelled {
    id: String,
    code: String,
    message: String,
}

/// Every fleet a cancellation reports as not cancelled, in document order.
fn not_cancelled(case: &str, body: &str) -> Vec<NotCancelled> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "unsuccessfulFleetRequestSet")
        .into_iter()
        .map(|item| NotCancelled {
            id: child_text(item, "spotFleetRequestId").unwrap_or_default(),
            code: child(item, "error")
                .and_then(|error| child_text(error, "code"))
                .unwrap_or_default(),
            message: child(item, "error")
                .and_then(|error| child_text(error, "message"))
                .unwrap_or_default(),
        })
        .collect()
}

/// The `BatchState` and the `ActivityStatus` a describe reports for one fleet.
async fn described_fleet(
    case: &str,
    router: &MultiServiceRouter,
    fleet_id: &str,
) -> (String, String) {
    let (status, body) = call(
        router,
        REGION,
        "DescribeSpotFleetRequests",
        &indexed("SpotFleetRequestId", &[fleet_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSpotFleetRequests, got {status}: {body}"
    );
    let parsed = document(case, &body);
    let item = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "spotFleetRequestState").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no fleet reported for {fleet_id} in {body}"));
    (
        child_text(item, "spotFleetRequestState").unwrap_or_default(),
        child_text(item, "activityStatus").unwrap_or_default(),
    )
}

/// One Spot Instance request as `DescribeSpotInstanceRequests` reports it.
#[derive(Debug)]
struct SpotRequest {
    state: String,
    status_code: String,
    instance_id: Option<String>,
}

/// Every Spot Instance request the account holds, in the order the describe reports them.
async fn spot_requests(case: &str, router: &MultiServiceRouter) -> Vec<SpotRequest> {
    let (status, body) = call(router, REGION, "DescribeSpotInstanceRequests", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSpotInstanceRequests, got {status}: {body}"
    );
    let parsed = document(case, &body);
    parsed
        .descendants()
        .filter(|node| {
            node.is_element()
                && child(*node, "spotInstanceRequestId").is_some()
                && child(*node, "state").is_some()
        })
        .map(|item| SpotRequest {
            state: child_text(item, "state").unwrap_or_default(),
            status_code: child(item, "status")
                .and_then(|status| child_text(status, "code"))
                .unwrap_or_default(),
            instance_id: child_text(item, "instanceId"),
        })
        .collect()
}

/// One row of the cancellation table: a fleet that differs from the others along what rules 1
/// and 2 generalise over.
struct CancelCase {
    case: &'static str,
    target_capacity: i64,
    /// One launch specification per instance type, in the order the request gives them.
    instance_types: &'static [&'static str],
    /// Whether the fleet's Spot Instance requests are left to be fulfilled before the
    /// cancellation, which is what puts the fleet in `active` with instances to terminate.
    fulfilled_first: bool,
    terminate_instances: bool,
    expected_previous: &'static str,
    expected_current: &'static str,
    expected_activity: &'static str,
    /// The Spot request status code every request the fleet placed carries afterwards.
    expected_status_code: &'static str,
    /// The `InstanceStateName` the instances the fleet launched are in afterwards, or `None`
    /// when the fleet never launched one.
    expected_instance_state: Option<&'static str>,
}

/// The rows of the cancellation table. No row repeats another's combination of
/// `TerminateInstances`, fulfillment, target capacity and pool count.
const CANCEL_CASES: &[CancelCase] = &[
    CancelCase {
        case: "cancelled before its requests are fulfilled, with its instances",
        target_capacity: 1,
        instance_types: &["t3.micro"],
        fulfilled_first: false,
        terminate_instances: true,
        expected_previous: SPOT_FLEET_STATE_SUBMITTED,
        expected_current: SPOT_FLEET_STATE_CANCELLED_TERMINATING,
        // Nothing was launched, so nothing is terminating and the fleet is still below its
        // target capacity.
        expected_activity: SPOT_FLEET_ACTIVITY_PENDING_FULFILLMENT,
        expected_status_code: SPOT_STATUS_CANCELED_BEFORE_FULFILLMENT,
        expected_instance_state: None,
    },
    CancelCase {
        case: "cancelled before its requests are fulfilled, leaving instances to run",
        target_capacity: 2,
        instance_types: &["t3.small"],
        fulfilled_first: false,
        terminate_instances: false,
        expected_previous: SPOT_FLEET_STATE_SUBMITTED,
        expected_current: SPOT_FLEET_STATE_CANCELLED_RUNNING,
        expected_activity: SPOT_FLEET_ACTIVITY_PENDING_FULFILLMENT,
        expected_status_code: SPOT_STATUS_CANCELED_BEFORE_FULFILLMENT,
        expected_instance_state: None,
    },
    CancelCase {
        case: "a fulfilled fleet cancelled with its instances",
        target_capacity: 1,
        instance_types: &["m5.large"],
        fulfilled_first: true,
        terminate_instances: true,
        expected_previous: SPOT_FLEET_STATE_ACTIVE,
        expected_current: SPOT_FLEET_STATE_CANCELLED_TERMINATING,
        expected_activity: SPOT_FLEET_ACTIVITY_PENDING_TERMINATION,
        expected_status_code: SPOT_STATUS_INSTANCE_TERMINATED_BY_USER,
        expected_instance_state: Some("shutting-down"),
    },
    CancelCase {
        case: "a fulfilled fleet of two pools cancelled leaving its instances running",
        target_capacity: 3,
        instance_types: &["t3.medium", "c5.large"],
        fulfilled_first: true,
        terminate_instances: false,
        expected_previous: SPOT_FLEET_STATE_ACTIVE,
        expected_current: SPOT_FLEET_STATE_CANCELLED_RUNNING,
        expected_activity: SPOT_FLEET_ACTIVITY_FULFILLED,
        expected_status_code: SPOT_STATUS_REQUEST_CANCELED_AND_INSTANCE_RUNNING,
        expected_instance_state: Some("running"),
    },
];

#[tokio::test]
async fn a_cancellation_moves_the_fleet_to_the_state_terminate_instances_chooses() {
    for row in CANCEL_CASES {
        let case = row.case;
        let (router, _engine) = emulator();
        let fleet_id =
            requested_fleet(case, &router, row.target_capacity, row.instance_types).await;
        if row.fulfilled_first {
            // One span fulfils the fleet's Spot Instance requests; reading them back is what
            // materialises the launch the clock owed each of them, and the instance it mints
            // takes one more span to reach `running`.
            past_the_transition().await;
            let launched = spot_requests(case, &router).await;
            assert!(
                launched
                    .iter()
                    .all(|request| request.instance_id.is_some()),
                "case {case:?}: every request of a fulfilled fleet has launched an instance, \
                 got {launched:?}"
            );
            past_the_transition().await;
        }
        let (status, body) = cancel(
            &router,
            &cancel_params(&[fleet_id.as_str()], row.terminate_instances),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CancelSpotFleetRequests, got {status}: {body}"
        );
        let reported = cancelled(case, &body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly one cancelled fleet, got {reported:?}"
        );
        let reported = &reported[0];
        assert_eq!(
            reported.id, fleet_id,
            "case {case:?}: expected the cancelled fleet {fleet_id}, got {reported:?}"
        );
        assert_eq!(
            reported.previous, row.expected_previous,
            "case {case:?}: expected the previous state {:?}, got {reported:?}",
            row.expected_previous
        );
        assert_eq!(
            reported.current, row.expected_current,
            "case {case:?}: expected the current state {:?}, got {reported:?}",
            row.expected_current
        );
        assert!(
            not_cancelled(case, &body).is_empty(),
            "case {case:?}: a cancelled fleet is in no unsuccessful set, got {:?}",
            not_cancelled(case, &body)
        );

        // What the call said is what a describe says.
        let (state, activity) = described_fleet(case, &router, &fleet_id).await;
        assert_eq!(
            state, row.expected_current,
            "case {case:?}: expected the describe to report {:?}, got {state:?}",
            row.expected_current
        );
        assert_eq!(
            activity, row.expected_activity,
            "case {case:?}: expected the activity status {:?}, got {activity:?}",
            row.expected_activity
        );

        // Every Spot Instance request the fleet placed is cancelled, carrying the status code
        // for the way it ended.
        let requests = spot_requests(case, &router).await;
        assert!(
            !requests.is_empty(),
            "case {case:?}: the fleet must have placed at least one Spot Instance request"
        );
        for request in &requests {
            assert_eq!(
                request.state, SPOT_REQUEST_STATE_CANCELLED,
                "case {case:?}: expected a cancelled Spot Instance request, got {request:?}"
            );
            assert_eq!(
                request.status_code, row.expected_status_code,
                "case {case:?}: expected the status code {:?}, got {request:?}",
                row.expected_status_code
            );
        }

        // The instances the fleet launched, if any, are in the state the flag chose.
        for request in &requests {
            match (&request.instance_id, row.expected_instance_state) {
                (Some(instance_id), Some(expected)) => {
                    let state = described_instance_state(&router, REGION, instance_id).await;
                    assert_eq!(
                        state, expected,
                        "case {case:?}: expected the instance {instance_id} in {expected:?}, \
                         got {state:?}"
                    );
                }
                (Some(instance_id), None) => panic!(
                    "case {case:?}: no instance may have been launched, got {instance_id}"
                ),
                (None, Some(expected)) => panic!(
                    "case {case:?}: a fulfilled request must report an instance in {expected:?}, \
                     got {request:?}"
                ),
                (None, None) => {}
            }
        }
    }
}

#[tokio::test]
async fn a_cancelled_fleet_launches_no_new_instances() {
    let case = "cancelled while its requests were still open";
    let (router, engine) = emulator();
    let fleet_id = requested_fleet(case, &router, 3, &["t3.micro"]).await;
    let (status, body) = cancel(&router, &cancel_params(&[fleet_id.as_str()], false)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CancelSpotFleetRequests, got {status}: {body}"
    );
    // Well past the span in which the fleet's requests would have been fulfilled.
    past_the_transition().await;
    past_the_transition().await;
    let requests = spot_requests(case, &router).await;
    assert_eq!(
        requests.len(),
        3,
        "case {case:?}: the fleet placed one request per unit of capacity, got {requests:?}"
    );
    for request in &requests {
        assert_eq!(
            request.instance_id, None,
            "case {case:?}: a cancelled fleet launches no new instance, got {request:?}"
        );
    }
    assert_eq!(
        stored_instances(&engine),
        0,
        "case {case:?}: a cancelled fleet leaves no instance behind"
    );
    // No instance of the fleet is running, so the cancellation has completed.
    let (state, _activity) = described_fleet(case, &router, &fleet_id).await;
    assert_eq!(
        state, SPOT_FLEET_STATE_CANCELLED,
        "case {case:?}: expected a cancelled fleet with no running instance to reach \
         {SPOT_FLEET_STATE_CANCELLED:?}, got {state:?}"
    );
}

#[tokio::test]
async fn a_cancelled_fleet_reaches_cancelled_when_its_instances_are_gone() {
    // The fleet's own termination, and a caller's: either way the fleet is `cancelled` once no
    // Spot Instance it launched is still running, and not before.
    for terminate_with_the_fleet in [true, false] {
        let case = if terminate_with_the_fleet {
            "the fleet terminates its own instance"
        } else {
            "the instance is terminated manually afterwards"
        };
        let (router, _engine) = emulator();
        let fleet_id = requested_fleet(case, &router, 1, &["t3.micro"]).await;
        past_the_transition().await;
        let (status, body) = cancel(
            &router,
            &cancel_params(&[fleet_id.as_str()], terminate_with_the_fleet),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CancelSpotFleetRequests, got {status}: {body}"
        );
        let instance_id = spot_requests(case, &router)
            .await
            .into_iter()
            .find_map(|request| request.instance_id)
            .unwrap_or_else(|| panic!("case {case:?}: the fulfilled fleet must have an instance"));

        past_the_transition().await;
        let (state, _activity) = described_fleet(case, &router, &fleet_id).await;
        if terminate_with_the_fleet {
            assert_eq!(
                state, SPOT_FLEET_STATE_CANCELLED,
                "case {case:?}: the fleet whose instance has terminated is \
                 {SPOT_FLEET_STATE_CANCELLED:?}, got {state:?}"
            );
            continue;
        }
        assert_eq!(
            state, SPOT_FLEET_STATE_CANCELLED_RUNNING,
            "case {case:?}: a fleet whose instance still runs stays \
             {SPOT_FLEET_STATE_CANCELLED_RUNNING:?}, got {state:?}"
        );
        let (status, body) = call(
            &router,
            REGION,
            "TerminateInstances",
            &indexed("InstanceId", &[instance_id.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from TerminateInstances, got {status}: {body}"
        );
        past_the_transition().await;
        let (state, _activity) = described_fleet(case, &router, &fleet_id).await;
        assert_eq!(
            state, SPOT_FLEET_STATE_CANCELLED,
            "case {case:?}: once its instance is gone the fleet is \
             {SPOT_FLEET_STATE_CANCELLED:?}, got {state:?}"
        );
    }
}

/// One row of the per-fleet error table: an id that names no cancellable fleet, the
/// `CancelBatchErrorCode` it is reported with and the message that carries it. The fixed text
/// of each message is EC2's; the id and the state in it come from the request and the store.
struct UnservableCase {
    case: &'static str,
    /// Whether the fleet the row names has to be cancelled before the call under test, which
    /// is what leaves it in a state it cannot be cancelled from.
    cancel_first: bool,
    /// The id to name, or `None` to name the fleet this row cancelled first.
    id: Option<&'static str>,
    expected_code: &'static str,
    /// The message, given the id the call named and the state the fleet is in.
    expected_message: fn(&str) -> String,
}

fn unservable_cases() -> Vec<UnservableCase> {
    vec![
        UnservableCase {
            case: "something that is not a Spot Fleet request id at all",
            cancel_first: false,
            id: Some(MALFORMED_FLEET_ID),
            expected_code: CANCEL_BATCH_FLEET_REQUEST_ID_MALFORMED,
            expected_message: |id| format!("Invalid id: \"{id}\""),
        },
        UnservableCase {
            case: "a well-formed id the account holds no fleet for",
            cancel_first: false,
            id: Some(UNKNOWN_FLEET_ID),
            expected_code: CANCEL_BATCH_FLEET_REQUEST_ID_DOES_NOT_EXIST,
            expected_message: |id| format!("The spot fleet request ID '{id}' does not exist"),
        },
        UnservableCase {
            case: "a fleet that has already been cancelled",
            cancel_first: true,
            id: None,
            expected_code: CANCEL_BATCH_FLEET_REQUEST_NOT_IN_CANCELLABLE_STATE,
            expected_message: |id| {
                format!(
                    "The spot fleet request '{id}' is in the state \
                     '{SPOT_FLEET_STATE_CANCELLED_RUNNING}' and cannot be canceled"
                )
            },
        },
    ]
}

#[tokio::test]
async fn an_id_that_names_no_cancellable_fleet_is_reported_per_fleet() {
    for row in unservable_cases() {
        let case = row.case;
        let (router, _engine) = emulator();
        // One fleet that CAN be cancelled, named in the same batch: the per-fleet error must
        // not stop it being cancelled.
        let healthy = requested_fleet(case, &router, 1, &["t3.micro"]).await;
        let other = requested_fleet(case, &router, 1, &["t3.small"]).await;
        if row.cancel_first {
            let (status, body) = cancel(&router, &cancel_params(&[other.as_str()], false)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first cancellation must succeed, got {status}: {body}"
            );
        }
        let named = row.id.unwrap_or(other.as_str()).to_string();
        let (status, body) = cancel(
            &router,
            &cancel_params(&[named.as_str(), healthy.as_str()], false),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a per-fleet error does not refuse the call, got {status}: {body}"
        );
        let failed = not_cancelled(case, &body);
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: expected exactly one unsuccessful fleet, got {failed:?}"
        );
        let failed = &failed[0];
        assert_eq!(
            failed.id, named,
            "case {case:?}: expected the unsuccessful fleet {named}, got {failed:?}"
        );
        assert_eq!(
            failed.code, row.expected_code,
            "case {case:?}: expected the code {:?}, got {failed:?}",
            row.expected_code
        );
        assert_eq!(
            failed.message,
            (row.expected_message)(&named),
            "case {case:?}: expected the message {:?}, got {failed:?}",
            (row.expected_message)(&named)
        );
        let reported = cancelled(case, &body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: the cancellable fleet of the batch is still cancelled, \
             got {reported:?}"
        );
        assert_eq!(
            reported[0].id, healthy,
            "case {case:?}: expected {healthy} cancelled, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_request_that_cannot_be_served_cancels_nothing() {
    let case = "a dry run";
    let (router, _engine) = emulator();
    let fleet_id = requested_fleet(case, &router, 1, &["t3.micro"]).await;
    let mut params = cancel_params(&[fleet_id.as_str()], true);
    params.push(p("DryRun", "true"));
    let (status, body) = cancel(&router, &params).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412 for a dry run, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got the body {body}"
    );
    let (state, _activity) = described_fleet(case, &router, &fleet_id).await;
    assert_eq!(
        state, SPOT_FLEET_STATE_SUBMITTED,
        "case {case:?}: a dry run cancels nothing, got {state:?}"
    );

    // "You can delete up to 100 fleets in a single request. If you exceed the specified number,
    // no fleets are deleted."
    let case = "one id more than the documented bound";
    let mut ids: Vec<String> = vec![fleet_id.clone()];
    while ids.len() <= MAX_SPOT_FLEET_REQUESTS_PER_CANCEL {
        ids.push(UNKNOWN_FLEET_ID.to_string());
    }
    let named: Vec<&str> = ids.iter().map(String::as_str).collect();
    let (status, body) = cancel(&router, &cancel_params(&named, false)).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for {} ids, got {status}: {body}",
        named.len()
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got the body {body}"
    );
    assert!(
        error_message(&body).contains(&MAX_SPOT_FLEET_REQUESTS_PER_CANCEL.to_string()),
        "case {case:?}: the refusal names the bound, got {:?}",
        error_message(&body)
    );
    let (state, _activity) = described_fleet(case, &router, &fleet_id).await;
    assert_eq!(
        state, SPOT_FLEET_STATE_SUBMITTED,
        "case {case:?}: an oversized batch deletes no fleet, got {state:?}"
    );

    // One fleet named twice asks for one cancellation.
    let case = "one fleet named twice";
    let (status, body) = cancel(
        &router,
        &cancel_params(&[fleet_id.as_str(), fleet_id.as_str()], false),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CancelSpotFleetRequests, got {status}: {body}"
    );
    let reported = cancelled(case, &body);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: one fleet gets one entry however often it is named, got {reported:?}"
    );
}
