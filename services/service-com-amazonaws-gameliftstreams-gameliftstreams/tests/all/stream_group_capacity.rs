// SPDX-License-Identifier: CC0-1.0

//! The capacity projection a stream-group read reports, and the applications it
//! reports the group is associated with.
//!
//! The rule under test: a stream group reports, for EVERY location it holds, the
//! whole capacity projection of that location — each capacity SETTING as a number
//! (the value its configuration asked for, or the service's default of 0 for one no
//! configuration named: "Default is 0 when creating a stream group or adding a
//! location", `LocationConfiguration::OnDemandCapacity`), the always-on capacity most
//! recently requested, what is allocated to the location and idle in it right now,
//! and the CIDR block of the service VPC that location's capacity runs in
//! (`LocationState::InternalVpcIpv4CidrBlock`: "The CIDR block of the service VPC for
//! this location") — and it reports the same projection through every operation that
//! answers with `LocationStates`.
//!
//! The tables below span that class: they differ in WHICH capacity settings a
//! location's configuration carries, in their values (including the published lower
//! bound of each and a value far above the recorded one), in how many locations one
//! group holds, and in whether a session is running in a location. The second table
//! is the other half of a group's read projection — the applications it is associated
//! with and the default among them — over groups that do and do not get a default
//! application at creation, named by both identifier forms.
//!
//! Every assertion reads a RESPONSE BODY, and every mutation is read back through
//! `GetStreamGroup`, so a handler that answered 2xx without persisting anything fails
//! here.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::stream_workflow::{
    as_label, call, emulator, ready_application, text, PAST_TRANSITION, REGION,
};

/// The capacity SETTINGS a `LocationConfiguration` carries and a `LocationState`
/// reports back, from `smithy_query(query="shape", shape="LocationConfiguration")`:
/// four optional integers, each `@range(min: 0)`.
const CAPACITY_SETTINGS: [&str; 4] = [
    "AlwaysOnCapacity",
    "OnDemandCapacity",
    "TargetIdleCapacity",
    "MaximumCapacity",
];

/// The capacities a `LocationState` DERIVES from those settings and the location's
/// own state, from `smithy_query(query="shape", shape="LocationState")`.
const DERIVED_CAPACITIES: [&str; 3] = [
    "RequestedCapacity",
    "AllocatedCapacity",
    "IdleCapacity",
];

/// Create a stream group with exactly `configurations` as its locations and answer
/// its id, ARN and the create response. The group is `ACTIVATING` on return.
///
/// `pub(crate)` so the other suites in this target configure a location's capacity
/// through this one helper rather than growing a second copy of it.
pub(crate) async fn group_with_locations(
    router: &MultiServiceRouter,
    description: &str,
    application: &str,
    configurations: &[Value],
) -> (String, String, Value) {
    let (status, created) = call(
        router,
        "POST",
        "/streamgroups",
        Some(json!({
            "Description": description,
            "StreamClass": "gen5n_high",
            "DefaultApplicationIdentifier": application,
            "LocationConfigurations": configurations,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {description:?}: expected CreateStreamGroup 201, got {status} {created}"
    );
    (
        text(&created, "/Id", description),
        text(&created, "/Arn", description),
        created,
    )
}

/// The `LocationStates` entry named `name`.
fn location(states: &Value, name: &str, case: &str) -> Value {
    states
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: LocationStates is not a list in {states}"))
        .iter()
        .find(|state| state["LocationName"] == json!(name))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no state for location {name} in {states}"))
}

/// What a capacity setting of `configuration` asks for: the number it carries, or the
/// service default of 0 when it carries none.
fn asked_for(configuration: &Value, setting: &str) -> i64 {
    configuration[setting].as_i64().unwrap_or(0)
}

/// Whether `value` is a well-formed IPv4 CIDR block, i.e. a value the model's
/// `Ipv4CidrBlock` pattern accepts: four decimal octets 0..=255 and a prefix length
/// 0..=32 (`smithy_query(query="shape", shape="Ipv4CidrBlock")`).
fn is_ipv4_cidr_block(value: &str) -> bool {
    let Some((address, prefix)) = value.split_once('/') else {
        return false;
    };
    let octets: Vec<&str> = address.split('.').collect();
    octets.len() == 4
        && octets.iter().all(|octet| octet.parse::<u8>().is_ok())
        && prefix.parse::<u8>().is_ok_and(|length| length <= 32)
}

/// Assert the whole capacity projection of one location, against the configuration
/// that asked for it: every setting is the number it asked for (0 by default), the
/// requested capacity is the always-on capacity it asked for, an `ACTIVE` location
/// holds that always-on capacity (it "is allocated to you and is never released back
/// to the service") while one still being prepared holds nothing, `running` sessions
/// are not idle, and the location reports the service VPC it runs in.
fn assert_projection(state: &Value, configuration: &Value, active: bool, running: i64, case: &str) {
    for setting in CAPACITY_SETTINGS {
        assert_eq!(
            state[setting].as_i64(),
            Some(asked_for(configuration, setting)),
            "case {case:?}: {setting} must be the number the configuration asked for \
             ({}), got {state}",
            asked_for(configuration, setting)
        );
    }
    for derived in DERIVED_CAPACITIES {
        assert!(
            state[derived].is_number(),
            "case {case:?}: {derived} must be reported as a number, got {state}"
        );
    }
    let requested = asked_for(configuration, "AlwaysOnCapacity");
    let allocated = if active { requested } else { 0 };
    assert_eq!(
        state["RequestedCapacity"].as_i64(),
        Some(requested),
        "case {case:?}: RequestedCapacity is the always-on capacity requested \
         ({requested}), got {state}"
    );
    assert_eq!(
        state["AllocatedCapacity"].as_i64(),
        Some(allocated),
        "case {case:?}: expected AllocatedCapacity {allocated}, got {state}"
    );
    assert_eq!(
        state["IdleCapacity"].as_i64(),
        Some((allocated - running).max(0)),
        "case {case:?}: idle capacity is what is allocated less the {running} session(s) \
         running, got {state}"
    );
    let cidr = state["InternalVpcIpv4CidrBlock"].as_str().unwrap_or_else(|| {
        panic!("case {case:?}: InternalVpcIpv4CidrBlock must be reported, got {state}")
    });
    assert!(
        is_ipv4_cidr_block(cidr),
        "case {case:?}: the service VPC block must be an IPv4 CIDR block, got {cidr:?}"
    );
}

/// One row of the capacity table: the case, and the `LocationConfigurations` the
/// group is created with.
type CapacityCase<'a> = (&'a str, Vec<Value>);

#[tokio::test]
async fn a_stream_group_read_reports_the_whole_capacity_projection_of_every_location() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "capacity-app").await;

    let cases: [CapacityCase; 7] = [
        (
            "always-on capacity alone",
            vec![json!({"LocationName": REGION, "AlwaysOnCapacity": 2})],
        ),
        (
            "on-demand capacity alone",
            vec![json!({"LocationName": REGION, "OnDemandCapacity": 3})],
        ),
        (
            "a maximum with a target idle level",
            vec![json!({
                "LocationName": REGION,
                "MaximumCapacity": 5,
                "TargetIdleCapacity": 1,
            })],
        ),
        (
            "every setting at the published lower bound",
            vec![json!({
                "LocationName": REGION,
                "AlwaysOnCapacity": 0,
                "OnDemandCapacity": 0,
                "TargetIdleCapacity": 0,
                "MaximumCapacity": 0,
            })],
        ),
        (
            "no capacity setting at all",
            vec![json!({"LocationName": REGION})],
        ),
        (
            "an always-on capacity far above the usual one",
            vec![json!({"LocationName": REGION, "AlwaysOnCapacity": 50})],
        ),
        (
            "two locations, each with its own capacity",
            vec![
                json!({"LocationName": REGION, "AlwaysOnCapacity": 1}),
                json!({"LocationName": "us-east-1", "MaximumCapacity": 4,
                       "TargetIdleCapacity": 2}),
            ],
        ),
    ];

    for (index, (case, configurations)) in cases.into_iter().enumerate() {
        // `Description` is modelled `pattern ^[a-zA-Z0-9-_.!+@/][a-zA-Z0-9-_.!+@/ ]*$`,
        // so the row's index names the group rather than its prose label.
        let (group_id, group_arn, created) = group_with_locations(
            &router,
            &format!("capacity-group-{index}"),
            &app_arn,
            &configurations,
        )
        .await;

        // The create answers the same projection it will read back: nothing is
        // provisioned yet, so the locations hold nothing.
        for configuration in &configurations {
            let name = configuration["LocationName"].as_str().unwrap();
            let state = location(&created["LocationStates"], name, case);
            assert_projection(&state, configuration, false, 0, case);
        }

        // The clock provisions the locations one transition later; the read reports
        // the same projection, now with the always-on capacity allocated.
        std::thread::sleep(PAST_TRANSITION);
        let (status, group) = call(&router, "GET", &format!("/streamgroups/{group_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetStreamGroup 200, got {status} {group}"
        );
        for configuration in &configurations {
            let name = configuration["LocationName"].as_str().unwrap();
            let state = location(&group["LocationStates"], name, case);
            assert_projection(&state, configuration, true, 0, case);
        }

        // Naming the group by its ARN reads the same resource, so it reports the
        // same projection: a read cannot depend on the identifier form.
        let (status, by_arn) = call(
            &router,
            "GET",
            &format!("/streamgroups/{}", as_label(&group_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetStreamGroup by ARN 200, got {status} {by_arn}"
        );
        assert_eq!(
            by_arn["LocationStates"], group["LocationStates"],
            "case {case:?}: the projection must not depend on the identifier form, got \
             {by_arn}"
        );
    }
}

#[tokio::test]
async fn a_running_session_is_subtracted_from_the_idle_capacity_of_its_location() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "idle-capacity-app").await;
    let configuration = json!({"LocationName": REGION, "AlwaysOnCapacity": 2});
    let (group_id, _, _) = group_with_locations(
        &router,
        "idle-capacity-group",
        &app_arn,
        std::slice::from_ref(&configuration),
    )
    .await;
    std::thread::sleep(PAST_TRANSITION);
    let path = format!("/streamgroups/{group_id}");

    let (status, session) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/streamsessions"),
        Some(json!({
            "Protocol": "WebRTC",
            "SignalRequest": "offer",
            "ApplicationIdentifier": app_id,
            "Locations": [REGION],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"a session placed in the location\": expected StartStreamSession 201, got \
         {status} {session}"
    );

    // The session is live, so one of the location's two allocated streams is in use
    // and only one is idle. Read straight away: the allocation itself is unchanged —
    // always-on capacity "is allocated to you and is never released back to the
    // service" — it is the IDLE half of the projection that moves.
    let case = "a session placed in the location";
    let (_, group) = call(&router, "GET", &path, None).await;
    assert_projection(&location(&group["LocationStates"], REGION, case), &configuration, true, 1, case);

    // Once the clock has taken that session to a terminal status it occupies nothing,
    // so the whole allocation is idle again.
    std::thread::sleep(PAST_TRANSITION);
    let case = "the session has reached a terminal status";
    let (_, group) = call(&router, "GET", &path, None).await;
    assert_projection(&location(&group["LocationStates"], REGION, case), &configuration, true, 0, case);
}

/// One row of the association table: the case, whether the group is created with a
/// default application, whether the association names the applications by ARN rather
/// than by bare id, and how many applications one call associates.
type AssociationCase<'a> = (&'a str, bool, bool, usize);

#[tokio::test]
async fn a_stream_group_read_reports_the_applications_it_is_associated_with() {
    let router = emulator();

    let cases: [AssociationCase; 4] = [
        ("one application, by id, no default yet", false, false, 1),
        ("one application, by ARN, no default yet", false, true, 1),
        ("two applications, by id, no default yet", false, false, 2),
        ("one application onto a group that has a default", true, true, 1),
    ];

    for (index, (case, with_default, by_arn, count)) in cases.into_iter().enumerate() {
        // The application the group may be created with, and the ones one
        // `AssociateApplications` call then links.
        let (default_id, default_arn) =
            ready_application(&router, &format!("assoc-default-{index}")).await;
        let mut linked: Vec<(String, String)> = Vec::new();
        for ordinal in 0..count {
            linked.push(ready_application(&router, &format!("assoc-{index}-{ordinal}")).await);
        }

        let (status, created) = call(
            &router,
            "POST",
            "/streamgroups",
            Some(if with_default {
                json!({
                    "Description": format!("assoc-group-{index}"),
                    "StreamClass": "gen5n_high",
                    "DefaultApplicationIdentifier": default_id,
                    "LocationConfigurations": [{"LocationName": REGION, "AlwaysOnCapacity": 1}],
                })
            } else {
                json!({
                    "Description": format!("assoc-group-{index}"),
                    "StreamClass": "gen5n_high",
                    "LocationConfigurations": [{"LocationName": REGION, "AlwaysOnCapacity": 1}],
                })
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateStreamGroup 201, got {status} {created}"
        );
        let group_id = text(&created, "/Id", case);
        // "The stream group must be in ACTIVE status" (`AssociateApplications`).
        std::thread::sleep(PAST_TRANSITION);

        let identifiers: Vec<String> = linked
            .iter()
            .map(|(id, arn)| if by_arn { arn.clone() } else { id.clone() })
            .collect();
        let (status, associated) = call(
            &router,
            "POST",
            &format!("/streamgroups/{group_id}/associations"),
            Some(json!({"ApplicationIdentifiers": identifiers})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected AssociateApplications 200, got {status} {associated}"
        );

        // Read the group back: it reports every application it is associated with —
        // the default it was created with first, then the ones this call linked — and
        // the default among them. "If a stream group does not already have a linked
        // application, Amazon GameLift Streams will automatically assign the first
        // application provided in ApplicationIdentifiers as the default."
        let mut expected: Vec<String> = Vec::new();
        if with_default {
            expected.push(default_arn.clone());
        }
        expected.extend(linked.iter().map(|(_, arn)| arn.clone()));
        let expected_default = if with_default {
            (default_id.clone(), default_arn.clone())
        } else {
            linked[0].clone()
        };

        let (status, group) = call(&router, "GET", &format!("/streamgroups/{group_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetStreamGroup 200, got {status} {group}"
        );
        assert_eq!(
            group["AssociatedApplications"],
            json!(expected),
            "case {case:?}: expected the associated application ARNs {expected:?}, got {group}"
        );
        assert_eq!(
            group["DefaultApplication"],
            json!({"Id": expected_default.0, "Arn": expected_default.1}),
            "case {case:?}: expected the default application {expected_default:?}, got {group}"
        );
        // A second read reports the same thing: reading a group changes nothing about
        // the associations it holds.
        let (_, again) = call(&router, "GET", &format!("/streamgroups/{group_id}"), None).await;
        assert_eq!(
            (&again["AssociatedApplications"], &again["DefaultApplication"]),
            (&group["AssociatedApplications"], &group["DefaultApplication"]),
            "case {case:?}: a read must not change what a later read sees, got {again}"
        );
    }
}
