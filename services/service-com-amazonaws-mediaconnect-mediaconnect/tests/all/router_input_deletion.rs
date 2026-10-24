// SPDX-License-Identifier: CC0-1.0

//! DeleteRouterInput, end to end over HTTP.
//!
//! The rule the cases below pin: a `DELETE /v1/routerInput/{Arn}` whose label satisfies the
//! modelled `RouterInputArn` pattern resolves that label against the STORE — an ARN the
//! store holds no router input for is refused `NotFoundException` (404) in this service's
//! not-found envelope, and a label outside the pattern never reaches the store at all — and
//! for the input it
//! does hold it starts a deletion that is neither instantaneous nor a soft delete: the
//! response reports that input's own ARN and name with state `DELETING` ("the current
//! state of the deleted router input, indicating where it is in the deletion process",
//! `smithy_query(query="doc",shape="DeleteRouterInputResponse",member="State")`), the
//! record stays `DELETING` for one [`mc::TRANSITION_SPAN`] — so a repeat of the
//! `@idempotent` call in that window answers the same response — and then it stops
//! existing, because `RouterInputState`
//! (`smithy_query(query="shape",shape="RouterInputState")`) declares no terminal `DELETED`
//! value a deleted input could be reported in.
//!
//! Each table's rows differ along what the rule generalises over: the state the addressed
//! input was in, the name it carries, how many inputs the store holds, how often the delete
//! is repeated and when, and the form of an ARN that addresses nothing.
//!
//! Each row is set up in the store rather than through CreateRouterInput, the way
//! `lifecycle_preconditions` sets up a flow status no implemented operation reaches: the
//! table spans every value of `RouterInputState`, and a create only ever answers `CREATING`.
//! Every call under test still travels the whole request path (routing, restJson1
//! deserialization, constraint validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`].

use std::time::{Duration, SystemTime};

use serde_json::Value;

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::{
    DefaultMaintenanceConfiguration, MaintenanceConfiguration, RouterInputConfiguration,
    RouterInputProtocolConfiguration, RtpRouterInputConfiguration,
    StandardRouterInputConfiguration,
};
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};

/// Longer than [`mc::TRANSITION_SPAN`], so a removal the clock owes is certainly due.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// Every value of the modelled `RouterInputState` enum
/// (`smithy_query(query="shape",shape="RouterInputState")`): CREATING | STANDBY | STARTING
/// | ACTIVE | STOPPING | DELETING | UPDATING | ERROR | RECOVERING | MIGRATING.
///
/// A delete addresses a router input whatever state it is in — AWS documents no
/// precondition on DeleteRouterInput (`smithy_query(query="doc",shape="DeleteRouterInput")`:
/// "Deletes a router input from AWS Elemental MediaConnect.") — so the whole catalog is a
/// row of the table below, including the states no implemented operation reaches.
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

/// Names a router input can carry, varying along length and character class.
///
/// `RouterInput$Name` targets a plain `String`
/// (`smithy_query(query="shape",shape="RouterInput",member="Name")`) and CreateRouterInput
/// bounds the request member at `@length(min: 1, max: 128)`
/// (`smithy_query(query="operation",operation="CreateRouterInput")`), so a stored name can
/// be anything in that span: one character, both published bounds, mixed case, separators
/// and multi-byte text.
fn router_input_names() -> Vec<String> {
    vec![
        "a".to_string(),
        "router-input-1".to_string(),
        "Router_Input.2".to_string(),
        "ri 3".to_string(),
        "ルーター入力".to_string(),
        "n".repeat(128),
    ]
}

/// Put one router input straight into the store and answer the ARN it is keyed by.
///
/// Shaped like a created one: the ARN is minted through the crate's own
/// [`mc::router_input_resource_fragment`], the configuration is the smallest CreateRouterInput
/// would accept and the derived members are the ones that create fills in, and the record is
/// live (only DeleteRouterInput owes its removal). The STATE, though, is the row's — which is
/// why the record is seeded rather than created: a create only ever answers `CREATING`.
///
/// `pub(crate)` because the router input restart suite fills the store through it too (see
/// `tests/all/router_input_restart.rs`), so there is one seeding helper rather than one per
/// suite.
pub(crate) fn seed_router_input(emulator: &Emulator, name: &str, state: &str) -> String {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = store.lock();
    let suffix = store.next_suffix();
    let id = mc::router_resource_id(suffix);
    let arn = engine::emulator_arn(
        mc::SERVICE_NAMESPACE,
        REGION,
        &mc::router_input_resource_fragment(&id),
    );
    let now = SystemTime::now();
    store.router_inputs.insert(
        arn.clone(),
        mc::StoredRouterInput {
            arn: arn.clone(),
            id,
            name: name.to_string(),
            state: state.to_string(),
            pending: None,
            configuration: RouterInputConfiguration::Standard(StandardRouterInputConfiguration {
                network_interface_arn:
                    "arn:aws:mediaconnect:us-east-1:000000000000:routerNetworkInterface:0000000000a1"
                        .to_string(),
                protocol_configuration: RouterInputProtocolConfiguration::Rtp(
                    RtpRouterInputConfiguration {
                        port: 5000,
                        forward_error_correction: None,
                    },
                ),
                protocol: Some("RTP".to_string()),
            }),
            maximum_bitrate: 20_000_000,
            tier: "INPUT_20".to_string(),
            routing_scope: "REGIONAL".to_string(),
            region_name: REGION.to_string(),
            availability_zone: mc::default_availability_zone(REGION),
            transit_encryption: mc::default_router_input_transit_encryption(),
            maintenance_configuration: MaintenanceConfiguration::Default(
                DefaultMaintenanceConfiguration {},
            ),
            content_quality_analysis_configuration:
                mc::default_router_content_quality_analysis_configuration(),
            tags: std::collections::BTreeMap::new(),
            ip_address: Some(mc::assigned_ingest_ip(suffix)),
            client_token: None,
            created_at: now,
            updated_at: now,
            removal_due_at: None,
        },
    );
    arn
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

async fn delete_router_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerInput/{arn}"), None)
        .await
}

/// The ARN every row of the table asserts the minted form of, so a seeded record is
/// addressed by an ARN a client could really have received.
fn assert_minted_router_input_arn(case: &str, arn: &str) {
    assert!(
        mc::is_mintable_router_input_arn(arn),
        "case {case:?}: the ARN must satisfy the modelled RouterInputArn pattern, got {arn:?}"
    );
    let segments: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        segments.len(),
        7,
        "case {case:?}: a router input ARN has 7 colon-separated segments, got {arn:?}"
    );
    assert_eq!(
        (segments[0], segments[2], segments[3], segments[5]),
        ("arn", "mediaconnect", REGION, "routerInput"),
        "case {case:?}: ARN prefix, service, Region and resource type, got {arn:?}"
    );
    assert!(
        segments[1].starts_with("aws"),
        "case {case:?}: the partition is an aws partition, got {arn:?}"
    );
    assert!(
        segments[4].len() == 12 && segments[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: the account id is 12 digits, got {arn:?}"
    );
    assert!(
        segments[6].len() == 12
            && segments[6]
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
        "case {case:?}: the router input id is 12 lowercase alphanumerics, got {arn:?}"
    );
}

/// A delete answers the addressed input's own identity and `DELETING`, from whatever state
/// that input was in and whatever name it carries — never a value fixed in the handler.
#[tokio::test]
async fn a_delete_reports_the_addressed_input_in_deleting() {
    let emulator = Emulator::new();
    let names = router_input_names();

    for (index, state) in ROUTER_INPUT_STATES.iter().enumerate() {
        // Pair each state with a different name, so the table varies along both.
        let name = &names[index % names.len()];
        let case = format!("state {state}, name {name:?}");
        let arn = seed_router_input(&emulator, name, state);
        assert_minted_router_input_arn(&case, &arn);

        let (status, code, body) = delete_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: deleting a stored router input is served, got {status} {code:?}: \
             {body}"
        );
        assert_eq!(
            body["arn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the response echoes the deleted input's ARN, got {body}"
        );
        assert_eq!(
            body["name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: the response carries the stored name, got {body}"
        );
        assert_eq!(
            body["state"].as_str(),
            Some("DELETING"),
            "case {case:?}: the response reports the deletion in progress, got {body}"
        );
        assert_eq!(
            stored_state(&emulator, &arn).as_deref(),
            Some("DELETING"),
            "case {case:?}: the stored input moved into DELETING, got {:?}",
            stored_state(&emulator, &arn)
        );
    }
}

/// DeleteRouterInput is `@idempotent`
/// (`smithy_query(query="operation",operation="DeleteRouterInput")`), so repeating it while
/// the deletion is still in progress answers the same response rather than restarting the
/// removal or refusing — however often it is repeated.
#[tokio::test]
async fn a_repeated_delete_answers_the_same_response_while_the_input_is_deleting() {
    let emulator = Emulator::new();

    for repeats in [1_usize, 2, 5] {
        let case = format!("{repeats} repeat(s)");
        let arn = seed_router_input(&emulator, &format!("idempotent-{repeats}"), "ACTIVE");
        let (status, code, first) = delete_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the first delete is served, got {status} {code:?}: {first}"
        );

        for repeat in 1..=repeats {
            let (status, code, again) = delete_router_input(&emulator, &arn).await;
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
            Some("DELETING"),
            "case {case:?}: the repeats left one record in DELETING"
        );
    }
}

/// The deletion is not instantaneous and not a soft delete: the input is still there, in
/// `DELETING`, for the whole transition span, and once the clock has made its removal due
/// the record stops existing — so the next call addressing it is a not-found, and a call
/// addressing another input materialises the removal on the way past.
#[tokio::test]
async fn a_deleted_input_stops_existing_once_its_removal_comes_due() {
    let emulator = Emulator::new();
    let deleted = seed_router_input(&emulator, "deleted-input", "STANDBY");
    let sibling = seed_router_input(&emulator, "sibling-input", "STANDBY");

    let (status, code, body) = delete_router_input(&emulator, &deleted).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the delete is served, got {status} {code:?}: {body}"
    );
    // The span is non-zero, so a caller acting straight after the delete still observes the
    // input in its transient state.
    assert_eq!(
        stored_state(&emulator, &deleted).as_deref(),
        Some("DELETING"),
        "straight after the delete the input is still stored, in DELETING"
    );

    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // A call addressing the SIBLING is what materialises the removal the clock already
    // owed: no delete of the removed input was needed to make it disappear.
    let (status, code, sibling_body) = delete_router_input(&emulator, &sibling).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the sibling's delete is served, got {status} {code:?}: {sibling_body}"
    );
    assert_eq!(
        stored_state(&emulator, &deleted),
        None,
        "the input whose removal came due stops existing, got {:?}",
        stored_state(&emulator, &deleted)
    );

    let (status, code, body) = delete_router_input(&emulator, &deleted).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "an input that no longer exists is a not-found, got {status} {code:?}: {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(mc::not_found_message(&deleted).as_str()),
        "the message is the request's own ARN in this service's envelope, got {body}"
    );
}

/// ARNs that satisfy the modelled `RouterInputArn` pattern — so they reach the handler as
/// identifiers — and that this store holds no router input for. The rows differ along the
/// way an ARN of that form can address nothing: never minted, another account, another
/// Region and partition.
fn arns_the_store_holds_nothing_for() -> Vec<(&'static str, String)> {
    vec![
        (
            "a router input ARN of the minted form that was never minted",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerInput:ri0000beef01"),
        ),
        (
            "a router input ARN of another account",
            format!("arn:aws:mediaconnect:{REGION}:111122223333:routerInput:ri0000beef02"),
        ),
        (
            "a router input ARN of another Region and partition",
            "arn:aws-cn:mediaconnect:cn-north-1:000000000000:routerInput:ri0000beef03".to_string(),
        ),
    ]
}

/// An ARN of the modelled form that the store holds no router input for is refused
/// `NotFoundException` (404) with the requested identifier in this service's not-found
/// envelope — whatever way that ARN addresses nothing.
#[tokio::test]
async fn a_delete_of_an_arn_the_store_holds_nothing_for_is_not_found() {
    let emulator = Emulator::new();
    // One live input, so a refusal below cannot be an empty store answering everything the
    // same way.
    let live = seed_router_input(&emulator, "live-input", "ACTIVE");

    for (case, arn) in arns_the_store_holds_nothing_for() {
        let (status, code, body) = delete_router_input(&emulator, &arn).await;
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
    let (status, code, body) = delete_router_input(&emulator, &live).await;
    assert_eq!(
        (status, code.as_deref(), body["state"].as_str()),
        (200, None, Some("DELETING")),
        "the one stored input is still served, got {status} {code:?}: {body}"
    );
}

/// Unlike a flow ARN, whose own member pattern admits almost anything and leaves the label
/// to the store (see `tests/all/error_envelopes.rs`), `Arn` on this operation declares the
/// narrow `RouterInputArn` pattern
/// (`smithy_query(query="operation",operation="DeleteRouterInput")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerInput:[a-z0-9]{12}$`), so a
/// label outside it is refused by the constraint walk BEFORE the handler — with the code
/// this service words a refused request in (`BadRequestException`, 400, the only
/// client-input error DeleteRouterInput models) and the `Validation failed:` envelope — and
/// never with the stub's 501 or a not-found. The rows break the pattern one way each:
/// character class, too short, too long, another resource type, no ARN at all.
#[tokio::test]
async fn a_delete_whose_label_breaks_the_modelled_pattern_is_refused_as_a_bad_request() {
    let emulator = Emulator::new();
    let live = seed_router_input(&emulator, "live-input", "STANDBY");

    let cases: Vec<(&str, String)> = vec![
        (
            "an id outside the modelled [a-z0-9] class",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerInput:RI_0000BEEF"),
        ),
        (
            "an eleven-character id, one short of the modelled length",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerInput:ri0000beef"),
        ),
        (
            "a thirteen-character id, one past the modelled length",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerInput:ri0000beef012"),
        ),
        (
            "another resource type of this service",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:flow:1-0000beef:f1"),
        ),
        ("not an ARN at all", "router-input-1".to_string()),
    ];

    for (case, arn) in cases {
        let (status, code, body) = delete_router_input(&emulator, &arn).await;
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

    // The positive control: the store's own input is untouched by any of those refusals and
    // is still served.
    assert_eq!(
        stored_state(&emulator, &live).as_deref(),
        Some("STANDBY"),
        "a refused request changed nothing in the store, got {:?}",
        stored_state(&emulator, &live)
    );
    let (status, code, body) = delete_router_input(&emulator, &live).await;
    assert_eq!(
        (status, code.as_deref(), body["state"].as_str()),
        (200, None, Some("DELETING")),
        "a label inside the modelled pattern that the store holds is served, got {status} \
         {code:?}: {body}"
    );
}

/// A delete touches ONLY the input its label addresses: every other router input in the
/// store keeps the state it had and stays exactly where it was, however many there are.
#[tokio::test]
async fn a_delete_touches_only_the_addressed_input() {
    let emulator = Emulator::new();

    // Five inputs in five different states, so an untouched record is recognisable by its
    // own state rather than by a value they all share.
    let seeded: Vec<(String, &str)> = ROUTER_INPUT_STATES
        .iter()
        .take(5)
        .enumerate()
        .map(|(index, state)| {
            (
                seed_router_input(&emulator, &format!("input-{index}"), state),
                *state,
            )
        })
        .collect();
    let before = stored_arns(&emulator);
    assert_eq!(
        before.len(),
        seeded.len(),
        "the store holds every seeded input, got {before:?}"
    );

    // Delete the middle one, so rows on both sides of it are asserted untouched.
    let (addressed, _) = &seeded[2];
    let (status, code, body) = delete_router_input(&emulator, addressed).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the delete is served, got {status} {code:?}: {body}"
    );

    for (arn, state) in &seeded {
        let expected = if arn == addressed { "DELETING" } else { *state };
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
