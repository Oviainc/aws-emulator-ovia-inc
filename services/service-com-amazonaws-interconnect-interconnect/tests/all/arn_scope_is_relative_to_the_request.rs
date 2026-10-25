// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the SCOPE an `AmazonResourceName` is judged against, driven
//! over the wire through the shared [`crate::emulator`] harness.
//!
//! RULE under test: the scope an ARN-identified operation judges its `arn` against is
//! the scope of the REQUEST it arrived on — the partition, the Region the request is
//! signed for and the account it serves — so one and the same ARN is out of scope
//! through one endpoint and in scope through another, and both ways an ARN can name no
//! resource are HTTP 400 with DIFFERENT codes: `InterconnectValidationException` for an
//! ARN outside the request's scope, `InterconnectClientException` for one inside it
//! that names no stored connection. Neither is the connection-scoped 404, and a
//! refusal writes nothing.
//!
//! The rows differ in the Region the resource lives in, the Region the request is
//! signed for (including a Region holding nothing at all), the environment and
//! bandwidth the resource sits on, the field that puts the ARN out of scope (Region,
//! account, partition) and its value, and the unallocated id the ARN names (both
//! prefixes `ConnectionId` allows) — crossed with every operation whose input
//! identifies its resource by `AmazonResourceName`
//! (`smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`).
//! The Region dimension is what [`crate::arn_scope_refusals`] does not reach: it drives
//! every row through the one default endpoint.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use service_com_amazonaws_interconnect_interconnect::state::{
    arn_scope_violation, connection_not_retrievable_message, parse_connection_arn,
};

use crate::arn_scope_refusals::{arn_operations, Fault};
use crate::emulator::{
    code, create_with_tags_in, message, placements, tags_of, text, Emulator,
    DEFAULT_SIGNING_REGION, SIGNING_NAME,
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

/// A Region no row creates anything in, so every ARN is foreign to its endpoint.
const EMPTY_REGION: &str = "eu-central-1";

/// Ids no create ever allocates, one per prefix `ConnectionId` allows
/// (`^(mcc|lmcc)-[a-z0-9]{8}$`).
const NEVER_ALLOCATED_IDS: [&str; 2] = ["mcc-00000000", "lmcc-zzzzzzzz"];

/// A connection ARN with the fields a row wants, built the way the member's pattern
/// writes them: `arn:<partition>:interconnect:<region>:<account>:connection/<id>`.
fn connection_arn(partition: &str, region: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:{SIGNING_NAME}:{region}:{account}:connection/{id}")
}

/// Assert `(status, body)` is the refusal `fault` names, worded the way this service
/// words it for `arn` when the request addresses `region`.
fn assert_refused(case: &str, fault: Fault, region: &str, arn: &str, status: u16, body: &serde_json::Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for an ARN that names no resource of this scope, got {status} with {body:?}"
    );
    let answered = code(body);
    assert!(
        answered.contains(fault.code()),
        "case {case:?}: expected {}, got {answered:?} with {body:?}",
        fault.code()
    );
    let reported = message(body);
    assert!(
        reported.contains(arn),
        "case {case:?}: expected the refusal to name the requested ARN, got {reported:?}"
    );
    let expected = match fault {
        Fault::OutOfScope => {
            let parsed = parse_connection_arn(arn)
                .expect("the row's ARN is a well-formed Interconnect connection ARN");
            arn_scope_violation(&parsed, region)
                .expect("the row's ARN is outside the scope the request addresses")
        }
        Fault::NoSuchConnection => connection_not_retrievable_message(arn),
    };
    assert_eq!(
        reported, expected,
        "case {case:?}: expected this service's own wording for the class, got {reported:?}"
    );
}

/// RULE: an ARN is in scope only through the endpoint of the Region its resource lives
/// in — every other Region's endpoint reports it as a fault in the request, while an
/// ARN carrying the called Region but naming nothing is the client fault, and the
/// resource's own endpoint keeps serving it throughout.
#[tokio::test]
async fn an_arn_is_in_scope_only_through_the_region_its_resource_lives_in() {
    let emulator = Emulator::new();
    let places = placements(&emulator).await;
    let live_tags: BTreeMap<String, String> = [("env", "test"), ("team", "network")]
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();

    for (seed, region) in LIVE_REGIONS.iter().enumerate() {
        let placement = &places[seed % places.len()];
        let live = create_with_tags_in(&emulator, region, placement, 2_000 + seed, &live_tags).await;
        let arn = text(&live, "arn");
        assert_eq!(
            arn,
            connection_arn(PARTITION, region, ACCOUNT_ID, &text(&live, "id")),
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
                // The live resource's own ARN, seen from another Region: outside the
                // scope THIS request addresses, so the request fault.
                let (status, answer) = emulator.ic_in(caller, operation, body(&arn)).await;
                assert_refused(
                    &format!("{arn} from {caller} on {operation}"),
                    Fault::OutOfScope,
                    caller,
                    &arn,
                    status,
                    &answer,
                );
                let reported = message(&answer);
                assert!(
                    reported.contains(caller) && reported.contains(*region),
                    "case {arn:?} from {caller} on {operation}: expected the refusal to name the ARN's Region and the one the request is for, got {reported:?}"
                );

                for id in NEVER_ALLOCATED_IDS {
                    // The same id, in the Region the request addresses: in scope and
                    // naming nothing, which is the OTHER class.
                    let in_scope = connection_arn(PARTITION, caller, ACCOUNT_ID, id);
                    let (status, answer) = emulator.ic_in(caller, operation, body(&in_scope)).await;
                    assert_refused(
                        &format!("{in_scope} from {caller} on {operation}"),
                        Fault::NoSuchConnection,
                        caller,
                        &in_scope,
                        status,
                        &answer,
                    );

                    // And the same id in the row's Region flips class with the
                    // request: foreign to this endpoint, so the request fault.
                    let foreign = connection_arn(PARTITION, region, ACCOUNT_ID, id);
                    let (status, answer) = emulator.ic_in(caller, operation, body(&foreign)).await;
                    assert_refused(
                        &format!("{foreign} from {caller} on {operation}"),
                        Fault::OutOfScope,
                        caller,
                        &foreign,
                        status,
                        &answer,
                    );
                }
            }
        }

        // None of the refusals above wrote anything, and the operations that answered
        // them serve the very same ARN through the resource's own endpoint.
        let listed = emulator
            .ic_ok_in(region, "ListTagsForResource", json!({ "arn": arn }))
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
            .ic_ok_in(region, "ListTagsForResource", json!({ "arn": arn }))
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
/// in EVERY Region — an ARN carrying the called Region but another account or another
/// partition is still the request fault, and still never a 404.
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
                connection_arn(
                    PARTITION,
                    region,
                    "000000000001",
                    NEVER_ALLOCATED_IDS[1],
                ),
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
                assert_refused(
                    &format!("{name} in {region} on {operation}"),
                    Fault::OutOfScope,
                    region,
                    arn,
                    status,
                    &answer,
                );
                codes.insert(code(&answer));
            }
            // The other class, in the same Region, answers a different code: the two
            // ways an ARN names nothing stay distinguishable in every Region.
            let in_scope = connection_arn(PARTITION, region, ACCOUNT_ID, NEVER_ALLOCATED_IDS[0]);
            let (status, answer) = emulator.ic_in(region, operation, body(&in_scope)).await;
            assert_refused(
                &format!("an unallocated id in {region} on {operation}"),
                Fault::NoSuchConnection,
                region,
                &in_scope,
                status,
                &answer,
            );
            assert!(
                !codes.contains(&code(&answer)),
                "case {region:?} on {operation}: expected a code distinct from the out-of-scope ones {codes:?}, got {:?}",
                code(&answer)
            );
        }
    }
}
