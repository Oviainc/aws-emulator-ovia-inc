// SPDX-License-Identifier: CC0-1.0

//! CreateRouterInput, end to end over HTTP, read back through GetRouterInput.
//!
//! The subject is the create itself: it must PERSIST a router input and compute its answer
//! from that record — the id and ARN it mints, the `InputType` and `StreamDetails` that
//! follow from the configuration union member, the address an input that ingests on a router
//! network interface of its own is reachable at, and the tag map and maintenance preference
//! it either stores or refuses. It must also answer a replayed `ClientToken` from the store
//! AS OF THE CALL: every router create materialises the state changes and removals the clock
//! already owes before it reads the store, the way the reporting operations and the deletes
//! do, so no two calls at one instant report one resource differently.
//!
//! Every case drives the same request path a MediaConnect client reaches (routing, restJson1
//! deserialization, constraint validation, handler, serialization) through the harness
//! `flow_lifecycle` owns, and the rows vary what each rule generalises over: the kind of
//! configuration, the routing scope and tier, the shape of the tag map, the time of day a
//! maintenance preference names, and how far the clock has moved when a token is replayed.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_input_restart::{
    create_body, create_router_input, get_router_input, minted_configuration_rows,
    past_transition_span, union, union_member,
};
use crate::router_network_interfaces::{create_network_interface, public_configuration};
use crate::router_output_lifecycle::delete_router_output;
use crate::router_outputs::{create_router_output, minted_network_interface, standard};
use crate::update_flow_output::without_keys;

/// Send one DeleteRouterInput request (`DELETE /v1/routerInput/{Arn}`).
async fn delete_router_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerInput/{arn}"), None)
        .await
}

/// How many router inputs the store holds — what proves a refused create stored nothing and
/// a replayed token minted nothing.
fn stored_router_inputs(emulator: &Emulator) -> usize {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let held = store.lock();
    held.router_inputs.len()
}

/// The `routerInput` member of a create or read response.
fn reported(body: &Value) -> &Value {
    &body["routerInput"]
}

/// Assert `address` is a dotted-quad IPv4 address, which is what `RouterInput$IpAddress`
/// ("the IP address of the router input") reports for an input that has one.
fn assert_ipv4(case: &str, address: &str) {
    let octets: Vec<&str> = address.split('.').collect();
    assert_eq!(
        octets.len(),
        4,
        "case {case:?}: IpAddress is a dotted-quad address, got {address:?}"
    );
    for octet in octets {
        assert!(
            !octet.is_empty()
                && octet.len() <= 3
                && octet.bytes().all(|byte| byte.is_ascii_digit())
                && octet.parse::<u16>().is_ok_and(|value| value <= 255),
            "case {case:?}: every octet of IpAddress is 0-255, got {address:?}"
        );
    }
}

/// A created router input is PERSISTED and reports what its configuration implies: the
/// `InputType` and `StreamDetails` of the configuration's union member, and an `IpAddress`
/// exactly when that configuration names a router network interface of its own.
///
/// The rows are the five `RouterInputConfiguration` members
/// (`smithy_query(query="shape",shape="RouterInputConfiguration")`), six configurations in
/// all: `Standard`, `Failover` and `Merge` carry a `NetworkInterfaceArn`, so an input of
/// those kinds ingests on an interface of this account and is reachable at an address of its
/// own, while `MediaConnectFlow` and `MediaLiveChannel` take their content from another AWS
/// resource and so name no interface and have no address.
#[tokio::test]
async fn every_kind_of_router_input_is_persisted_with_the_address_its_configuration_implies() {
    let emulator = Emulator::new();
    let rows = minted_configuration_rows(&emulator, "create-kinds").await;
    let row_count = rows.len();

    let mut minted: Vec<(String, String, String, Option<String>)> = Vec::new();
    for (kind, member_name, json_name, configuration, expected_input_type) in rows {
        let case = kind.to_string();
        let name = format!("ri-{kind}").replace('/', "-");
        let (status, code, created) =
            create_router_input(&emulator, create_body(&name, configuration)).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: CreateRouterInput answers 201, got {created}"
        );
        let answered = reported(&created).clone();

        assert_eq!(
            (
                answered["inputType"].as_str(),
                answered["name"].as_str(),
                answered["state"].as_str(),
                answered["routedOutputs"].as_i64(),
            ),
            (
                Some(expected_input_type),
                Some(name.as_str()),
                Some(mc::ROUTER_INPUT_STATE_CREATING),
                Some(0),
            ),
            "case {case:?}: InputType follows from the configuration union member, the name is \
             the one the create carried, a new input is answered in CREATING and nothing is \
             routed to it yet, got {answered}"
        );
        assert!(
            !union_member(&answered["streamDetails"], member_name, json_name).is_null(),
            "case {case:?}: StreamDetails reports the {member_name} member, got {answered}"
        );
        assert!(
            !union_member(&answered["configuration"], member_name, json_name).is_null(),
            "case {case:?}: the stored configuration keeps the {member_name} member the create \
             carried, got {answered}"
        );

        let expects_address = matches!(member_name, "Standard" | "Failover" | "Merge");
        let address = answered["ipAddress"].as_str().map(str::to_string);
        assert_eq!(
            address.is_some(),
            expects_address,
            "case {case:?}: an input ingesting on its own network interface reports an \
             IpAddress and one taking its content from another resource reports none, got \
             {answered}"
        );
        if let Some(address) = address.as_deref() {
            assert_ipv4(&case, address);
        }

        // The answer is the STORED record: the reader computes the same one from it.
        let arn = answered["arn"].as_str().unwrap_or_default().to_string();
        let (status, code, read_back) = get_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: GetRouterInput answers 200 for the input the create minted, got \
             {read_back}"
        );
        // Every member but `state` is compared: the state is the one member the CLOCK moves,
        // and it is asserted on its own where the clock is the subject (see
        // [`a_replayed_client_token_answers_the_router_input_as_the_clock_leaves_it`]).
        assert_eq!(
            without_keys(reported(&read_back), &["state"]),
            without_keys(&answered, &["state"]),
            "case {case:?}: the reader reports the record the create stored, got {read_back}"
        );

        minted.push((
            case,
            arn,
            answered["id"].as_str().unwrap_or_default().to_string(),
            address,
        ));
    }

    // Each create minted its OWN input: nothing in the model makes a router input unique by
    // name or configuration, so every row is a distinct resource with a distinct identity.
    for (index, (case, arn, id, address)) in minted.iter().enumerate() {
        for (other_case, other_arn, other_id, other_address) in minted.iter().skip(index + 1) {
            assert_ne!(
                (arn, id),
                (other_arn, other_id),
                "case {case:?} vs {other_case:?}: each create mints its own ARN and Id"
            );
            if address.is_some() {
                assert_ne!(
                    address, other_address,
                    "case {case:?} vs {other_case:?}: two inputs do not share one IpAddress"
                );
            }
        }
    }
    assert_eq!(
        stored_router_inputs(&emulator),
        row_count,
        "the store holds one router input per row, got {}",
        stored_router_inputs(&emulator)
    );
}

/// Every `RoutingScope` and `RouterInputTier` a create may name is stored and reported back,
/// and two inputs may share a name: the id in the ARN is what keeps them apart.
///
/// The rows span both `RoutingScope` values (`smithy_query(query="shape",shape="RoutingScope")`:
/// REGIONAL | GLOBAL) against tiers at either end of the published list, all under ONE name.
#[tokio::test]
async fn the_routing_scope_and_tier_are_stored_and_a_name_is_not_unique() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "scope-tier")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    // One name for every row: a create with a name another input already carries mints its
    // own input, because nothing in the model makes `Name` unique.
    const SHARED_NAME: &str = "ri-shared-name";

    // (case, RoutingScope, Tier, MaximumBitrate).
    let rows = [
        ("regional at the smallest tier", "REGIONAL", "INPUT_20", 1i64),
        ("global at the smallest tier", "GLOBAL", "INPUT_20", 20_000_000),
        ("global at the largest tier", "GLOBAL", "INPUT_500", 500_000_000),
        (
            "regional at a middle tier",
            "REGIONAL",
            "INPUT_100",
            99_000_000,
        ),
    ];

    let mut arns: Vec<(&str, String)> = Vec::new();
    for (case, routing_scope, tier, maximum_bitrate) in rows {
        let mut body = create_body(SHARED_NAME, configuration.clone());
        body["routingScope"] = json!(routing_scope);
        body["tier"] = json!(tier);
        body["maximumBitrate"] = json!(maximum_bitrate);

        let (status, code, created) = create_router_input(&emulator, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: the create is accepted, got {created}"
        );
        let arn = reported(&created)["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (_, _, read_back) = get_router_input(&emulator, &arn).await;
        let stored = reported(&read_back);
        assert_eq!(
            (
                stored["name"].as_str(),
                stored["routingScope"].as_str(),
                stored["tier"].as_str(),
                stored["maximumBitrate"].as_i64(),
                stored["regionName"].as_str(),
            ),
            (
                Some(SHARED_NAME),
                Some(routing_scope),
                Some(tier),
                Some(maximum_bitrate),
                Some(REGION),
            ),
            "case {case:?}: the stored input reports the scope, tier and bitrate the create \
             carried, got {read_back}"
        );

        for (other_case, other_arn) in &arns {
            assert_ne!(
                &arn, other_arn,
                "case {case:?} vs {other_case:?}: two inputs of one name are distinct resources"
            );
        }
        arns.push((case, arn));
    }
    assert_eq!(
        stored_router_inputs(&emulator),
        rows.len(),
        "every create minted its own input, got {} stored",
        stored_router_inputs(&emulator)
    );
}

/// A replayed `ClientToken` ("a unique identifier for the request to ensure idempotency", an
/// `@idempotencyToken`) answers with the input that token minted, AS THE CLOCK LEAVES IT: the
/// create materialises what the clock already owes before it reads the store, so a replay
/// reports the same record GetRouterInput reports at that instant, and a token whose input
/// the clock has already removed mints a new input instead of answering with a record that no
/// longer exists.
///
/// The rows are how far the clock has moved when the token is replayed: still inside the
/// create's own transition, past it, and past the removal a delete owed.
#[tokio::test]
async fn a_replayed_client_token_answers_the_router_input_as_the_clock_leaves_it() {
    const TOKEN: &str = "ri-replayed-token";

    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "replayed-token")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");
    let mut body = create_body("ri-replayed", configuration);
    body["clientToken"] = json!(TOKEN);

    let (status, code, created) = create_router_input(&emulator, body.clone()).await;
    assert_eq!(
        (status, code.as_deref(), reported(&created)["state"].as_str()),
        (201, None, Some(mc::ROUTER_INPUT_STATE_CREATING)),
        "case \"first create\": the create is accepted and answers in CREATING, got {created}"
    );
    let arn = reported(&created)["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Row 1: replayed while the create's own transition is still in flight.
    let (status, code, replay) = create_router_input(&emulator, body.clone()).await;
    assert_eq!(
        (
            status,
            code.as_deref(),
            reported(&replay)["arn"].as_str(),
            reported(&replay)["state"].as_str(),
        ),
        (
            201,
            None,
            Some(arn.as_str()),
            Some(mc::ROUTER_INPUT_STATE_CREATING),
        ),
        "case \"replayed while creating\": the replay answers with the input the token minted, \
         still in CREATING, got {replay}"
    );
    assert_eq!(
        stored_router_inputs(&emulator),
        1,
        "case \"replayed while creating\": a replayed token mints nothing, got {} stored",
        stored_router_inputs(&emulator)
    );

    // Row 2: replayed once the clock has settled the create. The replay must report what the
    // reader reports at the same instant, not the state the first create answered with.
    tokio::time::sleep(past_transition_span()).await;
    let (_, _, read_back) = get_router_input(&emulator, &arn).await;
    assert_eq!(
        reported(&read_back)["state"].as_str(),
        Some(mc::ROUTER_INPUT_STATE_STANDBY),
        "case \"replayed after the transition settled\": the clock settles CREATING into \
         STANDBY, got {read_back}"
    );
    let (status, code, replay) = create_router_input(&emulator, body.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case \"replayed after the transition settled\": the replay is accepted, got {replay}"
    );
    assert_eq!(
        reported(&replay),
        reported(&read_back),
        "case \"replayed after the transition settled\": the replay reports the record the \
         reader reports, got {replay}"
    );

    // Row 3: replayed after the removal a delete owed has come due. The input that token
    // minted no longer exists, so the create mints a new one.
    let (status, _, deleted) = delete_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, deleted["state"].as_str()),
        (200, Some(mc::ROUTER_INPUT_STATE_DELETING)),
        "case \"delete accepted\": the input is reported DELETING, got {deleted}"
    );
    tokio::time::sleep(past_transition_span()).await;

    let (status, code, replay) = create_router_input(&emulator, body).await;
    assert_eq!(
        (status, code.as_deref(), reported(&replay)["state"].as_str()),
        (201, None, Some(mc::ROUTER_INPUT_STATE_CREATING)),
        "case \"replayed after the removal came due\": the create mints a new input, got {replay}"
    );
    let minted_again = reported(&replay)["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        minted_again, arn,
        "case \"replayed after the removal came due\": the new input has its own ARN"
    );
    let (status, code, gone) = get_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "case \"replayed after the removal came due\": the removed input is gone, got {gone}"
    );
    assert_eq!(
        stored_router_inputs(&emulator),
        1,
        "case \"replayed after the removal came due\": the store holds only the new input, got \
         {} stored",
        stored_router_inputs(&emulator)
    );
}

/// The same rule on the two sibling router creates the same change touched: a replayed
/// `ClientToken` answers from the store AS OF THE CALL.
///
/// The rows are the other two router resources a create mints — a router output, whose record
/// the clock also removes once a delete has come due, and a router network interface, which
/// has no removal but does have a transition the clock settles.
#[tokio::test]
async fn a_replayed_client_token_answers_the_sibling_router_resources_as_of_the_call() {
    const OUTPUT_TOKEN: &str = "ro-replayed-token";
    const INTERFACE_TOKEN: &str = "rni-replayed-token";

    let emulator = Emulator::new();
    let output_interface = minted_network_interface(&emulator, "rni-ri-replayed").await;

    let output_body = json!({
        "name": "ro-replayed",
        "configuration": standard(&output_interface, json!({
            "Rtp": { "destinationAddress": "198.51.100.20", "destinationPort": 5020 }
        })),
        "maximumBitrate": 20_000_000i64,
        "routingScope": "REGIONAL",
        "tier": "OUTPUT_20",
        "clientToken": OUTPUT_TOKEN,
    });

    let (status, code, created) = create_router_output(&emulator, output_body.clone()).await;
    assert_eq!(
        (
            status,
            code.as_deref(),
            created["routerOutput"]["state"].as_str()
        ),
        (201, None, Some(mc::ROUTER_OUTPUT_STATE_CREATING)),
        "case \"router output first create\": accepted and answered in CREATING, got {created}"
    );
    let output_arn = created["routerOutput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    tokio::time::sleep(past_transition_span()).await;
    let (status, code, replay) = create_router_output(&emulator, output_body.clone()).await;
    assert_eq!(
        (
            status,
            code.as_deref(),
            replay["routerOutput"]["arn"].as_str(),
            replay["routerOutput"]["state"].as_str(),
        ),
        (
            201,
            None,
            Some(output_arn.as_str()),
            Some(mc::ROUTER_OUTPUT_STATE_STANDBY),
        ),
        "case \"router output replayed after the transition settled\": the replay reports the \
         output as the clock leaves it, got {replay}"
    );

    let (status, _, deleted) = delete_router_output(&emulator, &output_arn).await;
    assert_eq!(
        status, 200,
        "case \"router output delete accepted\": got {deleted}"
    );
    tokio::time::sleep(past_transition_span()).await;
    let (status, code, replay) = create_router_output(&emulator, output_body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case \"router output replayed after the removal came due\": accepted, got {replay}"
    );
    assert_ne!(
        replay["routerOutput"]["arn"].as_str(),
        Some(output_arn.as_str()),
        "case \"router output replayed after the removal came due\": the create mints a new \
         output, got {replay}"
    );

    let interface_body = json!({
        "name": "rni-replayed",
        "configuration": public_configuration(1),
        "clientToken": INTERFACE_TOKEN,
    });
    let (status, code, created) = create_network_interface(&emulator, interface_body.clone()).await;
    assert_eq!(
        (
            status,
            code.as_deref(),
            created["routerNetworkInterface"]["state"].as_str(),
        ),
        (
            201,
            None,
            Some(mc::ROUTER_NETWORK_INTERFACE_STATE_CREATING),
        ),
        "case \"network interface first create\": accepted and answered in CREATING, got {created}"
    );
    let interface_arn = created["routerNetworkInterface"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    tokio::time::sleep(past_transition_span()).await;
    let (status, code, replay) = create_network_interface(&emulator, interface_body).await;
    assert_eq!(
        (
            status,
            code.as_deref(),
            replay["routerNetworkInterface"]["arn"].as_str(),
            replay["routerNetworkInterface"]["state"].as_str(),
        ),
        (
            201,
            None,
            Some(interface_arn.as_str()),
            Some(mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE),
        ),
        "case \"network interface replayed after the transition settled\": the replay reports \
         the interface as the clock leaves it, got {replay}"
    );
}

/// A create stores only a tag map this service would accept from a tag write: the model
/// declares `Tags` as `__mapOfString` with no bounds, so the create applies the documented
/// ones — at most 50 pairs on a resource, a key of at most 128 characters and a value of at
/// most 256 ("Tag keys can have a maximum character length of 128 characters, and tag values
/// can have a maximum length of 256 characters",
/// `smithy_query(query="doc",shape="TagResourceRequest",member="Tags")`) — and refuses past
/// them with `BadRequestException` (400), storing nothing.
///
/// Every bound is tested AT its published value and one character or pair past it, and a key
/// that names no tag is refused too.
#[tokio::test]
async fn a_create_stores_only_a_tag_map_the_service_would_accept() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "tags")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    let many_tags = |count: usize| -> Value {
        let pairs: serde_json::Map<String, Value> = (0..count)
            .map(|index| (format!("key-{index}"), json!("value")))
            .collect();
        Value::Object(pairs)
    };
    let one_tag = |key: String, value: &str| -> Value {
        let mut pairs = serde_json::Map::new();
        pairs.insert(key, json!(value));
        Value::Object(pairs)
    };
    let long = |length: usize| "t".repeat(length);

    // (case, tags, accepted).
    let rows = [
        ("no tags", json!({}), true),
        ("one tag", json!({ "stage": "test" }), true),
        (
            "as many tags as a resource may carry",
            many_tags(mc::MAX_TAGS_PER_RESOURCE),
            true,
        ),
        (
            "one tag past the bound",
            many_tags(mc::MAX_TAGS_PER_RESOURCE + 1),
            false,
        ),
        ("an empty key", one_tag(String::new(), "value"), false),
        (
            "a key at the documented length",
            one_tag(long(mc::MAX_TAG_KEY_LENGTH), "value"),
            true,
        ),
        (
            "a key one character past it",
            one_tag(long(mc::MAX_TAG_KEY_LENGTH + 1), "value"),
            false,
        ),
        (
            "a value at the documented length",
            one_tag("stage".to_string(), &long(mc::MAX_TAG_VALUE_LENGTH)),
            true,
        ),
        (
            "a value one character past it",
            one_tag("stage".to_string(), &long(mc::MAX_TAG_VALUE_LENGTH + 1)),
            false,
        ),
    ];

    let mut accepted_count = 0usize;
    for (case, tags, accepted) in rows {
        let mut body = create_body(&format!("ri-tags-{accepted_count}"), configuration.clone());
        body["tags"] = tags.clone();
        let (status, code, answered) = create_router_input(&emulator, body).await;
        if accepted {
            accepted_count += 1;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: the create is accepted, got {answered}"
            );
            let arn = reported(&answered)["arn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let (_, _, read_back) = get_router_input(&emulator, &arn).await;
            assert_eq!(
                reported(&read_back)["tags"],
                tags,
                "case {case:?}: the stored input carries the tags the create named, got \
                 {read_back}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: the create is refused, got {answered}"
            );
            assert!(
                answered["message"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with(mc::VALIDATION_FAILED_PREFIX),
                "case {case:?}: the refusal carries this service's validation envelope, got \
                 {answered}"
            );
        }
        assert_eq!(
            stored_router_inputs(&emulator),
            accepted_count,
            "case {case:?}: a refused create stores nothing, got {} stored",
            stored_router_inputs(&emulator)
        );
    }
}

/// A `MaintenanceConfiguration` the create stores must name a maintenance preference the
/// service can schedule a window from: `PreferredDayTimeMaintenanceConfiguration$Time` is
/// "the preferred time for maintenance operations", a time of day in the 24-hour `HH:MM`
/// form this service documents for the same preference on a flow
/// (`smithy_query(query="doc",shape="UpdateMaintenance",member="MaintenanceStartHour")`:
/// "UTC time when the maintenance will happen. Use 24-hour HH:MM format"). `Time` is a plain
/// String in the model, so a string that names no time of day is refused by the create with
/// `BadRequestException` (400) rather than stored.
///
/// The accepted rows walk the whole day across the week (`Day` is the modelled enum, so every
/// weekday is a valid preference); the refused rows are the ways a string fails to name a
/// time of day: an hour or a minute past its range, a field that is not a number, and too few
/// or too many fields.
#[tokio::test]
async fn a_maintenance_preference_must_name_a_time_of_day() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "maintenance")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    // (case, Day, Time, accepted).
    let rows = [
        ("midnight", "MONDAY", "00:00", true),
        ("morning", "TUESDAY", "09:30", true),
        ("the last minute of the day", "WEDNESDAY", "23:59", true),
        ("a time written out in full", "THURSDAY", "17:45:30", true),
        ("midday", "SUNDAY", "12:00", true),
        ("an hour past the day", "FRIDAY", "24:00", false),
        ("a minute past the hour", "SATURDAY", "12:60", false),
        ("not a number", "MONDAY", "noon", false),
        ("no minutes", "MONDAY", "12", false),
        ("an empty time", "MONDAY", "", false),
        ("a fourth field", "MONDAY", "12:00:00:00", false),
    ];

    let mut accepted_count = 0usize;
    for (case, day, time, accepted) in rows {
        let mut body = create_body(&format!("ri-window-{accepted_count}"), configuration.clone());
        body["maintenanceConfiguration"] = union(
            "PreferredDayTime",
            "preferredDayTime",
            json!({ "day": day, "time": time }),
        );
        let (status, code, answered) = create_router_input(&emulator, body).await;
        if accepted {
            accepted_count += 1;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: the create is accepted, got {answered}"
            );
            let arn = reported(&answered)["arn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let (_, _, read_back) = get_router_input(&emulator, &arn).await;
            let stored = reported(&read_back);
            let preference = union_member(
                &stored["maintenanceConfiguration"],
                "PreferredDayTime",
                "preferredDayTime",
            );
            assert_eq!(
                (
                    stored["maintenanceType"].as_str(),
                    preference["day"].as_str(),
                    preference["time"].as_str(),
                ),
                (Some("PREFERRED_DAY_TIME"), Some(day), Some(time)),
                "case {case:?}: MaintenanceType follows from the union member and the \
                 preference is stored as given, got {read_back}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: a string that names no time of day is refused, got {answered}"
            );
        }
        assert_eq!(
            stored_router_inputs(&emulator),
            accepted_count,
            "case {case:?}: a refused create stores nothing, got {} stored",
            stored_router_inputs(&emulator)
        );
    }
}
