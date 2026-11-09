// SPDX-License-Identifier: CC0-1.0

//! `UpdateStreamGroup`: what one `PATCH /streamgroups/{Identifier}` persists, and what
//! it refuses.
//!
//! Every assertion reads a RESPONSE BODY, and every success is read back a second time
//! through `GetStreamGroup`, so a handler that answered 200 without writing to the store
//! fails here.
//!
//! The rules under test, from this service's Smithy model:
//!
//! * `UpdateStreamGroup` "updates the configuration settings for an Amazon GameLift
//!   Streams stream group resource … You can change the description, the set of
//!   locations, and the requested capacity of a stream group per location … To update a
//!   stream group, it must be in ACTIVE status … If the request is successful, Amazon
//!   GameLift Streams returns the complete updated metadata for the stream group."
//! * `LocationConfiguration`: "To update the capacity for an existing stream group, call
//!   UpdateStreamGroup. To add a new location and specify its capacity, call
//!   AddStreamGroupLocations."
//! * `OnDemandCapacity`: "This parameter cannot be used with `MaximumCapacity` or
//!   `TargetIdleCapacity` in the same location configuration."
//! * `DefaultApplicationIdentifier`: "The application that you specify must be in READY
//!   status … this parameter only sets the default application in a stream group. To
//!   associate a new application to an existing stream group, you must use
//!   AssociateApplications."

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::stream_workflow::{
    active_stream_group, application_body, as_label, call, emulator, ready_application, text,
    PAST_TRANSITION,
};

/// The capacity members of `LocationConfiguration`, from `smithy_query(query="shape",
/// shape="LocationConfiguration")`: each one is an optional integer `@range(min: 0)`.
const CAPACITY_MEMBERS: [&str; 4] = [
    "AlwaysOnCapacity",
    "OnDemandCapacity",
    "TargetIdleCapacity",
    "MaximumCapacity",
];

/// The `LocationStates` entry for `location`.
fn location_state(value: &Value, location: &str, case: &str) -> Value {
    value["LocationStates"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: LocationStates is not a list in {value}"))
        .iter()
        .find(|state| state["LocationName"] == location)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no state for location {location} in {value}"))
}

/// Assert one location reports exactly the capacities `expected` holds, and the derived
/// capacities that follow from its always-on capacity in an `ACTIVE` group: allocated
/// always-on capacity "is allocated to you and is never released back to the service",
/// and nothing is streaming, so all of it is idle.
///
/// A capacity setting no configuration ever named is reported at its default of 0
/// ("Default is 0 when creating a stream group or adding a location" —
/// `LocationConfiguration::OnDemandCapacity`), so every capacity member of a reported
/// location state is a number and `expected` holding no entry means zero.
fn assert_capacities(state: &Value, expected: &BTreeMap<&str, i64>, location: &str, case: &str) {
    for member in CAPACITY_MEMBERS {
        let wanted = expected.get(member).copied().unwrap_or(0);
        assert_eq!(
            state[member].as_i64(),
            Some(wanted),
            "case {case:?}: location {location} {member}: expected {wanted}, got {state}"
        );
    }
    let always_on = expected.get("AlwaysOnCapacity").copied().unwrap_or(0);
    for member in ["RequestedCapacity", "AllocatedCapacity", "IdleCapacity"] {
        assert_eq!(
            state[member].as_i64(),
            Some(always_on),
            "case {case:?}: location {location} {member}: expected {always_on}, got {state}"
        );
    }
    assert_eq!(
        state["Status"], "ACTIVE",
        "case {case:?}: location {location} of an ACTIVE group is ACTIVE, got {state}"
    );
}

#[tokio::test]
async fn an_update_persists_every_description_the_model_allows() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "description-app").await;
    let (group_id, group_arn) =
        active_stream_group(&router, "original-description", &app_arn, &["us-west-2"]).await;
    let path = format!("/streamgroups/{group_id}");

    let (status, before) = call(&router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "GetStreamGroup before the update: expected 200, got {status} {before}"
    );
    let created_at = before["CreatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("CreatedAt is not a timestamp in {before}"));
    let expires_at = before["ExpiresAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("ExpiresAt is not a timestamp in {before}"));
    let mut last_seen = before["LastUpdatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("LastUpdatedAt is not a timestamp in {before}"));

    // The rows differ along what `Description` generalises over — length and character
    // class — and include both published length bounds and one value past the upper
    // bound (`smithy_query(query="operation", operation="UpdateStreamGroup")`:
    // `Description`, len 1..80, pattern `^[a-zA-Z0-9-_.!+@/][a-zA-Z0-9-_.!+@/ ]*$`).
    let cases: Vec<(String, u16)> = vec![
        ("x".to_string(), 200),
        ("updated-description".to_string(), 200),
        ("A.b!+@/-_ and 123".to_string(), 200),
        ("d".repeat(80), 200),
        ("d".repeat(81), 400),
    ];

    let mut persisted = "original-description".to_string();
    for (description, expected_status) in cases {
        let case = format!("Description of {} character(s)", description.len());
        let (status, updated) = call(
            &router,
            "PATCH",
            &path,
            Some(json!({"Description": description})),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {updated}"
        );
        if expected_status == 200 {
            persisted = description.clone();
            assert_eq!(
                updated["Description"],
                Value::String(description.clone()),
                "case {case:?}: the response carries the new description, got {updated}"
            );
            // The complete updated metadata: identity and creation facts do not move.
            assert_eq!(
                text(&updated, "/Arn", &case),
                group_arn,
                "case {case:?}: the ARN must not change, got {updated}"
            );
            assert_eq!(
                updated["Id"],
                Value::String(group_id.clone()),
                "case {case:?}: the id must not change, got {updated}"
            );
            assert_eq!(
                updated["StreamClass"], "gen5n_high",
                "case {case:?}: the stream class must not change, got {updated}"
            );
            assert_eq!(
                updated["Status"], "ACTIVE",
                "case {case:?}: an updated ACTIVE group stays ACTIVE, got {updated}"
            );
            assert_eq!(
                updated["AssociatedApplications"],
                json!([app_arn]),
                "case {case:?}: the associations must not change, got {updated}"
            );
            assert_eq!(
                updated["CreatedAt"].as_f64(),
                Some(created_at),
                "case {case:?}: CreatedAt must not move, got {updated}"
            );
            assert_eq!(
                updated["ExpiresAt"].as_f64(),
                Some(expires_at),
                "case {case:?}: ExpiresAt must not move, got {updated}"
            );
            let last_updated = updated["LastUpdatedAt"]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: LastUpdatedAt missing in {updated}"));
            assert!(
                last_updated >= last_seen,
                "case {case:?}: LastUpdatedAt must not go backwards, expected at least \
                 {last_seen}, got {last_updated}"
            );
            last_seen = last_updated;
        }

        // The store, not the response: a refused update leaves the previous value.
        let (status, got) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetStreamGroup 200, got {status} {got}"
        );
        assert_eq!(
            got["Description"],
            Value::String(persisted.clone()),
            "case {case:?}: GetStreamGroup must report the persisted description, got {got}"
        );
    }
}

#[tokio::test]
async fn a_capacity_update_replaces_only_the_members_it_names() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "capacity-app").await;
    let (group_id, _) = active_stream_group(
        &router,
        "capacity-group",
        &app_arn,
        &["us-west-2", "us-east-1"],
    )
    .await;
    let path = format!("/streamgroups/{group_id}");

    // One row per capacity member of `LocationConfiguration`, over two locations, and
    // including the published lower bound of the `@range(min: 0)` on each of them. Each
    // row names ONE member, so what the row does not name must keep the value the
    // location already had.
    let rows: [(&str, &str, i64); 6] = [
        ("us-west-2", "AlwaysOnCapacity", 3),
        ("us-east-1", "OnDemandCapacity", 4),
        ("us-west-2", "MaximumCapacity", 5),
        ("us-west-2", "TargetIdleCapacity", 2),
        ("us-west-2", "AlwaysOnCapacity", 0),
        ("us-east-1", "OnDemandCapacity", 0),
    ];

    // What `CreateStreamGroup` stored for both locations.
    let mut expected: BTreeMap<&str, BTreeMap<&str, i64>> = BTreeMap::from([
        ("us-west-2", BTreeMap::from([("AlwaysOnCapacity", 1)])),
        ("us-east-1", BTreeMap::from([("AlwaysOnCapacity", 1)])),
    ]);

    for (location, member, value) in rows {
        let case = format!("{location} {member} = {value}");
        expected
            .get_mut(location)
            .expect("the row names a location the group holds")
            .insert(member, value);

        let (status, updated) = call(
            &router,
            "PATCH",
            &path,
            Some(json!({
                "LocationConfigurations": [{"LocationName": location, member: value}],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {updated}"
        );
        let (status, got) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetStreamGroup 200, got {status} {got}"
        );

        for (name, capacities) in &expected {
            assert_capacities(
                &location_state(&updated, name, &case),
                capacities,
                name,
                &format!("{case} (UpdateStreamGroup response)"),
            );
            assert_capacities(
                &location_state(&got, name, &case),
                capacities,
                name,
                &format!("{case} (GetStreamGroup response)"),
            );
        }
    }

    // The `OnDemandCapacity` exclusion is a property of the configuration a location is
    // LEFT with, so it holds across two calls as well as within one: `us-west-2` now
    // stores `MaximumCapacity` and `TargetIdleCapacity`, and `us-east-1` stores
    // `OnDemandCapacity`, so either side of the pair is refused from the other side.
    let across_calls: [(&str, &str, &str, i64); 3] = [
        (
            "OnDemandCapacity onto a stored MaximumCapacity",
            "us-west-2",
            "OnDemandCapacity",
            1,
        ),
        (
            "MaximumCapacity onto a stored OnDemandCapacity",
            "us-east-1",
            "MaximumCapacity",
            6,
        ),
        (
            "TargetIdleCapacity onto a stored OnDemandCapacity",
            "us-east-1",
            "TargetIdleCapacity",
            1,
        ),
    ];
    for (case, location, member, value) in across_calls {
        let (status, got) = call(
            &router,
            "PATCH",
            &path,
            Some(json!({
                "LocationConfigurations": [{"LocationName": location, member: value}],
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 ValidationException, got {status} {got}"
        );
    }

    // And none of those refusals changed what the locations hold.
    let (_, got) = call(&router, "GET", &path, None).await;
    for (name, capacities) in &expected {
        assert_capacities(
            &location_state(&got, name, "after the refused capacity updates"),
            capacities,
            name,
            "after the refused capacity updates",
        );
    }
}

#[tokio::test]
async fn an_update_refuses_what_the_documented_rules_forbid() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "refusal-app").await;
    let (group_id, _) =
        active_stream_group(&router, "refusal-group", &app_arn, &["us-west-2"]).await;
    let path = format!("/streamgroups/{group_id}");

    // A READY application the group is not associated with.
    let (unassociated_id, _) = ready_application(&router, "unassociated-app").await;
    // An application the clock has not made READY yet, ASSOCIATED with the group: the
    // status rule is the one that applies to an application the member may name at all,
    // so it is only reachable past the association rule.
    let (status, fresh) = call(
        &router,
        "POST",
        "/applications",
        Some(application_body("initialized-app")),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateApplication: expected 201, got {status} {fresh}"
    );
    let initialized_id = text(&fresh, "/Id", "an INITIALIZED application");
    let (status, associated) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/associations"),
        Some(json!({"ApplicationIdentifiers": [initialized_id.clone()]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssociateApplications: expected 200, got {status} {associated}"
    );

    let cases: Vec<(&str, String, Value, u16)> = vec![
        (
            "an identifier no stream group matches",
            "sg-notstored".to_string(),
            json!({"Description": "unreachable"}),
            404,
        ),
        (
            "a location the group does not hold",
            group_id.clone(),
            json!({"LocationConfigurations": [{"LocationName": "eu-west-1", "AlwaysOnCapacity": 1}]}),
            400,
        ),
        (
            "a negative capacity",
            group_id.clone(),
            json!({"LocationConfigurations": [{"LocationName": "us-west-2", "AlwaysOnCapacity": -1}]}),
            400,
        ),
        (
            "OnDemandCapacity together with MaximumCapacity",
            group_id.clone(),
            json!({"LocationConfigurations": [
                {"LocationName": "us-west-2", "OnDemandCapacity": 2, "MaximumCapacity": 3},
            ]}),
            400,
        ),
        (
            "OnDemandCapacity together with TargetIdleCapacity",
            group_id.clone(),
            json!({"LocationConfigurations": [
                {"LocationName": "us-west-2", "OnDemandCapacity": 2, "TargetIdleCapacity": 1},
            ]}),
            400,
        ),
        (
            "one location configured twice",
            group_id.clone(),
            json!({"LocationConfigurations": [
                {"LocationName": "us-west-2", "AlwaysOnCapacity": 2},
                {"LocationName": "us-west-2", "AlwaysOnCapacity": 3},
            ]}),
            400,
        ),
        (
            // `DefaultApplicationIdentifier` "only sets the default application in a
            // stream group", so an identifier the group holds no association for is a
            // bad value for the member, not a resource that is missing.
            "a default application no application matches",
            group_id.clone(),
            json!({"DefaultApplicationIdentifier": "a-notstored"}),
            400,
        ),
        (
            "an associated default application that is not READY",
            group_id.clone(),
            json!({"DefaultApplicationIdentifier": initialized_id}),
            409,
        ),
        (
            "a default application the group is not associated with",
            group_id.clone(),
            json!({"DefaultApplicationIdentifier": unassociated_id}),
            400,
        ),
    ];

    for (case, identifier, body, expected_status) in cases {
        let (status, got) = call(
            &router,
            "PATCH",
            &format!("/streamgroups/{identifier}"),
            Some(body),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {got}"
        );
    }

    // A refusal writes nothing: the group still reports what it was created with.
    let (status, got) = call(&router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "GetStreamGroup after the refusals: expected 200, got {status} {got}"
    );
    assert_eq!(
        got["Description"], "refusal-group",
        "a refused update must not change the description, got {got}"
    );
    assert_eq!(
        got["DefaultApplication"]["Arn"],
        Value::String(app_arn.clone()),
        "a refused update must not change the default application, got {got}"
    );
    assert_capacities(
        &location_state(&got, "us-west-2", "after the refusals"),
        &BTreeMap::from([("AlwaysOnCapacity", 1)]),
        "us-west-2",
        "after the refusals",
    );
}

#[tokio::test]
async fn a_group_the_clock_has_not_made_active_refuses_the_update() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "activating-app").await;
    let (status, created) = call(
        &router,
        "POST",
        "/streamgroups",
        Some(json!({
            "Description": "activating-group",
            "StreamClass": "gen5n_high",
            "DefaultApplicationIdentifier": app_arn,
            "LocationConfigurations": [{"LocationName": "us-west-2", "AlwaysOnCapacity": 1}],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateStreamGroup: expected 201, got {status} {created}"
    );
    assert_eq!(
        created["Status"], "ACTIVATING",
        "CreateStreamGroup: expected ACTIVATING, got {created}"
    );
    let group_id = text(&created, "/Id", "a new stream group");
    let path = format!("/streamgroups/{group_id}");
    let body = json!({"Description": "updated-while-activating"});

    // "To update a stream group, it must be in ACTIVE status."
    let (status, got) = call(&router, "PATCH", &path, Some(body.clone())).await;
    assert_eq!(
        status, 409,
        "case \"group still ACTIVATING\": expected 409 ConflictException, got {status} {got}"
    );
    let (_, unchanged) = call(&router, "GET", &path, None).await;
    assert_eq!(
        unchanged["Description"], "activating-group",
        "case \"group still ACTIVATING\": the refusal must persist nothing, got {unchanged}"
    );

    // Once the clock has made the group ACTIVE, the same request is accepted.
    std::thread::sleep(PAST_TRANSITION);
    let (status, updated) = call(&router, "PATCH", &path, Some(body)).await;
    assert_eq!(
        status, 200,
        "case \"group now ACTIVE\": expected 200, got {status} {updated}"
    );
    assert_eq!(
        updated["Description"], "updated-while-activating",
        "case \"group now ACTIVE\": expected the new description, got {updated}"
    );
}

#[tokio::test]
async fn an_update_waits_for_a_group_that_is_updating_its_locations() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "settling-app").await;
    let (group_id, _) =
        active_stream_group(&router, "settling-group", &app_arn, &["us-west-2"]).await;
    let path = format!("/streamgroups/{group_id}");

    // A location being prepared puts the group in UPDATING_LOCATIONS, and
    // "ACTIVATING: … You cannot stream from, scale the capacity of, or remove this
    // location yet."
    let (status, added) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/locations"),
        Some(json!({
            "LocationConfigurations": [{"LocationName": "us-east-1", "AlwaysOnCapacity": 2}],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "AddStreamGroupLocations: expected 200, got {status} {added}"
    );

    let body = json!({
        "Description": "settled-group",
        "LocationConfigurations": [{"LocationName": "us-east-1", "AlwaysOnCapacity": 4}],
    });
    let (status, got) = call(&router, "PATCH", &path, Some(body.clone())).await;
    assert_eq!(
        status, 409,
        "case \"group UPDATING_LOCATIONS\": expected 409 ConflictException, got {status} {got}"
    );
    let (_, unchanged) = call(&router, "GET", &path, None).await;
    assert_eq!(
        unchanged["Description"], "settling-group",
        "case \"group UPDATING_LOCATIONS\": the refusal must persist nothing, got {unchanged}"
    );
    let activating = location_state(&unchanged, "us-east-1", "group UPDATING_LOCATIONS");
    assert_eq!(
        activating["Status"], "ACTIVATING",
        "case \"group UPDATING_LOCATIONS\": the new location is still being prepared, \
         got {activating}"
    );
    assert_eq!(
        activating["AlwaysOnCapacity"].as_i64(),
        Some(2),
        "case \"group UPDATING_LOCATIONS\": the refused update must not scale the location, \
         got {activating}"
    );

    // Once the clock has provisioned the new location, the group is ACTIVE again and
    // the same request scales that location's capacity.
    std::thread::sleep(PAST_TRANSITION);
    let (status, updated) = call(&router, "PATCH", &path, Some(body)).await;
    assert_eq!(
        status, 200,
        "case \"locations settled\": expected 200, got {status} {updated}"
    );
    assert_eq!(
        updated["Description"], "settled-group",
        "case \"locations settled\": expected the new description, got {updated}"
    );
    assert_capacities(
        &location_state(&updated, "us-east-1", "locations settled"),
        &BTreeMap::from([("AlwaysOnCapacity", 4)]),
        "us-east-1",
        "locations settled",
    );
    assert_capacities(
        &location_state(&updated, "us-west-2", "locations settled"),
        &BTreeMap::from([("AlwaysOnCapacity", 1)]),
        "us-west-2",
        "locations settled",
    );
}

#[tokio::test]
async fn an_update_resolves_the_group_and_the_application_by_bare_id_and_by_arn() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "identifier-app").await;
    let (group_id, group_arn) =
        active_stream_group(&router, "identifier-group", &app_arn, &["us-west-2"]).await;

    // `Identifier` accepts "an Amazon Resource Name (ARN) or ID", so both forms must
    // reach the same record — for the group and for the default application alike.
    let cases: [(&str, String, String); 2] = [
        ("bare id", group_id.clone(), app_id.clone()),
        ("ARN", as_label(&group_arn), app_arn.clone()),
    ];
    for (case, group_identifier, application_identifier) in cases {
        let description = format!("updated-by-{}", case.replace(' ', "-"));
        let (status, updated) = call(
            &router,
            "PATCH",
            &format!("/streamgroups/{group_identifier}"),
            Some(json!({
                "Description": description,
                "DefaultApplicationIdentifier": application_identifier,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {updated}"
        );
        assert_eq!(
            text(&updated, "/Arn", case),
            group_arn,
            "case {case:?}: both identifier forms name the same group, got {updated}"
        );
        assert_eq!(
            updated["DefaultApplication"],
            json!({"Id": app_id, "Arn": app_arn}),
            "case {case:?}: the default application comes from the stored record, got {updated}"
        );

        let (status, got) = call(&router, "GET", &format!("/streamgroups/{group_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetStreamGroup 200, got {status} {got}"
        );
        assert_eq!(
            got["Description"],
            Value::String(description),
            "case {case:?}: GetStreamGroup must report the persisted description, got {got}"
        );
    }
}
