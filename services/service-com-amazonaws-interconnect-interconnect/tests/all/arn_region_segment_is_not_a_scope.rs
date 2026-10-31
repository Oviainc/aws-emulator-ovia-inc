// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the Region segment of an `AmazonResourceName` carried by the
//! operations whose `arn` member IS the resource they act on, driven over the wire
//! through the shared [`crate::emulator`] harness.
//!
//! RULE under test: the Region segment of an ARN-addressed `arn` is a value the member
//! leaves unconstrained, so it addresses no scope of its own — the resource is the one
//! the ARN's resource segment names among those the Region the REQUEST is signed for
//! holds, and the scope judged against the request is the ARN's partition and account
//! alone. An `arn` of that scope which resolves to no resource is therefore not a
//! malformed request: the two tag WRITES refuse it as this service's own client fault,
//! worded with a fixed sentence that reproduces nothing of the ARN, and the `@readonly`
//! tag LISTING answers HTTP 200 with the empty tag set, because nothing was ever tagged.
//!
//! Every assertion is about that rule over a TABLE whose rows differ in the Region
//! segment the ARN carries (another continent's Region, a Region this emulator holds
//! nothing in, a Region name no partition publishes, a one-character Region, and one
//! long enough to take the ARN to its modelled length ceiling), in the Region the
//! request is signed for, in the resource segment (both id prefixes, an id that was
//! never allocated, an id of a live connection and one whose delete has settled), in
//! the environment and bandwidth the live resource sits on, and in the field that does
//! put an ARN outside the request's scope (account, partition) — crossed with every
//! operation whose input addresses its resource by `AmazonResourceName`
//! (`smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`:
//! each takes `arn` -> `AmazonResourceName`, `@length 59..150`, `@pattern
//! ^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$`,
//! whose Region segment is `[^:]+`).

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::json;

use service_com_amazonaws_interconnect_interconnect::state::parse_connection_arn;

use crate::arn_scope_refusals::{arn_operations, assert_answers_fault, Fault, TAG_READER};
use crate::emulator::{
    create_with_tags_in, message, placements_in, tags_of, text, Emulator, DEFAULT_SIGNING_REGION,
    SIGNING_NAME,
};

/// The account id every ARN this emulator mints carries, and so the account every
/// request it serves is for.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The partition every ARN this emulator mints carries.
const PARTITION: &str = "aws";

/// `AmazonResourceName` is modelled `@length(min: 59, max: 150)`
/// (`smithy_query(query="shape",shape="AmazonResourceName")`), so an ARN at either end
/// of that span is one the boundary validator hands to the handler.
const ARN_LENGTH: (usize, usize) = (59, 150);

/// The sentence this service words for an ARN-addressed refusal of a resource that is
/// not there, spelled out here rather than taken from the helper the handlers build it
/// with: entirely FIXED text, carrying no ARN, id, Region or resource type, so a
/// reworded helper fails this suite instead of passing it.
const RESOURCE_ABSENT_MESSAGE: &str = "Unable to process request: Resource does not exist or is deleted";

/// The Regions the rows sign their requests for, so no row's answer depends on the
/// harness default Region.
const CALLER_REGIONS: [&str; 2] = [DEFAULT_SIGNING_REGION, "ap-southeast-2"];

/// `ConnectionId`s no create ever allocates, one per prefix the id's pattern allows
/// (`^(mcc|lmcc)-[a-z0-9]{8}$`).
const NEVER_ALLOCATED_IDS: [&str; 2] = ["mcc-00000000", "lmcc-zzzzzzzz"];

/// Long enough to cover the provisioning span `state.rs` documents (400 ms) with
/// margin; the transition is due at a stored instant, so any wait past it settles.
const SETTLE_WAIT: Duration = Duration::from_millis(700);

/// A connection ARN with the fields a row wants, built the way the member's pattern
/// writes them: `arn:<partition>:interconnect:<region>:<account>:connection/<id>`.
fn connection_arn(partition: &str, region: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:{SIGNING_NAME}:{region}:{account}:connection/{id}")
}

/// The Region segments a row's ARN carries, every one of them a value the member's
/// `[^:]+` Region segment admits: a Region on another continent, one this emulator
/// holds nothing in, a Region name no partition publishes, a single character, and one
/// long enough to take the ARN to its modelled length ceiling for `id`.
fn region_segments(id: &str) -> Vec<(String, String)> {
    let fixed = connection_arn(PARTITION, "", ACCOUNT_ID, id).len();
    let longest = "r".repeat(ARN_LENGTH.1 - fixed);
    let mut rows: Vec<(String, String)> = [
        "another continent's Region",
        "a Region holding nothing",
        "a Region name no partition publishes",
        "a one-character Region",
    ]
    .into_iter()
    .zip(["eu-west-1", "eu-central-1", "not-a-region-9", "r"])
    .map(|(name, segment)| (name.to_string(), segment.to_string()))
    .collect();
    rows.push(("a Region at the ARN length ceiling".to_string(), longest));
    for (name, segment) in &rows {
        let arn = connection_arn(PARTITION, segment, ACCOUNT_ID, id);
        assert!(
            arn.len() >= ARN_LENGTH.0 && arn.len() <= ARN_LENGTH.1,
            "case {name:?}: expected an ARN inside the modelled {ARN_LENGTH:?} length, got {} characters",
            arn.len()
        );
    }
    rows
}

/// Assert every ARN-addressed operation answers `arn` the way the rule answers an
/// `arn` of the request's scope that resolves to NO resource: the two writes with the
/// fixed client-fault refusal, the read with HTTP 200 and the empty tag set. The
/// shared judge ([`assert_answers_fault`]) owns the status, the code and the wording;
/// what is added here is that the refusal reproduces nothing of the ARN.
async fn assert_resolves_to_nothing(emulator: &Emulator, caller: &str, arn: &str, case: &str) {
    let parsed = parse_connection_arn(arn).expect("the row's ARN is a connection ARN");
    for (operation, body) in arn_operations() {
        let (status, answer) = emulator.ic_in(caller, operation, body(arn)).await;
        assert_answers_fault(case, operation, Fault::NoSuchConnection, arn, status, &answer);
        if operation == TAG_READER {
            assert_eq!(
                tags_of(&answer),
                BTreeMap::new(),
                "case {case:?} on {operation}: expected no tag at all, got {:?}",
                answer["tags"]
            );
            continue;
        }
        // The refusal's sentence is fixed text, asserted here as text rather than
        // through the helper the handler builds it with — so nothing the caller sent
        // appears in it, and a reworded helper fails this case. (The Region segment is
        // covered by the whole ARN: a row spelling it as one character would otherwise
        // match the sentence's own letters.)
        let reported = message(&answer);
        assert_eq!(
            reported, RESOURCE_ABSENT_MESSAGE,
            "case {case:?} on {operation}: expected the fixed not-found sentence, got {reported:?}"
        );
        for value in [arn, parsed.account, parsed.id] {
            assert!(
                !reported.contains(value),
                "case {case:?} on {operation}: expected the refusal to reproduce nothing of the ARN, got {reported:?} carrying {value:?}"
            );
        }
    }
}

/// RULE: an `arn` whose resource segment names no connection the called endpoint holds
/// resolves to nothing WHATEVER its Region segment spells — the writes answer the fixed
/// client-fault refusal, the read answers the empty tag set — and nothing is created
/// for it.
#[tokio::test]
async fn an_unallocated_id_resolves_to_nothing_whatever_region_the_arn_spells() {
    let emulator = Emulator::new();

    for caller in CALLER_REGIONS {
        for id in NEVER_ALLOCATED_IDS {
            // The Region the request itself addresses is a row too: the spelling makes
            // no difference, so the one that matches the endpoint answers the same.
            let mut segments = region_segments(id);
            segments.push(("the called Region itself".to_string(), caller.to_string()));
            for (name, segment) in segments {
                let arn = connection_arn(PARTITION, &segment, ACCOUNT_ID, id);
                let case = format!("{name} / {id} / signed for {caller}");
                assert_resolves_to_nothing(&emulator, caller, &arn, &case).await;
                // Nothing above brought a resource into existence: the read still
                // answers the empty set rather than a tag the refused write asked for.
                let listed = emulator
                    .ic_ok_in(caller, TAG_READER, json!({ "arn": arn }))
                    .await;
                assert_eq!(
                    tags_of(&listed),
                    BTreeMap::new(),
                    "case {case:?}: expected the refused writes to have stored nothing, got {:?}",
                    listed["tags"]
                );
            }
        }
    }
}

/// RULE: the resource an `arn` addresses is the one its resource segment names among
/// the connections the called endpoint holds, so a LIVE connection is reached through
/// its own endpoint whichever Region its ARN spells — the listing reports that
/// connection's tags, a write lands on that connection and the identifier-addressed
/// reader reads the result back.
#[tokio::test]
async fn a_live_connection_is_reached_whatever_region_its_arn_spells() {
    let emulator = Emulator::new();
    let initial: BTreeMap<String, String> = [("env", "test"), ("team", "network")]
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();

    for (seed, caller) in CALLER_REGIONS.into_iter().enumerate() {
        let places = placements_in(&emulator, caller).await;
        for (offset, (name, segment)) in region_segments(NEVER_ALLOCATED_IDS[0])
            .into_iter()
            .enumerate()
        {
            // One connection per row, on a placement of the Region the request
            // addresses, so the rows differ in the environment and bandwidth too.
            let placement = &places[(seed + offset) % places.len()];
            let live = create_with_tags_in(
                &emulator,
                caller,
                placement,
                3_000 + seed * 10 + offset,
                &initial,
            )
            .await;
            let id = text(&live, "id");
            let stored_arn = text(&live, "arn");
            let respelled = connection_arn(PARTITION, &segment, ACCOUNT_ID, &id);
            let case = format!("{name} / {id} / signed for {caller}");
            assert_ne!(
                respelled, stored_arn,
                "case {case:?}: expected the row's ARN to differ from the stored one in its Region segment"
            );

            // The listing: this connection's own tag set, through an ARN whose Region
            // segment is not the one the resource was created in.
            let listed = emulator
                .ic_ok_in(caller, TAG_READER, json!({ "arn": respelled }))
                .await;
            assert_eq!(
                tags_of(&listed),
                initial,
                "case {case:?}: expected the live connection's tags {initial:?}, got {:?}",
                listed["tags"]
            );

            // A write through the same ARN lands on the same connection, and the
            // identifier-addressed reader reads the result back.
            emulator
                .ic_ok_in(
                    caller,
                    "TagResource",
                    json!({ "arn": respelled, "tags": { "owner": "ops" } }),
                )
                .await;
            emulator
                .ic_ok_in(
                    caller,
                    "UntagResource",
                    json!({ "arn": respelled, "tagKeys": ["team"] }),
                )
                .await;
            let expected: BTreeMap<String, String> = [("env", "test"), ("owner", "ops")]
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            let fetched = emulator
                .ic_ok_in(caller, "GetConnection", json!({ "identifier": id }))
                .await;
            assert_eq!(
                tags_of(&fetched["connection"]),
                expected,
                "case {case:?}: expected the writes to land on the connection the id names, got {:?}",
                fetched["connection"]["tags"]
            );
            // And the resource's own ARN reports the same set, so the two spellings
            // address one resource rather than two.
            let through_stored = emulator
                .ic_ok_in(caller, TAG_READER, json!({ "arn": stored_arn }))
                .await;
            assert_eq!(
                tags_of(&through_stored),
                expected,
                "case {case:?}: expected the stored ARN to report the same set, got {:?}",
                through_stored["tags"]
            );
        }
    }
}

/// RULE: the scope an ARN-addressed request IS judged against is the ARN's partition
/// and account — an ARN carrying either of another scope is a fault in the request
/// (`InterconnectValidationException`, HTTP 400) from every one of the three
/// operations, the READ included, whatever Region segment it spells.
#[tokio::test]
async fn an_arn_of_another_account_or_partition_is_a_request_fault() {
    let emulator = Emulator::new();

    for caller in CALLER_REGIONS {
        // Each row is out of the request's scope by exactly one field, and carries a
        // Region segment that is not the called one — so what refuses it is the field,
        // not the Region.
        let rows = vec![
            (
                "another account",
                connection_arn(PARTITION, "eu-west-1", "123456789012", NEVER_ALLOCATED_IDS[0]),
            ),
            (
                "an account differing in one digit",
                connection_arn(PARTITION, "eu-west-1", "000000000001", NEVER_ALLOCATED_IDS[1]),
            ),
            (
                "another partition",
                connection_arn("aws-cn", "eu-west-1", ACCOUNT_ID, NEVER_ALLOCATED_IDS[0]),
            ),
            (
                "the government partition",
                connection_arn("aws-us-gov", "eu-west-1", ACCOUNT_ID, NEVER_ALLOCATED_IDS[1]),
            ),
        ];

        for (name, arn) in &rows {
            for (operation, body) in arn_operations() {
                let case = format!("{name} / signed for {caller} / {operation}");
                let (status, answer) = emulator.ic_in(caller, operation, body(arn)).await;
                // The scope refusal's own sentence is judged through the shared judge
                // rather than as text: what this suite pins is that the field, and not
                // the Region segment, is what refuses the ARN.
                assert_answers_fault(
                    &case,
                    operation,
                    Fault::OutOfScope,
                    arn,
                    status,
                    &answer,
                );
                // The READ is refused too: an ARN of another scope is not an ARN this
                // request may read, which is a different answer from the empty tag set
                // an ARN of this scope resolving to nothing gets.
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the refusal to reach the read as well, got {status} with {answer:?}"
                );
            }
        }
    }
}

/// RULE: the answer follows the resource — a connection whose delete the clock has
/// settled into the terminal `deleted` state resolves to nothing, through every Region
/// spelling of its ARN, while one still being torn down is still served.
#[tokio::test]
async fn a_settled_delete_resolves_to_nothing_whatever_region_its_arn_spells() {
    let emulator = Emulator::new();
    let caller = CALLER_REGIONS[1];
    let places = placements_in(&emulator, caller).await;
    let initial: BTreeMap<String, String> = [("env".to_string(), "test".to_string())]
        .into_iter()
        .collect();
    let doomed = create_with_tags_in(&emulator, caller, &places[0], 3_500, &initial).await;
    let id = text(&doomed, "id");
    let stored_arn = text(&doomed, "arn");
    let respelled = connection_arn(PARTITION, "eu-west-1", ACCOUNT_ID, &id);

    let deleted = emulator
        .ic_ok_in(caller, "DeleteConnection", json!({ "identifier": id }))
        .await;
    assert_eq!(
        text(&deleted["connection"], "state"),
        "deleting",
        "case \"straight after the delete\": expected the transient state, got {:?}",
        deleted["connection"]["state"]
    );
    // Still being torn down, so the record is still there to be addressed — through the
    // respelled ARN exactly as through the stored one.
    let listed = emulator
        .ic_ok_in(caller, TAG_READER, json!({ "arn": respelled }))
        .await;
    assert_eq!(
        tags_of(&listed),
        initial,
        "case \"while deleting\": expected the tags of a resource that still exists, got {:?}",
        listed["tags"]
    );

    tokio::time::sleep(SETTLE_WAIT).await;
    let settled = emulator
        .ic_ok_in(caller, "GetConnection", json!({ "identifier": id }))
        .await;
    assert_eq!(
        text(&settled["connection"], "state"),
        "deleted",
        "case \"after the delete settles\": expected the terminal state, got {:?}",
        settled["connection"]["state"]
    );

    for (name, arn) in [
        ("its own ARN", stored_arn.clone()),
        ("another Region's spelling", respelled.clone()),
    ] {
        assert_resolves_to_nothing(
            &emulator,
            caller,
            &arn,
            &format!("after the delete settles / {name}"),
        )
        .await;
    }
}
