// SPDX-License-Identifier: CC0-1.0

//! DeleteRouterNetworkInterface, end to end over HTTP.
//!
//! The rule the cases below pin: a `DELETE /v1/routerNetworkInterface/{Arn}` whose label
//! satisfies the modelled `RouterNetworkInterfaceArn` pattern resolves that label against
//! the STORE — an ARN the store holds no interface for is refused `NotFoundException` (404)
//! in this service's not-found envelope, and a label outside the pattern never reaches the
//! store at all — and for the interface it does hold it starts a deletion that is neither
//! instantaneous nor a soft delete: the response carries that interface's own ARN and name
//! with state `DELETING` ("the current state of the deleted router network interface,
//! indicating where it is in the deletion process",
//! `smithy_query(query="doc",shape="DeleteRouterNetworkInterfaceResponse",member="State")`),
//! the record stays `DELETING` for one [`mc::TRANSITION_SPAN`] — so a repeat of the
//! `@idempotent` call in that window answers the same response — and then it stops
//! existing, because `RouterNetworkInterfaceState`
//! (`smithy_query(query="shape",shape="RouterNetworkInterfaceState")`) declares no terminal
//! deleted value a deleted interface could be reported in.
//!
//! Each table's rows differ along what the rule generalises over: the state the addressed
//! interface was in, the name it carries, which configuration union variant (and so which
//! network interface type) it is, how many interfaces the store holds, how often the delete
//! is repeated, and the form of an ARN that addresses nothing or breaks the pattern.
//!
//! Rows whose state no implemented operation reaches are set up in the store, the way
//! `router_input_deletion` sets up a router input state a create never answers; the
//! round-trip case mints its subject through CreateRouterNetworkInterface over the wire and
//! reads it back through the implemented reader, BatchGetRouterNetworkInterface. Every call
//! under test travels the whole request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness [`crate::flow_lifecycle::Emulator`].

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::{
    PublicRouterNetworkInterfaceConfiguration, PublicRouterNetworkInterfaceRule,
    RouterNetworkInterfaceConfiguration, VpcRouterNetworkInterfaceConfiguration,
};
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_network_interfaces::{
    batch_get_network_interfaces, create_network_interface, public_configuration,
    vpc_configuration,
};

/// Longer than [`mc::TRANSITION_SPAN`], so a removal the clock owes is certainly due.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// Every value of the modelled `RouterNetworkInterfaceState` enum
/// (`smithy_query(query="shape",shape="RouterNetworkInterfaceState")`): CREATING | ACTIVE |
/// UPDATING | DELETING | ERROR | RECOVERING.
///
/// A delete addresses an interface whatever state it is in — AWS documents no precondition
/// on DeleteRouterNetworkInterface
/// (`smithy_query(query="doc",shape="DeleteRouterNetworkInterface")`: "Deletes a router
/// network interface from AWS Elemental MediaConnect.") — so the whole catalog is a row of
/// the table below, including the states no implemented operation reaches.
const ROUTER_NETWORK_INTERFACE_STATES: [&str; 6] = [
    "CREATING",
    "ACTIVE",
    "UPDATING",
    "DELETING",
    "ERROR",
    "RECOVERING",
];

/// Names a router network interface can carry, varying along length and character class.
///
/// `RouterNetworkInterface$Name` targets a plain `String`
/// (`smithy_query(query="shape",shape="RouterNetworkInterface",member="Name")`) and
/// CreateRouterNetworkInterface bounds the request member at `@length(min: 1, max: 128)`
/// (`smithy_query(query="operation",operation="CreateRouterNetworkInterface")`), so a stored
/// name can be anything in that span: one character, both published bounds, mixed case,
/// separators and multi-byte text.
fn interface_names() -> Vec<String> {
    vec![
        "a".to_string(),
        "router-interface-1".to_string(),
        "Router_Interface.2".to_string(),
        "rni 3".to_string(),
        "ネットワーク".to_string(),
        "n".repeat(128),
    ]
}

/// The two variants of `RouterNetworkInterfaceConfiguration`
/// (`smithy_query(query="shape",shape="RouterNetworkInterfaceConfiguration")`: Public |
/// Vpc), which is what an interface's `NetworkInterfaceType` is — so the table deletes
/// interfaces of both types rather than only the one the recorded case carried.
fn stored_configurations() -> Vec<(&'static str, RouterNetworkInterfaceConfiguration)> {
    vec![
        (
            "PUBLIC",
            RouterNetworkInterfaceConfiguration::Public(
                PublicRouterNetworkInterfaceConfiguration {
                    allow_rules: vec![PublicRouterNetworkInterfaceRule {
                        cidr: "198.51.100.0/24".to_string(),
                    }],
                },
            ),
        ),
        (
            "VPC",
            RouterNetworkInterfaceConfiguration::Vpc(VpcRouterNetworkInterfaceConfiguration {
                security_group_ids: vec!["sg-0000000000000000a".to_string()],
                subnet_id: "subnet-0123456789abcdef0".to_string(),
            }),
        ),
    ]
}

/// Put one router network interface straight into the store and answer the ARN it is keyed
/// by.
///
/// Shaped like a created one: the ARN is minted through the crate's own
/// [`mc::router_network_interface_resource_fragment`] and the derived members are the ones
/// the create fills in, and the record is live (only DeleteRouterNetworkInterface owes its
/// removal). The STATE, though, is the row's — which is why the record is seeded rather than
/// created: a create only ever answers `CREATING`.
fn seed_interface(
    emulator: &Emulator,
    name: &str,
    state: &str,
    configuration: RouterNetworkInterfaceConfiguration,
) -> String {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = store.lock();
    let suffix = store.next_suffix();
    let id = mc::router_resource_id(suffix);
    let arn = engine::emulator_arn(
        mc::SERVICE_NAMESPACE,
        REGION,
        &mc::router_network_interface_resource_fragment(&id),
    );
    let now = SystemTime::now();
    store.router_network_interfaces.insert(
        arn.clone(),
        mc::StoredRouterNetworkInterface {
            arn: arn.clone(),
            id,
            name: name.to_string(),
            configuration,
            region_name: REGION.to_string(),
            tags: BTreeMap::new(),
            state: state.to_string(),
            pending: None,
            client_token: None,
            created_at: now,
            updated_at: now,
            removal_due_at: None,
        },
    );
    arn
}

/// The state the store holds for `arn`, or `None` when it holds no interface for it.
fn stored_state(emulator: &Emulator, arn: &str) -> Option<String> {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    store
        .router_network_interfaces
        .get(arn)
        .map(|interface| interface.state.clone())
}

/// Every router network interface ARN the store holds, in its stable order.
fn stored_arns(emulator: &Emulator) -> Vec<String> {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    store.router_network_interfaces.keys().cloned().collect()
}

/// Send one DeleteRouterNetworkInterface over the wire.
///
/// `DELETE /v1/routerNetworkInterface/{Arn}`
/// (`smithy_query(query="operation",operation="DeleteRouterNetworkInterface")`), with `Arn`
/// bound as an `@httpLabel`.
async fn delete_network_interface(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerNetworkInterface/{arn}"), None)
        .await
}

/// A delete answers the addressed interface's own identity and `DELETING`, from whatever
/// state that interface was in, whatever name it carries and whichever configuration variant
/// it is — never a value fixed in the handler.
#[tokio::test]
async fn a_delete_reports_the_addressed_interface_in_deleting() {
    let emulator = Emulator::new();
    let names = interface_names();
    let configurations = stored_configurations();

    for (index, state) in ROUTER_NETWORK_INTERFACE_STATES.iter().enumerate() {
        // Pair each state with a different name and configuration variant, so the table
        // varies along all three.
        let name = &names[index % names.len()];
        let (type_name, configuration) = configurations[index % configurations.len()].clone();
        let case = format!("state {state}, name {name:?}, {type_name} configuration");
        let arn = seed_interface(&emulator, name, state, configuration);
        assert!(
            mc::is_mintable_router_network_interface_arn(&arn),
            "case {case:?}: the seeded ARN must satisfy the modelled \
             RouterNetworkInterfaceArn pattern, got {arn:?}"
        );

        let (status, code, body) = delete_network_interface(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: deleting a stored interface is served, got {status} {code:?}: \
             {body}"
        );
        assert_eq!(
            body["arn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the response carries the deleted interface's ARN, got {body}"
        );
        assert_eq!(
            body["name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: the response carries the stored name, got {body}"
        );
        assert_eq!(
            body["state"].as_str(),
            Some(mc::ROUTER_NETWORK_INTERFACE_STATE_DELETING),
            "case {case:?}: the response reports the deletion in progress, got {body}"
        );
        assert_eq!(
            stored_state(&emulator, &arn).as_deref(),
            Some(mc::ROUTER_NETWORK_INTERFACE_STATE_DELETING),
            "case {case:?}: the stored interface moved into DELETING, got {:?}",
            stored_state(&emulator, &arn)
        );
    }
}

/// DeleteRouterNetworkInterface is `@idempotent`
/// (`smithy_query(query="operation",operation="DeleteRouterNetworkInterface")`), so repeating
/// it while the deletion is still in progress answers the same response rather than
/// restarting the removal or refusing — however often it is repeated.
#[tokio::test]
async fn a_repeated_delete_answers_the_same_response_while_the_interface_is_deleting() {
    let emulator = Emulator::new();
    let configurations = stored_configurations();

    for (index, repeats) in [1_usize, 2, 5].into_iter().enumerate() {
        let (type_name, configuration) = configurations[index % configurations.len()].clone();
        let case = format!("{repeats} repeat(s), {type_name} configuration");
        let arn = seed_interface(
            &emulator,
            &format!("idempotent-{repeats}"),
            mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE,
            configuration,
        );
        let (status, code, first) = delete_network_interface(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the first delete is served, got {status} {code:?}: {first}"
        );

        for repeat in 1..=repeats {
            let (status, code, again) = delete_network_interface(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?}: repeat {repeat} is served, got {status} {code:?}: {again}"
            );
            assert_eq!(
                again, first,
                "case {case:?}: repeat {repeat} answers the first response, got {again}"
            );
        }
        assert_eq!(
            stored_state(&emulator, &arn).as_deref(),
            Some(mc::ROUTER_NETWORK_INTERFACE_STATE_DELETING),
            "case {case:?}: the repeats left one record in DELETING, got {:?}",
            stored_state(&emulator, &arn)
        );
    }
}

/// The round trip, through the front door: an interface minted by
/// CreateRouterNetworkInterface is deleted, the implemented reader still reports it in
/// `DELETING` for the whole transition span — an update is refused while that deletion is in
/// flight — and once the clock has made its removal due the record stops existing, so the
/// reader reports that ARN as an `Errors` entry and a further delete is a not-found.
///
/// Both configuration variants are driven, because the deletion rule is the same for either
/// type.
#[tokio::test]
async fn a_deleted_interface_stops_existing_once_its_removal_comes_due() {
    let emulator = Emulator::new();

    for (case, configuration) in [
        ("PUBLIC", public_configuration(2)),
        ("VPC", vpc_configuration(3)),
    ] {
        let (status, code, created) = create_network_interface(
            &emulator,
            json!({ "name": format!("rni-delete-{case}"), "configuration": configuration }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: the create expected 201, got {status} {code:?}: {created}"
        );
        let arn = created["routerNetworkInterface"]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
            .to_string();

        let (status, code, deleted) = delete_network_interface(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the delete is served, got {status} {code:?}: {deleted}"
        );
        assert_eq!(
            (deleted["arn"].as_str(), deleted["name"].as_str()),
            (
                Some(arn.as_str()),
                created["routerNetworkInterface"]["name"].as_str()
            ),
            "case {case:?}: the delete answers the created interface's identity, got {deleted}"
        );

        // The span is non-zero, so a caller acting straight after the delete still reads the
        // interface in its transient state through the implemented reader.
        let (status, code, read) = batch_get_network_interfaces(&emulator, &[&arn]).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the read-back expected 200, got {status} {code:?}: {read}"
        );
        assert_eq!(
            read["routerNetworkInterfaces"][0]["state"].as_str(),
            Some(mc::ROUTER_NETWORK_INTERFACE_STATE_DELETING),
            "case {case:?}: the reader reports the interface in DELETING, got {read}"
        );
        assert_eq!(
            read["errors"],
            json!([]),
            "case {case:?}: an interface still being deleted is still readable, got {read}"
        );

        // And it is on its way out, so an operation that MOVES its state conflicts with the
        // deletion already in progress.
        let (status, code, refused) = emulator
            .send(
                "PUT",
                &format!("/v1/routerNetworkInterface/{arn}"),
                Some(json!({ "name": "rni-delete-renamed" })),
            )
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: an interface being deleted cannot be updated, expected 409 \
             ConflictException, got {status} {code:?}: {refused}"
        );

        tokio::time::sleep(PAST_THE_TRANSITION).await;

        // A READ is what materialises the removal the clock already owed: no second delete
        // was needed to make the record disappear.
        let (status, code, read) = batch_get_network_interfaces(&emulator, &[&arn]).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the read past the transition expected 200, got {status} {code:?}: \
             {read}"
        );
        assert_eq!(
            read["routerNetworkInterfaces"],
            json!([]),
            "case {case:?}: the interface whose removal came due is gone, got {read}"
        );
        assert_eq!(
            read["errors"][0]["message"].as_str(),
            Some(mc::not_found_message(&arn).as_str()),
            "case {case:?}: the reader reports the removed ARN as an error entry, got {read}"
        );
        assert_eq!(
            stored_state(&emulator, &arn),
            None,
            "case {case:?}: the store no longer holds the removed interface, got {:?}",
            stored_state(&emulator, &arn)
        );

        let (status, code, body) = delete_network_interface(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: an interface that no longer exists is a not-found, got {status} \
             {code:?}: {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(mc::not_found_message(&arn).as_str()),
            "case {case:?}: the message is the request's own ARN in this service's envelope, \
             got {body}"
        );
    }
}

/// ARNs that satisfy the modelled `RouterNetworkInterfaceArn` pattern — so they reach the
/// handler as identifiers — and that this store holds no interface for. The rows differ along
/// the way an ARN of that form can address nothing: never minted, another account, another
/// Region and partition.
fn arns_the_store_holds_nothing_for() -> Vec<(&'static str, String)> {
    vec![
        (
            "an interface ARN of the minted form that was never minted",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerNetworkInterface:00000beef001"),
        ),
        (
            "an interface ARN of another account",
            format!("arn:aws:mediaconnect:{REGION}:111122223333:routerNetworkInterface:00000beef002"),
        ),
        (
            "an interface ARN of another Region and partition",
            "arn:aws-cn:mediaconnect:cn-north-1:000000000000:routerNetworkInterface:00000beef003"
                .to_string(),
        ),
    ]
}

/// An ARN of the modelled form that the store holds no interface for is refused
/// `NotFoundException` (404) with the requested identifier in this service's not-found
/// envelope — whatever way that ARN addresses nothing.
#[tokio::test]
async fn a_delete_of_an_arn_the_store_holds_nothing_for_is_not_found() {
    let emulator = Emulator::new();
    // One live interface, so a refusal below cannot be an empty store answering everything
    // the same way.
    let live = seed_interface(
        &emulator,
        "live-interface",
        mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE,
        stored_configurations()[0].1.clone(),
    );

    for (case, arn) in arns_the_store_holds_nothing_for() {
        let (status, code, body) = delete_network_interface(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: an ARN the store holds nothing for is 404 NotFoundException, got \
             {status} {code:?}: {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("NotFoundException"),
            "case {case:?}: the body names the modelled shape, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(mc::not_found_message(&arn).as_str()),
            "case {case:?}: expected {:?}, got {body}",
            mc::not_found_message(&arn)
        );
    }

    // The positive control for the same rule: the ARN the store DOES hold is served.
    let (status, code, body) = delete_network_interface(&emulator, &live).await;
    assert_eq!(
        (status, code.as_deref(), body["state"].as_str()),
        (
            200,
            None,
            Some(mc::ROUTER_NETWORK_INTERFACE_STATE_DELETING)
        ),
        "the one stored interface is still served, got {status} {code:?}: {body}"
    );
}

/// `Arn` on this operation declares the narrow `RouterNetworkInterfaceArn` pattern
/// (`smithy_query(query="operation",operation="DeleteRouterNetworkInterface")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerNetworkInterface:[a-z0-9]{12}$`),
/// so a label outside it is refused by the constraint walk BEFORE the handler — with the code
/// this service words a refused request in (`BadRequestException`, 400, the only
/// client-input error this operation models beside the not-found) and the `Validation
/// failed:` envelope — and never with a not-found or the stub's 501. The rows break the
/// pattern one way each: character class, too short, too long, another resource type, no ARN
/// at all.
#[tokio::test]
async fn a_delete_whose_label_breaks_the_modelled_pattern_is_refused_as_a_bad_request() {
    let emulator = Emulator::new();
    let live = seed_interface(
        &emulator,
        "live-interface",
        mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE,
        stored_configurations()[1].1.clone(),
    );

    let cases: Vec<(&str, String)> = vec![
        (
            "an id outside the modelled [a-z0-9] class",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerNetworkInterface:RNI_0000BEEF"),
        ),
        (
            "an eleven-character id, one short of the modelled length",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerNetworkInterface:0000beef001"),
        ),
        (
            "a thirteen-character id, one past the modelled length",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerNetworkInterface:00000beef0012"),
        ),
        (
            "another resource type of this service",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerOutput:00000beef001"),
        ),
        ("not an ARN at all", "router-network-interface-1".to_string()),
    ];

    for (case, arn) in cases {
        let (status, code, body) = delete_network_interface(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: a label outside the modelled pattern is 400 BadRequestException, \
             got {status} {code:?}: {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {case:?}: the message carries this service's validation envelope, got \
             {message:?}"
        );
    }

    // The positive control: a refused request changed nothing in the store, and the label
    // inside the modelled pattern is still served.
    assert_eq!(
        stored_state(&emulator, &live).as_deref(),
        Some(mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE),
        "a refused request changed nothing in the store, got {:?}",
        stored_state(&emulator, &live)
    );
    let (status, code, body) = delete_network_interface(&emulator, &live).await;
    assert_eq!(
        (status, code.as_deref(), body["state"].as_str()),
        (
            200,
            None,
            Some(mc::ROUTER_NETWORK_INTERFACE_STATE_DELETING)
        ),
        "a label inside the modelled pattern that the store holds is served, got {status} \
         {code:?}: {body}"
    );
}

/// A delete touches ONLY the interface its label addresses: every other router network
/// interface in the store keeps the state it had and stays exactly where it was, however many
/// there are.
#[tokio::test]
async fn a_delete_touches_only_the_addressed_interface() {
    let emulator = Emulator::new();
    let configurations = stored_configurations();

    // Five interfaces in five different states, so an untouched record is recognisable by
    // its own state rather than by a value they all share.
    let seeded: Vec<(String, &str)> = ROUTER_NETWORK_INTERFACE_STATES
        .iter()
        .take(5)
        .enumerate()
        .map(|(index, state)| {
            (
                seed_interface(
                    &emulator,
                    &format!("interface-{index}"),
                    state,
                    configurations[index % configurations.len()].1.clone(),
                ),
                *state,
            )
        })
        .collect();
    let before = stored_arns(&emulator);
    assert_eq!(
        before.len(),
        seeded.len(),
        "the store holds every seeded interface, got {before:?}"
    );

    // Delete the middle one, so rows on both sides of it are asserted untouched.
    let (addressed, _) = &seeded[2];
    let (status, code, body) = delete_network_interface(&emulator, addressed).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the delete is served, got {status} {code:?}: {body}"
    );

    for (arn, state) in &seeded {
        let expected = if arn == addressed {
            mc::ROUTER_NETWORK_INTERFACE_STATE_DELETING
        } else {
            *state
        };
        assert_eq!(
            stored_state(&emulator, arn).as_deref(),
            Some(expected),
            "case {arn:?}: expected state {expected:?}, got {:?}",
            stored_state(&emulator, arn)
        );
    }
    assert_eq!(
        stored_arns(&emulator),
        before,
        "no record was added or removed by the delete"
    );
}
