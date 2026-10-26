// SPDX-License-Identifier: CC0-1.0

//! UpdateGatewayInstance, end to end over HTTP: "updates an existing gateway instance".
//!
//! The rules asserted here are:
//!
//! * the update writes the request's `BridgePlacement` onto the stored instance its ARN
//!   addresses and answers that stored record — the ARN and the placement now in force —
//!   whatever placement the record carried before and whatever value the request names
//!   (`smithy_query(query="shape",shape="BridgePlacement")`: AVAILABLE | LOCKED);
//! * `BridgePlacement` is optional (`smithy_query(query="operation",
//!   operation="UpdateGatewayInstance")`), so a request that omits it changes nothing and
//!   reports the stored placement, and a repeat of this `@idempotent` call answers what the
//!   first one did;
//! * the change is in force as soon as the call returns: the operation answers HTTP 200 and
//!   the modelled `InstanceState` enum carries no updating value, so the update moves no
//!   state and the clock is owed nothing afterwards;
//! * an instance between two states (`REGISTERING`, `DEREGISTERING`) is refused with the
//!   modelled `ConflictException` (409) and an instance that reached the terminal
//!   `DEREGISTERED` — no longer registered with its gateway, so hosting no bridges at all —
//!   with the modelled `BadRequestException` (400) in this service's state-refusal envelope;
//!   either way the stored record is left exactly as it was;
//! * an ARN no instance has is the modelled `NotFoundException` (404), in this service's
//!   not-found envelope.
//!
//! A `BridgePlacement` outside the two values the enum names is refused with HTTP 400 and
//! nothing is stored, so only a modelled placement ever reaches the store.
//!
//! Each table's rows differ along what the rule generalises over: the `BridgePlacement` the
//! record starts in and the one the request names (both values of the enum, in both
//! directions and in both no-op pairings), a request that names no placement at all, the
//! `InstanceState` the record is in (every value of the modelled enum) and how many
//! instances the store holds at once.
//!
//! Every update is proved by reading the instance back through DescribeGatewayInstance, the
//! reader `tests/all/describe_gateway_instance.rs` owns, so the assertion is about what the
//! service PERSISTED and not only about what one response said.
//!
//! An instance is never created by a client: the model declares no operation that registers
//! one, so the fixtures register through the store's own out-of-band channel, exactly as
//! `tests/all/gateway_instances.rs` does.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::gateway_instances::{
    describe_gateway_instance, gateway_with, instance_in_state, PAST_THE_TRANSITION,
};

/// `PUT /v1/gateway-instances/{GatewayInstanceArn}`, with `BridgePlacement` in the payload
/// under the `bridgePlacement` key the model binds it to, or an empty payload when the row
/// names no placement at all.
async fn update_gateway_instance(
    emulator: &Emulator,
    arn: &str,
    bridge_placement: Option<&str>,
) -> (u16, Option<String>, Value) {
    let body = match bridge_placement {
        Some(placement) => json!({ "bridgePlacement": placement }),
        None => json!({}),
    };
    emulator
        .send("PUT", &format!("/v1/gateway-instances/{arn}"), Some(body))
        .await
}

/// Register one instance with `gateway_arn`, put its record in `instance_state` and give it
/// `bridge_placement`.
///
/// The state goes through the seam `gateway_instances` owns ([`instance_in_state`]); the
/// placement a registration starts with is reported out of band too, so a row that needs
/// another starting value forces it the same way.
fn instance_with(
    emulator: &Emulator,
    gateway_arn: &str,
    instance_state: &str,
    bridge_placement: &str,
) -> String {
    let instance_arn = instance_in_state(emulator, gateway_arn, instance_state);
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    let instance = store
        .gateway_instances
        .get_mut(&instance_arn)
        .unwrap_or_else(|| panic!("the fixture instance {instance_arn} is stored"));
    instance.bridge_placement = bridge_placement.to_string();
    instance_arn
}

/// The whole gateway-instance map as the store holds it, for comparing a before and an
/// after: a refused update must leave every record exactly as it was.
fn stored_instances(emulator: &Emulator) -> String {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    format!("{:?}", store.gateway_instances)
}

/// Whether the store owes `arn` a state change — nothing an update is allowed to start.
fn transition_is_pending(emulator: &Emulator, arn: &str) -> bool {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    store
        .gateway_instances
        .get(arn)
        .unwrap_or_else(|| panic!("the fixture instance {arn} is stored"))
        .pending
        .is_some()
}

/// The `GatewayInstance` DescribeGatewayInstance reports for `arn`, asserted to be reported
/// at all.
async fn instance_of(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let (status, code, described) = describe_gateway_instance(emulator, arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: describe expected 200, got {status} {code:?}: {described}"
    );
    described["gatewayInstance"].clone()
}

/// One accepted update.
#[derive(Debug)]
struct Accepted {
    case: &'static str,
    /// The `InstanceState` the record is in when the update arrives.
    instance_state: &'static str,
    /// The `BridgePlacement` the record carries beforehand.
    stored: &'static str,
    /// `BridgePlacement` as the request carries it; `None` leaves it out of the payload.
    requested: Option<&'static str>,
    /// The `BridgePlacement` the record must carry afterwards.
    expected: &'static str,
}

/// The accepted-update table. The rows vary both values of the modelled `BridgePlacement`
/// enum as the stored value and as the requested one — so both directions of change and both
/// no-op pairings are covered — a request that names no placement at all, and the
/// `InstanceState` values an instance can be updated in.
const ACCEPTED: &[Accepted] = &[
    Accepted {
        case: "an active instance locked against new bridges",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        stored: mc::BRIDGE_PLACEMENT_AVAILABLE,
        requested: Some(mc::BRIDGE_PLACEMENT_LOCKED),
        expected: mc::BRIDGE_PLACEMENT_LOCKED,
    },
    Accepted {
        case: "a locked instance opened up for new bridges again",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        stored: mc::BRIDGE_PLACEMENT_LOCKED,
        requested: Some(mc::BRIDGE_PLACEMENT_AVAILABLE),
        expected: mc::BRIDGE_PLACEMENT_AVAILABLE,
    },
    Accepted {
        case: "an available instance asked for the placement it already has",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        stored: mc::BRIDGE_PLACEMENT_AVAILABLE,
        requested: Some(mc::BRIDGE_PLACEMENT_AVAILABLE),
        expected: mc::BRIDGE_PLACEMENT_AVAILABLE,
    },
    Accepted {
        case: "a locked instance asked for the placement it already has",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        stored: mc::BRIDGE_PLACEMENT_LOCKED,
        requested: Some(mc::BRIDGE_PLACEMENT_LOCKED),
        expected: mc::BRIDGE_PLACEMENT_LOCKED,
    },
    Accepted {
        case: "a request that names no placement leaves the available one alone",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        stored: mc::BRIDGE_PLACEMENT_AVAILABLE,
        requested: None,
        expected: mc::BRIDGE_PLACEMENT_AVAILABLE,
    },
    Accepted {
        case: "a request that names no placement leaves the locked one alone",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        stored: mc::BRIDGE_PLACEMENT_LOCKED,
        requested: None,
        expected: mc::BRIDGE_PLACEMENT_LOCKED,
    },
    Accepted {
        case: "an instance whose registration failed is still registered with its gateway",
        instance_state: mc::INSTANCE_STATE_REGISTRATION_ERROR,
        stored: mc::BRIDGE_PLACEMENT_AVAILABLE,
        requested: Some(mc::BRIDGE_PLACEMENT_LOCKED),
        expected: mc::BRIDGE_PLACEMENT_LOCKED,
    },
    Accepted {
        case: "an instance whose deregistration failed is still registered with its gateway",
        instance_state: mc::INSTANCE_STATE_DEREGISTRATION_ERROR,
        stored: mc::BRIDGE_PLACEMENT_LOCKED,
        requested: Some(mc::BRIDGE_PLACEMENT_AVAILABLE),
        expected: mc::BRIDGE_PLACEMENT_AVAILABLE,
    },
];

/// The rule over the whole accepted table: the placement the request names becomes the
/// stored one, the response is that stored record, and nothing else about the instance moves.
#[tokio::test]
async fn an_update_stores_the_requested_bridge_placement_and_reports_the_stored_record() {
    let emulator = Emulator::new();
    let gateway_arn = gateway_with(&emulator, "gw-update-instance").await;

    // One store holding every row at once, so a row that wrote another row's record would
    // fail rather than pass unnoticed.
    let arns: Vec<String> = ACCEPTED
        .iter()
        .map(|row| instance_with(&emulator, &gateway_arn, row.instance_state, row.stored))
        .collect();

    for (row, arn) in ACCEPTED.iter().zip(&arns) {
        let case = row.case;
        let (status, code, answered) = update_gateway_instance(&emulator, arn, row.requested).await;
        assert_eq!(
            (
                status,
                code.as_deref(),
                answered["gatewayInstanceArn"].as_str(),
                answered["bridgePlacement"].as_str(),
            ),
            (200, None, Some(arn.as_str()), Some(row.expected)),
            "case {case:?}: the update answers the stored record, got {status} {code:?}: \
             {answered}"
        );

        // The write is proved through the reader, not through the response that made it.
        let described = instance_of(&emulator, case, arn).await;
        assert_eq!(
            (
                described["bridgePlacement"].as_str(),
                described["instanceState"].as_str(),
                described["gatewayArn"].as_str(),
            ),
            (
                Some(row.expected),
                Some(row.instance_state),
                Some(gateway_arn.as_str()),
            ),
            "case {case:?}: the placement is persisted and no other member moved, got \
             {described}"
        );

        // The operation answers 200 and `InstanceState` has no updating value, so the
        // update owes the instance no later state change.
        assert!(
            !transition_is_pending(&emulator, arn),
            "case {case:?}: an update starts no transition, but one is pending on {arn}"
        );
    }

    // The placements the rows left behind are still there once any span a transition could
    // have used has passed: the update settled immediately rather than in flight.
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    for (row, arn) in ACCEPTED.iter().zip(&arns) {
        let case = row.case;
        let described = instance_of(&emulator, case, arn).await;
        assert_eq!(
            (
                described["bridgePlacement"].as_str(),
                described["instanceState"].as_str(),
            ),
            (Some(row.expected), Some(row.instance_state)),
            "case {case:?}: the stored record is unchanged by the passage of time, got \
             {described}"
        );
    }
}

/// The `@idempotent` call repeated: the same request answers the same document and leaves
/// the same record, however many times it is sent. The rows vary the placement asked for,
/// including the one the record already has and the request that names none.
#[tokio::test]
async fn repeating_an_update_answers_what_the_first_one_did() {
    let rows = [
        (
            "repeating a change to LOCKED",
            mc::BRIDGE_PLACEMENT_AVAILABLE,
            Some(mc::BRIDGE_PLACEMENT_LOCKED),
        ),
        (
            "repeating a change to AVAILABLE",
            mc::BRIDGE_PLACEMENT_LOCKED,
            Some(mc::BRIDGE_PLACEMENT_AVAILABLE),
        ),
        (
            "repeating a request that names no placement",
            mc::BRIDGE_PLACEMENT_LOCKED,
            None,
        ),
    ];

    for (case, stored, requested) in rows {
        let emulator = Emulator::new();
        let gateway_arn = gateway_with(&emulator, "gw-idempotent-update").await;
        let arn = instance_with(
            &emulator,
            &gateway_arn,
            mc::INSTANCE_STATE_ACTIVE,
            stored,
        );

        let (status, code, first) = update_gateway_instance(&emulator, &arn, requested).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the first update expected 200, got {status} {code:?}: {first}"
        );
        let after_first = stored_instances(&emulator);

        for repeat in 2..=3 {
            let (status, code, again) = update_gateway_instance(&emulator, &arn, requested).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?}: update {repeat} expected 200, got {status} {code:?}: {again}"
            );
            assert_eq!(
                again, first,
                "case {case:?}: update {repeat} must answer what update 1 did; expected \
                 {first}, got {again}"
            );
            let after_repeat = stored_instances(&emulator);
            assert_eq!(
                after_repeat, after_first,
                "case {case:?}: update {repeat} must store what update 1 stored; expected \
                 {after_first}, got {after_repeat}"
            );
        }
    }
}

/// What a refused update answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// 409 `ConflictException`: the instance is between two states.
    Conflict,
    /// 400 `BadRequestException` in the state-refusal envelope: the instance is no longer
    /// registered with its gateway.
    NotRegistered,
}

/// The refusal table: every `InstanceState` an update is not accepted in, in both
/// placements, asked for both values of the enum and for no value at all.
const REFUSED: &[(&str, &str, &str, Option<&str>, Refusal)] = &[
    (
        "a registration still in flight",
        mc::INSTANCE_STATE_REGISTERING,
        mc::BRIDGE_PLACEMENT_AVAILABLE,
        Some(mc::BRIDGE_PLACEMENT_LOCKED),
        Refusal::Conflict,
    ),
    (
        "a registration still in flight, asked for no placement at all",
        mc::INSTANCE_STATE_REGISTERING,
        mc::BRIDGE_PLACEMENT_LOCKED,
        None,
        Refusal::Conflict,
    ),
    (
        "a deregistration still in flight",
        mc::INSTANCE_STATE_DEREGISTERING,
        mc::BRIDGE_PLACEMENT_LOCKED,
        Some(mc::BRIDGE_PLACEMENT_AVAILABLE),
        Refusal::Conflict,
    ),
    (
        "a deregistration still in flight, asked for the placement it has",
        mc::INSTANCE_STATE_DEREGISTERING,
        mc::BRIDGE_PLACEMENT_AVAILABLE,
        Some(mc::BRIDGE_PLACEMENT_AVAILABLE),
        Refusal::Conflict,
    ),
    (
        "an instance in the terminal DEREGISTERED state",
        mc::INSTANCE_STATE_DEREGISTERED,
        mc::BRIDGE_PLACEMENT_AVAILABLE,
        Some(mc::BRIDGE_PLACEMENT_LOCKED),
        Refusal::NotRegistered,
    ),
    (
        "a deregistered instance asked for no placement at all",
        mc::INSTANCE_STATE_DEREGISTERED,
        mc::BRIDGE_PLACEMENT_LOCKED,
        None,
        Refusal::NotRegistered,
    ),
];

/// The rule over the refusal table: an instance whose availability to host bridges is not
/// settled takes no new placement, and the refusal writes nothing.
#[tokio::test]
async fn an_instance_that_is_not_settled_and_registered_takes_no_new_placement() {
    let emulator = Emulator::new();
    let gateway_arn = gateway_with(&emulator, "gw-refused-update").await;

    let arns: Vec<String> = REFUSED
        .iter()
        .map(|(_, instance_state, stored, _, _)| {
            instance_with(&emulator, &gateway_arn, instance_state, stored)
        })
        .collect();
    let before = stored_instances(&emulator);

    for ((case, instance_state, stored, requested, refusal), arn) in REFUSED.iter().zip(&arns) {
        let (status, code, answered) = update_gateway_instance(&emulator, arn, *requested).await;
        let expected = match refusal {
            Refusal::Conflict => (409, Some("ConflictException")),
            Refusal::NotRegistered => (400, Some("BadRequestException")),
        };
        assert_eq!(
            (status, code.as_deref()),
            expected,
            "case {case:?}: the update is refused with {expected:?}, got {status} {code:?}: \
             {answered}"
        );
        let message = answered["message"].as_str().unwrap_or_default();
        match refusal {
            Refusal::Conflict => assert!(
                message.contains(arn.as_str()) && message.contains(instance_state),
                "case {case:?}: the conflict names the instance and the state it is in, got \
                 {message:?}"
            ),
            Refusal::NotRegistered => assert_eq!(
                message,
                mc::state_precondition_message("Instance", mc::INSTANCE_STATE_ACTIVE, "PUT"),
                "case {case:?}: the refusal is this service's state-refusal envelope, got \
                 {message:?}"
            ),
        }

        let described = instance_of(&emulator, case, arn).await;
        assert_eq!(
            (
                described["bridgePlacement"].as_str(),
                described["instanceState"].as_str(),
            ),
            (Some(*stored), Some(*instance_state)),
            "case {case:?}: a refused update leaves the record as it was, got {described}"
        );
    }

    let after = stored_instances(&emulator);
    assert_eq!(
        after, before,
        "a refused update must store nothing; expected {before}, got {after}"
    );
}

/// A registration that settles while nobody is looking: the update refused inside the
/// transition span is accepted once the clock has made the instance active, which is the
/// retry the retryable `ConflictException` invites.
#[tokio::test]
async fn an_update_refused_during_a_registration_succeeds_once_it_has_settled() {
    let emulator = Emulator::new();
    let gateway_arn = gateway_with(&emulator, "gw-settling-update").await;
    // The registration itself, not a forced state: it leaves the instance `REGISTERING`
    // with the clock owing it `ACTIVE`.
    let arn = crate::gateway_instances::register_instance(&emulator, &gateway_arn);

    let (status, code, answered) =
        update_gateway_instance(&emulator, &arn, Some(mc::BRIDGE_PLACEMENT_LOCKED)).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "an update inside the registration span is refused, got {status} {code:?}: {answered}"
    );

    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (status, code, answered) =
        update_gateway_instance(&emulator, &arn, Some(mc::BRIDGE_PLACEMENT_LOCKED)).await;
    assert_eq!(
        (
            status,
            code.as_deref(),
            answered["bridgePlacement"].as_str(),
        ),
        (200, None, Some(mc::BRIDGE_PLACEMENT_LOCKED)),
        "once the registration has settled the update is accepted, got {status} {code:?}: \
         {answered}"
    );

    let described = instance_of(&emulator, "after the registration settled", &arn).await;
    assert_eq!(
        (
            described["bridgePlacement"].as_str(),
            described["instanceState"].as_str(),
        ),
        (
            Some(mc::BRIDGE_PLACEMENT_LOCKED),
            Some(mc::INSTANCE_STATE_ACTIVE),
        ),
        "the settled instance carries the placement the update stored, got {described}"
    );
}

/// A `BridgePlacement` outside the enum is refused and stores nothing: only a modelled
/// placement ever reaches the store. The rows are the ways a value can miss the enum.
#[tokio::test]
async fn a_placement_outside_the_enum_is_refused() {
    let emulator = Emulator::new();
    let gateway_arn = gateway_with(&emulator, "gw-update-bad-placement").await;
    let arn = instance_with(
        &emulator,
        &gateway_arn,
        mc::INSTANCE_STATE_ACTIVE,
        mc::BRIDGE_PLACEMENT_AVAILABLE,
    );
    let before = stored_instances(&emulator);

    // The values the enum names, and nothing else: AVAILABLE | LOCKED
    // (`smithy_query(query="shape",shape="BridgePlacement")`).
    let rows = [
        ("a value no enum member has", "UNAVAILABLE"),
        ("the enum member in the wrong case", "locked"),
        ("an empty placement", ""),
        ("a member name with surrounding space", " AVAILABLE "),
    ];

    for (case, placement) in rows {
        let (status, code, answered) = update_gateway_instance(&emulator, &arn, Some(placement)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {placement:?} is outside the enum, so the update is refused with \
             400, got {status} {code:?}: {answered}"
        );

        let after = stored_instances(&emulator);
        assert_eq!(
            after, before,
            "case {case:?}: a refused update stores nothing; expected {before}, got {after}"
        );
    }
}

/// An ARN no instance has is the modelled `NotFoundException`, in this service's not-found
/// envelope — never a 501, never a success, and never storing anything. The rows are the
/// ways an instance ARN can miss.
#[tokio::test]
async fn an_arn_no_instance_has_is_not_found() {
    let emulator = Emulator::new();
    let gateway_arn = gateway_with(&emulator, "gw-update-not-found").await;
    let registered = instance_with(
        &emulator,
        &gateway_arn,
        mc::INSTANCE_STATE_ACTIVE,
        mc::BRIDGE_PLACEMENT_AVAILABLE,
    );

    let unknown_gateway = format!(
        "{}:instance:mi-0000000000000beef",
        engine::emulator_arn(
            mc::SERVICE_NAMESPACE,
            "us-east-1",
            &mc::gateway_resource_fragment(4_242, "never-created"),
        )
    );
    let rows = [
        (
            "an instance id no instance has, on a gateway that exists",
            mc::gateway_instance_arn(&gateway_arn, "mi-0000000000000dead"),
        ),
        (
            "an instance of a gateway that was never created",
            unknown_gateway,
        ),
        (
            "an instance id of a form this service never mints",
            format!("{gateway_arn}:instance:-"),
        ),
    ];

    for (case, arn) in rows {
        let (status, code, answered) =
            update_gateway_instance(&emulator, &arn, Some(mc::BRIDGE_PLACEMENT_LOCKED)).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: the update expected a 404 NotFoundException, got {status} \
             {code:?}: {answered}"
        );
        assert_eq!(
            answered["message"].as_str(),
            Some(mc::not_found_message(&arn).as_str()),
            "case {case:?}: the message is this service's not-found envelope around the \
             request's own label, got {answered}"
        );
    }

    // The instance that does exist is untouched by every miss above.
    let described = instance_of(&emulator, "the registered instance", &registered).await;
    assert_eq!(
        (
            described["bridgePlacement"].as_str(),
            described["instanceState"].as_str(),
        ),
        (
            Some(mc::BRIDGE_PLACEMENT_AVAILABLE),
            Some(mc::INSTANCE_STATE_ACTIVE),
        ),
        "a not-found on another ARN leaves the registered instance alone, got {described}"
    );
}
