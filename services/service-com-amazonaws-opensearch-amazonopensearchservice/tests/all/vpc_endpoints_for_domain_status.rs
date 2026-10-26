// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT `ListVpcEndpointsForDomain` reports about each endpoint it
//! lists, and for what reporting it leaves behind.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`],
//! the create, read and release helpers from [`super::vpc_endpoints`], the per-domain
//! listing from [`super::vpc_endpoints_for_domain`] and the modification from
//! [`super::vpc_endpoint_updates`]; this suite adds no second copy of any of them.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the listing reports every endpoint of the named domain in the status its stored
//!   record holds AT THAT INSTANT, over a table walking the endpoint through the states
//!   its lifecycle passes — and the per-id read agrees with it at each one, because both
//!   report the same record. The UPDATING-then-ACTIVE span itself is asserted of the
//!   per-id read in [`super::vpc_endpoint_updates`]; what this suite adds is that the
//!   per-domain LISTING reports the same record at the same instants, so a change to
//!   either suite's expectation must be made in both;
//! * reporting a domain's endpoints changes nothing a later caller sees: repeated
//!   listings before a status move falls due keep reporting the status in flight and
//!   neither bring the move forward nor drop the record, the move lands on the clock
//!   alone, and only a release takes an endpoint out of the listing — leaving the
//!   domain's other endpoints in it.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    VPC_ENDPOINT_STATUS_ACTIVE, VPC_ENDPOINT_STATUS_CREATING, VPC_ENDPOINT_STATUS_DELETING,
    VPC_ENDPOINT_STATUS_UPDATING,
};

use super::domain_lifecycle::{Emulator, SETTLE_WAIT};
use super::vpc_endpoint_updates::update_endpoint;
use super::vpc_endpoints::{delete_endpoint, describe_endpoints, vpc_domain};
use super::vpc_endpoints_for_domain::{endpoint_on, list_for_domain, listed_ids};

/// The summary the per-domain listing of `domain` reports for the endpoint `id` names,
/// with the case named on failure.
async fn summary_of(emulator: &Emulator, domain: &str, id: &str, case: &str) -> Value {
    let answer = list_for_domain(emulator, domain, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["VpcEndpointSummaryList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a VpcEndpointSummaryList, got {:?}",
                answer.body
            )
        })
        .iter()
        .find(|summary| summary["VpcEndpointId"] == json!(id))
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected the listing to report endpoint {id:?}, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The status `DescribeVpcEndpoints` reports for the endpoint `id` names — the per-id
/// read of the same stored record the listing projects.
async fn described_status(emulator: &Emulator, id: &str, case: &str) -> Value {
    let answer = describe_endpoints(emulator, &[id]).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["VpcEndpoints"][0]["Status"].clone()
}

/// What happens to the endpoint before a row's listing is read.
enum Step {
    /// Nothing at all: the listing is read the instant the previous call returned.
    Nothing,
    /// The span the clock owes the endpoint passes, with no call asking for the move.
    Settle,
    /// A modification of the endpoint's subnets is asked for.
    Modify,
}

#[tokio::test]
async fn the_listing_reports_each_endpoint_in_the_status_its_record_holds() {
    let emulator = Emulator::new();
    let domain = "vpce-domain-statuses";
    let arn = vpc_domain(&emulator, domain).await;
    let id = endpoint_on(&emulator, &arn, "subnet-0fff0000000000001").await;
    // A second endpoint of the SAME domain, left untouched: every row reports it too, so
    // the status asserted below is the record's own and not something the listing says
    // about whichever endpoint was last called for.
    let idle = endpoint_on(&emulator, &arn, "subnet-0fff0000000000002").await;

    // One row per state an endpoint's lifecycle passes through, in the order it passes
    // through them: what happens first, the status the listing must then report for the
    // endpoint that was called for, and the status it must report for the untouched one
    // beside it — which no call of another endpoint may move. The values are
    // `VpcEndpointStatus`'s, from the model
    // (`smithy_query(query="shape",shape="VpcEndpointStatus")`, also exposed as
    // `ops::types::VPC_ENDPOINT_STATUS_VALUES`).
    let rows: Vec<(&str, Step, &str, &str)> = vec![
        (
            "the instant the create returned",
            Step::Nothing,
            VPC_ENDPOINT_STATUS_CREATING,
            VPC_ENDPOINT_STATUS_CREATING,
        ),
        (
            "once the create's span has passed",
            Step::Settle,
            VPC_ENDPOINT_STATUS_ACTIVE,
            VPC_ENDPOINT_STATUS_ACTIVE,
        ),
        (
            "the instant a modification returned",
            Step::Modify,
            VPC_ENDPOINT_STATUS_UPDATING,
            VPC_ENDPOINT_STATUS_ACTIVE,
        ),
        (
            "once the modification's span has passed",
            Step::Settle,
            VPC_ENDPOINT_STATUS_ACTIVE,
            VPC_ENDPOINT_STATUS_ACTIVE,
        ),
    ];

    for (case, step, expected, expected_beside) in rows {
        match step {
            Step::Nothing => {}
            Step::Settle => tokio::time::sleep(SETTLE_WAIT).await,
            Step::Modify => {
                let answer = update_endpoint(
                    &emulator,
                    &id,
                    &json!({ "SubnetIds": ["subnet-0fff0000000000003"] }),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the modification to be served with 200, got {} with \
                     {:?}",
                    answer.status, answer.body
                );
            }
        }

        let summary = summary_of(&emulator, domain, &id, case).await;
        assert_eq!(
            summary["Status"],
            json!(expected),
            "case {case}: expected the listing to report {expected}, got {summary:?}"
        );
        // The per-id read is the same record, so it cannot report a different status at
        // the same instant.
        let described = described_status(&emulator, &id, case).await;
        assert_eq!(
            described,
            summary["Status"],
            "case {case}: expected the per-id read to report {:?}, got {described:?}",
            summary["Status"]
        );
        // And the untouched endpoint of the same domain is reported throughout, in the
        // status ITS OWN record holds: a call about another endpoint moves nothing here,
        // and the two read paths agree about it as they do about the one called for.
        let beside = summary_of(&emulator, domain, &idle, case).await;
        let beside_described = described_status(&emulator, &idle, case).await;
        assert_eq!(
            beside["Status"],
            json!(expected_beside),
            "case {case}: expected the untouched endpoint to be reported {expected_beside}, got \
             {beside:?}"
        );
        assert_eq!(
            beside["Status"], beside_described,
            "case {case}: expected the untouched endpoint's two reads to agree, got {beside:?} \
             against {beside_described:?}"
        );
    }
}

#[tokio::test]
async fn reporting_a_domain_s_endpoints_leaves_the_set_a_later_caller_finds() {
    let emulator = Emulator::new();
    let domain = "vpce-domain-rereads";
    let arn = vpc_domain(&emulator, domain).await;
    let kept = endpoint_on(&emulator, &arn, "subnet-0a1b0000000000001").await;
    let released = endpoint_on(&emulator, &arn, "subnet-0a1b0000000000002").await;

    // Listings back to back, before the span the create started falls due: a read
    // neither brings the move the clock owes forward nor drops the record.
    for round in 0..2 {
        let case = format!("listing round {round} before the create's span fell due");
        for id in [&kept, &released] {
            let summary = summary_of(&emulator, domain, id, &case).await;
            assert_eq!(
                summary["Status"],
                json!(VPC_ENDPOINT_STATUS_CREATING),
                "case {case}: expected {VPC_ENDPOINT_STATUS_CREATING} for endpoint {id:?}, got \
                 {summary:?}"
            );
        }
    }

    // The move lands on the clock alone: no call between the listings above and this one
    // asked for it.
    tokio::time::sleep(SETTLE_WAIT).await;
    for id in [&kept, &released] {
        let case = format!("endpoint {id:?} once the create's span fell due");
        let summary = summary_of(&emulator, domain, id, &case).await;
        assert_eq!(
            summary["Status"],
            json!(VPC_ENDPOINT_STATUS_ACTIVE),
            "case {case}: expected {VPC_ENDPOINT_STATUS_ACTIVE}, got {summary:?}"
        );
    }

    // Only a release takes an endpoint out of the listing, and it takes out exactly the
    // one it names: the domain's other endpoint is still reported afterwards.
    let answer = delete_endpoint(&emulator, &released).await;
    assert_eq!(
        answer.status, 200,
        "case releasing endpoint {released:?}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["VpcEndpointSummary"]["Status"],
        json!(VPC_ENDPOINT_STATUS_DELETING),
        "case releasing endpoint {released:?}: expected {VPC_ENDPOINT_STATUS_DELETING}, got {:?}",
        answer.body
    );
    let remaining = listed_ids(&emulator, domain, "after one endpoint was released").await;
    assert_eq!(
        remaining,
        vec![kept.clone()],
        "case after one endpoint was released: expected only {kept:?} to be reported, got \
         {remaining:?}"
    );
}
