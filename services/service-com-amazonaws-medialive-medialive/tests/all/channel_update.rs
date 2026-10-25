// SPDX-License-Identifier: CC0-1.0

//! `UpdateChannel`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: a single renamed channel is ONE row of each.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    MAX_CHANNEL_DESTINATIONS, MAX_INPUT_ATTACHMENTS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{describe, wait_for_the_clock};
use crate::wire::{call, error_code, error_message, medialive};

/// The `EncoderSettings` an update can store: the four members the shape
/// requires (`smithy_query(query="shape", shape="EncoderSettings")`:
/// AudioDescriptions, OutputGroups, TimecodeConfig, VideoDescriptions).
fn encoder_settings(timecode_source: &str) -> Value {
    json!({
        "audioDescriptions": [],
        "outputGroups": [],
        "timecodeConfig": { "source": timecode_source },
        "videoDescriptions": [],
    })
}

/// The channel every row of the update table starts from: a channel carrying a
/// name, a role, a maintenance window, one destination, one input attachment and
/// a tag, so a patch that replaces one member can be seen to leave every other
/// one alone.
fn baseline_body() -> Value {
    json!({
        "name": "baseline",
        "channelClass": "STANDARD",
        "roleArn": "arn:aws:iam::111122223333:role/Baseline",
        "logLevel": "INFO",
        "cdiInputSpecification": { "resolution": "HD" },
        "inputSpecification": {
            "codec": "AVC",
            "maximumBitrate": "MAX_10_MBPS",
            "resolution": "HD",
        },
        "maintenance": { "maintenanceDay": "MONDAY", "maintenanceStartTime": "02:00" },
        "destinations": [{ "id": "baseline-destination" }],
        "inputAttachments": [{ "inputAttachmentName": "baseline", "inputId": "1000000" }],
        "encoderSettings": encoder_settings("EMBEDDED"),
        "tags": { "team": "video" },
    })
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

async fn update_channel(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    call(router, "PUT", &format!("/prod/channels/{id}"), Some(body)).await
}

/// One channel created from `body` and left in `IDLE`: the create's transient
/// `CREATING` has settled, which is the state an update is accepted from.
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

/// Apply one expected change to the channel document a row compares against: a
/// `Null` value means the member is ABSENT afterwards, which is a response with
/// no such member rather than one carrying a null.
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

/// One row of the member table: the case, the request body, and the members the
/// stored channel must report differently once that request has been applied.
type MemberRow = (&'static str, Value, Vec<(&'static str, Value)>);

/// Rule: `UpdateChannel` replaces exactly the members its request names on the
/// stored channel and leaves every member the request omits at the value the
/// channel already had — except `RoleArn`, which the model documents as removed
/// when an update omits it — and answers the channel read back out of state, so
/// the same channel reads back through `DescribeChannel`.
///
/// The table spans the members this operation's input carries
/// (`smithy_query(query="operation", operation="UpdateChannel")`: Name, RoleArn,
/// CdiInputSpecification, InputSpecification, LogLevel, Destinations,
/// InputAttachments, ChannelSecurityGroups, Maintenance, EncoderSettings,
/// AnywhereSettings, LinkedChannelSettings, InferenceSettings,
/// SpecialRouterSettings), at one and at two list entries and at an empty list,
/// plus the row that names no member at all. `ChannelEngineVersion` has its own
/// documented rule and its own suite below.
#[tokio::test]
async fn an_update_replaces_only_the_members_the_request_names() {
    // (case, request body, the members the stored channel must now report
    //  differently — every other member must still read back unchanged)
    let table: [MemberRow; 17] = [
        ("no member named", json!({}), vec![]),
        (
            "name only",
            json!({ "name": "renamed" }),
            vec![("name", json!("renamed"))],
        ),
        (
            "log level only",
            json!({ "logLevel": "DEBUG" }),
            vec![("logLevel", json!("DEBUG"))],
        ),
        (
            "cdi input specification",
            json!({ "cdiInputSpecification": { "resolution": "UHD" } }),
            vec![("cdiInputSpecification", json!({ "resolution": "UHD" }))],
        ),
        (
            "input specification",
            json!({ "inputSpecification": {
                "codec": "HEVC",
                "maximumBitrate": "MAX_50_MBPS",
                "resolution": "UHD",
            } }),
            vec![("inputSpecification", json!({
                "codec": "HEVC",
                "maximumBitrate": "MAX_50_MBPS",
                "resolution": "UHD",
            }))],
        ),
        (
            "two destinations replace one",
            json!({ "destinations": [{ "id": "a" }, { "id": "b" }] }),
            vec![("destinations", json!([{ "id": "a" }, { "id": "b" }]))],
        ),
        (
            // A list the request names is replaced by what it names, so an
            // empty list leaves the channel with no destination at all.
            "an empty destination list",
            json!({ "destinations": [] }),
            vec![("destinations", json!([]))],
        ),
        (
            "two input attachments replace one",
            json!({ "inputAttachments": [
                { "inputAttachmentName": "primary", "inputId": "1000001" },
                { "inputAttachmentName": "backup", "inputId": "1000002" },
            ] }),
            vec![("inputAttachments", json!([
                { "inputAttachmentName": "primary", "inputId": "1000001" },
                { "inputAttachmentName": "backup", "inputId": "1000002" },
            ]))],
        ),
        (
            "an empty input attachment list",
            json!({ "inputAttachments": [] }),
            vec![("inputAttachments", json!([]))],
        ),
        (
            "two channel security groups",
            json!({ "channelSecurityGroups": ["3000000", "3000001"] }),
            vec![("channelSecurityGroups", json!(["3000000", "3000001"]))],
        ),
        (
            // Each `MaintenanceUpdateSettings` member is documented on its own,
            // so naming the day leaves the stored start time alone.
            "the maintenance day alone",
            json!({ "maintenance": { "maintenanceDay": "FRIDAY" } }),
            vec![("maintenance", json!({
                "maintenanceDay": "FRIDAY",
                "maintenanceStartTime": "02:00",
            }))],
        ),
        (
            // `MaintenanceScheduledDate` is a member only the UPDATE carries,
            // so this is the one way a caller sets it.
            "a maintenance start time and scheduled date",
            json!({ "maintenance": {
                "maintenanceStartTime": "23:00",
                "maintenanceScheduledDate": "2031-02-03",
            } }),
            vec![("maintenance", json!({
                "maintenanceDay": "MONDAY",
                "maintenanceStartTime": "23:00",
                "maintenanceScheduledDate": "2031-02-03",
            }))],
        ),
        (
            "encoder settings",
            json!({ "encoderSettings": encoder_settings("ZEROBASED") }),
            vec![("encoderSettings", encoder_settings("ZEROBASED"))],
        ),
        (
            "anywhere settings",
            json!({ "anywhereSettings": {
                "channelPlacementGroupId": "7000000",
                "clusterId": "5000000",
            } }),
            vec![("anywhereSettings", json!({
                "channelPlacementGroupId": "7000000",
                "clusterId": "5000000",
            }))],
        ),
        (
            "linked channel settings",
            json!({ "linkedChannelSettings": { "followerChannelSettings": {
                "primaryChannelArn": "arn:aws:medialive:us-west-2:000000000000:channel:9999999",
            } } }),
            vec![("linkedChannelSettings", json!({ "followerChannelSettings": {
                "primaryChannelArn": "arn:aws:medialive:us-west-2:000000000000:channel:9999999",
            } }))],
        ),
        (
            "inference settings",
            json!({ "inferenceSettings": {
                "feedArn": "arn:aws:elemental-inference:us-west-2:111122223333:feed/f1",
            } }),
            vec![("inferenceSettings", json!({
                "feedArn": "arn:aws:elemental-inference:us-west-2:111122223333:feed/f1",
            }))],
        ),
        (
            // `SpecialRouterSettings` carries only the router's ARN, and the
            // `Channel` response shape has no member that reports one, so the
            // stored channel reads back exactly as before. This row pins the
            // projection, not the store: the modelled response cannot tell a
            // handler that stored the member from one that dropped it.
            "special router settings",
            json!({ "specialRouterSettings": { "routerArn": "arn:aws:mediaconnect:us-west-2:111122223333:router:r1" } }),
            vec![],
        ),
    ];

    for (case, patch, changes) in table {
        let router = medialive();
        let id = idle_channel(&router, case, baseline_body()).await;
        let (_, before) = describe(&router, &id).await;

        // What the stored channel must look like afterwards: the one it had,
        // with the members this request names replaced — and, because the
        // request names no `RoleArn`, without the role it previously had
        // ("If you do not specify this on an update call but the role was
        // previously set that role will be removed").
        let mut expected = before.clone();
        for (member, value) in &changes {
            apply(&mut expected, member, value);
        }
        if patch.get("roleArn").is_none() {
            apply(&mut expected, "roleArn", &Value::Null);
        }

        let (status, response) = update_channel(&router, &id, patch.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateChannel: expected 200, got {status}: {response}"
        );
        assert_eq!(
            response["channel"]["state"],
            json!("UPDATING"),
            "case {case:?}: expected an accepted update to report the transient UPDATING, got {:?}",
            response["channel"]["state"]
        );
        // The transient state is the one difference between what the update
        // answered and the configuration it stored, so it is normalised away
        // before the two are compared member by member.
        let mut answered = response["channel"].clone();
        apply(&mut answered, "state", &before["state"]);
        assert_eq!(
            answered, expected,
            "case {case:?}: expected UpdateChannel to answer the stored channel with {changes:?} replaced, got {answered:?}"
        );

        // Round trip: the update is what is stored, not what was answered — and
        // the `UPDATING` the update owes settles back into the `IDLE` it started
        // from.
        wait_for_the_clock();
        let (status, after) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel after the update: got {status}: {after}"
        );
        assert_eq!(
            after, expected,
            "case {case:?}: expected the updated channel to read back through DescribeChannel, got {after:?}"
        );

        // The identifiers and the members no update member can reach are the
        // ones the create minted.
        for member in ["id", "arn", "channelClass", "tags"] {
            assert_eq!(
                after[member], before[member],
                "case {case:?}: expected {member} to survive the update, got {:?}",
                after[member]
            );
        }

        // Idempotence: applying the same patch again lands the same channel.
        let (status, repeated) = update_channel(&router, &id, patch).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated UpdateChannel: got {status}: {repeated}"
        );
        let mut answered = repeated["channel"].clone();
        apply(&mut answered, "state", &before["state"]);
        assert_eq!(
            answered, expected,
            "case {case:?}: expected a repeated update to land the same channel, got {answered:?}"
        );
    }
}

/// Rule: `RoleArn` is the one member whose OMISSION writes — the model
/// documents it as "An optional Amazon Resource Name (ARN) of the role to assume
/// when running the Channel. If you do not specify this on an update call but
/// the role was previously set that role will be removed" — so an update that
/// names it replaces the stored role and an update that omits it removes it.
///
/// The rows differ along what the channel had before the update (a role, and no
/// role) and what the update names (a role, and no role at all), which is every
/// combination the rule distinguishes.
#[tokio::test]
async fn an_update_that_omits_the_role_arn_removes_the_stored_role() {
    const FIRST: &str = "arn:aws:iam::111122223333:role/First";
    const SECOND: &str = "arn:aws:iam::111122223333:role/Second";

    // (case, the role the channel was created with, the patch, the role the
    //  channel must report afterwards)
    let table: [(&str, Option<&str>, Value, Option<&str>); 4] = [
        ("a stored role, replaced", Some(FIRST), json!({ "roleArn": SECOND }), Some(SECOND)),
        ("a stored role, omitted", Some(FIRST), json!({ "name": "no-role" }), None),
        ("no stored role, named", None, json!({ "roleArn": SECOND }), Some(SECOND)),
        ("no stored role, omitted", None, json!({ "name": "still-no-role" }), None),
    ];

    for (case, created_role, patch, expected_role) in table {
        let router = medialive();
        let mut body = json!({ "name": "role-rules" });
        if let Some(role) = created_role {
            body["roleArn"] = json!(role);
        }
        let id = idle_channel(&router, case, body).await;

        let (status, response) = update_channel(&router, &id, patch).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateChannel: expected 200, got {status}: {response}"
        );

        wait_for_the_clock();
        let (status, after) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel: got {status}: {after}"
        );
        let expected = expected_role.map_or(Value::Null, |role| json!(role));
        assert_eq!(
            after["roleArn"], expected,
            "case {case:?}: expected roleArn {expected:?} after the update, got {:?}",
            after["roleArn"]
        );
    }
}

/// One row of the engine-version table: the case and the steps to apply in
/// order, each the `Version` the update names and the version the channel must
/// report once it has been applied.
type EngineVersionRow = (&'static str, Vec<(&'static str, Option<&'static str>)>);

/// Rule: `ChannelEngineVersion$Version` is "The build identifier of the engine
/// version to use for this channel. Specify 'DEFAULT' to reset to the default
/// version", so every other value pins that build identifier on the channel and
/// that one value un-pins it — leaving the channel reporting the engine version
/// a channel created without one reports.
///
/// The rows differ along the value the update names: a build identifier, a
/// second one replacing it, the documented reset literal, and a reset applied to
/// a channel that was never pinned.
#[tokio::test]
async fn the_engine_version_reset_literal_un_pins_the_channel() {
    // (case, the steps: the version named, and the version reported after it)
    let table: [EngineVersionRow; 3] = [
        (
            "a build identifier, then another",
            vec![("0.1.0", Some("0.1.0")), ("0.2.0", Some("0.2.0"))],
        ),
        (
            "pinned, then reset to the default",
            vec![("0.1.0", Some("0.1.0")), ("DEFAULT", None)],
        ),
        (
            "reset on a channel that was never pinned",
            vec![("DEFAULT", None)],
        ),
    ];

    for (case, steps) in table {
        let router = medialive();
        let id = idle_channel(&router, case, json!({ "name": "engine-versions" })).await;

        for (version, expected) in steps {
            let (status, response) = update_channel(
                &router,
                &id,
                json!({ "channelEngineVersion": { "version": version } }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}/{version}: UpdateChannel: expected 200, got {status}: {response}"
            );

            wait_for_the_clock();
            let (status, after) = describe(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{version}: DescribeChannel: got {status}: {after}"
            );
            let want = expected.map_or(Value::Null, |version| json!({ "version": version }));
            assert_eq!(
                after["channelEngineVersion"], want,
                "case {case:?}/{version}: expected channelEngineVersion {want:?}, got {:?}",
                after["channelEngineVersion"]
            );
        }
    }
}

/// Rule: a channel's configuration is changed only once that channel has
/// STOPPED — in every other state the update is refused with this operation's
/// modelled `ConflictException` (409) and writes nothing.
///
/// The table walks one channel through the states a channel reaches
/// (`smithy_query(query="shape", shape="ChannelState")`): the transient
/// `CREATING`, the settled `IDLE`, `STARTING`, `RUNNING`, the `UPDATING` an
/// accepted update itself owes, and back to `IDLE`.
#[tokio::test]
async fn a_channel_that_has_not_stopped_cannot_be_updated() {
    let router = medialive();
    let created = create_channel(&router, baseline_body()).await;
    let id = created["id"].as_str().unwrap_or_default().to_string();

    // Each step renames the channel and then reads it back: an accepted update
    // is the stored name, a refused one leaves the name the last accepted
    // update wrote. `stored` carries that name forward through the walk.
    let mut stored = "baseline".to_string();

    // CREATING: the channel has not reached IDLE yet.
    step(&router, &id, "while the channel is CREATING", "creating", 409, &mut stored).await;

    // IDLE: the channel has settled and encodes nothing, so the edit lands —
    // and the update's own UPDATING is a state it has not stopped in either.
    wait_for_the_clock();
    step(&router, &id, "once the channel is IDLE", "idle", 200, &mut stored).await;
    step(&router, &id, "while the channel is UPDATING", "updating", 409, &mut stored).await;

    // STARTING, then RUNNING: the channel is encoding.
    wait_for_the_clock();
    let (status, starting) = call(&router, "POST", &format!("/prod/channels/{id}/start"), None).await;
    assert_eq!(status, 200, "StartChannel: got {status}: {starting}");
    step(&router, &id, "while the channel is STARTING", "starting", 409, &mut stored).await;

    wait_for_the_clock();
    step(&router, &id, "while the channel is RUNNING", "running", 409, &mut stored).await;

    // Stopped and settled: editable again.
    let (status, stopping) = call(&router, "POST", &format!("/prod/channels/{id}/stop"), None).await;
    assert_eq!(status, 200, "StopChannel: got {status}: {stopping}");
    step(&router, &id, "while the channel is STOPPING", "stopping", 409, &mut stored).await;
    wait_for_the_clock();
    step(&router, &id, "once the channel has stopped", "stopped", 200, &mut stored).await;
}

/// One step of the walk above: rename the channel, check the status the
/// channel's state calls for, then read the channel back to show what was
/// written. `stored` is the name the channel must now report, which an accepted
/// update advances and a refused one leaves alone.
async fn step(
    router: &MultiServiceRouter,
    id: &str,
    case: &str,
    name: &str,
    expected_status: u16,
    stored: &mut String,
) {
    let (status, body) = update_channel(router, id, json!({ "name": name })).await;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected {expected_status}, got {status}: {body}"
    );
    if expected_status == 409 {
        assert_eq!(
            error_code(status, &body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
    } else {
        *stored = name.to_string();
        assert_eq!(
            body["channel"]["name"],
            json!(name),
            "case {case:?}: expected the update to have been applied, got {:?}",
            body["channel"]["name"]
        );
    }

    let (status, after) = describe(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeChannel: got {status}: {after}"
    );
    assert_eq!(
        after["name"],
        json!(stored.as_str()),
        "case {case:?}: expected the stored name to be {stored:?}, got {:?}",
        after["name"]
    );
}

/// Rule: `UpdateChannel` on an id it can resolve no live channel for — one no
/// channel ever had, or one whose delete this service has accepted — writes
/// nothing and is refused with this operation's modelled `BadRequestException`
/// (400), because its model declares no `NotFoundException`
/// (`smithy_query(query="operation", operation="UpdateChannel")`).
#[tokio::test]
async fn updating_a_channel_that_cannot_be_resolved_is_a_bad_request() {
    let router = medialive();
    let id = idle_channel(&router, "live", baseline_body()).await;

    // An id no channel has: one before the minted range, a non-numeric one, and
    // one past the channels this store minted.
    for unknown in ["999999", "nope", "1000009"] {
        let (status, body) = update_channel(
            &router,
            unknown,
            json!({ "name": "should-not-be-stored" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {unknown:?}: expected 400 for an unknown channel, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {unknown:?}: expected BadRequestException, got {body}"
        );
        assert!(
            error_message(&body).contains(unknown),
            "case {unknown:?}: expected the refusal to name the id it refused, got {:?}",
            error_message(&body)
        );
    }

    // The channel that does exist was not touched by any of those refusals.
    let (status, after) = describe(&router, &id).await;
    assert_eq!(status, 200, "DescribeChannel: got {status}: {after}");
    assert_eq!(
        after["name"],
        json!("baseline"),
        "expected a refused update to have written nothing, got {:?}",
        after["name"]
    );

    // A delete this service has accepted leaves the record readable but the id
    // dead for mutation, inside the transient DELETING window and in the
    // terminal DELETED state it keeps.
    let (status, deleting) = call(&router, "DELETE", &format!("/prod/channels/{id}"), None).await;
    assert_eq!(status, 200, "DeleteChannel: got {status}: {deleting}");
    for phase in ["DELETING", "DELETED"] {
        let (status, body) =
            update_channel(&router, &id, json!({ "name": "renamed-after-delete" })).await;
        assert_eq!(
            status, 400,
            "case {phase:?}: expected 400 on a deleted channel, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {phase:?}: expected BadRequestException, got {body}"
        );
        assert!(
            error_message(&body).contains(&id),
            "case {phase:?}: expected the refusal to name the id it refused, got {:?}",
            error_message(&body)
        );
        wait_for_the_clock();
    }

    let (status, after) = describe(&router, &id).await;
    assert_eq!(status, 200, "DescribeChannel: got {status}: {after}");
    assert_eq!(
        after["name"],
        json!("baseline"),
        "expected the refused updates on the deleted channel to have written nothing, got {:?}",
        after["name"]
    );
}

/// Rule: a request this emulator cannot store is refused with the modelled
/// `BadRequestException` (400) before anything is written — a caller-chosen list
/// longer than the ceiling this emulator keeps on one channel, and the `DryRun`
/// member whose semantics this model documents nothing for (its doc text is the
/// generated placeholder), exactly as `CreateChannel` refuses both.
///
/// The list rows are each ceiling at its published bound and one entry past it.
#[tokio::test]
async fn an_update_this_emulator_cannot_store_is_refused() {
    // (case, the member, how many entries, whether it is accepted)
    let mut rows: Vec<(String, &str, usize, bool)> = Vec::new();
    for (member, limit) in [
        ("inputAttachments", MAX_INPUT_ATTACHMENTS),
        ("destinations", MAX_CHANNEL_DESTINATIONS),
    ] {
        rows.push((format!("{member} at the bound"), member, limit, true));
        rows.push((format!("{member} one past the bound"), member, limit + 1, false));
    }

    for (case, member, count, accepted) in rows {
        let router = medialive();
        let id = idle_channel(&router, &case, baseline_body()).await;

        let entries: Vec<Value> = (0..count)
            .map(|index| {
                if member == "destinations" {
                    json!({ "id": format!("destination-{index}") })
                } else {
                    json!({ "inputAttachmentName": format!("attachment-{index}"), "inputId": "1000000" })
                }
            })
            .collect();

        let (status, body) =
            update_channel(&router, &id, json!({ member: entries.clone() })).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected {count} entries to be accepted, got {status}: {body}"
            );
            assert_eq!(
                body["channel"][member],
                json!(entries),
                "case {case:?}: expected the accepted list to be stored, got {:?}",
                body["channel"][member]
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected {count} entries to be refused, got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body}"
            );
            wait_for_the_clock();
            let (status, after) = describe(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeChannel: got {status}: {after}"
            );
            assert_eq!(
                after["name"],
                json!("baseline"),
                "case {case:?}: expected the refused update to have written nothing, got {:?}",
                after["name"]
            );
        }
    }

    // `DryRun` is refused whatever else the request names, and the request that
    // omits it is the control that shows the refusal is about that member.
    let dry_run_table: [(&str, Value, u16); 3] = [
        ("dryRun true", json!({ "name": "dry", "dryRun": true }), 400),
        ("dryRun false", json!({ "name": "wet", "dryRun": false }), 200),
        ("dryRun omitted", json!({ "name": "plain" }), 200),
    ];
    for (case, patch, expected_status) in dry_run_table {
        let router = medialive();
        let id = idle_channel(&router, case, baseline_body()).await;
        let (status, body) = update_channel(&router, &id, patch).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(status, &body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body}"
            );
            wait_for_the_clock();
            let (_, after) = describe(&router, &id).await;
            assert_eq!(
                after["name"],
                json!("baseline"),
                "case {case:?}: expected the refused update to have written nothing, got {:?}",
                after["name"]
            );
        }
    }
}
