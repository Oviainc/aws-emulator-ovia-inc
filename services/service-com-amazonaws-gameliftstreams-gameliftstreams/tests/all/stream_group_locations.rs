// SPDX-License-Identifier: CC0-1.0

//! `AddStreamGroupLocations` end to end, over a SET of location sets.
//!
//! The rule under test: a stream group's location set is whatever the requests
//! that configured it asked for — `AddStreamGroupLocations` joins the locations
//! ONE call configures to the set of the group it names, each in `ACTIVATING`
//! until the clock has provisioned the capacity it asked for, and reports back
//! exactly those locations. The rows below differ in how many locations one call
//! configures, which capacity members they carry, and whether the request names
//! the group by id or by ARN; the refusal rows differ in which precondition the
//! request breaks.
//!
//! Every assertion reads a RESPONSE BODY, and every mutation is read back through
//! `GetStreamGroup`, so a handler that answered 2xx without persisting anything
//! fails here.

use serde_json::{json, Value};

use crate::stream_workflow::{
    active_stream_group, as_label, call, emulator, ready_application, stream_group, text,
    PAST_TRANSITION,
};

/// The `LocationStates` entry named `name`, or `Value::Null` when the reported set
/// does not hold it.
fn location(states: &Value, name: &str) -> Value {
    states
        .as_array()
        .into_iter()
        .flatten()
        .find(|state| state["LocationName"] == json!(name))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Every `LocationName` a `LocationStates`-shaped array reports, in order.
fn names(states: &Value) -> Vec<String> {
    states
        .as_array()
        .into_iter()
        .flatten()
        .map(|state| state["LocationName"].as_str().unwrap_or("?").to_string())
        .collect()
}

/// The capacity members `LocationConfiguration` carries and `LocationState`
/// echoes back (`smithy_query(query="shape", shape="LocationConfiguration")`).
///
/// A setting the configuration left out is echoed at its default of 0 ("Default is 0
/// when creating a stream group or adding a location" —
/// `LocationConfiguration::OnDemandCapacity`), so these members are always numbers on
/// a reported location state.
const CAPACITY_MEMBERS: [&str; 4] = [
    "AlwaysOnCapacity",
    "OnDemandCapacity",
    "TargetIdleCapacity",
    "MaximumCapacity",
];

/// What `member` of a `LocationConfiguration` asks for: the value it carries, or the
/// default of 0 when it carries none.
fn configured(configuration: &Value, member: &str) -> Value {
    json!(configuration[member].as_i64().unwrap_or(0))
}

/// The location set of a group is the union of what its requests configured, and
/// a location this call adds is `ACTIVATING` until the clock provisions it.
#[tokio::test]
async fn adding_locations_persists_exactly_the_locations_the_call_configures() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "locations-app").await;

    // (case, name the group by its ARN, the LocationConfigurations one call sends).
    // The primary location of every group below is us-west-2.
    let cases: [(&str, bool, Vec<Value>); 4] = [
        (
            "one location with always-on capacity",
            false,
            vec![json!({"LocationName": "us-east-1", "AlwaysOnCapacity": 2})],
        ),
        (
            "two locations in one call",
            false,
            vec![
                json!({"LocationName": "eu-west-1", "AlwaysOnCapacity": 0}),
                json!({"LocationName": "ap-south-1", "MaximumCapacity": 5}),
            ],
        ),
        (
            "every capacity member, group named by ARN",
            true,
            vec![json!({
                "LocationName": "eu-central-1",
                "AlwaysOnCapacity": 3,
                "TargetIdleCapacity": 1,
                "MaximumCapacity": 7,
            })],
        ),
        (
            "a location with no capacity configured",
            false,
            vec![json!({"LocationName": "sa-east-1"})],
        ),
    ];

    for (case, by_arn, configurations) in cases {
        let (group_id, group_arn) =
            active_stream_group(&router, "locations-group", &app_id, &["us-west-2"]).await;
        let identifier = if by_arn {
            as_label(&group_arn)
        } else {
            group_id.clone()
        };

        let (status, added) = call(
            &router,
            "POST",
            &format!("/streamgroups/{identifier}/locations"),
            Some(json!({"LocationConfigurations": configurations.clone()})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected AddStreamGroupLocations 200, got {status} {added}"
        );
        // `Identifier` resolves the group's `Arn` property, whichever form the
        // request named the group by.
        assert_eq!(
            text(&added, "/Identifier", case),
            group_arn,
            "case {case:?}: expected the stream group's ARN, got {added}"
        );
        // The call reports the locations IT configured, and nothing else.
        let expected: Vec<String> = configurations
            .iter()
            .map(|configuration| configuration["LocationName"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names(&added["Locations"]),
            expected,
            "case {case:?}: expected exactly the configured locations, got {added}"
        );
        for configuration in &configurations {
            let name = configuration["LocationName"].as_str().unwrap();
            let state = location(&added["Locations"], name);
            assert_eq!(
                state["Status"], "ACTIVATING",
                "case {case:?}: a location being prepared is ACTIVATING, got {state}"
            );
            for member in CAPACITY_MEMBERS {
                assert_eq!(
                    state[member],
                    configured(configuration, member),
                    "case {case:?}: expected {member} {}, got {state}",
                    configured(configuration, member)
                );
            }
            let requested = configuration["AlwaysOnCapacity"].as_i64().unwrap_or(0);
            assert_eq!(
                state["RequestedCapacity"],
                json!(requested),
                "case {case:?}: RequestedCapacity is the always-on capacity asked for, \
                 got {state}"
            );
            assert_eq!(
                state["AllocatedCapacity"],
                json!(0),
                "case {case:?}: a location that is not provisioned yet holds nothing, \
                 got {state}"
            );
        }

        // Read the group back: it holds its primary location AND the new ones, and
        // reports UPDATING_LOCATIONS while they are being prepared.
        let path = format!("/streamgroups/{group_id}");
        let (status, got) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetStreamGroup 200, got {status} {got}"
        );
        let mut whole_set = vec!["us-west-2".to_string()];
        whole_set.extend(expected.iter().cloned());
        assert_eq!(
            names(&got["LocationStates"]),
            whole_set,
            "case {case:?}: the group holds the union of what its requests configured, \
             got {got}"
        );
        assert_eq!(
            got["Status"], "UPDATING_LOCATIONS",
            "case {case:?}: a group with a location being prepared is UPDATING_LOCATIONS, \
             got {got}"
        );
        assert_eq!(
            location(&got["LocationStates"], "us-west-2")["Status"],
            json!("ACTIVE"),
            "case {case:?}: the locations already provisioned stay ACTIVE, got {got}"
        );

        // The clock owes each new location its capacity, and the group ACTIVE again.
        std::thread::sleep(PAST_TRANSITION);
        let (_, got) = call(&router, "GET", &path, None).await;
        assert_eq!(
            got["Status"], "ACTIVE",
            "case {case:?}: the clock owed ACTIVE by now, got {got}"
        );
        for configuration in &configurations {
            let name = configuration["LocationName"].as_str().unwrap();
            let state = location(&got["LocationStates"], name);
            assert_eq!(
                state["Status"],
                json!("ACTIVE"),
                "case {case:?}: location {name} is provisioned by now, got {got}"
            );
            let requested = configuration["AlwaysOnCapacity"].as_i64().unwrap_or(0);
            assert_eq!(
                state["AllocatedCapacity"],
                json!(requested),
                "case {case:?}: an ACTIVE location holds its always-on capacity, got {state}"
            );
        }

        // "This operation also copies the content files of all associated
        // applications to an internal S3 bucket at each location."
        let (_, application) = call(&router, "GET", &format!("/applications/{app_id}"), None).await;
        let replicated: Vec<String> = application["ReplicationStatuses"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|row| row["Location"].as_str().unwrap_or("?").to_string())
            .collect();
        for name in &expected {
            assert!(
                replicated.contains(name),
                "case {case:?}: the associated application must replicate to {name}, \
                 got {replicated:?}"
            );
        }
    }
}

/// One row of the replay table below: the case, the locations the group is
/// created with, the `LocationConfigurations` one call then sends, whether to name
/// the group by its ARN, the names the group holds afterwards and the status it
/// reports afterwards.
type ReplayCase<'a> = (
    &'a str,
    &'a [&'a str],
    Vec<Value>,
    bool,
    &'a [&'a str],
    &'a str,
);

/// Configuring a location the stream group ALREADY holds is served, not refused:
/// that entry leaves the stored location and the group exactly as they stand (no
/// second row, no capacity rewrite, no status move) and is still reported with its
/// current state, while any location of the SAME call that the group does not hold
/// yet is provisioned as usual.
///
/// The rows differ in how many held locations one call replays, whether the replay
/// carries the capacity the location holds, a different one or none at all,
/// whether the call also names a location the group lacks, and whether the group is
/// named by id or by ARN.
#[tokio::test]
async fn re_adding_a_location_the_group_holds_is_an_idempotent_no_op() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "idempotent-app").await;

    // `active_stream_group` creates each of its locations with
    // `AlwaysOnCapacity: 1`, so that is the stored capacity a replay must leave
    // alone.
    let cases: [ReplayCase; 5] = [
        (
            "the held location replayed at the capacity it holds",
            &["us-west-2"],
            vec![json!({"LocationName": "us-west-2", "AlwaysOnCapacity": 1})],
            false,
            &["us-west-2"],
            "ACTIVE",
        ),
        (
            "the held location replayed at a different capacity",
            &["us-west-2"],
            vec![json!({"LocationName": "us-west-2", "AlwaysOnCapacity": 7,
                        "MaximumCapacity": 9})],
            false,
            &["us-west-2"],
            "ACTIVE",
        ),
        (
            "the held location replayed with no capacity at all, named by ARN",
            &["eu-central-1"],
            vec![json!({"LocationName": "eu-central-1"})],
            true,
            &["eu-central-1"],
            "ACTIVE",
        ),
        (
            "the whole held set replayed",
            &["us-west-2", "ap-south-1"],
            vec![
                json!({"LocationName": "ap-south-1", "AlwaysOnCapacity": 1}),
                json!({"LocationName": "us-west-2", "AlwaysOnCapacity": 1}),
            ],
            false,
            &["us-west-2", "ap-south-1"],
            "ACTIVE",
        ),
        (
            "a held location alongside one the group lacks",
            &["us-west-2"],
            vec![
                json!({"LocationName": "us-west-2", "AlwaysOnCapacity": 1}),
                json!({"LocationName": "sa-east-1", "AlwaysOnCapacity": 2}),
            ],
            false,
            &["us-west-2", "sa-east-1"],
            "UPDATING_LOCATIONS",
        ),
    ];

    for (case, created_with, configurations, by_arn, whole_set, group_status) in cases {
        let (group_id, group_arn) =
            active_stream_group(&router, "idempotent-group", &app_id, created_with).await;
        let identifier = if by_arn {
            as_label(&group_arn)
        } else {
            group_id.clone()
        };
        let path = format!("/streamgroups/{group_id}");
        let (_, before) = call(&router, "GET", &path, None).await;

        let (status, added) = call(
            &router,
            "POST",
            &format!("/streamgroups/{identifier}/locations"),
            Some(json!({"LocationConfigurations": configurations.clone()})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replayed location is served, expected 200, got {status} {added}"
        );
        assert_eq!(
            text(&added, "/Identifier", case),
            group_arn,
            "case {case:?}: expected the stream group's ARN, got {added}"
        );
        // Every location the call named is reported, held or new alike.
        let reported: Vec<String> = configurations
            .iter()
            .map(|configuration| configuration["LocationName"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names(&added["Locations"]),
            reported,
            "case {case:?}: expected exactly the configured locations, got {added}"
        );
        for configuration in &configurations {
            let name = configuration["LocationName"].as_str().unwrap();
            let state = location(&added["Locations"], name);
            let held = created_with.contains(&name);
            assert_eq!(
                state["Status"],
                json!(if held { "ACTIVE" } else { "ACTIVATING" }),
                "case {case:?}: location {name} is reported with its current status, got {state}"
            );
            if held {
                // The stored configuration stands: adding is not how a held
                // location's capacity changes.
                assert_eq!(
                    state["AlwaysOnCapacity"],
                    json!(1),
                    "case {case:?}: location {name} keeps the capacity it holds, got {state}"
                );
                assert_eq!(
                    state["MaximumCapacity"],
                    json!(0),
                    "case {case:?}: location {name} keeps the capacity it holds, got {state}"
                );
                assert_eq!(
                    state["AllocatedCapacity"],
                    json!(1),
                    "case {case:?}: a provisioned location holds its always-on capacity, \
                     got {state}"
                );
            } else {
                for member in CAPACITY_MEMBERS {
                    assert_eq!(
                        state[member],
                        configured(configuration, member),
                        "case {case:?}: expected {member} {}, got {state}",
                        configured(configuration, member)
                    );
                }
            }
        }

        // Read the group back: one row per location it holds (no duplicate row for
        // a replayed one), and a status that moved only if the call joined
        // something.
        let (status, got) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetStreamGroup 200, got {status} {got}"
        );
        assert_eq!(
            names(&got["LocationStates"]),
            whole_set.iter().map(|n| n.to_string()).collect::<Vec<_>>(),
            "case {case:?}: expected one row per location the group holds, got {got}"
        );
        assert_eq!(
            got["Status"],
            json!(group_status),
            "case {case:?}: expected the group in {group_status}, got {got}"
        );
        for name in created_with {
            assert_eq!(
                location(&got["LocationStates"], name)["Status"],
                json!("ACTIVE"),
                "case {case:?}: the location {name} the group already held stays ACTIVE, got {got}"
            );
        }
        if whole_set.len() == created_with.len() {
            // A call that joined nothing changed nothing, so the group's own
            // `LastUpdatedAt` cannot have moved either.
            assert_eq!(
                got["LastUpdatedAt"], before["LastUpdatedAt"],
                "case {case:?}: a no-op add leaves LastUpdatedAt alone, got {got}"
            );
        }
    }
}

/// The preconditions of `AddStreamGroupLocations`: the group must exist, it must
/// be in `ACTIVE` status ("To add a location, the stream group must be in ACTIVE
/// status"), the same request must not name one location twice, and a capacity
/// the model bounds at `@range(min: 0)` is refused. Each refusal leaves the
/// group's location set as it was.
#[tokio::test]
async fn adding_locations_refuses_a_request_that_breaks_a_precondition() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "refusal-app").await;
    let (active_id, _) =
        active_stream_group(&router, "refusal-group", &app_id, &["us-west-2"]).await;

    // A group that is still ACTIVATING cannot take a location. Asserted first,
    // straight after the create: the clock settles the group one TRANSITION
    // later, so this case cannot wait behind the table below.
    let (activating_id, _) =
        stream_group(&router, "activating-group", &app_id, &["us-west-2"]).await;
    let (status, got) = call(
        &router,
        "POST",
        &format!("/streamgroups/{activating_id}/locations"),
        Some(json!({"LocationConfigurations": [
            {"LocationName": "us-east-1", "AlwaysOnCapacity": 1}
        ]})),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a group that is still ACTIVATING\": expected 400, got {status} {got}"
    );
    assert_eq!(
        got["__type"], "ValidationException",
        "case \"a group that is still ACTIVATING\": expected ValidationException, got {got}"
    );

    // (case, the group the request names, the LocationConfigurations, expected
    // status, the modelled error shape that status must carry).
    let cases: [(&str, &str, Value, u16, &str); 5] = [
        (
            "a group that does not exist",
            "sg-doesnotexist",
            json!([{"LocationName": "us-east-1", "AlwaysOnCapacity": 1}]),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the same new location configured twice",
            active_id.as_str(),
            json!([
                {"LocationName": "us-east-2", "AlwaysOnCapacity": 1},
                {"LocationName": "us-east-2", "AlwaysOnCapacity": 2},
            ]),
            400,
            "ValidationException",
        ),
        (
            "always-on capacity one below the published bound",
            active_id.as_str(),
            json!([{"LocationName": "us-east-1", "AlwaysOnCapacity": -1}]),
            400,
            "ValidationException",
        ),
        (
            "maximum capacity at i32::MIN",
            active_id.as_str(),
            json!([{"LocationName": "us-east-1", "MaximumCapacity": -2_147_483_648i64}]),
            400,
            "ValidationException",
        ),
        (
            "every capacity at the published lower bound",
            active_id.as_str(),
            json!([{
                "LocationName": "us-east-1",
                "AlwaysOnCapacity": 0,
                "TargetIdleCapacity": 0,
                "MaximumCapacity": 0,
            }]),
            200,
            "",
        ),
    ];

    for (case, identifier, configurations, expected, error) in cases {
        let (status, got) = call(
            &router,
            "POST",
            &format!("/streamgroups/{identifier}/locations"),
            Some(json!({"LocationConfigurations": configurations})),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {got}"
        );
        if expected != 200 {
            // The status carries the error shape the model declares for it.
            assert_eq!(
                got["__type"], error,
                "case {case:?}: expected {error}, got {got}"
            );
            // A refused request changes nothing: the group still holds exactly
            // the location it was created with.
            let (_, group) = call(&router, "GET", &format!("/streamgroups/{active_id}"), None).await;
            assert_eq!(
                names(&group["LocationStates"]),
                vec!["us-west-2".to_string()],
                "case {case:?}: a refused add must not change the location set, got {group}"
            );
        }
    }
}

/// One stream group's location set is bounded: the model caps one request's
/// `LocationConfigurations` at `length 1..100`, and the group's accumulated set
/// holds the same 100, after which the add is refused with the modelled
/// `ServiceQuotaExceededException` (HTTP 402).
#[tokio::test]
async fn the_location_set_of_one_stream_group_is_bounded() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "bound-app").await;
    let (group_id, _) = active_stream_group(&router, "bound-group", &app_id, &["us-west-2"]).await;
    let path = format!("/streamgroups/{group_id}/locations");

    // 99 more locations reach the bound exactly: 1 primary + 99 = 100.
    let batch: Vec<Value> = (0..99)
        .map(|index| json!({"LocationName": format!("loc-{index:02}"), "AlwaysOnCapacity": 1}))
        .collect();
    let (status, got) = call(
        &router,
        "POST",
        &path,
        Some(json!({"LocationConfigurations": batch})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the set filled to its bound\": expected 200, got {status} {got}"
    );
    assert_eq!(
        got["Locations"].as_array().map(Vec::len),
        Some(99),
        "case \"the set filled to its bound\": expected the 99 configured locations, got {got}"
    );

    // The clock settles those locations, so the next add is refused for the bound
    // and not for the group's status.
    std::thread::sleep(PAST_TRANSITION);
    let (status, got) = call(
        &router,
        "POST",
        &path,
        Some(json!({"LocationConfigurations": [{"LocationName": "loc-99", "AlwaysOnCapacity": 1}]})),
    )
    .await;
    assert_eq!(
        status, 402,
        "case \"one past the bound\": expected 402, got {status} {got}"
    );
    assert_eq!(
        got["__type"], "ServiceQuotaExceededException",
        "case \"one past the bound\": expected ServiceQuotaExceededException, got {got}"
    );
    let (_, group) = call(&router, "GET", &format!("/streamgroups/{group_id}"), None).await;
    assert_eq!(
        group["LocationStates"].as_array().map(Vec::len),
        Some(100),
        "case \"one past the bound\": the refused add must not change the set, got {group}"
    );
}

/// A group that is updating its locations keeps streaming from the locations it
/// has already provisioned, and a location that is still being prepared cannot
/// host a stream: "ACTIVATING: Amazon GameLift Streams is preparing the location.
/// You cannot stream from, scale the capacity of, or remove this location yet."
#[tokio::test]
async fn a_group_updating_its_locations_streams_only_from_the_ones_it_has_provisioned() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "streaming-app").await;
    let (group_id, _) =
        active_stream_group(&router, "streaming-group", &app_id, &["us-west-2"]).await;

    let (status, added) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/locations"),
        Some(json!({"LocationConfigurations": [
            {"LocationName": "us-east-1", "AlwaysOnCapacity": 1}
        ]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "AddStreamGroupLocations: expected 200, got {status} {added}"
    );

    // (case, the Locations the start asks for, expected status).
    let sessions = format!("/streamgroups/{group_id}/streamsessions");
    let cases: [(&str, Value, u16); 3] = [
        (
            "the location still being prepared",
            json!(["us-east-1"]),
            400,
        ),
        (
            "the location already provisioned",
            json!(["us-west-2"]),
            201,
        ),
        (
            "the prepared location ahead of the one being prepared",
            json!(["us-east-1", "us-west-2"]),
            201,
        ),
    ];
    for (case, locations, expected) in cases {
        let (status, got) = call(
            &router,
            "POST",
            &sessions,
            Some(json!({
                "Protocol": "WebRTC",
                "SignalRequest": "offer",
                "ApplicationIdentifier": app_id,
                "Locations": locations,
            })),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {got}"
        );
        if expected == 201 {
            // The session runs in the location that could host it.
            assert_eq!(
                got["Location"], "us-west-2",
                "case {case:?}: expected the provisioned location, got {got}"
            );
        }
    }
}
