// SPDX-License-Identifier: CC0-1.0

//! Which Region the VPC an endpoint reports for its subnet is resolved in, end to end
//! over HTTP.
//!
//! The rule under test: a subnet, and the VPC it belongs to, is a resource of ONE Region,
//! so the `VpcId` an endpoint reports for a `SubnetId` is resolved from the endpoints of
//! that endpoint's own Region alone. Endpoints of one Region in one subnet report one VPC
//! id, whichever Outposts of that Region they sit on; the same `SubnetId` named in another
//! Region is a different subnet and carries an id of its own; and the id a Region knows
//! for a subnet lasts exactly as long as an endpoint of that Region is left in it, so the
//! next endpoint there is given a fresh one and the endpoints of another Region neither
//! keep the old one alive nor have theirs disturbed.
//!
//! The rows span what the rule generalises over: the relationship between the two Region
//! codes (another Region of the same partition, one that shares the other's prefix, one of
//! another partition, one whose code extends the other's), the width of the `SubnetId`
//! (both forms `SubnetId`'s `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$` admits) and how many
//! endpoints each Region puts in the subnet. Every case goes through the router, so each
//! exercises routing, deserialization, the handler, the persisted state and serialization.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_s3outposts_s3outposts::state::ENDPOINT_TRANSITION;
use smithy_server_runtime::MultiServiceRouter;

use crate::endpoint_lifecycle::{
    create_in, delete_endpoint_in, emulator_holding, endpoint_id_of, endpoint_with_arn,
    list_endpoints_in, string_at,
};

/// Two Regions that name the SAME subnet id, and how many endpoints each puts in it.
struct Row {
    case: &'static str,
    /// The Region whose endpoints are created first, and whose subnet is emptied again by
    /// the lifetime case.
    here: &'static str,
    /// A DIFFERENT Region, whose Outposts carry endpoints in a subnet of the same id.
    there: &'static str,
    /// The id both Regions name. Both widths `SubnetId` admits appear across the rows.
    subnet_id: &'static str,
    /// How many endpoints each Region puts in that subnet, each on an Outpost of its own.
    endpoints_per_region: usize,
}

/// One row per shape of relationship between the two Regions. The Region codes are AWS's
/// own; the pairs that share or extend a prefix are here so no case can pass by comparing
/// Region codes loosely.
const ROWS: &[Row] = &[
    Row {
        case: "another Region of the same partition, one endpoint each",
        here: "us-east-1",
        there: "eu-west-1",
        subnet_id: "subnet-0a1b2c3d4e5f60718",
        endpoints_per_region: 1,
    },
    Row {
        case: "a Region sharing the other's prefix, two endpoints each",
        here: "us-east-1",
        there: "us-east-2",
        subnet_id: "subnet-0b1c2d3e",
        endpoints_per_region: 2,
    },
    Row {
        case: "a Region of another partition, one endpoint each",
        here: "us-east-1",
        there: "cn-north-1",
        subnet_id: "subnet-0c1d2e3f405162738",
        endpoints_per_region: 1,
    },
    Row {
        case: "a Region whose code extends the other's, two endpoints each",
        here: "us-iso-east-1",
        there: "us-isob-east-1",
        subnet_id: "subnet-0d1e2f30",
        endpoints_per_region: 2,
    },
];

/// The security group every endpoint of a row is reached through: one value, so nothing
/// but the Region and the Outpost differs between the endpoints a row creates.
const SECURITY_GROUP_ID: &str = "sg-0a1b2c3d4e5f60718";

/// One emulator holding `endpoints_per_region` Outposts in each of the two Regions, and
/// the `(Region, OutpostId)` pairs it holds.
///
/// Every endpoint of a Region sits on an Outpost of its own, so a case covers endpoints
/// that share a subnet across Outposts as well as across Regions.
fn emulator_for(row: &Row) -> (MultiServiceRouter, Vec<(&'static str, String)>) {
    let mut outposts: Vec<(&'static str, String)> = Vec::new();
    for (index, region) in [row.here, row.there].into_iter().enumerate() {
        for ordinal in 0..row.endpoints_per_region {
            outposts.push((region, format!("op-{:017x}", 1 + index * 8 + ordinal)));
        }
    }
    let declared: Vec<(String, &str)> = outposts
        .iter()
        .map(|(region, outpost_id)| (outpost_id.clone(), *region))
        .collect();
    (emulator_holding(&declared), outposts)
}

/// `vpc-` plus the hex id EC2 publishes for a VPC, in either width an EC2 resource id
/// takes (`vpc-0a1b2c3d`, `vpc-0a1b2c3d4e5f60718`).
fn is_vpc_id(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("vpc-") else {
        return false;
    };
    matches!(hex.len(), 8 | 17) && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Create an endpoint on `outpost_id` in `region` in `subnet_id`, and read back the
/// `(EndpointArn, VpcId)` that Region's own listing reports for it.
async fn endpoint_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    outpost_id: &str,
    subnet_id: &str,
) -> (String, String) {
    let created = create_in(
        router,
        region,
        json!({
            "OutpostId": outpost_id,
            "SubnetId": subnet_id,
            "SecurityGroupId": SECURITY_GROUP_ID,
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateEndpoint on {outpost_id} in {region} must answer 200, got {} \
         with {:?}",
        created.status, created.body
    );
    let arn = string_at(&created.body, "EndpointArn");
    let listed = list_endpoints_in(router, region, "?maxResults=100").await;
    let endpoint = endpoint_with_arn(&listed, &arn);
    assert_eq!(
        string_at(&endpoint, "SubnetId"),
        subnet_id,
        "case {case:?}: the endpoint must report the subnet it was created in, got {:?}",
        endpoint
    );
    let vpc_id = string_at(&endpoint, "VpcId");
    assert!(
        is_vpc_id(&vpc_id),
        "case {case:?}: the VpcId reported in {region} must be a VPC id, got {vpc_id:?}"
    );
    (arn, vpc_id)
}

/// Every endpoint a Region puts in `subnet_id`, one per Outpost of that Region, as the
/// `(OutpostId, EndpointArn, VpcId)` triples its own listing reports.
async fn endpoints_of_region(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    outposts: &[(&'static str, String)],
    subnet_id: &str,
) -> Vec<(String, String, String)> {
    let mut placed = Vec::new();
    for (_, outpost_id) in outposts.iter().filter(|(home, _)| *home == region) {
        let (arn, vpc_id) = endpoint_in(router, case, region, outpost_id, subnet_id).await;
        placed.push((outpost_id.clone(), arn, vpc_id));
    }
    placed
}

/// The one VPC id a Region reports for every endpoint it has in the subnet: the invariant
/// that a subnet belongs to exactly one VPC, inside one Region.
fn one_vpc_id(case: &str, region: &str, placed: &[(String, String, String)]) -> String {
    let reported: BTreeSet<&String> = placed.iter().map(|(_, _, vpc_id)| vpc_id).collect();
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: every endpoint of {region} in the subnet must report one VPC id, got \
         {reported:?}"
    );
    placed[0].2.clone()
}

/// The same `SubnetId` in two Regions is two subnets: each Region reports a VPC id of its
/// own for it, and every endpoint of a Region in that subnet reports that one id — whichever
/// of the Region's Outposts it sits on.
#[tokio::test]
async fn a_subnet_named_in_two_regions_carries_one_vpc_id_per_region() {
    for row in ROWS {
        let case = row.case;
        let (router, outposts) = emulator_for(row);

        let here = endpoints_of_region(&router, case, row.here, &outposts, row.subnet_id).await;
        let there = endpoints_of_region(&router, case, row.there, &outposts, row.subnet_id).await;
        let here_vpc = one_vpc_id(case, row.here, &here);
        let there_vpc = one_vpc_id(case, row.there, &there);

        assert_ne!(
            here_vpc, there_vpc,
            "case {case:?}: the subnet {} named in {} is not the one named in {}, so the two \
             must report different VPC ids, got {here_vpc} in both",
            row.subnet_id, row.here, row.there
        );

        // Creating in the second Region left the first Region's answer exactly as it was:
        // what a Region reports for its subnet is decided by its own endpoints alone.
        for (outpost_id, arn, _) in &here {
            let listed = list_endpoints_in(&router, row.here, "?maxResults=100").await;
            let reported = string_at(&endpoint_with_arn(&listed, arn), "VpcId");
            assert_eq!(
                reported, here_vpc,
                "case {case:?}: the endpoint on {outpost_id} in {} must still report {here_vpc}, \
                 got {reported}",
                row.here
            );
        }
    }
}

/// The VPC id a Region knows for a subnet lasts exactly while an endpoint of that Region is
/// left in it: once this Region's endpoints there are torn down, the next endpoint in that
/// subnet is given a fresh id — the endpoints another Region has in a subnet of the same id
/// keep neither the old id alive nor lose their own.
#[tokio::test]
async fn a_region_with_no_endpoint_left_in_the_subnet_mints_a_fresh_vpc_id() {
    for row in ROWS {
        let case = row.case;
        let (router, outposts) = emulator_for(row);

        let here = endpoints_of_region(&router, case, row.here, &outposts, row.subnet_id).await;
        let there = endpoints_of_region(&router, case, row.there, &outposts, row.subnet_id).await;
        let here_vpc = one_vpc_id(case, row.here, &here);
        let there_vpc = one_vpc_id(case, row.there, &there);

        // Every endpoint this Region has in the subnet is torn down. The teardown is the
        // clock's to finish, so the record — and what it held — goes once the span has
        // passed.
        for (outpost_id, arn, _) in &here {
            let deleted =
                delete_endpoint_in(&router, row.here, outpost_id, &endpoint_id_of(arn)).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: DeleteEndpoint on {outpost_id} in {} must answer 200, got {} \
                 with {:?}",
                row.here, deleted.status, deleted.body
            );
        }
        std::thread::sleep(ENDPOINT_TRANSITION + Duration::from_millis(200));
        let emptied = list_endpoints_in(&router, row.here, "?maxResults=100").await;
        assert!(
            emptied.endpoints().is_empty(),
            "case {case:?}: {} must have no endpoint left once the teardowns complete, got {:?}",
            row.here,
            emptied.body
        );

        // The next endpoint in that subnet is in a Region that knows no VPC for it any
        // more, so it mints one — the one the other Region holds included, which is not
        // this Region's subnet.
        let (_, outpost_id) = outposts
            .iter()
            .find(|(home, _)| *home == row.here)
            .expect("the fixture holds an Outpost in each of the row's Regions");
        let (_, fresh_vpc) = endpoint_in(&router, case, row.here, outpost_id, row.subnet_id).await;
        assert_ne!(
            fresh_vpc, here_vpc,
            "case {case:?}: a subnet of {} no endpoint is left in must mint a fresh VPC id, got \
             {here_vpc} again",
            row.here
        );
        assert_ne!(
            fresh_vpc, there_vpc,
            "case {case:?}: the new endpoint in {} must not be handed the VPC id {} reports, got \
             {there_vpc}",
            row.here, row.there
        );

        // And the other Region's endpoints are untouched by all of it.
        let listed = list_endpoints_in(&router, row.there, "?maxResults=100").await;
        for (outpost_id, arn, _) in &there {
            let reported = string_at(&endpoint_with_arn(&listed, arn), "VpcId");
            assert_eq!(
                reported, there_vpc,
                "case {case:?}: the endpoint on {outpost_id} in {} must still report \
                 {there_vpc}, got {reported}",
                row.there
            );
        }
    }
}
