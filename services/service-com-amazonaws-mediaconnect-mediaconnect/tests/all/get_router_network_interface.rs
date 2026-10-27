// SPDX-License-Identifier: CC0-1.0

//! GetRouterNetworkInterface, end to end over HTTP: `GET /v1/routerNetworkInterface/{Arn}`.
//!
//! The rule the cases below pin: this reporting read resolves its `@httpLabel` ARN against the
//! STORE and answers the router network interface that ARN addresses — "Retrieves information
//! about a specific router network interface"
//! (`smithy_query(query="doc",shape="GetRouterNetworkInterface")`, `@readonly`) — every member
//! computed from that one record and the records that NAME it, so it agrees member for member
//! with the other read sides of the same record (BatchGetRouterNetworkInterface, and
//! ListRouterNetworkInterfaces for the members a summary declares), answers the modelled
//! `NotFoundException` (404) for an ARN the store holds no interface for, and leaves nothing a
//! later caller sees different except a state change the CLOCK had already made due.
//!
//! The rows vary what that rule generalises over: which `RouterNetworkInterfaceConfiguration`
//! variant (and so which network interface type) the addressed interface carries, how many
//! allow rules a PUBLIC one lists and how many security groups a VPC one lists across the
//! modelled bounds, the size of its tag set, whether the request named the Region, how many
//! router inputs and outputs name the interface (the two association counts), how many
//! interfaces the store holds beside the addressed one, which state the record is in as the
//! clock moves it (CREATING -> ACTIVE, and DELETING on its way out), and how often the read is
//! repeated.
//!
//! Every call travels the whole request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness [`crate::flow_lifecycle::Emulator`].
//! The create helper, the configuration fixtures and the persisted-member list are the ones
//! `router_network_interfaces` owns — the suite that pins CreateRouterNetworkInterface's own
//! rules — so the router network interface suites cannot drift apart.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_input_restart::{create_body, create_router_input, union};
use crate::router_network_interfaces::{
    batch_get_network_interfaces, create_network_interface, public_configuration, stored_count,
    vpc_configuration, FLOW_ARN, PERSISTED_MEMBERS, UNKNOWN_ARN,
};
use crate::router_outputs::{create_router_output, standard};

/// Long enough that a transition the clock owes a freshly created interface is certainly due:
/// twice `mc::TRANSITION_SPAN`, so a settled state is never read early.
fn past_transition_span() -> Duration {
    mc::TRANSITION_SPAN * 2
}

/// Far enough behind that a removal the store owes is certainly due, and far enough ahead that
/// one certainly is not — the two sides of the instant a reader compares against.
const DUE_A_WHILE_AGO: Duration = Duration::from_secs(600);
/// See [`DUE_A_WHILE_AGO`].
const NOT_DUE_FOR_A_WHILE: Duration = Duration::from_secs(600);

/// The operation under test
/// (`smithy_query(query="operation",operation="GetRouterNetworkInterface")`:
/// `GET /v1/routerNetworkInterface/{Arn}`).
async fn get_network_interface(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/routerNetworkInterface/{arn}"), None)
        .await
}

/// DeleteRouterNetworkInterface — the call that puts an interface on its way out, which is what
/// the DELETING and the gone-record cases below read afterwards.
async fn delete_network_interface(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerNetworkInterface/{arn}"), None)
        .await
}

/// ListRouterNetworkInterfaces — the summary read side, for the members both reads declare
/// (`smithy_query(query="operation",operation="ListRouterNetworkInterfaces")`:
/// `POST /v1/routerNetworkInterfaces`).
async fn list_network_interfaces(emulator: &Emulator) -> Value {
    let (status, code, body) = emulator
        .send("POST", "/v1/routerNetworkInterfaces", None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "ListRouterNetworkInterfaces expected 200, got {status} {code:?}: {body}"
    );
    body
}

/// The members a `ListedRouterNetworkInterface` summary and a `RouterNetworkInterface` resource
/// BOTH declare (`smithy_query(query="shape",shape="ListedRouterNetworkInterface")`), so the two
/// read sides can be compared on them.
const MEMBERS_SHARED_WITH_THE_SUMMARY: [&str; 9] = [
    "name",
    "arn",
    "id",
    "networkInterfaceType",
    "state",
    "regionName",
    "associatedInputCount",
    "associatedOutputCount",
    "createdAt",
];

/// The whole stored record for `arn` as the store holds it, for comparing a before and an
/// after: a read must leave every member of it exactly as it was.
fn stored_record(emulator: &Emulator, arn: &str) -> Option<String> {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    store
        .router_network_interfaces
        .get(arn)
        .map(|interface| format!("{interface:?}"))
}

/// Owe the stored interface `arn`'s removal at `due_at` — the record a delete leaves behind
/// (`mc::StoredRouterNetworkInterface::begin_deletion` writes exactly this, one
/// `mc::TRANSITION_SPAN` out).
///
/// Written onto the record so the cases below do not race the span: what they assert is what the
/// READER does with a removal that is owed and not yet due, and with one the clock has already
/// made due. The real span is exercised over the wire by
/// [`the_read_reports_the_state_the_clock_has_made_due`].
fn owe_removal(emulator: &Emulator, arn: &str, due_at: SystemTime) {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    let interface = store
        .router_network_interfaces
        .get_mut(arn)
        .expect("the store holds the router network interface under test");
    interface.removal_due_at = Some(due_at);
}

/// One create, and what the reader must report back for it beyond the create's own answer.
struct Row {
    case: &'static str,
    body: Value,
    /// `RouterNetworkInterface$NetworkInterfaceType`, which the service derives from the
    /// configuration's union variant rather than reading from the request.
    network_interface_type: &'static str,
    /// How many router inputs name this interface, so `AssociatedInputCount` is counted rather
    /// than stored.
    inputs: usize,
    /// How many router outputs name it, for `AssociatedOutputCount`.
    outputs: usize,
    /// The `routerNetworkInterface` the create answered, filled in once the row is created.
    created: Value,
    /// The ARN the create minted for it.
    arn: String,
}

/// The rows span the class the rule generalises over: both configuration variants (so both
/// network interface types), a PUBLIC interface at no allow rules, at one and at the modelled
/// maximum of ten (`PublicRouterNetworkInterfaceConfiguration$AllowRules`, len 0..10), a VPC
/// interface at the modelled minimum and maximum security group count
/// (`VpcRouterNetworkInterfaceConfiguration$SecurityGroupIds`, len 1..5), tag sets of three
/// sizes, a request that names the Region beside ones that let the service derive it, and
/// association counts of zero, one and two on each side.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "public with no allow rules, nothing named and nothing associated",
            body: json!({
                "name": "get-rni-public-open",
                "configuration": public_configuration(0),
            }),
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC,
            inputs: 0,
            outputs: 0,
            created: Value::Null,
            arn: String::new(),
        },
        Row {
            case: "public with one allow rule, the Region named, one input",
            body: json!({
                "name": "get-rni-public-one",
                "configuration": public_configuration(1),
                "regionName": REGION,
            }),
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC,
            inputs: 1,
            outputs: 0,
            created: Value::Null,
            arn: String::new(),
        },
        Row {
            case: "public at the modelled ten allow rules, two tags, two inputs and one output",
            body: json!({
                "name": "get-rni-public-ten",
                "configuration": public_configuration(10),
                "tags": { "env": "test", "team": "media" },
            }),
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC,
            inputs: 2,
            outputs: 1,
            created: Value::Null,
            arn: String::new(),
        },
        Row {
            case: "vpc at the modelled one security group, one tag, two outputs",
            body: json!({
                "name": "get-rni-vpc-one",
                "configuration": vpc_configuration(1),
                "tags": { "env": "test" },
            }),
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_VPC,
            inputs: 0,
            outputs: 2,
            created: Value::Null,
            arn: String::new(),
        },
        Row {
            case: "vpc at the modelled five security groups, one input and one output",
            body: json!({
                "name": "GETRNI0",
                "configuration": vpc_configuration(5),
            }),
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_VPC,
            inputs: 1,
            outputs: 1,
            created: Value::Null,
            arn: String::new(),
        },
    ]
}

/// Create one router input whose STANDARD configuration names `interface_arn`, so the interface
/// counts it among its associated inputs.
async fn associate_input(emulator: &Emulator, name: &str, interface_arn: &str, port: i64) {
    let configuration = union(
        "Standard",
        "standard",
        json!({
            "networkInterfaceArn": interface_arn,
            "protocolConfiguration": union("Rtp", "rtp", json!({ "port": port })),
        }),
    );
    let (status, code, created) =
        create_router_input(emulator, create_body(name, configuration)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "CreateRouterInput expected 201 for {name:?}, got {status} {code:?}: {created}"
    );
}

/// Create one router output whose STANDARD configuration names `interface_arn`, so the interface
/// counts it among its associated outputs.
async fn associate_output(emulator: &Emulator, name: &str, interface_arn: &str, port: i64) {
    let body = json!({
        "name": name,
        "configuration": standard(interface_arn, json!({"Rtp": {
            "destinationAddress": "198.51.100.30",
            "destinationPort": port,
        }})),
        "maximumBitrate": 20_000_000i64,
        "routingScope": "REGIONAL",
        "tier": "OUTPUT_20",
    });
    let (status, code, created) = create_router_output(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "CreateRouterOutput expected 201 for {name:?}, got {status} {code:?}: {created}"
    );
}

/// Create every row on `emulator`, associate the inputs and outputs it asks for, and fill in
/// what each create answered.
async fn created_rows(emulator: &Emulator) -> Vec<Row> {
    let mut built = rows();
    let mut port = 5000i64;
    for (index, row) in built.iter_mut().enumerate() {
        let case = row.case;
        let (status, code, created) = create_network_interface(emulator, row.body.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: CreateRouterNetworkInterface answers 201, got {status} {code:?}: \
             {created}"
        );
        row.arn = created["routerNetworkInterface"]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create must report an ARN, got {created}"))
            .to_string();
        row.created = created["routerNetworkInterface"].clone();

        for input in 0..row.inputs {
            associate_input(
                emulator,
                &format!("get-rni-in-{index}-{input}"),
                &row.arn,
                port,
            )
            .await;
            port += 2;
        }
        for output in 0..row.outputs {
            associate_output(
                emulator,
                &format!("get-rni-out-{index}-{output}"),
                &row.arn,
                port,
            )
            .await;
            port += 2;
        }
    }
    built
}

/// The reader answers the record its ARN addresses — for every configuration variant, bound and
/// tag set — with the association counts counted from the inputs and outputs that name it, and
/// that answer is the SAME record the other read sides report for the same ARN, because all of
/// them are computed from the one stored interface.
#[tokio::test]
async fn get_reports_the_stored_record_and_agrees_with_the_other_read_sides() {
    let emulator = Emulator::new();
    let built = created_rows(&emulator).await;

    // Past the span the create's own CREATING -> ACTIVE transition is owed at, so every read
    // below sees one settled record rather than a state the clock is still moving.
    tokio::time::sleep(past_transition_span()).await;
    let listing = list_network_interfaces(&emulator).await;

    for row in &built {
        let case = row.case;
        let arn = row.arn.as_str();
        let (status, code, body) = get_network_interface(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: GetRouterNetworkInterface answers 200 for an interface the store \
             holds, got {status} {code:?}: {body}"
        );

        let reported = &body["routerNetworkInterface"];
        assert_eq!(
            (
                reported["arn"].as_str(),
                reported["name"].as_str(),
                reported["networkInterfaceType"].as_str(),
                reported["regionName"].as_str(),
                reported["state"].as_str(),
                reported["associatedInputCount"].as_u64(),
                reported["associatedOutputCount"].as_u64(),
            ),
            (
                Some(arn),
                row.body["name"].as_str(),
                Some(row.network_interface_type),
                Some(REGION),
                Some(mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE),
                Some(row.inputs as u64),
                Some(row.outputs as u64),
            ),
            "case {case:?}: the reader reports this interface's own identity, type, Region, \
             settled state and the counts of what names it, got {reported}"
        );
        assert_eq!(
            reported["configuration"], row.created["configuration"],
            "case {case:?}: the reported Configuration is the one the create stored, got {:?}",
            reported["configuration"]
        );
        assert_eq!(
            reported["tags"],
            row.body
                .get("tags")
                .cloned()
                .unwrap_or_else(|| json!({})),
            "case {case:?}: the reported Tags are the ones the create stored, got {:?}",
            reported["tags"]
        );
        for member in PERSISTED_MEMBERS {
            // The two association counts are the members the row's own inputs and outputs
            // moved AFTER the create answered, so they are asserted above instead.
            if member == "associatedInputCount" || member == "associatedOutputCount" {
                continue;
            }
            assert_eq!(
                reported[member], row.created[member],
                "case {case:?}: the reader reports the created {member}, got {:?}",
                reported[member]
            );
        }

        // The batch read side answers the same resource, member for member.
        let (status, code, batch) = batch_get_network_interfaces(&emulator, &[arn]).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the batch read answers 200, got {status} {code:?}: {batch}"
        );
        assert_eq!(
            &batch["routerNetworkInterfaces"][0], reported,
            "case {case:?}: both read sides report one record identically, got {batch}"
        );

        // And the listing's summary agrees on every member a summary declares.
        let summary = listing["routerNetworkInterfaces"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|summary| summary["arn"].as_str() == Some(arn))
            .unwrap_or_else(|| panic!("case {case:?}: the listing reports {arn}: {listing}"));
        for member in MEMBERS_SHARED_WITH_THE_SUMMARY {
            assert_eq!(
                summary[member], reported[member],
                "case {case:?}: the summary and the resource report one {member}, got {:?} and \
                 {:?}",
                summary[member], reported[member]
            );
        }
    }
}

/// The state the reader reports is the one the CLOCK has made due: the transient state straight
/// after the call that starts a transition, the settled one once the span has passed, and the
/// DELETING of an interface on its way out — with every other member left as it was stored.
#[tokio::test]
async fn the_read_reports_the_state_the_clock_has_made_due() {
    let emulator = Emulator::new();
    let (status, code, created) = create_network_interface(
        &emulator,
        json!({
            "name": "get-rni-lifecycle",
            "configuration": vpc_configuration(2),
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the create expected 201, got {status} {code:?}: {created}"
    );
    let arn = created["routerNetworkInterface"]["arn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    // A caller reading straight after the create still observes the transient state.
    let (status, _, transient) = get_network_interface(&emulator, &arn).await;
    assert_eq!(
        (
            status,
            transient["routerNetworkInterface"]["state"].as_str()
        ),
        (200, Some(mc::ROUTER_NETWORK_INTERFACE_STATE_CREATING)),
        "case \"read straight after the create\": the interface is still CREATING, got \
         {transient}"
    );

    // Once the span has passed, the settled state is reported and nothing else moved.
    tokio::time::sleep(past_transition_span()).await;
    let (status, _, settled) = get_network_interface(&emulator, &arn).await;
    assert_eq!(
        (status, settled["routerNetworkInterface"]["state"].as_str()),
        (200, Some(mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE)),
        "case \"read after the span\": the clock settles a new interface into ACTIVE, got \
         {settled}"
    );
    for member in PERSISTED_MEMBERS {
        assert_eq!(
            settled["routerNetworkInterface"][member], created["routerNetworkInterface"][member],
            "case \"read after the span\": settling the state leaves {member} as the create \
             stored it, got {:?}",
            settled["routerNetworkInterface"][member]
        );
    }

    // An interface whose delete has been accepted is still readable, in DELETING, until the
    // clock takes the record away.
    let (status, _, deleted) = delete_network_interface(&emulator, &arn).await;
    assert_eq!(
        status, 200,
        "the delete that puts the interface on its way out is served, got {deleted}"
    );
    let (status, _, deleting) = get_network_interface(&emulator, &arn).await;
    assert_eq!(
        (
            status,
            deleting["routerNetworkInterface"]["state"].as_str()
        ),
        (200, Some(mc::ROUTER_NETWORK_INTERFACE_STATE_DELETING)),
        "case \"mid-deletion\": the interface is still readable, in DELETING, got {deleting}"
    );

    // And once the removal the delete owes is due, the record stops existing.
    tokio::time::sleep(past_transition_span()).await;
    let (status, code, gone) = get_network_interface(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "case \"after the removal came due\": the interface is gone, got {status} {code:?}: \
         {gone}"
    );
}

/// An ARN the store holds no router network interface for is the modelled `NotFoundException`
/// (404) in this service's not-found envelope, and a label outside the modelled
/// `RouterNetworkInterfaceArn` pattern never reaches the handler at all —
/// `BadRequestException` (400). Never a 501 and never a success.
///
/// The rows differ along the way the ARN addresses nothing: never minted, another account,
/// another Region and partition, and an interface of this store whose removal the clock has
/// already made due.
#[tokio::test]
async fn an_arn_the_store_holds_no_interface_for_is_not_found() {
    let emulator = Emulator::new();

    // One live interface, so a refusal below cannot be an empty store answering everything the
    // same way.
    let (status, _, live) = create_network_interface(
        &emulator,
        json!({
            "name": "get-rni-live",
            "configuration": public_configuration(1),
        }),
    )
    .await;
    assert_eq!(status, 201, "the live create expected 201, got {live}");
    let live_arn = live["routerNetworkInterface"]["arn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    // An interface this store did hold, deleted and then taken away by the clock:
    // `RouterNetworkInterfaceState` declares no terminal state a deleted one could be reported
    // in.
    let (status, _, doomed) = create_network_interface(
        &emulator,
        json!({
            "name": "get-rni-doomed",
            "configuration": public_configuration(0),
        }),
    )
    .await;
    assert_eq!(status, 201, "the create expected 201, got {doomed}");
    let doomed_arn = doomed["routerNetworkInterface"]["arn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();
    let (status, _, accepted) = delete_network_interface(&emulator, &doomed_arn).await;
    assert_eq!(status, 200, "the delete is served, got {accepted}");

    // While the removal is not due, that interface is still a readable record.
    owe_removal(
        &emulator,
        &doomed_arn,
        SystemTime::now() + NOT_DUE_FOR_A_WHILE,
    );
    let (status, code, mid) = get_network_interface(&emulator, &doomed_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case \"removal not yet due\": the interface is still readable, got {status} {code:?}: \
         {mid}"
    );
    // Once the clock has made it due, the record stops existing.
    owe_removal(&emulator, &doomed_arn, SystemTime::now() - DUE_A_WHILE_AGO);

    let unresolved = vec![
        ("never minted", UNKNOWN_ARN.to_string()),
        (
            "another account",
            format!("arn:aws:mediaconnect:{REGION}:111122223333:routerNetworkInterface:0123456789ab"),
        ),
        (
            "another Region and partition",
            "arn:aws-cn:mediaconnect:cn-north-1:000000000000:routerNetworkInterface:0123456789ab"
                .to_string(),
        ),
        (
            "an interface whose removal the clock has made due",
            doomed_arn.clone(),
        ),
    ];
    for (case, arn) in unresolved {
        let (status, code, body) = get_network_interface(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected the modelled NotFoundException, got {status} {code:?}: \
             {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(mc::not_found_message(&arn).as_str()),
            "case {case:?}: the message is this service's not-found envelope naming the \
             identifier the request carried, got {body}"
        );
    }

    // Labels that are not `RouterNetworkInterfaceArn`s at all: the id is the part the pattern
    // fixes exactly (twelve lowercase alphanumerics), so these are refused before the handler.
    let malformed = [
        (
            "id too short",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerNetworkInterface:0123456789a"),
        ),
        (
            "id too long",
            format!(
                "arn:aws:mediaconnect:{REGION}:000000000000:routerNetworkInterface:0123456789abc"
            ),
        ),
        (
            "id not lowercase",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerNetworkInterface:0123456789AB"),
        ),
        (
            "an ARN of this service that is not a network interface ARN",
            FLOW_ARN.to_string(),
        ),
        (
            "not an arn at all",
            "routerNetworkInterface:0123456789ab".to_string(),
        ),
    ];
    for (case, arn) in malformed {
        let (status, code, body) = get_network_interface(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: a label failing the modelled pattern is refused at the boundary, \
             got {status} {code:?}: {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {case:?}: the refusal carries this service's validation envelope, got {body}"
        );
    }

    // The live interface is still readable, and still its own record: no refusal above touched
    // it.
    let (status, code, read_back) = get_network_interface(&emulator, &live_arn).await;
    assert_eq!(
        (
            status,
            code.as_deref(),
            read_back["routerNetworkInterface"]["name"].as_str(),
        ),
        (200, None, Some("get-rni-live")),
        "the live interface is unaffected by the refusals, got {read_back}"
    );
}

/// A read changes nothing a later caller sees: repeated reads answer the same body, the stored
/// record is untouched, and the store holds exactly the interfaces the creates made.
#[tokio::test]
async fn repeated_reads_change_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let built = created_rows(&emulator).await;
    let created_count = built.len();

    // Past the span, so the only write a read is allowed to make — settling a transition the
    // clock already owed — has nothing left to do.
    tokio::time::sleep(past_transition_span()).await;
    for row in &built {
        let case = row.case;
        let arn = row.arn.as_str();
        let (_, _, first) = get_network_interface(&emulator, arn).await;
        let before = stored_record(&emulator, arn);
        let (_, _, second) = get_network_interface(&emulator, arn).await;
        let (_, _, third) = get_network_interface(&emulator, arn).await;
        assert_eq!(
            first, second,
            "case {case:?}: a read must not change what the next one sees, got {first} then \
             {second}"
        );
        assert_eq!(
            second, third,
            "case {case:?}: a third read answers the same body again, got {second} then {third}"
        );
        assert_eq!(
            stored_record(&emulator, arn),
            before,
            "case {case:?}: a read leaves the stored record exactly as it was, got {:?}",
            stored_record(&emulator, arn)
        );
    }
    assert_eq!(
        stored_count(&emulator),
        created_count,
        "the reads stored no router network interface, got {} stored",
        stored_count(&emulator)
    );
}
