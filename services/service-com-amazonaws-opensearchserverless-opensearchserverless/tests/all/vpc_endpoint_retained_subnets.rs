// SPDX-License-Identifier: CC0-1.0

//! The subnet bound is per ENDPOINT, not per account.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! `SubnetIds` publishes no maximum, so `crate::state::MAX_SUBNET_IDS` is what keeps one
//! endpoint's retained list finite; the rows at that bound and one past it, on the create
//! and on each update list, are `vpc_endpoint_update.rs`'s. What is pinned HERE is the
//! dimension those rows leave open: the bound is on what ONE endpoint carries, so a second
//! endpoint of the same account may name its own full list, and each endpoint reports back
//! exactly the subnets its own create named.

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::MAX_SUBNET_IDS;

use crate::collection_lifecycle::{at, call, emulator};
use crate::vpc_endpoint::read_one;

/// `count` well-formed subnet ids starting at `first`, alternating the two widths
/// `SubnetId`'s own `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$` admits
/// (`smithy_query(query="shape", shape="SubnetIds")` -> `member`).
fn subnet_ids(first: usize, count: usize) -> Vec<String> {
    (first..first + count)
        .map(|index| {
            if index % 2 == 0 {
                format!("subnet-{index:08x}")
            } else {
                format!("subnet-{index:017x}")
            }
        })
        .collect()
}

/// RULE: the subnet bound is what ONE endpoint retains, so two endpoints of one account
/// each carry their own full list, and each reports the subnets its own create named.
#[tokio::test]
async fn each_endpoint_carries_its_own_full_subnet_list() {
    let router = emulator();
    // Two endpoints, each naming a disjoint list at the bound: if the bound were an
    // account-wide total, the second create would be refused.
    let rows = [
        ("first-endpoint", subnet_ids(0, MAX_SUBNET_IDS)),
        ("second-endpoint", subnet_ids(MAX_SUBNET_IDS, MAX_SUBNET_IDS)),
    ];

    for (name, requested) in rows {
        let (status, code, answer) = call(
            &router,
            "CreateVpcEndpoint",
            json!({
                "name": name,
                "vpcId": "vpc-1a2b3c4d",
                "subnetIds": requested.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 for a list of {} subnet(s), got {status} {code} with \
             {answer}",
            requested.len()
        );

        // `CreateVpcEndpointDetail` carries only id, name and status, so what the endpoint
        // RETAINED is read back the way the next caller would read it.
        let id = at(&at(&answer, "createVpcEndpointDetail").clone(), "id")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let detail = read_one(&router, name, &id).await;
        let reported: Vec<String> = at(&detail, "subnetIds")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|value: &Value| value.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported, requested,
            "case {name:?}: expected the endpoint to report its own {} subnet(s) in request \
             order, got {detail}",
            requested.len()
        );
    }
}
