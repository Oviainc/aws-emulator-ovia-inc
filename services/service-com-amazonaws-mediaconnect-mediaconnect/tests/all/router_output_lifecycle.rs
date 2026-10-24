// SPDX-License-Identifier: CC0-1.0

//! GetRouterOutput and DeleteRouterOutput, end to end over HTTP.
//!
//! Two rules are pinned here, each over a table rather than the one request that exposed
//! them. CreateRouterOutput is the write side both read (its own rules are pinned in
//! `tests/all/router_outputs.rs`), so every case below creates what it then reads or
//! deletes.
//!
//! 1. GetRouterOutput REPORTS the router output the store holds for the ARN it is given,
//!    computed from the stored record — including the state the clock has moved it to.
//! 2. DeleteRouterOutput answers the ARN, the name and the deletion state of the router
//!    output the store holds for that ARN, and the clock then completes the deletion:
//!    `RouterOutputState` carries no terminal deleted value, so the record stays readable
//!    in `DELETING` for one transition span — a repeated delete answering the same thing,
//!    as `@idempotent` requires — and then LEAVES the store, after which both operations
//!    answer `NotFoundException`.
//!
//! The cases vary what those rules generalise over: which `RouterOutputConfiguration`
//! variant (and so which output type and stream details) the output carries, its tier,
//! tags, maintenance configuration and Region, how many router outputs the store holds,
//! how many times the same output is deleted, and whether the clock has completed the
//! deletion yet.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization. The fixtures and the
//! create helper are the ones `router_outputs` owns, so the two suites' tables cannot drift
//! apart — including its spelling of a union on the wire, where a member is keyed by its
//! MODELLED member name (`Standard`, `Rtp`, …) because the framework's union schemas carry
//! no `@jsonName`.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
// The fixtures and the create helper are the ONES `router_outputs` owns — the suite that
// pins CreateRouterOutput's own rules — so the two tables cannot drift apart.
use crate::router_outputs::{
    assert_minted_router_output_arn, create_router_output, standard_with_protocol, FLOW_ARN,
    MEDIALIVE_INPUT_ARN, ROLE_ARN, SECRET_ARN,
};

/// Longer than [`mc::TRANSITION_SPAN`], so a transition the clock owes — including the
/// completion of a delete — is certainly due.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

async fn get_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/routerOutput/{arn}"), None)
        .await
}

async fn delete_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerOutput/{arn}"), None)
        .await
}

fn string_at<'a>(value: &'a Value, member: &str) -> &'a str {
    value.get(member).and_then(Value::as_str).unwrap_or_default()
}

/// The `routerOutput` of a response body, or `Value::Null` when there is none.
fn reported(body: &Value) -> Value {
    body.get("routerOutput").cloned().unwrap_or(Value::Null)
}

/// One create request plus what the service DERIVES for the router output it mints, as
/// distinct from what it echoes: the echoed members are asserted against `body` itself.
struct Case {
    case: &'static str,
    body: Value,
    /// `RouterOutput$OutputType`, derived from which `RouterOutputConfiguration` member the
    /// request carries.
    output_type: &'static str,
    /// The whole expected `RouterOutput$StreamDetails`, derived from the stored
    /// configuration.
    stream_details: Value,
    /// `RouterOutput$MaintenanceType`, derived from the maintenance configuration.
    maintenance_type: &'static str,
    /// The Region the ARN is minted in and `RegionName` reports.
    region: &'static str,
    /// The Availability Zone reported for the router output.
    availability_zone: &'static str,
}

/// One row per `RouterOutputConfiguration` kind the model declares — all three union
/// members, and for the standard kind both a protocol that names a destination and one that
/// does not (`smithy_query(query="closure",operation="CreateRouterOutput")`) — differing
/// also in tier, maximum bitrate, routing scope, tags, maintenance configuration and
/// Region / Availability Zone.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "standard rtp, default maintenance",
            body: json!({
                "name": "rtp-output",
                "configuration": standard_with_protocol(json!({
                    "Rtp": {
                        "destinationAddress": "203.0.113.10",
                        "destinationPort": 5004,
                        "forwardErrorCorrection": "ENABLED",
                    }
                }), "RTP"),
                "maximumBitrate": 20_000_000_i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_20",
            }),
            output_type: "STANDARD",
            stream_details: json!({ "Standard": { "destinationIpAddress": "203.0.113.10" } }),
            maintenance_type: "DEFAULT",
            region: REGION,
            availability_zone: "us-east-1a",
        },
        Case {
            case: "standard srt listener: no destination address at all, tagged",
            body: json!({
                "name": "srt-listener-output",
                "configuration": standard_with_protocol(json!({
                    "SrtListener": {
                        "port": 3100,
                        "minimumLatencyMilliseconds": 120,
                    }
                }), "SRT_LISTENER"),
                "maximumBitrate": 100_000_000_i64,
                "routingScope": "GLOBAL",
                "tier": "OUTPUT_100",
                "availabilityZone": "us-east-1c",
                "tags": { "team": "broadcast", "tier": "primary" },
            }),
            output_type: "STANDARD",
            // An SRT listener waits for the receiver to call it, so the configuration names
            // a port and no destination: the optional member is absent, not empty.
            stream_details: json!({ "Standard": {} }),
            maintenance_type: "DEFAULT",
            region: REGION,
            availability_zone: "us-east-1c",
        },
        Case {
            case: "mediaconnect flow destination in another Region, preferred window",
            body: json!({
                "name": "flow-output",
                "configuration": json!({
                    "MediaConnectFlow": {
                        "flowArn": FLOW_ARN,
                        "destinationTransitEncryption": {
                            "encryptionKeyType": "SECRETS_MANAGER",
                            "encryptionKeyConfiguration": {
                                "SecretsManager": {
                                    "secretArn": SECRET_ARN,
                                    "roleArn": ROLE_ARN,
                                }
                            },
                        },
                    }
                }),
                "maximumBitrate": 500_000_000_i64,
                "routingScope": "GLOBAL",
                "tier": "OUTPUT_500",
                "regionName": "eu-west-1",
                "availabilityZone": "eu-west-1b",
                "maintenanceConfiguration": {
                    "PreferredDayTime": { "day": "TUESDAY", "time": "03:00" }
                },
            }),
            output_type: "MEDIACONNECT_FLOW",
            // `MediaConnectFlowRouterOutputStreamDetails` models no members.
            stream_details: json!({ "MediaConnectFlow": {} }),
            maintenance_type: "PREFERRED_DAY_TIME",
            region: "eu-west-1",
            availability_zone: "eu-west-1b",
        },
        Case {
            case: "medialive input destination, automatic transit key",
            body: json!({
                "name": "medialive-output",
                "configuration": json!({
                    "MediaLiveInput": {
                        "mediaLiveInputArn": MEDIALIVE_INPUT_ARN,
                        "mediaLivePipelineId": "PIPELINE_1",
                        "destinationTransitEncryption": {
                            "encryptionKeyType": "AUTOMATIC",
                            "encryptionKeyConfiguration": { "Automatic": {} },
                        },
                    }
                }),
                "maximumBitrate": 250_i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_250",
                "maintenanceConfiguration": { "Default": {} },
            }),
            output_type: "MEDIALIVE_INPUT",
            // `MediaLiveInputRouterOutputStreamDetails` models no members.
            stream_details: json!({ "MediaLiveInput": {} }),
            maintenance_type: "DEFAULT",
            region: REGION,
            availability_zone: "us-east-1a",
        },
    ]
}

/// Create the router output of `expected` and return its `(Arn, Name)`.
async fn create(emulator: &Emulator, expected: &Case) -> (String, String) {
    let case = expected.case;
    let (status, code, body) = create_router_output(emulator, expected.body.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: CreateRouterOutput answers 201, got {status} {code:?} with {body:?}"
    );
    let created = reported(&body);
    (
        string_at(&created, "arn").to_string(),
        string_at(&created, "name").to_string(),
    )
}

/// Assert the `RouterOutput` `router_output` against the request `expected.body` (the
/// echoed members) and against `expected` (the derived ones).
fn assert_reported_router_output(expected: &Case, router_output: &Value) {
    let case = expected.case;
    let body = &expected.body;

    // The minted ARN is asserted through the ONE helper `router_outputs` owns, so neither
    // suite can drift from the other's reading of the modelled `RouterOutputArn` pattern.
    // It also pins `Id` against the ARN's last segment, which is why the id is passed in.
    assert_minted_router_output_arn(
        case,
        string_at(router_output, "arn"),
        string_at(router_output, "id"),
        expected.region,
    );

    for member in ["name", "maximumBitrate", "routingScope", "tier", "configuration"] {
        assert_eq!(
            router_output.get(member),
            body.get(member),
            "case {case:?}: {member} is the value the request carried, got {:?}",
            router_output.get(member)
        );
    }

    assert_eq!(
        router_output.get("outputType").and_then(Value::as_str),
        Some(expected.output_type),
        "case {case:?}: outputType is derived from the configuration, got {:?}",
        router_output.get("outputType")
    );
    assert_eq!(
        router_output.get("streamDetails"),
        Some(&expected.stream_details),
        "case {case:?}: streamDetails is derived from the configuration, got {:?}",
        router_output.get("streamDetails")
    );
    assert_eq!(
        router_output.get("maintenanceType").and_then(Value::as_str),
        Some(expected.maintenance_type),
        "case {case:?}: maintenanceType is derived from the maintenance configuration, got \
         {:?}",
        router_output.get("maintenanceType")
    );
    assert_eq!(
        router_output.get("regionName").and_then(Value::as_str),
        Some(expected.region),
        "case {case:?}: regionName, got {:?}",
        router_output.get("regionName")
    );
    assert_eq!(
        router_output.get("availabilityZone").and_then(Value::as_str),
        Some(expected.availability_zone),
        "case {case:?}: availabilityZone, got {:?}",
        router_output.get("availabilityZone")
    );
    assert_eq!(
        router_output.get("routedState").and_then(Value::as_str),
        Some(mc::ROUTED_STATE_UNROUTED),
        "case {case:?}: a router output nothing has been routed to is UNROUTED, got {:?}",
        router_output.get("routedState")
    );
    assert_eq!(
        router_output.get("routedInputArn"),
        None,
        "case {case:?}: no input is routed to a new router output, got {:?}",
        router_output.get("routedInputArn")
    );
    assert_eq!(
        router_output.get("messages").and_then(Value::as_array),
        Some(&Vec::<Value>::new()),
        "case {case:?}: the required messages list is empty, got {:?}",
        router_output.get("messages")
    );
    assert_eq!(
        router_output.get("tags"),
        Some(body.get("tags").unwrap_or(&json!({}))),
        "case {case:?}: tags are the pairs the request carried, got {:?}",
        router_output.get("tags")
    );
    for member in ["createdAt", "updatedAt"] {
        assert!(
            router_output
                .get(member)
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                > 0.0,
            "case {case:?}: {member} is an epoch timestamp, got {:?}",
            router_output.get(member)
        );
    }
}

/// GetRouterOutput reports the record the create wrote, for every configuration the model
/// declares — not a value fixed in the handler.
#[tokio::test]
async fn get_router_output_reports_the_record_the_create_wrote() {
    let emulator = Emulator::new();

    for expected in cases() {
        let case = expected.case;
        let (arn, _) = create(&emulator, &expected).await;

        let (status, code, body) = get_router_output(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: GetRouterOutput answers 200 for a stored router output, got \
             {status} {code:?} with {body:?}"
        );
        let fetched = reported(&body);
        assert_reported_router_output(&expected, &fetched);
        assert_eq!(
            fetched.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: the read reports the router output the ARN named, got {fetched:?}"
        );
    }
}

/// GetRouterOutput reports the state the CLOCK has moved the router output to: a caller
/// acting straight after the create still sees the transient `CREATING`, and once the
/// transition span has passed the same read answers `STANDBY`.
#[tokio::test]
async fn get_router_output_reports_the_state_the_clock_has_moved_the_output_to() {
    let emulator = Emulator::new();
    let expected = cases().swap_remove(0);
    let (arn, _) = create(&emulator, &expected).await;

    let (_, _, immediate) = get_router_output(&emulator, &arn).await;
    assert_eq!(
        reported(&immediate).get("state").and_then(Value::as_str),
        Some(mc::ROUTER_OUTPUT_STATE_CREATING),
        "the transient CREATING state is visible to a caller acting straight away, got \
         {immediate:?}"
    );

    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (_, _, settled) = get_router_output(&emulator, &arn).await;
    assert_eq!(
        reported(&settled).get("state").and_then(Value::as_str),
        Some(mc::ROUTER_OUTPUT_STATE_STANDBY),
        "the clock settles a created router output into STANDBY, got {settled:?}"
    );
}

/// DeleteRouterOutput answers the ARN, the name and the deletion state of the router output
/// the STORE holds for the ARN it is given, leaves every other record alone, answers the
/// same thing when it is repeated, and the clock then completes the deletion — after which
/// the same ARN addresses nothing.
///
/// The rows are every configuration kind, deleted out of a store that holds a growing
/// number of router outputs, so the answer cannot be a value fixed in the handler.
#[tokio::test]
async fn delete_router_output_reports_and_completes_the_deletion_of_the_stored_output() {
    let suite = cases();

    for (index, expected) in suite.iter().enumerate() {
        let case = expected.case;
        let emulator = Emulator::new();

        // A store holding several router outputs, so the delete has to pick the one its ARN
        // names: the count varies with the row.
        let mut untouched: Vec<String> = Vec::new();
        for other in suite.iter().take(index) {
            let (other_arn, _) = create(&emulator, other).await;
            untouched.push(other_arn);
        }
        let (arn, name) = create(&emulator, expected).await;

        // Delete it twice: `@idempotent`, so the repeat answers the same thing rather than
        // restarting the deletion or failing.
        for attempt in 1..=2 {
            let (status, code, deleted) = delete_router_output(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?} (attempt {attempt}): DeleteRouterOutput answers 200, got \
                 {status} {code:?} with {deleted:?}"
            );
            assert_eq!(
                (
                    deleted.get("arn").and_then(Value::as_str),
                    deleted.get("name").and_then(Value::as_str),
                    deleted.get("state").and_then(Value::as_str),
                ),
                (
                    Some(arn.as_str()),
                    Some(name.as_str()),
                    Some(mc::ROUTER_OUTPUT_STATE_DELETING),
                ),
                "case {case:?} (attempt {attempt}): the delete answers the stored output's \
                 ARN and name and the state its deletion is in, got {deleted:?}"
            );

            // The record is still readable while the deletion is in progress, in DELETING.
            let (status, code, body) = get_router_output(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?} (attempt {attempt}): a router output being deleted is still \
                 readable, got {status} {code:?} with {body:?}"
            );
            assert_eq!(
                reported(&body).get("state").and_then(Value::as_str),
                Some(mc::ROUTER_OUTPUT_STATE_DELETING),
                "case {case:?} (attempt {attempt}): it reads back in DELETING, got {body:?}"
            );
        }

        // Nothing else was deleted.
        for other in &untouched {
            let (status, code, body) = get_router_output(&emulator, other).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?}: the delete left {other:?} alone, got {status} {code:?} with \
                 {body:?}"
            );
            assert_ne!(
                reported(&body).get("state").and_then(Value::as_str),
                Some(mc::ROUTER_OUTPUT_STATE_DELETING),
                "case {case:?}: {other:?} is not being deleted, got {body:?}"
            );
        }

        // The clock completes the deletion: `RouterOutputState` has no terminal deleted
        // value, so the record LEAVES the store and the ARN addresses nothing.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        for (operation, (status, code, body)) in [
            ("GetRouterOutput", get_router_output(&emulator, &arn).await),
            (
                "DeleteRouterOutput",
                delete_router_output(&emulator, &arn).await,
            ),
        ] {
            assert_eq!(
                (status, code.as_deref()),
                (404, Some("NotFoundException")),
                "case {case:?}: {operation} answers NotFoundException once the deletion is \
                 complete, got {status} {code:?} with {body:?}"
            );
        }
        let still_held = {
            let state = emulator
                .engine
                .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
            let held = state.lock().router_outputs.contains_key(&arn);
            held
        };
        assert!(
            !still_held,
            "case {case:?}: the deleted record left the store, but {arn:?} is still held"
        );
    }
}

/// An ARN of the modelled form that the store holds no router output for is a
/// `NotFoundException` (404) from GetRouterOutput and DeleteRouterOutput alike — never a
/// success and never a 501 — and the message is the request's own label in this service's
/// not-found envelope. An ARN outside the modelled `RouterOutputArn` pattern never reaches
/// the handler: the `Arn` member declares that pattern, so the boundary refuses it with
/// `BadRequestException` (400).
///
/// The rows span the ways an ARN can address no router output: an id never minted, and each
/// ARN segment changed in turn.
#[tokio::test]
async fn get_and_delete_refuse_an_arn_that_addresses_no_router_output() {
    let emulator = Emulator::new();

    // One stored router output, so a not-found is about the ARN rather than an empty store.
    let (held, _) = create(&emulator, &cases().swap_remove(0)).await;

    let not_found = [
        (
            "an id this store never minted",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:ffffffffffff",
        ),
        (
            "another Region",
            "arn:aws:mediaconnect:eu-west-1:000000000000:routerOutput:000000000001",
        ),
        (
            "another account",
            "arn:aws:mediaconnect:us-east-1:000000000001:routerOutput:000000000001",
        ),
        (
            "another partition",
            "arn:aws-us-gov:mediaconnect:us-east-1:000000000000:routerOutput:000000000001",
        ),
    ];
    for (case, arn) in not_found {
        for (operation, (status, code, body)) in [
            ("GetRouterOutput", get_router_output(&emulator, arn).await),
            (
                "DeleteRouterOutput",
                delete_router_output(&emulator, arn).await,
            ),
        ] {
            assert_eq!(
                (status, code.as_deref()),
                (404, Some("NotFoundException")),
                "case {case:?}: {operation} answers NotFoundException, got {status} {code:?} \
                 with {body:?}"
            );
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(mc::not_found_message(arn).as_str()),
                "case {case:?}: {operation} names the identifier the request carried, got \
                 {body:?}"
            );
        }
    }

    let outside_the_pattern = [
        ("not an ARN at all", "router-output-1"),
        (
            "a flow ARN",
            "arn:aws:mediaconnect:us-east-1:000000000000:flow:1-0000002a:router-flow",
        ),
        (
            "an id that is one character too short",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:00000000001",
        ),
        (
            "an id with an uppercase character",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:00000000000A",
        ),
    ];
    for (case, arn) in outside_the_pattern {
        for (operation, (status, code, body)) in [
            ("GetRouterOutput", get_router_output(&emulator, arn).await),
            (
                "DeleteRouterOutput",
                delete_router_output(&emulator, arn).await,
            ),
        ] {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: {operation} refuses an ARN outside the modelled pattern at \
                 the boundary, got {status} {code:?} with {body:?}"
            );
        }
    }

    // The positive control for the same rule: the ARN the store DOES hold is still served,
    // so none of the refusals above removed or hid it.
    let (status, code, body) = get_router_output(&emulator, &held).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the ARN the store holds is served, got {status} {code:?} with {body:?}"
    );
}
