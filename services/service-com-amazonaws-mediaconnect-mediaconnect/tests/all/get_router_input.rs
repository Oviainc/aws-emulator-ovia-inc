// SPDX-License-Identifier: CC0-1.0

//! GetRouterInput, end to end over HTTP.
//!
//! The rule the cases below pin: a reporting `GET /v1/routerInput/{Arn}` resolves its
//! `@httpLabel` ARN against the STORE and answers the record that ARN addresses —
//! "Retrieves information about a specific router input"
//! (`smithy_query(query="doc",shape="GetRouterInput")`) — every member computed from that
//! record and never a value fixed in the handler, an ARN the store holds no input for
//! refused with the modelled `NotFoundException` (404), and nothing a later caller sees
//! changed by the read except a state change the clock had already made due.
//!
//! Each table's rows differ along what the rule generalises over: which kind of router
//! input configuration the addressed input was created with, its tier, bitrate, tags and
//! name, which state the stored record is in (the whole modelled `RouterInputState`
//! catalog), how many inputs the store holds beside the addressed one, how often the read
//! is repeated and when, and the way an ARN can address nothing.
//!
//! Every call travels the whole request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`]; the records whose state a create cannot reach are
//! seeded through the store with the shared
//! [`crate::router_input_deletion::seed_router_input`], the way the deletion suite seeds
//! one. The configuration-kind table, the create body and the union payload reader are the
//! shared [`crate::router_input_restart::configuration_rows`] /
//! [`crate::router_input_restart::create_body`] /
//! [`crate::router_input_restart::union_member`], so the modelled union catalog they encode
//! exists once for every router input suite; what this suite layers on top is the per-row
//! name, tier, bitrate and tag set a reader must report back, the modelled state catalog it
//! seeds over, and the ways an ARN can address nothing.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_input_deletion::seed_router_input;
use crate::router_input_restart::{configuration_rows, create_body, union_member};

/// Every value of the modelled `RouterInputState` enum
/// (`smithy_query(query="shape",shape="RouterInputState")`): CREATING | STANDBY | STARTING |
/// ACTIVE | STOPPING | DELETING | UPDATING | ERROR | RECOVERING | MIGRATING.
///
/// The reader retrieves the input's information whatever state it is in, so the whole
/// catalog is a row of the state table below — including the states no implemented
/// operation reaches yet.
const ROUTER_INPUT_STATES: [&str; 10] = [
    "CREATING",
    "STANDBY",
    "STARTING",
    "ACTIVE",
    "STOPPING",
    "DELETING",
    "UPDATING",
    "ERROR",
    "RECOVERING",
    "MIGRATING",
];

/// ARNs that satisfy the modelled `RouterInputArn` pattern — so they reach the handler as
/// identifiers — and that this store holds no router input for. The rows differ along the
/// way an ARN of that form can address nothing: never minted, another account, another
/// Region and partition.
fn arns_the_store_holds_nothing_for() -> Vec<(&'static str, String)> {
    vec![
        (
            "a router input ARN of the minted form that was never minted",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerInput:ri0000beef11"),
        ),
        (
            "a router input ARN of another account",
            format!("arn:aws:mediaconnect:{REGION}:111122223333:routerInput:ri0000beef12"),
        ),
        (
            "a router input ARN of another Region and partition",
            "arn:aws-cn:mediaconnect:cn-north-1:000000000000:routerInput:ri0000beef13".to_string(),
        ),
    ]
}

/// The state the store holds for `arn`, or `None` when it holds no router input for it.
fn stored_state(emulator: &Emulator, arn: &str) -> Option<String> {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    store
        .router_inputs
        .get(arn)
        .map(|router_input| router_input.state.clone())
}

/// Every router input ARN the store holds, in its stable order.
fn stored_arns(emulator: &Emulator) -> Vec<String> {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    store.router_inputs.keys().cloned().collect()
}

/// Far enough ahead that a transition or removal the store owes is NOT due during a case,
/// and far enough behind that one is certainly due — the two sides of the instant a reader
/// compares against.
const NOT_DUE_FOR_A_WHILE: Duration = Duration::from_secs(600);
/// See [`NOT_DUE_FOR_A_WHILE`].
const DUE_A_WHILE_AGO: Duration = Duration::from_secs(600);

/// Owe the stored router input `arn` the state change `settled`, entering `transient` now
/// and due at `due_at` — the record a call that starts a transition leaves behind
/// (`mc::StoredRouterInput::begin_transition` writes exactly this, at
/// `now + mc::TRANSITION_SPAN`).
///
/// Written onto the record so the cases below do not race the create's own span: what they
/// assert is what the READER does with a transition that is owed and not yet due, and with
/// one the clock has already made due. The real span is exercised over the wire by
/// `a_restart_is_refused_while_a_state_change_is_in_flight` in `router_input_restart`.
fn owe_transition(
    emulator: &Emulator,
    arn: &str,
    transient: &str,
    settled: &str,
    due_at: SystemTime,
) {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = store.lock();
    let router_input = store
        .router_inputs
        .get_mut(arn)
        .expect("the store holds the router input under test");
    router_input.state = transient.to_string();
    router_input.pending = Some(mc::PendingStatus {
        status: settled.to_string(),
        due_at,
    });
}

/// Whether the store still owes the router input `arn` a state change.
fn transition_is_owed(emulator: &Emulator, arn: &str) -> bool {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    store
        .router_inputs
        .get(arn)
        .is_some_and(|router_input| router_input.pending.is_some())
}

/// Push the due instant of the transition the store already owes `arn` out past this case,
/// leaving the transient state and the owed outcome exactly as the operation that started it
/// recorded them.
///
/// Used after a call that starts a transition, so what the next read reports is that call's
/// own transient state rather than whatever the 500 ms span has settled by then.
fn defer_owed_transition(emulator: &Emulator, arn: &str) {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = store.lock();
    let router_input = store
        .router_inputs
        .get_mut(arn)
        .expect("the store holds the router input under test");
    let pending = router_input
        .pending
        .as_mut()
        .expect("the call under test left a transition owed");
    pending.due_at = SystemTime::now() + NOT_DUE_FOR_A_WHILE;
}

/// Make the removal DeleteRouterInput accepted for `arn` come due at `due_at`, the way the
/// delete records it (`mc::StoredRouterInput::begin_deletion`), for the same reason
/// [`owe_transition`] writes a transition.
fn owe_removal(emulator: &Emulator, arn: &str, due_at: SystemTime) {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = store.lock();
    let router_input = store
        .router_inputs
        .get_mut(arn)
        .expect("the store holds the router input under test");
    router_input.removal_due_at = Some(due_at);
}

async fn create_router_input(emulator: &Emulator, body: Value) -> (u16, Option<String>, Value) {
    emulator.send("POST", "/v1/routerInput", Some(body)).await
}

/// The operation under test: `GET /v1/routerInput/{Arn}`
/// (`smithy_query(query="operation",operation="GetRouterInput")`).
async fn get_router_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/routerInput/{arn}"), None)
        .await
}

async fn restart_router_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("POST", &format!("/v1/routerInput/restart/{arn}"), None)
        .await
}

async fn delete_router_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerInput/{arn}"), None)
        .await
}

/// A reported `RouterInput` without its `State`, so two reports of one record can be
/// compared across an instant at which the clock may have settled a transition it owed.
fn without_state(router_input: &Value) -> Value {
    let mut copy = router_input.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("state");
    }
    copy
}

/// The published tier catalog, as `RouterInput$Tier` documents it
/// (`smithy_query(query="shape",shape="RouterInput",member="Tier")`: "INPUT_20 – 20 Mbps,
/// INPUT_50 – 50 Mbps, INPUT_100 – 100 Mbps, INPUT_250 – 250 Mbps, INPUT_500 – 500 Mbps"),
/// written out here rather than read from the crate so the rows below vary the tier and
/// bitrate the reader must report over the whole published list, and so a bug in the
/// crate's own encoding of that list cannot hide here.
const TIERS: [&str; 5] = [
    "INPUT_20",
    "INPUT_50",
    "INPUT_100",
    "INPUT_250",
    "INPUT_500",
];

/// The reader answers the record its ARN addresses, for every kind of router input the
/// model declares and whatever tier, bitrate and tags that input carries — and never
/// another input's record, though the store holds one per kind.
///
/// The rows are the five `RouterInputConfiguration` union members (the create table
/// `router_input_restart` owns), each created with its own name, tier, bitrate and tag set,
/// so what the reader reports has to be computed per record.
#[tokio::test]
async fn get_reports_the_router_input_its_arn_addresses() {
    let emulator = Emulator::new();

    struct Row {
        case: String,
        member_name: &'static str,
        json_name: &'static str,
        input_type: &'static str,
        name: String,
        tier: &'static str,
        maximum_bitrate: i64,
        tags: Value,
        created: Value,
    }

    let mut rows: Vec<Row> = Vec::new();
    for (index, (kind, member_name, json_name, configuration, input_type)) in
        configuration_rows().into_iter().enumerate()
    {
        let tier = TIERS[index % TIERS.len()];
        let maximum_bitrate = mc::router_input_tier_bitrate_ceiling(tier)
            .expect("every published tier has a published ceiling");
        // The tag set varies in SIZE as well as content: none, one pair, two pairs.
        let tags = match index % 3 {
            0 => json!({}),
            1 => json!({ "stage": format!("stage-{index}") }),
            _ => json!({ "stage": format!("stage-{index}"), "team": "video" }),
        };
        let name = format!("get-{kind}").replace('/', "-");
        let mut body = create_body(&name, configuration);
        body["tier"] = json!(tier);
        body["maximumBitrate"] = json!(maximum_bitrate);
        body["tags"] = tags.clone();

        let (status, code, created) = create_router_input(&emulator, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {kind:?}: CreateRouterInput answers 201, got {created}"
        );
        rows.push(Row {
            case: kind.to_string(),
            member_name,
            json_name,
            input_type,
            name,
            tier,
            maximum_bitrate,
            tags,
            created: created["routerInput"].clone(),
        });
    }

    for row in &rows {
        let case = &row.case;
        let arn = row.created["arn"].as_str().unwrap_or_default().to_string();
        let (status, code, read_back) = get_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: GetRouterInput answers 200 for an input the store holds, got \
             {read_back}"
        );
        let reported = &read_back["routerInput"];
        assert_eq!(
            (
                reported["arn"].as_str(),
                reported["name"].as_str(),
                reported["inputType"].as_str(),
                reported["tier"].as_str(),
                reported["maximumBitrate"].as_i64(),
                reported["routingScope"].as_str(),
                reported["regionName"].as_str(),
            ),
            (
                Some(arn.as_str()),
                Some(row.name.as_str()),
                Some(row.input_type),
                Some(row.tier),
                Some(row.maximum_bitrate),
                Some("REGIONAL"),
                Some(REGION),
            ),
            "case {case:?}: the reader reports this input's own identity and configuration, \
             got {reported}"
        );
        assert_eq!(
            &reported["tags"], &row.tags,
            "case {case:?}: the tags this input was created with, got {reported}"
        );
        assert_eq!(
            reported["id"].as_str(),
            arn.split(':').next_back(),
            "case {case:?}: Id is the id the addressing ARN carries, got {reported}"
        );
        assert!(
            !union_member(&reported["configuration"], row.member_name, row.json_name).is_null(),
            "case {case:?}: Configuration reports the {} member the create carried, got \
             {reported}",
            row.member_name
        );
        assert!(
            !union_member(&reported["streamDetails"], row.member_name, row.json_name).is_null(),
            "case {case:?}: StreamDetails follows from that configuration, got {reported}"
        );
        assert_eq!(
            without_state(reported),
            without_state(&row.created),
            "case {case:?}: the reader reports the record the create stored, got {reported}"
        );
    }

    // No two reads answered the same record: every ARN addresses its own input, though the
    // store holds one per kind.
    let mut reported_ids = Vec::new();
    for row in &rows {
        let arn = row.created["arn"].as_str().unwrap_or_default();
        let (_, _, read_back) = get_router_input(&emulator, arn).await;
        reported_ids.push(read_back["routerInput"]["id"].as_str().unwrap_or("").to_string());
    }
    let mut distinct = reported_ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        rows.len(),
        "every ARN addresses a distinct record, got {reported_ids:?}"
    );
}

/// The `State` the reader reports is the one the STORE holds — for every value of the
/// modelled `RouterInputState` enum, including the states no implemented operation reaches
/// yet.
///
/// The catalog is the model's own
/// (`smithy_query(query="shape",shape="RouterInputState")`: CREATING | STANDBY | STARTING |
/// ACTIVE | STOPPING | DELETING | UPDATING | ERROR | RECOVERING | MIGRATING), shared with
/// the deletion suite. The records are seeded because a create only ever answers
/// `CREATING`.
#[tokio::test]
async fn the_reader_reports_the_state_the_store_holds() {
    let emulator = Emulator::new();

    let mut seeded: Vec<(&str, String)> = Vec::new();
    for state in ROUTER_INPUT_STATES {
        let name = format!("state-{state}");
        let arn = seed_router_input(&emulator, &name, state);
        seeded.push((state, arn));
    }

    for (state, arn) in &seeded {
        let name = format!("state-{state}");
        let (status, code, read_back) = get_router_input(&emulator, arn).await;
        assert_eq!(
            (
                status,
                code.as_deref(),
                read_back["routerInput"]["state"].as_str(),
                read_back["routerInput"]["name"].as_str(),
            ),
            (200, None, Some(*state), Some(name.as_str())),
            "case {state:?}: the reader reports the stored state and name, got {read_back}"
        );
    }

    // Reading them all left every record in the state it was seeded in: a read reports a
    // state, it never moves one.
    for (state, arn) in &seeded {
        assert_eq!(
            stored_state(&emulator, arn).as_deref(),
            Some(*state),
            "case {state:?}: the stored state is unchanged by the reads, got {:?}",
            stored_state(&emulator, arn)
        );
    }
}

/// A read changes nothing a later caller sees: repeated reads of one input answer the same
/// record, a read of one input leaves another's report alone, the store keeps exactly the
/// records the creates minted, and `UpdatedAt` moves only when an operation that MUTATES
/// the input runs ("a restart is such a change; a read never moves it").
///
/// The one write the reporting operation does make is the state change the clock had
/// already made due: the create's `CREATING` -> `STANDBY` transition, which is still
/// reported as `CREATING` by the reads taken before the span has passed and is materialised
/// in the store by the read taken after it.
#[tokio::test]
async fn reading_a_router_input_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = configuration_rows()
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    let (status, _, created) =
        create_router_input(&emulator, create_body("reported", configuration.clone())).await;
    assert_eq!(
        (status, created["routerInput"]["state"].as_str()),
        (201, Some(mc::ROUTER_INPUT_STATE_CREATING)),
        "the create under test is accepted and answers the transient state: {created}"
    );
    let arn = created["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (_, _, neighbour) =
        create_router_input(&emulator, create_body("neighbour", configuration)).await;
    let neighbour_arn = neighbour["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A transition the store owes and the clock has NOT made due: both reads report the
    // transient state, and neither settles the record early.
    owe_transition(
        &emulator,
        &arn,
        mc::ROUTER_INPUT_STATE_CREATING,
        mc::ROUTER_INPUT_STATE_STANDBY,
        SystemTime::now() + NOT_DUE_FOR_A_WHILE,
    );
    let (_, _, first) = get_router_input(&emulator, &arn).await;
    let (_, _, second) = get_router_input(&emulator, &arn).await;
    assert_eq!(
        first, second,
        "case \"two reads mid-transition\": the second read answers what the first did, got \
         {second}"
    );
    assert_eq!(
        first["routerInput"]["state"].as_str(),
        Some(mc::ROUTER_INPUT_STATE_CREATING),
        "case \"two reads mid-transition\": the transient state is observable, got {first}"
    );
    assert_eq!(
        stored_state(&emulator, &arn).as_deref(),
        Some(mc::ROUTER_INPUT_STATE_CREATING),
        "case \"two reads mid-transition\": the reads did not settle the record early, got \
         {:?}",
        stored_state(&emulator, &arn)
    );
    assert!(
        transition_is_owed(&emulator, &arn),
        "case \"two reads mid-transition\": the owed transition is still owed after the reads"
    );

    // The same transition, now one the clock HAS made due: the read materialises it, so the
    // next caller sees the settled state although nothing but the clock moved it.
    owe_transition(
        &emulator,
        &arn,
        mc::ROUTER_INPUT_STATE_CREATING,
        mc::ROUTER_INPUT_STATE_STANDBY,
        SystemTime::now() - DUE_A_WHILE_AGO,
    );

    let (_, _, settled) = get_router_input(&emulator, &arn).await;
    assert_eq!(
        settled["routerInput"]["state"].as_str(),
        Some(mc::ROUTER_INPUT_STATE_STANDBY),
        "case \"read after the change came due\": the clock's change is materialised, got \
         {settled}"
    );
    assert_eq!(
        stored_state(&emulator, &arn).as_deref(),
        Some(mc::ROUTER_INPUT_STATE_STANDBY),
        "case \"read after the change came due\": the settled state is PERSISTED, not just \
         reported, got {:?}",
        stored_state(&emulator, &arn)
    );
    assert!(
        !transition_is_owed(&emulator, &arn),
        "case \"read after the change came due\": the store owes the input nothing further"
    );
    let (_, _, again) = get_router_input(&emulator, &arn).await;
    assert_eq!(
        again, settled,
        "case \"read twice after the change came due\": one record, reported identically, got \
         {again}"
    );
    assert_eq!(
        (
            again["routerInput"]["createdAt"].as_str(),
            again["routerInput"]["updatedAt"].as_str(),
        ),
        (
            created["routerInput"]["createdAt"].as_str(),
            created["routerInput"]["updatedAt"].as_str(),
        ),
        "case \"read twice after the change came due\": no read moved CreatedAt or UpdatedAt, \
         got {again}"
    );

    // Reading one input left the other one's record alone.
    let (_, _, neighbour_read) = get_router_input(&emulator, &neighbour_arn).await;
    assert_eq!(
        without_state(&neighbour_read["routerInput"]),
        without_state(&neighbour["routerInput"]),
        "case \"the neighbour\": the reads of another input changed nothing here, got \
         {neighbour_read}"
    );

    // The store holds exactly the two records the creates minted: no read added, removed or
    // duplicated one.
    let mut held = stored_arns(&emulator);
    held.sort();
    let mut expected = vec![arn.clone(), neighbour_arn.clone()];
    expected.sort();
    assert_eq!(
        held, expected,
        "the reads left the store holding exactly the created inputs, got {held:?}"
    );

    // A MUTATION is what moves `UpdatedAt`, which no read above did.
    let (status, _, restarted) = restart_router_input(&emulator, &arn).await;
    assert_eq!(
        status, 202,
        "the restart that mutates the record is accepted: {restarted}"
    );
    // The restart owes the clock a change of its own; its due instant is pushed out so what
    // the read below reports is the restart's transient state, not the span's outcome.
    defer_owed_transition(&emulator, &arn);
    let (_, _, after_restart) = get_router_input(&emulator, &arn).await;
    assert_eq!(
        after_restart["routerInput"]["state"].as_str(),
        Some(mc::ROUTER_INPUT_STATE_RECOVERING),
        "case \"read after a mutation\": the reader reports the change, got {after_restart}"
    );
    assert_ne!(
        after_restart["routerInput"]["updatedAt"],
        created["routerInput"]["updatedAt"],
        "case \"read after a mutation\": a mutation moves UpdatedAt, got {after_restart}"
    );
    assert_eq!(
        after_restart["routerInput"]["createdAt"],
        created["routerInput"]["createdAt"],
        "case \"read after a mutation\": CreatedAt is the instant the input was created, \
         got {after_restart}"
    );
}

/// An ARN the store holds no router input for is the modelled `NotFoundException` (404) in
/// this service's not-found envelope, and a label outside the modelled `RouterInputArn`
/// pattern never reaches the handler at all — `BadRequestException` (400). Never a 501 and
/// never a success.
///
/// The rows differ along the way the ARN addresses nothing: never minted, another account,
/// another Region and partition, and an input of this store whose deletion the clock has
/// already made due.
#[tokio::test]
async fn an_arn_the_store_holds_no_router_input_for_is_not_found() {
    let emulator = Emulator::new();
    // One live input, so a refusal below cannot be an empty store answering everything the
    // same way.
    let live = seed_router_input(&emulator, "live-input", "ACTIVE");

    let mut unresolved = arns_the_store_holds_nothing_for();

    // An input this store did hold, deleted and then taken away by the clock:
    // `RouterInputState` declares no terminal state a deleted input could be reported in.
    let deleted = seed_router_input(&emulator, "deleted-input", "STANDBY");
    let (status, _, accepted) = delete_router_input(&emulator, &deleted).await;
    assert_eq!(
        status, 200,
        "the delete that puts the input on its way out is served: {accepted}"
    );
    // While the removal the delete owes is not due, the input is still a readable record.
    owe_removal(&emulator, &deleted, SystemTime::now() + NOT_DUE_FOR_A_WHILE);
    let (status, code, deleting) = get_router_input(&emulator, &deleted).await;
    assert_eq!(
        (
            status,
            code.as_deref(),
            deleting["routerInput"]["state"].as_str(),
        ),
        (200, None, Some(mc::ROUTER_INPUT_STATE_DELETING)),
        "case \"mid-deletion\": the input is still readable, in DELETING, got {deleting}"
    );
    // Once the clock has made it due, the record stops existing.
    owe_removal(&emulator, &deleted, SystemTime::now() - DUE_A_WHILE_AGO);
    unresolved.push(("an input whose removal the clock has made due", deleted));

    for (case, arn) in unresolved {
        let (status, code, body) = get_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected the modelled NotFoundException, got {status} {code:?}: \
             {body}"
        );
        // The message is built from the request: it names the identifier the caller asked
        // about, whatever the wording around it.
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(arn.as_str()),
            "case {case:?}: the message names the identifier the request carried, got {body}"
        );
    }

    // Labels that are not `RouterInputArn`s at all: the id is the part the pattern fixes
    // exactly (twelve lowercase alphanumerics), so these are refused before the handler.
    let malformed = [
        (
            "id too short",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerInput:0000000000a"),
        ),
        (
            "id too long",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerInput:0000000000a1b"),
        ),
        (
            "id not lowercase",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerInput:0000000000A1"),
        ),
        ("not an arn at all", "routerInput:0000000000a1".to_string()),
    ];
    for (case, arn) in malformed {
        let (status, code, body) = get_router_input(&emulator, &arn).await;
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

    // The live input is still readable, and still its own record: no refusal above touched
    // it.
    let (status, code, read_back) = get_router_input(&emulator, &live).await;
    assert_eq!(
        (
            status,
            code.as_deref(),
            read_back["routerInput"]["name"].as_str(),
            read_back["routerInput"]["state"].as_str(),
        ),
        (200, None, Some("live-input"), Some("ACTIVE")),
        "the live input is unaffected by the refusals, got {read_back}"
    );
}
