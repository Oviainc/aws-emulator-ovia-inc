// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the SCOPE an `AmazonResourceName` is resolved in, driven over
//! the wire through the shared [`crate::emulator`] harness.
//!
//! RULE under test: the Region an ARN-addressed operation resolves its `arn` in is the
//! Region of the REQUEST it arrived on, never the Region segment the ARN spells (the
//! member leaves that segment unconstrained, so it addresses no scope of its own) —
//! so one and the same connection is reached through its own Region's endpoint however
//! its ARN spells the Region, and through every other Region's endpoint its id
//! resolves to nothing: the two tag WRITES answer this service's own client fault and
//! the `@readonly` LISTING answers the empty tag set, while an ARN whose account or
//! partition is not the request's is refused as a fault in the request from all three.
//! A refusal writes nothing, and no endpoint's calls touch another Region's tags.
//!
//! The rows differ in the Region the resource lives in, the Region the request is
//! signed for (including a Region holding nothing at all), the Region segment the ARN
//! carries, the environment and bandwidth the resource sits on, the field that puts
//! the ARN out of the request's scope (account, partition) and its value, and the
//! unallocated id the ARN names (both prefixes `ConnectionId` allows) — crossed with
//! every operation whose input addresses its resource by `AmazonResourceName`
//! (`smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`).
//! The Region dimension is what [`crate::arn_scope_refusals`] does not reach: it
//! drives every row through the one default endpoint.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use crate::arn_scope_refusals::{arn_operations, assert_answers_fault, Fault, TAG_READER};
use crate::emulator::{
    code, create_with_tags_in, placements_in, tags_of, text, Emulator, DEFAULT_SIGNING_REGION,
    SIGNING_NAME,
};

/// The account id every ARN this emulator mints carries, and so the account every
/// request it serves is for.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The partition every ARN this emulator mints carries.
const PARTITION: &str = "aws";

/// The Regions a row's resource lives in: the Region the rest of this crate's suites
/// sign for, plus three more on three other continents, so a row's Region is never the
/// harness default by accident.
const LIVE_REGIONS: [&str; 4] = [
    DEFAULT_SIGNING_REGION,
    "eu-west-1",
    "ap-southeast-2",
    "sa-east-1",
];

/// A Region no row creates anything in, so every ARN names a resource its endpoint
/// does not hold.
const EMPTY_REGION: &str = "eu-central-1";

/// Ids no create ever allocates, one per prefix `ConnectionId` allows
/// (`^(mcc|lmcc)-[a-z0-9]{8}$`).
const NEVER_ALLOCATED_IDS: [&str; 2] = ["mcc-00000000", "lmcc-zzzzzzzz"];

/// A connection ARN with the fields a row wants, built the way the member's pattern
/// writes them: `arn:<partition>:interconnect:<region>:<account>:connection/<id>`.
fn connection_arn(partition: &str, region: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:{SIGNING_NAME}:{region}:{account}:connection/{id}")
}

/// RULE: a connection is addressed only through the endpoint of the Region it lives in
/// — every other Region's endpoint resolves its id to nothing, whichever Region that
/// ARN spells — while the resource's own endpoint keeps serving it throughout.
#[tokio::test]
async fn an_arn_is_resolved_only_through_the_region_the_request_addresses() {
    let emulator = Emulator::new();
    let live_tags: BTreeMap<String, String> = [("env", "test"), ("team", "network")]
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();

    for (seed, region) in LIVE_REGIONS.iter().enumerate() {
        // The row's resource is created on a placement of ITS OWN Region: an
        // Environment belongs to the Region whose endpoint offers it.
        let places = placements_in(&emulator, region).await;
        let placement = &places[seed % places.len()];
        let live = create_with_tags_in(&emulator, region, placement, 2_000 + seed, &live_tags).await;
        let arn = text(&live, "arn");
        let id = text(&live, "id");
        assert_eq!(
            arn,
            connection_arn(PARTITION, region, ACCOUNT_ID, &id),
            "case {region:?}: expected the ARN to carry the Region the create was signed for, got {arn:?}"
        );

        let elsewhere: Vec<&str> = LIVE_REGIONS
            .iter()
            .copied()
            .chain(std::iter::once(EMPTY_REGION))
            .filter(|caller| caller != region)
            .collect();
        for caller in elsewhere {
            for (operation, body) in arn_operations() {
                // The live resource's own ARN, seen from another Region: that endpoint
                // holds no connection of this id, so the ARN resolves to nothing.
                let (status, answer) = emulator.ic_in(caller, operation, body(&arn)).await;
                assert_answers_fault(
                    &format!("{arn} from {caller}"),
                    operation,
                    Fault::NoSuchConnection,
                    &arn,
                    status,
                    &answer,
                );

                for unallocated in NEVER_ALLOCATED_IDS {
                    // An id no endpoint ever allocated, spelled with the called Region
                    // and with the row's: the same answer either way, because the
                    // Region segment is not what the resolution keys on.
                    for spelling in [caller, *region] {
                        let other = connection_arn(PARTITION, spelling, ACCOUNT_ID, unallocated);
                        let (status, answer) =
                            emulator.ic_in(caller, operation, body(&other)).await;
                        assert_answers_fault(
                            &format!("{other} from {caller}"),
                            operation,
                            Fault::NoSuchConnection,
                            &other,
                            status,
                            &answer,
                        );
                    }
                }
            }
        }

        // None of the answers above wrote anything, and the operations that gave them
        // serve the very same ARN through the resource's own endpoint.
        let listed = emulator
            .ic_ok_in(region, TAG_READER, json!({ "arn": arn }))
            .await;
        assert_eq!(
            tags_of(&listed),
            live_tags,
            "case {region:?}: expected the live resource's tags untouched by every refusal, got {:?}",
            listed["tags"]
        );
        emulator
            .ic_ok_in(
                region,
                "UntagResource",
                json!({ "arn": arn, "tagKeys": ["env"] }),
            )
            .await;
        let after = emulator
            .ic_ok_in(region, TAG_READER, json!({ "arn": arn }))
            .await;
        assert_eq!(
            tags_of(&after),
            [("team".to_string(), "network".to_string())]
                .into_iter()
                .collect::<BTreeMap<String, String>>(),
            "case {region:?}: expected the resource's own endpoint to serve the untag, got {:?}",
            after["tags"]
        );
    }
}

/// RULE: the account and the partition of the request's scope are judged the same way
/// in EVERY Region — an ARN carrying another account or another partition is a fault in
/// the request wherever it is sent, and still never a 404 — and that answer is
/// distinguishable from the one an ARN of this scope resolving to nothing gets.
#[tokio::test]
async fn an_arn_outside_the_requests_account_or_partition_is_refused_in_every_region() {
    let emulator = Emulator::new();

    for region in LIVE_REGIONS.iter().copied().chain([EMPTY_REGION]) {
        let rows = vec![
            (
                "another account",
                connection_arn(PARTITION, region, "123456789012", NEVER_ALLOCATED_IDS[0]),
            ),
            (
                "an account differing in one digit",
                connection_arn(PARTITION, region, "000000000001", NEVER_ALLOCATED_IDS[1]),
            ),
            (
                "another partition",
                connection_arn("aws-cn", region, ACCOUNT_ID, NEVER_ALLOCATED_IDS[0]),
            ),
            (
                "the government partition",
                connection_arn("aws-us-gov", region, ACCOUNT_ID, NEVER_ALLOCATED_IDS[1]),
            ),
        ];

        let mut codes: BTreeSet<String> = BTreeSet::new();
        for (operation, body) in arn_operations() {
            for (name, arn) in &rows {
                let (status, answer) = emulator.ic_in(region, operation, body(arn)).await;
                assert_answers_fault(
                    &format!("{name} in {region}"),
                    operation,
                    Fault::OutOfScope,
                    arn,
                    status,
                    &answer,
                );
                codes.insert(code(&answer));
            }
            // The other class, in the same Region: an ARN of this scope that resolves
            // to nothing, which the writes report under a code a client can tell apart
            // and the read answers with no error at all.
            let absent = connection_arn(PARTITION, region, ACCOUNT_ID, NEVER_ALLOCATED_IDS[0]);
            let (status, answer) = emulator.ic_in(region, operation, body(&absent)).await;
            assert_answers_fault(
                &format!("an unallocated id in {region}"),
                operation,
                Fault::NoSuchConnection,
                &absent,
                status,
                &answer,
            );
            assert!(
                !codes.contains(&code(&answer)),
                "case {region:?} on {operation}: expected an answer distinct from the out-of-scope ones {codes:?}, got {:?}",
                code(&answer)
            );
        }
    }
}
