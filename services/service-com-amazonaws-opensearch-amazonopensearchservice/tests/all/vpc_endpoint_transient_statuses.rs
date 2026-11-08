// SPDX-License-Identifier: CC0-1.0

//! The TRANSIENT statuses a service-managed VPC endpoint rests in, driven over the real
//! wire path: `CREATING` after a create and `UPDATING` after a change to its subnets or
//! security groups.
//!
//! One rule, over a set of inputs: a status the service leaves an endpoint in while work is
//! in flight is one a client WAITS FOR, so it has to stay observable to a client's own poll
//! — a poll written for a service that takes minutes samples seconds apart, and the status
//! has to be reported on a sample taken that long after the call that started the work,
//! not only on one taken in the same instant. The rows span both transitions and a table of
//! delays: the instant the call returned, one client-paced sample later, half the span in,
//! and past the span — where the clock has settled the endpoint `ACTIVE` with no call
//! having asked for the move.
//!
//! Both read paths are asserted at every sample, because a transient status is only
//! observable if the read a client polls with reports it: `DescribeVpcEndpoints`, which
//! addresses the endpoint by id, and `ListVpcEndpoints`, which reports the account's set.
//!
//! The harness comes from [`super::domain_lifecycle`] and every call path from
//! [`super::vpc_endpoints`] and [`super::vpc_endpoint_updates`]; this suite adds no second
//! copy of one.

use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    VPC_ENDPOINT_CHANGE, VPC_ENDPOINT_STATUS_ACTIVE, VPC_ENDPOINT_STATUS_CREATING,
    VPC_ENDPOINT_STATUS_UPDATING,
};

use super::domain_lifecycle::{Emulator, SETTLE_WAIT};
use super::vpc_endpoint_updates::update_endpoint;
use super::vpc_endpoints::{
    create_endpoint, describe_endpoints, list_endpoints, member, minted, vpc_domain,
};

/// The slowest sample a client's poll of this surface takes.
///
/// A client polls a status the service told it to wait for, and a poll written against a
/// service whose work takes minutes sleeps seconds between reads. One such sample is
/// therefore the least a transient status has to outlast to be observable at all; stated
/// here independently of the span the implementation chooses, so shortening that span back
/// below a client's pace fails this suite.
const CLIENT_POLL: Duration = Duration::from_secs(1);

/// The status `DescribeVpcEndpoints` reports for `id`.
async fn described(emulator: &Emulator, id: &str, case: &str) -> String {
    let answer = describe_endpoints(emulator, &[id]).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from DescribeVpcEndpoints, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["VpcEndpoints"][0]["Status"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected DescribeVpcEndpoints to report endpoint {id:?}, got {:?}",
                answer.body
            )
        })
        .to_string()
}

/// The status `ListVpcEndpoints` reports for `id`.
async fn listed(emulator: &Emulator, id: &str, case: &str) -> String {
    let answer = list_endpoints(emulator, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from ListVpcEndpoints, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["VpcEndpointSummaryList"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|summary| summary["VpcEndpointId"].as_str() == Some(id))
        .and_then(|summary| summary["Status"].as_str())
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected ListVpcEndpoints to report endpoint {id:?}, got {:?}",
                answer.body
            )
        })
        .to_string()
}

/// RULE: the status the service leaves an endpoint in while work on it is in flight stays
/// observable to a client-paced poll, and the clock alone settles it `ACTIVE` once the span
/// has passed.
#[tokio::test]
async fn a_transient_endpoint_status_outlasts_a_client_paced_poll() {
    // The span the implementation chose has to be at least one client sample long, whatever
    // value it is: a shorter one is a status no poll can observe.
    assert!(
        VPC_ENDPOINT_CHANGE >= CLIENT_POLL,
        "case \"the span a transient status rests\": expected at least one client sample \
         ({CLIENT_POLL:?}), got {VPC_ENDPOINT_CHANGE:?}"
    );

    // (the case, when the sample is taken after the call, the status both reads report)
    let samples: Vec<(&str, Duration, bool)> = vec![
        ("the instant the call returned", Duration::from_millis(0), true),
        ("one client-paced sample later", CLIENT_POLL, true),
        ("half the span in", VPC_ENDPOINT_CHANGE / 2, true),
        ("past the span", SETTLE_WAIT, false),
    ];

    // Both transitions this surface holds a transient status for, each with its own
    // endpoint so one row's sample cannot be the other's.
    for (change, transient) in [
        ("create", VPC_ENDPOINT_STATUS_CREATING),
        ("update", VPC_ENDPOINT_STATUS_UPDATING),
    ] {
        for (position, (when, delay, in_flight)) in samples.iter().enumerate() {
            let emulator = Emulator::new();
            let domain = format!("vpce-transient-{change}-{position}");
            let arn = vpc_domain(&emulator, &domain).await;
            let case = format!("{change}, sampled {when}");
            let answer = create_endpoint(
                &emulator,
                json!({
                    "DomainArn": arn,
                    "VpcOptions": { "SubnetIds": ["subnet-0ccc0000000000001"] },
                }),
            )
            .await;
            let id = member(&minted(&answer, &case), "VpcEndpointId", &case);
            if change == "update" {
                // An update is served on an endpoint that is idle, so the create's own span
                // is waited out first; what the row then samples is the update's span.
                tokio::time::sleep(SETTLE_WAIT).await;
                let started = update_endpoint(
                    &emulator,
                    &id,
                    &json!({ "SecurityGroupIds": ["sg-0ccc0000000000002"] }),
                )
                .await;
                assert_eq!(
                    started.status, 200,
                    "case {case}: expected the update to be served, got {} with {:?}",
                    started.status, started.body
                );
            }

            tokio::time::sleep(*delay).await;
            let expected = if *in_flight {
                transient
            } else {
                VPC_ENDPOINT_STATUS_ACTIVE
            };
            let by_id = described(&emulator, &id, &case).await;
            assert_eq!(
                by_id, expected,
                "case {case}: expected DescribeVpcEndpoints to report {expected}, got {by_id}"
            );
            let in_listing = listed(&emulator, &id, &case).await;
            assert_eq!(
                in_listing, expected,
                "case {case}: expected ListVpcEndpoints to report {expected}, got {in_listing}"
            );
        }
    }
}
