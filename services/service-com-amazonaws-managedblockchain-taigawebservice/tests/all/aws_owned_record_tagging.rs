// SPDX-License-Identifier: CC0-1.0

//! The tag plane addresses an ACCOUNT's resources only.
//!
//! RULE: a tag set belongs to a resource an account holds, so a record no request created
//! and no account owns — one of AWS's standing public networks, the one record every
//! Region's endpoint reads — carries no tag set a caller reads, writes or removes: all
//! three `ResourceArn`-addressed tagging operations answer such an ARN the way they answer
//! an ARN naming nothing stored, from every Region alike, and the record is left as it was.
//! What an account creates ON such a network is its own resource again, and is tagged like
//! any other.
//!
//! The tables below span what that rule generalises over:
//!
//! * the RECORD: every network in the catalog this service seeds
//!   (`state::PUBLIC_NETWORKS`), so a catalog entry added later is a row here too — the
//!   issue cited only mainnet;
//! * the ARN the caller sends: the one the record's own read publishes, and the one a
//!   caller would build for the Region it is calling from, since neither names a resource
//!   of the account;
//! * the REGION the call is signed for: the Region the published ARN names and three
//!   others of different AWS Region shapes, because this record is the one every Region
//!   reads;
//! * the OPERATION: `ListTagsForResource`, `TagResource` and `UntagResource`;
//! * the CONTRAST: an account's own network, and an account's own node on a public
//!   network, which stay taggable through the same ARNs.
//!
//! The request harness (`new_router`, `call_in_region`, the create bodies) and the
//! percent-encoded `@httpLabel` an ARN travels in are the ones the sibling suites define.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::state;

use crate::harness::{
    call, call_in_region, create_network_body, get_network, new_router, Answer, ACCOUNT,
    PAST_TRANSITION, REGION,
};
use crate::resource_tags::{label, tag_map};

/// The Regions the tables run over: the Region the seeded records' ARNs name
/// (`state::DEFAULT_REGION`), the Region the sibling suites sign for, an Asia-Pacific one
/// whose geography code is two words, and a GovCloud one, which names its partition in the
/// id.
const REGIONS: &[&str] = &[state::DEFAULT_REGION, REGION, "ap-southeast-2", "us-gov-west-1"];

/// The modelled refusal for an ARN that names no resource of this account: "A requested
/// resource doesn't exist. It may have been deleted or referenced incorrectly."
const NOT_FOUND: (u16, &str) = (404, "ResourceNotFoundException");

/// One `bc.*` node size Managed Blockchain offers, as the sibling suites create nodes with.
const INSTANCE_TYPE: &str = "bc.t3.small";

/// The ARN of one of this service's resources, as a caller addressing `region` spells it.
fn arn_in(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:managedblockchain:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// `ListTagsForResource` (`GET /tags/{ResourceArn}`), signed for `region`.
async fn list_tags_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    resource_arn: &str,
) -> Answer {
    call_in_region(
        router,
        region,
        "GET",
        &format!("/tags/{}", label(resource_arn)),
        None,
    )
    .await
}

/// `TagResource` (`POST /tags/{ResourceArn}`), signed for `region`.
async fn tag_resource_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    resource_arn: &str,
    tags: Value,
) -> Answer {
    call_in_region(
        router,
        region,
        "POST",
        &format!("/tags/{}", label(resource_arn)),
        Some(json!({ "Tags": tags })),
    )
    .await
}

/// `UntagResource` (`DELETE /tags/{ResourceArn}?tagKeys=…`), signed for `region`.
async fn untag_resource_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    resource_arn: &str,
    keys: &[&str],
) -> Answer {
    let query: Vec<String> = keys.iter().map(|key| format!("tagKeys={key}")).collect();
    call_in_region(
        router,
        region,
        "DELETE",
        &format!("/tags/{}?{}", label(resource_arn), query.join("&")),
        None,
    )
    .await
}

/// Every tagging operation addressed at a record that is no account's is refused as an ARN
/// naming nothing stored, from every Region, and nothing it sent is stored: each public
/// network keeps the empty tag set every Region's `GetNetwork` reports.
#[tokio::test]
async fn no_tagging_operation_reaches_a_record_no_account_owns() {
    let router = new_router();

    for public in state::PUBLIC_NETWORKS {
        // The ARN the record's own read publishes, and the ARN a caller would build for the
        // Region it is calling from: neither names a resource of this account.
        for region in REGIONS {
            for (addressed, resource_arn) in [
                ("the published ARN", arn_in(state::DEFAULT_REGION, "networks", public.id)),
                ("an ARN of the calling Region", arn_in(region, "networks", public.id)),
            ] {
                for (operation, answer) in [
                    (
                        "ListTagsForResource",
                        list_tags_in(&router, region, &resource_arn).await,
                    ),
                    (
                        "TagResource",
                        tag_resource_in(&router, region, &resource_arn, json!({ "owner": "me" }))
                            .await,
                    ),
                    (
                        "UntagResource",
                        untag_resource_in(&router, region, &resource_arn, &["owner"]).await,
                    ),
                ] {
                    let case = format!("{} / {addressed} / {region} / {operation}", public.id);
                    assert_eq!(
                        (answer.status, answer.error_type.as_str()),
                        NOT_FOUND,
                        "case {case:?}: expected {NOT_FOUND:?} for {resource_arn}, got {} {} {:?}",
                        answer.status,
                        answer.error_type,
                        answer.body
                    );
                    assert_eq!(
                        answer.body["ResourceName"].as_str(),
                        Some(resource_arn.as_str()),
                        "case {case:?}: expected the refusal to name the ARN the request sent, \
                         got {:?}",
                        answer.body
                    );
                }
            }
        }
    }

    // Nothing the refused writes carried was stored: every Region's read of each public
    // network reports the same record, with the empty tag set it stands with — so no write
    // made in one Region became visible in another.
    for public in state::PUBLIC_NETWORKS {
        let published = get_network(&router, public.id).await;
        for region in REGIONS {
            let answer =
                call_in_region(&router, region, "GET", &format!("/networks/{}", public.id), None)
                    .await;
            assert_eq!(
                answer.status, 200,
                "case {} / {region}: expected GetNetwork 200, got {} {:?}",
                public.id, answer.status, answer.body
            );
            assert_eq!(
                tag_map(&answer.body["Network"]["Tags"]),
                BTreeMap::new(),
                "case {} / {region}: expected no tags on a record no account owns, got {:?}",
                public.id,
                answer.body
            );
            assert_eq!(
                answer.body["Network"], published.body["Network"],
                "case {} / {region}: expected every Region to read the same record, got {:?}",
                public.id, answer.body
            );
        }
    }
}

/// The contrast that keeps the rule about OWNERSHIP rather than about networks or about
/// Ethereum: a network this account created is tagged through its ARN, and so is a node
/// this account created ON one of AWS's standing public networks — the write lands in that
/// record, its own read reports it, and the public network it runs on still holds nothing.
#[tokio::test]
async fn an_accounts_own_resources_are_tagged_including_those_on_a_public_network() {
    let router = new_router();

    // The account's own network, created through this Region's endpoint.
    let created = call(
        &router,
        "POST",
        "/networks",
        Some(create_network_body("owned-network", "ownednet")),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "expected CreateNetwork 200, got {} {:?}",
        created.status, created.body
    );
    let network_id = created.text(&["NetworkId"]);

    // The account's own node, created ON a public network.
    // `NodeConfiguration$AvailabilityZone` is "Required for Ethereum nodes".
    let public_id = state::PUBLIC_NETWORKS[0].id;
    let created = call(
        &router,
        "POST",
        &format!("/networks/{public_id}/nodes"),
        Some(json!({
            "ClientRequestToken": "owned-node",
            "NodeConfiguration": {
                "InstanceType": INSTANCE_TYPE,
                "AvailabilityZone": format!("{REGION}a"),
            }
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "expected CreateNode on a public network 200, got {} {:?}",
        created.status, created.body
    );
    let node_id = created.text(&["NodeId"]);

    // Both were just created, and a tagging operation is refused while a resource is still
    // `CREATING`.
    tokio::time::sleep(PAST_TRANSITION).await;

    // Rows: each resource the account holds, the ARN it is addressed by, and the read that
    // projects its own tag set.
    let rows: &[(&str, String, String, &str)] = &[
        (
            "the account's own network",
            arn_in(REGION, "networks", &network_id),
            format!("/networks/{network_id}"),
            "Network",
        ),
        (
            "the account's own node on a public network",
            arn_in(REGION, "nodes", &node_id),
            format!("/networks/{public_id}/nodes/{node_id}"),
            "Node",
        ),
    ];

    for (case, resource_arn, read_path, shape) in rows {
        let written = tag_resource_in(&router, REGION, resource_arn, json!({ "owner": case })).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected TagResource 200 for {resource_arn}, got {} {:?}",
            written.status, written.body
        );
        let expected: BTreeMap<String, String> =
            [("owner".to_string(), (*case).to_string())].into_iter().collect();

        let listed = list_tags_in(&router, REGION, resource_arn).await;
        assert_eq!(
            tag_map(&listed.body["Tags"]),
            expected,
            "case {case:?}: expected ListTagsForResource to report {expected:?}, got {} {:?}",
            listed.status,
            listed.body
        );
        let own = call(&router, "GET", read_path, None).await;
        assert_eq!(
            tag_map(&own.body[*shape]["Tags"]),
            expected,
            "case {case:?}: expected the resource's own read to report {expected:?}, got {:?}",
            own.body
        );

        let removed = untag_resource_in(&router, REGION, resource_arn, &["owner"]).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected UntagResource 200, got {} {:?}",
            removed.status, removed.body
        );
        assert_eq!(
            tag_map(&list_tags_in(&router, REGION, resource_arn).await.body["Tags"]),
            BTreeMap::new(),
            "case {case:?}: expected the removal to leave no tags"
        );
    }

    // The public network the node runs on holds nothing of its own: tagging a resource ON
    // a record no account owns never writes to that record.
    let public = get_network(&router, public_id).await;
    assert_eq!(
        tag_map(&public.body["Network"]["Tags"]),
        BTreeMap::new(),
        "expected the public network to hold no tags, got {:?}",
        public.body
    );
}
