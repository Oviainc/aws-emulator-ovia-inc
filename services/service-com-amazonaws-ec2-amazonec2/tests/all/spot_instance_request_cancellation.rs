// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CancelSpotInstanceRequests`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. a cancellation MOVES EVERY REQUEST IT NAMES THAT IS STILL `open` OR `active` TO
//!    `cancelled`, carrying the Spot request status code for the way it ended —
//!    `canceled-before-fulfillment` when nothing had been launched for it,
//!    `request-canceled-and-instance-running` when it had launched an instance — and the call
//!    reports the state a later `DescribeSpotInstanceRequests` reports;
//! 2. "canceling a Spot Instance request does not terminate running Spot Instances associated
//!    with the request": the instance a cancelled request launched keeps running, and keeps
//!    running after the cancellation's own span has passed;
//! 3. a cancelled request owes NO FULFILLMENT: one cancelled before its fulfillment fell due
//!    launches nothing, however long the clock runs on;
//! 4. the reported state is READ OUT OF THE STORE, so a request that had already left `open`
//!    and `active` keeps the state it is in — a second cancellation of the same request
//!    reports `cancelled` and changes nothing, and a request whose validity window closed
//!    stays `closed` / `schedule-expired`;
//! 5. a call that cannot be served refuses the WHOLE call and cancels nothing: a well-formed
//!    id no request of the account has, something that is not a request id at all, more ids
//!    than one call may name, a call naming no id, and a dry run.
//!
//! Each table row is a distinct input along what the rule generalises over — how many
//! requests one call names, which `SpotInstanceType`, which instance type, whether the
//! requests had been fulfilled first, which unservable request — so no row is the
//! specification.

use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INVALID_SPOT_INSTANCE_REQUEST_ID_MALFORMED,
    INVALID_SPOT_INSTANCE_REQUEST_ID_NOT_FOUND, MAX_RESOURCE_IDS, SPOT_INSTANCE_TYPE_ONE_TIME,
    SPOT_INSTANCE_TYPE_PERSISTENT, SPOT_REQUEST_STATE_CANCELLED, SPOT_REQUEST_STATE_CLOSED,
    SPOT_REQUEST_STATE_OPEN, SPOT_STATUS_CANCELED_BEFORE_FULFILLMENT,
    SPOT_STATUS_REQUEST_CANCELED_AND_INSTANCE_RUNNING, SPOT_STATUS_SCHEDULE_EXPIRED,
    TRANSITION_DELAY, iso8601_utc,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, described_instance_state, error_code, error_message, indexed};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// A well-formed Spot Instance request id no request of the account is stored under: the
/// mint counts from one, so a high counter value names nothing.
const UNKNOWN_REQUEST_ID: &str = "sir-0000ffff";

/// Something that is not a Spot Instance request id at all.
const MALFORMED_REQUEST_ID: &str = "sir-not-an-id";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// An AMI of the published catalog a fresh account can already see, read out of the
/// service's OWN state: a launch specification must name an image the account can resolve,
/// so the test cannot invent an id.
fn image_id() -> String {
    Ec2State::default()
        .images
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must publish at least one image")
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// How many instances the store holds.
fn stored_instances(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().instances.len()
}

/// A timestamp `ago` before now, in the UTC form `ValidUntil` is documented in. Built
/// through the service's own formatter so the test cannot spell a timestamp the service
/// would not.
fn before_now(ago: Duration) -> String {
    iso8601_utc(std::time::SystemTime::now() - ago)
}

/// Ask for `count` Spot Instances with `extra` beside the launch specification, and answer
/// the request ids it minted, in the order the response reported them.
async fn requested(
    case: &str,
    router: &MultiServiceRouter,
    count: i32,
    extra: &[(String, String)],
) -> Vec<String> {
    let params: Params = [
        vec![
            p("LaunchSpecification.ImageId", image_id()),
            p("InstanceCount", count.to_string()),
        ],
        extra.to_vec(),
    ]
    .concat();
    let (status, body) = call(router, REGION, "RequestSpotInstances", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from RequestSpotInstances, got {status}: {body}"
    );
    let ids = spot_request_ids(case, &body);
    assert_eq!(
        ids.len(),
        usize::try_from(count).unwrap_or_default(),
        "case {case:?}: expected one request per unit of InstanceCount, got {ids:?}"
    );
    ids
}

/// Every `spotInstanceRequestId` a response carries, in document order. Only the ids of the
/// Spot requests themselves are read: those sit on an element that carries the request's own
/// `state`.
fn spot_request_ids(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "spotInstanceRequestId")
        .filter(|node| {
            node.parent()
                .is_some_and(|parent| child(parent, "state").is_some())
        })
        .map(|node| node.text().unwrap_or_default().to_string())
        .collect()
}

/// One `CancelSpotInstanceRequests` call over `ids`, with `extra` parameters beside them.
async fn cancel(
    router: &MultiServiceRouter,
    ids: &[&str],
    extra: &[(String, String)],
) -> (u16, String) {
    let params: Params = [indexed("SpotInstanceRequestId", ids), extra.to_vec()].concat();
    call(router, REGION, "CancelSpotInstanceRequests", &params).await
}

/// One entry of `CancelledSpotInstanceRequests`.
#[derive(Debug, PartialEq, Eq)]
struct Cancelled {
    id: String,
    state: String,
}

/// Every request a cancellation reports, in document order.
fn cancelled(case: &str, body: &str) -> Vec<Cancelled> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "spotInstanceRequestSet")
        .into_iter()
        .map(|item| Cancelled {
            id: child_text(item, "spotInstanceRequestId").unwrap_or_default(),
            state: child_text(item, "state").unwrap_or_default(),
        })
        .collect()
}

/// One Spot Instance request as `DescribeSpotInstanceRequests` reports it.
#[derive(Debug)]
struct Described {
    id: String,
    state: String,
    status_code: String,
    instance_id: Option<String>,
}

/// Every Spot Instance request the account holds, in the order the describe reports them.
async fn described(case: &str, router: &MultiServiceRouter) -> Vec<Described> {
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
        .map(|item| Described {
            id: child_text(item, "spotInstanceRequestId").unwrap_or_default(),
            state: child_text(item, "state").unwrap_or_default(),
            status_code: child(item, "status")
                .and_then(|status| child_text(status, "code"))
                .unwrap_or_default(),
            instance_id: child_text(item, "instanceId"),
        })
        .collect()
}

/// The one request `request_id` names, as the describe reports it.
async fn described_one(case: &str, router: &MultiServiceRouter, request_id: &str) -> Described {
    described(case, router)
        .await
        .into_iter()
        .find(|request| request.id == request_id)
        .unwrap_or_else(|| panic!("case {case:?}: the describe must report {request_id}"))
}

// ---------------------------------------------------------------------------
// RULES 1-3: what a cancellation writes, and what it leaves running
// ---------------------------------------------------------------------------

/// One row of the cancellation table: requests that differ from the others along how many a
/// call names, which Spot request type they are, which instance type they launch and whether
/// they had been fulfilled before the cancellation.
struct CancelCase {
    case: &'static str,
    /// How many requests one `RequestSpotInstances` call places, and so how many ids the
    /// cancellation names.
    count: i32,
    request_type: &'static str,
    instance_type: Option<&'static str>,
    /// Whether the requests are left to be fulfilled before the cancellation, which is what
    /// puts them in `active` with an instance to leave running.
    fulfilled_first: bool,
    expected_status_code: &'static str,
    /// Whether each cancelled request reports the instance it launched.
    expects_instance: bool,
}

/// The rows of the cancellation table. No row repeats another's combination of request
/// count, Spot request type, instance type and prior fulfillment.
const CANCEL_CASES: &[CancelCase] = &[
    CancelCase {
        case: "one open one-time request",
        count: 1,
        request_type: SPOT_INSTANCE_TYPE_ONE_TIME,
        instance_type: None,
        fulfilled_first: false,
        expected_status_code: SPOT_STATUS_CANCELED_BEFORE_FULFILLMENT,
        expects_instance: false,
    },
    CancelCase {
        case: "three open persistent requests cancelled in one call",
        count: 3,
        request_type: SPOT_INSTANCE_TYPE_PERSISTENT,
        instance_type: Some("t3.small"),
        fulfilled_first: false,
        expected_status_code: SPOT_STATUS_CANCELED_BEFORE_FULFILLMENT,
        expects_instance: false,
    },
    CancelCase {
        case: "a fulfilled one-time request, its instance left running",
        count: 1,
        request_type: SPOT_INSTANCE_TYPE_ONE_TIME,
        instance_type: Some("m5.large"),
        fulfilled_first: true,
        expected_status_code: SPOT_STATUS_REQUEST_CANCELED_AND_INSTANCE_RUNNING,
        expects_instance: true,
    },
    CancelCase {
        case: "two fulfilled persistent requests, their instances left running",
        count: 2,
        request_type: SPOT_INSTANCE_TYPE_PERSISTENT,
        instance_type: Some("c5.large"),
        fulfilled_first: true,
        expected_status_code: SPOT_STATUS_REQUEST_CANCELED_AND_INSTANCE_RUNNING,
        expects_instance: true,
    },
];

#[tokio::test]
async fn a_cancellation_moves_every_request_it_names_to_cancelled() {
    for row in CANCEL_CASES {
        let case = row.case;
        let (router, engine) = emulator();
        let mut extra: Params = vec![p("Type", row.request_type)];
        if let Some(instance_type) = row.instance_type {
            extra.push(p("LaunchSpecification.InstanceType", instance_type));
        }
        let request_ids = requested(case, &router, row.count, &extra).await;
        if row.fulfilled_first {
            // One span fulfils the requests; reading them back is what materialises the
            // launch the clock owed each of them, and the instance it mints takes one more
            // span to reach `running`.
            past_the_transition().await;
            let launched = described(case, &router).await;
            assert!(
                launched
                    .iter()
                    .all(|request| request.instance_id.is_some()),
                "case {case:?}: every fulfilled request must report its instance, got {launched:?}"
            );
            past_the_transition().await;
        }

        let named: Vec<&str> = request_ids.iter().map(String::as_str).collect();
        let (status, body) = cancel(&router, &named, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CancelSpotInstanceRequests, got {status}: \
             {body}"
        );
        let reported = cancelled(case, &body);
        let expected: Vec<Cancelled> = request_ids
            .iter()
            .map(|id| Cancelled {
                id: id.clone(),
                state: SPOT_REQUEST_STATE_CANCELLED.to_string(),
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected every named request reported cancelled as {expected:?}, \
             got {reported:?}"
        );

        // What the call said is what a describe says, with the status code for the way each
        // request ended.
        for request_id in &request_ids {
            let request = described_one(case, &router, request_id).await;
            assert_eq!(
                request.state, SPOT_REQUEST_STATE_CANCELLED,
                "case {case:?}: expected the describe to report a cancelled request, got \
                 {request:?}"
            );
            assert_eq!(
                request.status_code, row.expected_status_code,
                "case {case:?}: expected the status code {:?}, got {request:?}",
                row.expected_status_code
            );
            assert_eq!(
                request.instance_id.is_some(),
                row.expects_instance,
                "case {case:?}: expected an instance reported: {}, got {request:?}",
                row.expects_instance
            );
        }

        // RULE 2: the instances a cancelled request launched keep running — and still run
        // after the cancellation's own span has passed. RULE 3: a request cancelled before
        // its fulfillment fell due launches nothing, however long the clock runs on.
        past_the_transition().await;
        let after = described(case, &router).await;
        for request in &after {
            assert_eq!(
                request.state, SPOT_REQUEST_STATE_CANCELLED,
                "case {case:?}: a cancelled request stays cancelled, got {request:?}"
            );
            match &request.instance_id {
                Some(instance_id) => {
                    let state = described_instance_state(&router, REGION, instance_id).await;
                    assert_eq!(
                        state, "running",
                        "case {case:?}: cancelling a request must leave {instance_id} running, \
                         got {state:?}"
                    );
                }
                None => assert_eq!(
                    stored_instances(&engine),
                    0,
                    "case {case:?}: a request cancelled before fulfillment must launch nothing"
                ),
            }
        }
        if row.expects_instance {
            assert_eq!(
                stored_instances(&engine),
                usize::try_from(row.count).unwrap_or_default(),
                "case {case:?}: the instances the requests launched must still be held"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 4: the reported state is the store's
// ---------------------------------------------------------------------------

/// RULE 4: cancelling the same request again reports `cancelled` and changes nothing — the
/// status code the first cancellation wrote is still the one the describe reports.
#[tokio::test]
async fn cancelling_a_cancelled_request_again_changes_nothing() {
    for count in [1, 2] {
        let case = format!("{count} request(s) cancelled twice");
        let case = case.as_str();
        let (router, _engine) = emulator();
        let request_ids = requested(case, &router, count, &[]).await;
        let named: Vec<&str> = request_ids.iter().map(String::as_str).collect();
        for attempt in 1..=2 {
            let (status, body) = cancel(&router, &named, &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: attempt {attempt} expected HTTP 200, got {status}: {body}"
            );
            let reported = cancelled(case, &body);
            assert!(
                reported
                    .iter()
                    .all(|entry| entry.state == SPOT_REQUEST_STATE_CANCELLED),
                "case {case:?}: attempt {attempt} expected every request cancelled, got \
                 {reported:?}"
            );
        }
        for request_id in &request_ids {
            let request = described_one(case, &router, request_id).await;
            assert_eq!(
                request.status_code, SPOT_STATUS_CANCELED_BEFORE_FULFILLMENT,
                "case {case:?}: the second cancellation must not rewrite the status, got \
                 {request:?}"
            );
        }
    }
}

/// RULE 4: a request that had already left `open` and `active` keeps the state it is in — a
/// request whose validity window closed is reported `closed`, with the status code the
/// expiry wrote, over a table of windows that have closed.
#[tokio::test]
async fn cancelling_a_closed_request_reports_the_state_it_is_in() {
    let rows: [(&str, Duration); 3] = [
        ("a window that closed a second ago", Duration::from_secs(1)),
        ("a window that closed an hour ago", Duration::from_secs(3_600)),
        ("a window that closed a day ago", Duration::from_secs(86_400)),
    ];
    for (case, ago) in rows {
        let (router, engine) = emulator();
        let request_ids = requested(
            case,
            &router,
            1,
            &[p("ValidUntil", before_now(ago))],
        )
        .await;
        let request_id = request_ids[0].clone();
        // The expiry the clock owed the request is what moves it out of `open`; reading it
        // back is what materialises it.
        past_the_transition().await;
        let before = described_one(case, &router, &request_id).await;
        assert_eq!(
            before.state, SPOT_REQUEST_STATE_CLOSED,
            "case {case:?}: an expired request must be closed before the cancellation, got \
             {before:?}"
        );

        let (status, body) = cancel(&router, &[request_id.as_str()], &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CancelSpotInstanceRequests, got {status}: \
             {body}"
        );
        let reported = cancelled(case, &body);
        assert_eq!(
            reported,
            vec![Cancelled {
                id: request_id.clone(),
                state: SPOT_REQUEST_STATE_CLOSED.to_string(),
            }],
            "case {case:?}: expected the state the request is in, got {reported:?}"
        );
        let after = described_one(case, &router, &request_id).await;
        assert_eq!(
            (after.state.as_str(), after.status_code.as_str()),
            (SPOT_REQUEST_STATE_CLOSED, SPOT_STATUS_SCHEDULE_EXPIRED),
            "case {case:?}: cancelling a closed request must rewrite nothing, got {after:?}"
        );
        assert_eq!(
            stored_instances(&engine),
            0,
            "case {case:?}: an expired request must leave no instance behind"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 5: a call that cannot be served cancels nothing
// ---------------------------------------------------------------------------

/// One row of the refusal table: the ids and parameters of a call that cannot be served, and
/// the EC2 error code and message it is refused with.
struct RefusalCase {
    case: &'static str,
    /// The ids the call names beside the one open request the account holds, by position:
    /// `None` is that request's own id.
    ids: &'static [Option<&'static str>],
    extra: &'static [(&'static str, &'static str)],
    expected_code: &'static str,
    /// The message EC2 words the refusal with; `{id}` stands for the id the row's unservable
    /// entry named.
    expected_message: &'static str,
}

const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        case: "a well-formed id no request of the account has",
        ids: &[Some(UNKNOWN_REQUEST_ID)],
        extra: &[],
        expected_code: INVALID_SPOT_INSTANCE_REQUEST_ID_NOT_FOUND,
        expected_message: "The spot instance request ID 'sir-0000ffff' does not exist",
    },
    RefusalCase {
        case: "an unknown id beside the account's own open request",
        ids: &[None, Some(UNKNOWN_REQUEST_ID)],
        extra: &[],
        expected_code: INVALID_SPOT_INSTANCE_REQUEST_ID_NOT_FOUND,
        expected_message: "The spot instance request ID 'sir-0000ffff' does not exist",
    },
    RefusalCase {
        case: "something that is not a request id at all",
        ids: &[Some(MALFORMED_REQUEST_ID)],
        extra: &[],
        expected_code: INVALID_SPOT_INSTANCE_REQUEST_ID_MALFORMED,
        expected_message: "Invalid id: \"sir-not-an-id\"",
    },
    RefusalCase {
        case: "a malformed id beside the account's own open request",
        ids: &[None, Some("spot-request-7")],
        extra: &[],
        expected_code: INVALID_SPOT_INSTANCE_REQUEST_ID_MALFORMED,
        expected_message: "Invalid id: \"spot-request-7\"",
    },
    RefusalCase {
        case: "a dry run of a call that would have succeeded",
        ids: &[None],
        extra: &[("DryRun", "true")],
        expected_code: "DryRunOperation",
        expected_message: "Request would have succeeded, but DryRun flag is set.",
    },
];

#[tokio::test]
async fn a_call_that_cannot_be_served_cancels_nothing() {
    for row in REFUSAL_CASES {
        let case = row.case;
        let (router, _engine) = emulator();
        // One request of the account to cancel, so every row proves the refusal left a
        // cancellable request alone.
        let request_ids = requested(case, &router, 1, &[]).await;
        let own_id = request_ids[0].clone();
        let named: Vec<&str> = row
            .ids
            .iter()
            .map(|id| id.unwrap_or(own_id.as_str()))
            .collect();
        let extra: Params = row
            .extra
            .iter()
            .map(|(name, value)| p(name, *value))
            .collect();
        let (status, body) = cancel(&router, &named, &extra).await;
        assert_ne!(
            status, 200,
            "case {case:?}: the call must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            row.expected_code,
            "case {case:?}: expected the error code {:?}, got {body}",
            row.expected_code
        );
        assert_eq!(
            error_message(&body),
            row.expected_message,
            "case {case:?}: expected the message {:?}, got {body}",
            row.expected_message
        );
        // Nothing was cancelled: the account's own request is still open and waiting.
        let request = described_one(case, &router, &own_id).await;
        assert_eq!(
            request.state, SPOT_REQUEST_STATE_OPEN,
            "case {case:?}: a refused call must cancel nothing, got {request:?}"
        );
    }
}

/// RULE 5: a call naming more ids than one may carry is refused for the whole call, before
/// any of them is looked up — the bound is the service's own, not a literal.
#[tokio::test]
async fn a_batch_above_the_id_bound_cancels_nothing() {
    let case = "one id past the maximum";
    let (router, _engine) = emulator();
    let request_ids = requested(case, &router, 1, &[]).await;
    let own_id = request_ids[0].clone();
    // The account's own request first, so a bound checked after the lookups would have
    // cancelled it.
    let mut ids: Vec<String> = vec![own_id.clone()];
    while ids.len() <= MAX_RESOURCE_IDS {
        ids.push(format!("sir-{:08x}", ids.len()));
    }
    let named: Vec<&str> = ids.iter().map(String::as_str).collect();
    let (status, body) = cancel(&router, &named, &[]).await;
    assert_ne!(
        status, 200,
        "case {case:?}: the call must be refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got {body}"
    );
    assert!(
        error_message(&body).contains(&MAX_RESOURCE_IDS.to_string()),
        "case {case:?}: the refusal must name the bound {MAX_RESOURCE_IDS}, got {body}"
    );
    let request = described_one(case, &router, &own_id).await;
    assert_eq!(
        request.state, SPOT_REQUEST_STATE_OPEN,
        "case {case:?}: a refused call must cancel nothing, got {request:?}"
    );
}

/// RULE 5: a call naming no Spot Instance request id at all has nothing to cancel — the
/// member is modelled `@required`, so the refusal is the boundary's own and nothing is
/// cancelled behind it.
#[tokio::test]
async fn a_call_naming_no_request_is_refused() {
    let case = "no SpotInstanceRequestId";
    let (router, _engine) = emulator();
    let request_ids = requested(case, &router, 1, &[]).await;
    let own_id = request_ids[0].clone();
    let (status, body) = cancel(&router, &[], &[]).await;
    assert_ne!(
        status, 200,
        "case {case:?}: the call must be refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "MalformedInput",
        "case {case:?}: expected the refusal of a request missing a required member, got {body}"
    );
    assert!(
        error_message(&body).contains("spotInstanceRequestIds"),
        "case {case:?}: the refusal must name the member it is about, got {body}"
    );
    let request = described_one(case, &router, &own_id).await;
    assert_eq!(
        request.state, SPOT_REQUEST_STATE_OPEN,
        "case {case:?}: a refused call must cancel nothing, got {request:?}"
    );
}
