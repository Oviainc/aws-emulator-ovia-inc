// SPDX-License-Identifier: CC0-1.0

//! `UpdateChannelClass`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: one channel moved from one class to another is ONE row
//! of each.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_CHANNEL_DESTINATIONS;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{describe, wait_for_the_clock};
use crate::wire::{call, error_code, error_message, medialive};

/// `UpdateChannelClass` as a client sends it: the class, and the destinations the
/// request names (or none at all).
async fn update_class(
    router: &MultiServiceRouter,
    id: &str,
    class: &str,
    destinations: Option<Value>,
) -> (u16, Value) {
    let mut body = json!({ "channelClass": class });
    if let Some(destinations) = destinations {
        body["destinations"] = destinations;
    }
    call(
        router,
        "PUT",
        &format!("/prod/channels/{id}/channelClass"),
        Some(body),
    )
    .await
}

/// One created channel, answered as the create reported it.
async fn create_channel(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, response) = call(router, "POST", "/prod/channels", Some(body)).await;
    assert_eq!(
        status, 201,
        "CreateChannel: expected 201, got {status}: {response}"
    );
    response["channel"].clone()
}

/// One channel created from `body` and left in `IDLE`: the create's transient
/// `CREATING` has settled, which is the state a class update is accepted from.
async fn idle_channel(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let created = create_channel(router, body).await;
    let id = created["id"].as_str().unwrap_or_default().to_string();
    wait_for_the_clock();
    let (status, idle) = describe(router, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeChannel: got {status}: {idle}"
    );
    assert_eq!(
        idle["state"],
        json!("IDLE"),
        "case {case:?}: expected the created channel to settle to IDLE, got {:?}",
        idle["state"]
    );
    id
}

/// The channel each row starts from: a named channel carrying one destination, a
/// role, a tag and an input attachment, so a class update can be seen to leave
/// every member but the class alone.
fn baseline_body(class: Option<&str>) -> Value {
    let mut body = json!({
        "name": "class-baseline",
        "roleArn": "arn:aws:iam::111122223333:role/Baseline",
        "logLevel": "INFO",
        "destinations": [{ "id": "baseline-destination", "settings": [{ "url": "rtmp://a" }] }],
        "inputAttachments": [{ "inputAttachmentName": "baseline", "inputId": "1000000" }],
        "tags": { "team": "video" },
    });
    if let Some(class) = class {
        body["channelClass"] = json!(class);
    }
    body
}

/// Replace one member of the channel document a row compares against; a `Null`
/// means the member is ABSENT afterwards.
fn apply(expected: &mut Value, member: &str, value: &Value) {
    let object = expected
        .as_object_mut()
        .expect("the described channel is a JSON object");
    if value.is_null() {
        object.remove(member);
    } else {
        object.insert(member.to_string(), value.clone());
    }
}

/// The `EgressEndpoints` of one channel-shaped body, as a client reads them: the
/// member carries one entry per pipeline, so a body reports none only if it has
/// no pipelines.
fn egress_endpoints(body: &Value) -> Vec<Value> {
    body["egressEndpoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One egress endpoint per pipeline of the channel's class, with every pipeline
/// that survived the class change still egressing from the endpoint it already
/// reported: a class change re-provisions the endpoints along with the
/// pipelines, it does not move the ones a surviving pipeline uses.
fn assert_egress_endpoints(
    case: &str,
    read: &str,
    body: &Value,
    before: &[Value],
    pipelines: usize,
) {
    let reported = egress_endpoints(body);
    assert_eq!(
        reported.len(),
        pipelines,
        "case {case:?}: {read}: expected one egress endpoint per pipeline ({pipelines}), got {reported:?}"
    );
    let kept = before.len().min(pipelines);
    assert_eq!(
        reported[..kept],
        before[..kept],
        "case {case:?}: {read}: expected the surviving pipelines to keep the egress endpoints {before:?}, got {reported:?}"
    );
}

/// Rule: `UpdateChannelClass` stores the class its request names on the stored
/// channel and changes nothing else about it, which re-provisions the channel's
/// encoding pipelines — the `ChannelClass` doc: "A standard channel has two
/// encoding pipelines and a single pipeline channel only has one" — so the
/// channel afterwards runs, and reports, the number of pipelines its NEW class
/// gives it; the accepted change reports the transient `UPDATING` and settles
/// back into the `IDLE` it started from.
///
/// The table spans the `ChannelClass` enum (`smithy_query(query="shape",
/// shape="ChannelClass")`: STANDARD | SINGLE_PIPELINE) in both directions, each
/// class re-stated on a channel that already has it, and the channel created
/// with no class at all (which the service defaults to STANDARD).
#[tokio::test]
async fn a_class_update_re_provisions_the_pipelines_the_new_class_gives_the_channel() {
    // (case, the class the channel was created with, the class the update names,
    //  the pipelines the channel runs afterwards)
    let table: [(&str, Option<&str>, &str, usize); 5] = [
        ("single pipeline to standard", Some("SINGLE_PIPELINE"), "STANDARD", 2),
        ("standard to single pipeline", Some("STANDARD"), "SINGLE_PIPELINE", 1),
        ("single pipeline restated", Some("SINGLE_PIPELINE"), "SINGLE_PIPELINE", 1),
        ("standard restated", Some("STANDARD"), "STANDARD", 2),
        ("class omitted at create, then single pipeline", None, "SINGLE_PIPELINE", 1),
    ];

    for (case, created_class, requested, pipelines) in table {
        let router = medialive();
        let id = idle_channel(&router, case, baseline_body(created_class)).await;
        let (_, before) = describe(&router, &id).await;
        let endpoints_before = egress_endpoints(&before);

        // The class is the ONE member this operation writes, so everything else
        // the channel reported must read back unchanged — except its
        // `EgressEndpoints`, which carry one entry per pipeline, so
        // re-provisioning the pipelines re-provisions them too. Those are
        // asserted on their own, against the new class's pipeline count.
        let mut expected = before.clone();
        apply(&mut expected, "channelClass", &json!(requested));
        apply(&mut expected, "egressEndpoints", &Value::Null);

        let (status, response) = update_class(&router, &id, requested, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateChannelClass: expected 200, got {status}: {response}"
        );
        assert_eq!(
            response["channel"]["state"],
            json!("UPDATING"),
            "case {case:?}: expected the accepted change to report the transient UPDATING, got {:?}",
            response["channel"]["state"]
        );
        // That transient state is the one difference between what the call
        // answered and the configuration it stored, so it is normalised away
        // before the two are compared member by member.
        let mut answered = response["channel"].clone();
        apply(&mut answered, "state", &before["state"]);
        assert_egress_endpoints(case, "the answer", &answered, &endpoints_before, pipelines);
        apply(&mut answered, "egressEndpoints", &Value::Null);
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the answer to be the stored channel with the new class, got {answered:?}"
        );

        // Round trip: the class is what is STORED, not just what was answered —
        // and the `UPDATING` the change owes settles back into `IDLE`.
        wait_for_the_clock();
        let (status, mut after) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel after the class update: got {status}: {after}"
        );
        assert_egress_endpoints(case, "DescribeChannel", &after, &endpoints_before, pipelines);
        apply(&mut after, "egressEndpoints", &Value::Null);
        assert_eq!(
            after, expected,
            "case {case:?}: expected the new class to read back through DescribeChannel, got {after:?}"
        );

        // The pipelines the channel actually runs come from the stored class:
        // `PipelinesRunningCount` is "The number of currently healthy
        // pipelines", so it is the new class's pipeline count exactly while the
        // channel is RUNNING.
        let (status, started) =
            call(&router, "POST", &format!("/prod/channels/{id}/start"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartChannel: expected 200, got {status}: {started}"
        );
        wait_for_the_clock();
        let (status, running) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel while running: got {status}: {running}"
        );
        assert_eq!(
            running["state"],
            json!("RUNNING"),
            "case {case:?}: the test did not reach RUNNING, it reached {:?}",
            running["state"]
        );
        assert_eq!(
            running["pipelinesRunningCount"],
            json!(pipelines),
            "case {case:?}: expected {pipelines} healthy pipelines after the class update, got {:?}",
            running["pipelinesRunningCount"]
        );
        assert_eq!(
            running["pipelineDetails"].as_array().map(Vec::len),
            Some(pipelines),
            "case {case:?}: expected {pipelines} pipeline details after the class update, got {:?}",
            running["pipelineDetails"]
        );
    }
}

/// Rule: `ChannelClass` targets an `enum` shape (`smithy_query(query="shape",
/// shape="ChannelClass")`: STANDARD | SINGLE_PIPELINE), so a value that shape
/// does not declare names no class the service can store: the request is refused
/// as a client error and the channel keeps the class it had.
///
/// The rows span what the enum's membership rules out: a spelling in the wrong
/// case, a modelled value with trailing whitespace, a plausible value the enum
/// does not declare, and the empty value.
#[tokio::test]
async fn a_class_outside_the_enum_is_refused_and_writes_nothing() {
    let table: [(&str, &str); 4] = [
        ("a modelled value in the wrong case", "standard"),
        ("a modelled value with trailing whitespace", "STANDARD "),
        ("a value the enum does not declare", "DUAL_PIPELINE"),
        ("the empty value", ""),
    ];

    for (case, class) in table {
        let router = medialive();
        let id = idle_channel(&router, case, baseline_body(Some("SINGLE_PIPELINE"))).await;
        let (_, before) = describe(&router, &id).await;

        let (status, refusal) = update_class(&router, &id, class, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a class outside the enum, got {status}: {refusal}"
        );
        let code = error_code(status, &refusal);
        assert!(
            code == "BadRequestException" || code == "ValidationException",
            "case {case:?}: expected a client refusal, got {code}: {refusal}"
        );

        let (status, after) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel after the refusal: got {status}: {after}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused request to write nothing, got {after:?}"
        );
    }
}

/// Rule: `Destinations` is optional on this request, so a request that names it
/// REPLACES the channel's stored destinations and one that omits it leaves them
/// exactly as they were — the member rule `UpdateChannel` applies to the same
/// list, and why the class change can carry the per-encoder destinations the new
/// class needs (`OutputDestination$Settings`: "one destination for each
/// redundant encoder").
///
/// The rows differ along what the request names: nothing, one entry, two entries
/// where the channel had one, an entry carrying one per-encoder setting and one
/// carrying two, the empty list, and a list AT the ceiling this emulator stores
/// (`MAX_CHANNEL_DESTINATIONS`).
#[tokio::test]
async fn a_class_update_replaces_the_destinations_only_when_the_request_names_them() {
    let at_the_ceiling: Vec<Value> = (0..MAX_CHANNEL_DESTINATIONS)
        .map(|index| json!({ "id": format!("destination-{index}") }))
        .collect();

    // (case, the destinations the request names, the destinations the channel
    //  must report afterwards)
    let table: [(&str, Option<Value>, Value); 6] = [
        (
            "no destinations named",
            None,
            json!([{ "id": "baseline-destination", "settings": [{ "url": "rtmp://a" }] }]),
        ),
        (
            "one destination replaced by one",
            Some(json!([{ "id": "replacement" }])),
            json!([{ "id": "replacement" }]),
        ),
        (
            "one destination replaced by two",
            Some(json!([{ "id": "a" }, { "id": "b" }])),
            json!([{ "id": "a" }, { "id": "b" }]),
        ),
        (
            // One per-encoder destination setting per pipeline of the class the
            // request moves the channel to.
            "two per-encoder settings on one destination",
            Some(json!([{
                "id": "redundant",
                "settings": [{ "url": "rtmp://a" }, { "url": "rtmp://b" }],
            }])),
            json!([{
                "id": "redundant",
                "settings": [{ "url": "rtmp://a" }, { "url": "rtmp://b" }],
            }]),
        ),
        (
            // A list the request names is replaced by what it names, so an empty
            // list leaves the channel with no destination at all.
            "an empty destination list",
            Some(json!([])),
            json!([]),
        ),
        (
            "a list at the ceiling",
            Some(Value::Array(at_the_ceiling.clone())),
            Value::Array(at_the_ceiling),
        ),
    ];

    for (case, named, expected_destinations) in table {
        let router = medialive();
        let id = idle_channel(&router, case, baseline_body(Some("SINGLE_PIPELINE"))).await;

        let (status, response) = update_class(&router, &id, "STANDARD", named).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateChannelClass: expected 200, got {status}: {response}"
        );
        assert_eq!(
            response["channel"]["destinations"], expected_destinations,
            "case {case:?}: expected the answered destinations to be {expected_destinations:?}, got {:?}",
            response["channel"]["destinations"]
        );

        wait_for_the_clock();
        let (status, after) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel after the class update: got {status}: {after}"
        );
        assert_eq!(
            after["destinations"], expected_destinations,
            "case {case:?}: expected the stored destinations to be {expected_destinations:?}, got {:?}",
            after["destinations"]
        );
        assert_eq!(
            after["channelClass"],
            json!("STANDARD"),
            "case {case:?}: expected the class to be stored beside the destinations, got {:?}",
            after["channelClass"]
        );
    }
}

/// What a refusal row has to do to the channel before the refused request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    /// Leave the channel in the transient `CREATING` the create started.
    Creating,
    /// Settle it to `IDLE`, then start it and let the start settle.
    Running,
    /// Settle it to `IDLE`, then start it and refuse inside `STARTING`.
    Starting,
    /// Settle it to `IDLE`, then delete it and refuse inside `DELETING`.
    Deleting,
    /// Settle it to `IDLE`, delete it and let the delete settle.
    Deleted,
    /// Settle it to `IDLE` — the state a class update is accepted from, so the
    /// refusal is about the request and not about the channel.
    Idle,
}

/// One row of the refusal table: what the channel is doing, what the request
/// names, and the refusal it gets.
struct Refusal {
    case: &'static str,
    setup: Setup,
    /// The `Destinations` the refused request names, if any.
    named: Option<Value>,
    status: u16,
    code: &'static str,
    /// The state the channel reports once the clock has settled whatever it was
    /// already owed — the refusal itself writes nothing.
    state_afterwards: &'static str,
    /// What the refusal's message names; `None` for the channel id the request
    /// carried.
    names: Option<&'static str>,
}

/// Rule: a class update is refused — and writes NOTHING — on a channel id that
/// resolves to no channel a caller may still change (its modelled
/// `NotFoundException`, 404, for an id no channel ever had and for one whose
/// delete this service has accepted), on a channel that has not stopped (its
/// modelled `ConflictException`, 409, because such a channel is encoding through
/// exactly the pipelines the change would re-provision), and on a request
/// carrying more destinations than this emulator stores on one channel (its
/// modelled `BadRequestException`, 400).
///
/// The rows differ along WHY the request cannot be served: the id (unknown,
/// deleting, deleted), the state the channel is in (every non-stopped state this
/// slice can reach), and the request itself.
#[tokio::test]
async fn a_class_update_is_refused_without_writing_anything() {
    let over_the_ceiling: Vec<Value> = (0..=MAX_CHANNEL_DESTINATIONS)
        .map(|index| json!({ "id": format!("destination-{index}") }))
        .collect();

    let table: [Refusal; 6] = [
        Refusal {
            case: "while CREATING",
            setup: Setup::Creating,
            named: None,
            status: 409,
            code: "ConflictException",
            state_afterwards: "IDLE",
            names: None,
        },
        Refusal {
            case: "while STARTING",
            setup: Setup::Starting,
            named: None,
            status: 409,
            code: "ConflictException",
            state_afterwards: "RUNNING",
            names: None,
        },
        Refusal {
            case: "while RUNNING",
            setup: Setup::Running,
            named: None,
            status: 409,
            code: "ConflictException",
            state_afterwards: "RUNNING",
            names: None,
        },
        Refusal {
            case: "while DELETING",
            setup: Setup::Deleting,
            named: None,
            status: 404,
            code: "NotFoundException",
            state_afterwards: "DELETED",
            names: None,
        },
        Refusal {
            case: "once DELETED",
            setup: Setup::Deleted,
            named: None,
            status: 404,
            code: "NotFoundException",
            state_afterwards: "DELETED",
            names: None,
        },
        Refusal {
            case: "more destinations than the ceiling",
            setup: Setup::Idle,
            named: Some(Value::Array(over_the_ceiling)),
            status: 400,
            code: "BadRequestException",
            state_afterwards: "IDLE",
            // The request, not the channel, is what is out of bounds, so the
            // refusal names the member it refused.
            names: Some("destinations"),
        },
    ];

    for row in table {
        let Refusal {
            case,
            setup,
            named,
            status: status_expected,
            code,
            state_afterwards,
            names,
        } = row;
        let router = medialive();
        let created = create_channel(&router, baseline_body(Some("SINGLE_PIPELINE"))).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        if setup != Setup::Creating {
            wait_for_the_clock();
        }
        match setup {
            Setup::Running | Setup::Starting => {
                let (status, started) =
                    call(&router, "POST", &format!("/prod/channels/{id}/start"), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: StartChannel: got {status}: {started}"
                );
                if setup == Setup::Running {
                    wait_for_the_clock();
                }
            }
            Setup::Deleting | Setup::Deleted => {
                let (status, deleting) =
                    call(&router, "DELETE", &format!("/prod/channels/{id}"), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DeleteChannel: got {status}: {deleting}"
                );
                if setup == Setup::Deleted {
                    wait_for_the_clock();
                }
            }
            Setup::Creating | Setup::Idle => {}
        }

        // What the channel looks like one instant before the refused request.
        let (status, before) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel before the refusal: got {status}: {before}"
        );

        let (status, refusal) = update_class(&router, &id, "STANDARD", named).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected {status_expected}, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            code,
            "case {case:?}: expected {code}, got {refusal}"
        );
        let named_in_message = names.unwrap_or(&id);
        assert!(
            error_message(&refusal).contains(named_in_message),
            "case {case:?}: expected the refusal to name {named_in_message:?}, got {:?}",
            error_message(&refusal)
        );

        // Nothing was written: every member but the lifecycle state the clock
        // was already owed reads back exactly as it did before the refusal.
        wait_for_the_clock();
        let (status, after) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel after the refusal: got {status}: {after}"
        );
        assert_eq!(
            after["state"],
            json!(state_afterwards),
            "case {case:?}: expected the refused channel to be {state_afterwards}, got {:?}",
            after["state"]
        );
        for member in ["channelClass", "destinations", "name", "roleArn", "tags"] {
            assert_eq!(
                after[member], before[member],
                "case {case:?}: expected the refusal to leave {member} alone, got {:?}",
                after[member]
            );
        }
    }

    // An id no channel ever had: the same refusal, with no channel to compare.
    let router = medialive();
    let (status, refusal) = update_class(&router, "0000000", "STANDARD", None).await;
    assert_eq!(
        status, 404,
        "case \"an unknown channel id\": expected 404, got {status}: {refusal}"
    );
    assert_eq!(
        error_code(status, &refusal),
        "NotFoundException",
        "case \"an unknown channel id\": expected NotFoundException, got {refusal}"
    );
    assert_eq!(
        error_message(&refusal),
        "Channel 0000000 not found",
        "case \"an unknown channel id\": expected the message composed from the request, got {:?}",
        error_message(&refusal)
    );
}
