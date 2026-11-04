// SPDX-License-Identifier: CC0-1.0

//! `RemoveStreamGroupLocations` end to end, over a SET of removal requests.
//!
//! The rule under test: a stream group releases exactly the locations one call
//! names that the group holds — each moving to `REMOVING` while the group reports
//! `UPDATING_LOCATIONS`, and leaving the group's set once the clock has released
//! its capacity — while the group's primary location is never released and a
//! location the group does not hold leaves the set exactly as it stands.
//!
//! The rows below differ in how many locations one call names, whether the names
//! are held, unheld or repeated, and whether the request names the group by id or
//! by ARN; the refusal rows differ in which precondition the request breaks.
//!
//! Every mutation is read back through `GetStreamGroup`, so a handler that
//! answered 2xx without persisting anything fails here.

use serde_json::{json, Value};

use crate::stream_workflow::{
    active_stream_group, as_label, call, emulator, ready_application, stream_group, REGION,
    PAST_TRANSITION,
};

/// Every `LocationName` a `LocationStates`-shaped array reports, in order.
fn names(states: &Value) -> Vec<String> {
    states
        .as_array()
        .into_iter()
        .flatten()
        .map(|state| state["LocationName"].as_str().unwrap_or("?").to_string())
        .collect()
}

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

/// The `locations` query the modelled `@httpQuery("locations")` list is carried in:
/// one `locations=<name>` parameter per entry, in the order the request names them.
fn locations_query(locations: &[&str]) -> String {
    locations
        .iter()
        .map(|location| format!("locations={location}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// One row of the table below: the case, the locations the group is created with
/// (the first is NOT the primary — the primary is the Region the create is
/// answered in, [`REGION`]), the locations one call asks to remove, whether the
/// request names the group by its ARN, and the set the group holds once the clock
/// has released what it owed.
type RemovalCase<'a> = (&'a str, &'a [&'a str], &'a [&'a str], bool, &'a [&'a str]);

/// A call releases exactly the locations it names that the group holds, and a
/// released location is gone from the set once the clock has released its capacity.
#[tokio::test]
async fn removing_locations_releases_exactly_the_held_locations_the_call_names() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "removal-app").await;

    let cases: [RemovalCase; 5] = [
        (
            "one held location",
            &[REGION, "us-east-1"],
            &["us-east-1"],
            false,
            &[REGION],
        ),
        (
            "two held locations in one call",
            &[REGION, "eu-west-1", "ap-south-1"],
            &["eu-west-1", "ap-south-1"],
            false,
            &[REGION],
        ),
        (
            "a held location beside one the group never held, named by ARN",
            &[REGION, "eu-central-1"],
            &["eu-central-1", "sa-east-1"],
            true,
            &[REGION],
        ),
        (
            "the same held location named twice",
            &[REGION, "us-east-2"],
            &["us-east-2", "us-east-2"],
            false,
            &[REGION],
        ),
        (
            "one of three held locations, leaving the others",
            &[REGION, "ca-central-1", "ap-northeast-1"],
            &["ca-central-1"],
            false,
            &[REGION, "ap-northeast-1"],
        ),
    ];

    for (case, created_with, removing, by_arn, remaining) in cases {
        let (group_id, group_arn) =
            active_stream_group(&router, "removal-group", &app_id, created_with).await;
        let identifier = if by_arn {
            as_label(&group_arn)
        } else {
            group_id.clone()
        };
        let path = format!("/streamgroups/{group_id}");

        let (status, body) = call(
            &router,
            "DELETE",
            &format!(
                "/streamgroups/{identifier}/locations?{}",
                locations_query(removing)
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected RemoveStreamGroupLocations 204, got {status} {body}"
        );
        assert_eq!(
            body,
            Value::Null,
            "case {case:?}: the modelled output is empty, got {body}"
        );

        // While the service releases the capacity, each released location is
        // REMOVING and the group is UPDATING_LOCATIONS; the locations the call did
        // not release stay ACTIVE.
        let (status, got) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetStreamGroup 200, got {status} {got}"
        );
        assert_eq!(
            got["Status"],
            json!("UPDATING_LOCATIONS"),
            "case {case:?}: a group releasing a location is UPDATING_LOCATIONS, got {got}"
        );
        for name in removing.iter().filter(|name| created_with.contains(name)) {
            let state = location(&got["LocationStates"], name);
            assert_eq!(
                state["Status"],
                json!("REMOVING"),
                "case {case:?}: location {name} is being released, got {state}"
            );
            assert_eq!(
                state["AllocatedCapacity"],
                json!(0),
                "case {case:?}: a location being released holds no capacity, got {state}"
            );
        }
        for name in remaining {
            assert_eq!(
                location(&got["LocationStates"], name)["Status"],
                json!("ACTIVE"),
                "case {case:?}: the location {name} the call left alone stays ACTIVE, got {got}"
            );
        }

        // The clock owes the release, after which the group holds exactly what the
        // call left it and is serving again.
        std::thread::sleep(PAST_TRANSITION);
        let (_, got) = call(&router, "GET", &path, None).await;
        assert_eq!(
            names(&got["LocationStates"]),
            remaining.iter().map(|n| n.to_string()).collect::<Vec<_>>(),
            "case {case:?}: expected exactly the locations the call left, got {got}"
        );
        assert_eq!(
            got["Status"],
            json!("ACTIVE"),
            "case {case:?}: the clock owed ACTIVE by now, got {got}"
        );
    }
}

/// A call that names only locations the group does not hold releases nothing: the
/// set, the group's status and its `LastUpdatedAt` all stand, which is also what
/// makes the operation's `@idempotent` retry safe.
#[tokio::test]
async fn removing_a_location_the_group_does_not_hold_changes_nothing() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "noop-app").await;

    // (case, the locations one call asks to remove).
    let cases: [(&str, &[&str]); 3] = [
        ("one location the group never held", &["sa-east-1"]),
        (
            "several locations the group never held",
            &["sa-east-1", "me-south-1"],
        ),
        (
            "the location a previous call already released",
            &["us-east-1"],
        ),
    ];

    for (case, removing) in cases {
        let (group_id, _) =
            active_stream_group(&router, "noop-group", &app_id, &[REGION, "us-east-1"]).await;
        let path = format!("/streamgroups/{group_id}");
        let locations = format!(
            "/streamgroups/{group_id}/locations?{}",
            locations_query(removing)
        );
        let expected: Vec<String> = if case == "the location a previous call already released" {
            // Release it once, let the clock finish, then replay the same call.
            let (status, body) = call(&router, "DELETE", &locations, None).await;
            assert_eq!(
                status, 204,
                "case {case:?}: the first removal expected 204, got {status} {body}"
            );
            std::thread::sleep(PAST_TRANSITION);
            vec![REGION.to_string()]
        } else {
            vec![REGION.to_string(), "us-east-1".to_string()]
        };
        let (_, before) = call(&router, "GET", &path, None).await;

        let (status, body) = call(&router, "DELETE", &locations, None).await;
        assert_eq!(
            status, 204,
            "case {case:?}: a removal that releases nothing is served, got {status} {body}"
        );
        let (_, got) = call(&router, "GET", &path, None).await;
        assert_eq!(
            names(&got["LocationStates"]),
            expected,
            "case {case:?}: the location set must stand, got {got}"
        );
        assert_eq!(
            got["Status"],
            json!("ACTIVE"),
            "case {case:?}: a group that released nothing is still ACTIVE, got {got}"
        );
        assert_eq!(
            got["LastUpdatedAt"], before["LastUpdatedAt"],
            "case {case:?}: a removal that released nothing leaves LastUpdatedAt alone, got {got}"
        );
    }
}

/// The preconditions of `RemoveStreamGroupLocations`: the group must exist, it must
/// be in `ACTIVE` status ("To remove a location, the stream group must be in ACTIVE
/// status"), and its primary location — "the Amazon Web Services Region location
/// where you initially created this stream group" — can never be released. Each
/// refusal leaves the group's location set exactly as it was.
#[tokio::test]
async fn removing_a_location_refuses_a_request_that_breaks_a_precondition() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "refuse-app").await;
    let (group_id, group_arn) =
        active_stream_group(&router, "refuse-group", &app_id, &[REGION, "us-east-1"]).await;

    // A group that is still ACTIVATING cannot release a location. Asserted first,
    // straight after the create: the clock settles the group one TRANSITION later,
    // so this case cannot wait behind the table below.
    let (activating_id, _) =
        stream_group(&router, "activating-group", &app_id, &[REGION, "us-east-1"]).await;
    let (status, got) = call(
        &router,
        "DELETE",
        &format!("/streamgroups/{activating_id}/locations?locations=us-east-1"),
        None,
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

    // (case, the group the request names, the locations it asks to remove, the
    // expected status, the modelled error shape that status must carry).
    let cases: [(&str, String, &[&str], u16, &str); 5] = [
        (
            "the primary location on its own",
            group_id.clone(),
            &[REGION],
            400,
            "ValidationException",
        ),
        (
            "the primary location beside a held location",
            group_id.clone(),
            &["us-east-1", REGION],
            400,
            "ValidationException",
        ),
        (
            "the primary location, group named by ARN",
            as_label(&group_arn),
            &[REGION],
            400,
            "ValidationException",
        ),
        (
            "a group that does not exist",
            "sg-doesnotexist".to_string(),
            &["us-east-1"],
            404,
            "ResourceNotFoundException",
        ),
        (
            "an identifier that is an ARN of another resource type",
            as_label(&group_arn.replace("streamgroup/", "application/")),
            &["us-east-1"],
            400,
            "ValidationException",
        ),
    ];

    for (case, identifier, removing, expected, error) in cases {
        let (status, got) = call(
            &router,
            "DELETE",
            &format!(
                "/streamgroups/{identifier}/locations?{}",
                locations_query(removing)
            ),
            None,
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {got}"
        );
        // The status carries the error shape the model declares for it.
        assert_eq!(
            got["__type"], error,
            "case {case:?}: expected {error}, got {got}"
        );
        if removing.contains(&REGION) {
            // The refusal names the location the request asked to remove and the
            // stream group it addressed, and points at the capacity setting that
            // stands in for removing the primary location.
            let expected_message = format!(
                "The primary service location '{REGION}' cannot be removed from stream \
                 group '{group_arn}', set capacity to zero instead"
            );
            let message = got["Message"].as_str().unwrap_or_default();
            assert!(
                message.starts_with(&expected_message),
                "case {case:?}: expected {expected_message:?}, got {message:?}"
            );
        }
        // A refused request releases nothing: the group still holds both of the
        // locations it was created with, each ACTIVE.
        let (_, group) = call(&router, "GET", &format!("/streamgroups/{group_id}"), None).await;
        assert_eq!(
            names(&group["LocationStates"]),
            vec![REGION.to_string(), "us-east-1".to_string()],
            "case {case:?}: a refused removal must not change the location set, got {group}"
        );
        assert_eq!(
            group["Status"],
            json!("ACTIVE"),
            "case {case:?}: a refused removal must not move the group's status, got {group}"
        );
    }
}

/// Releasing a location releases what ran there: the stream sessions it hosted end,
/// and the content the associated applications had replicated to it is deleted.
#[tokio::test]
async fn releasing_a_location_releases_the_streams_and_the_content_it_held() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "release-app").await;
    let (group_id, _) =
        active_stream_group(&router, "release-group", &app_id, &[REGION, "us-east-1"]).await;

    // "This operation also copies the content files of all associated applications
    // to an internal S3 bucket at each location" (`AddStreamGroupLocations`), so the
    // application starts out replicated to both locations.
    let application = format!("/applications/{app_id}");
    let (_, before) = call(&router, "GET", &application, None).await;
    let replicated: Vec<String> = before["ReplicationStatuses"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| row["Location"].as_str().unwrap_or("?").to_string())
        .collect();
    assert!(
        replicated.contains(&"us-east-1".to_string()),
        "case \"before the removal\": the application replicates to us-east-1, got {replicated:?}"
    );

    let (status, got) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/streamsessions"),
        Some(json!({
            "Protocol": "WebRTC",
            "SignalRequest": "offer",
            "ApplicationIdentifier": app_id,
            "Locations": ["us-east-1"],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"a session in the location to release\": expected 201, got {status} {got}"
    );
    let session_arn = got["Arn"].as_str().unwrap_or_default().to_string();
    let session_id = session_arn
        .rsplit('/')
        .next()
        .expect("a session ARN ends in the session id")
        .to_string();

    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/streamgroups/{group_id}/locations?locations=us-east-1"),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "case \"releasing the location\": expected 204, got {status} {body}"
    );

    // The compute the session ran on is being released, so the session is ending.
    let session_path = format!("/streamgroups/{group_id}/streamsessions/{session_id}");
    let (_, session) = call(&router, "GET", &session_path, None).await;
    assert_eq!(
        session["Status"], "TERMINATING",
        "case \"a session in a released location\": expected TERMINATING, got {session}"
    );
    assert_eq!(
        session["StatusReason"], "apiTerminated",
        "case \"a session in a released location\": expected apiTerminated, got {session}"
    );

    // "Amazon GameLift Streams also deletes the content files of all associated
    // applications that were in Amazon GameLift Streams's internal Amazon S3 bucket
    // at this location."
    let (_, after) = call(&router, "GET", &application, None).await;
    let replicated: Vec<String> = after["ReplicationStatuses"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| row["Location"].as_str().unwrap_or("?").to_string())
        .collect();
    assert_eq!(
        replicated,
        vec![REGION.to_string()],
        "case \"after the removal\": the released location's content is deleted, \
         got {replicated:?}"
    );

    std::thread::sleep(PAST_TRANSITION);
    let (_, session) = call(&router, "GET", &session_path, None).await;
    assert_eq!(
        session["Status"], "TERMINATED",
        "case \"a session in a released location\": the clock owed TERMINATED by now, \
         got {session}"
    );
    // A released location can no longer start a stream session: it is not part of
    // the group at all.
    let (status, got) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/streamsessions"),
        Some(json!({
            "Protocol": "WebRTC",
            "SignalRequest": "offer",
            "ApplicationIdentifier": app_id,
            "Locations": ["us-east-1"],
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a stream in a released location\": expected 400, got {status} {got}"
    );
}
