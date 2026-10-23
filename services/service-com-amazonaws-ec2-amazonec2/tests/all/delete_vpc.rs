// SPDX-License-Identifier: CC0-1.0

//! What `DeleteVpc` takes with the VPC, and the ids it refuses — end to end over the wire
//! the service actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST
//! whose body carries `Action=DeleteVpc`, answered with XML (README.md / OPERATIONS.md
//! "Wire protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that the delete REMOVES THE VPC THE REQUEST NAMES AND EVERY
//! RECORD THAT NAMES IT, whatever shape that VPC has, and leaves every other VPC alone:
//! the VPC stops being reported by `DescribeVpcs`, the default security group it came with
//! stops being reported by `DescribeSecurityGroups` ("when you delete the VPC, it deletes
//! the default security group, network ACL, and route table for the VPC" — `DeleteVpc`),
//! the attributes and the clock-owed `available` transition the create wrote for it are
//! gone with it, and the id now names nothing — so a reader and a second delete both refuse
//! it rather than answering as a no-op.
//!
//! What a delete refuses WHILE something is still associated with the VPC, the `DryRun`
//! answer and the byte budget a delete gives back are the subject of
//! [`super::vpc_network_lifecycle`], which covers them across the VPC-scoped resources
//! together; this suite is the VPC's own removal: the shapes of VPC it works for, the
//! records that must not outlive one, and the forms of id it refuses.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code};
use super::wire::{param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// One row of a refusal table: the case, the `VpcId` the request carries, and the EC2 error
/// code the service answers.
type RefusalCase = (&'static str, &'static str, &'static str);

/// Long enough past [`ec2::state::TRANSITION_DELAY`] that a transition the clock owes is
/// due, without depending on the exact span.
fn past_the_transition() -> Duration {
    ec2::state::TRANSITION_DELAY + Duration::from_millis(250)
}

/// One `DeleteVpc` call carrying `params`, as its HTTP status and XML body.
async fn delete_vpc(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DeleteVpc", params).await
}

/// One `DescribeVpcs` call carrying `params`, as its HTTP status and XML body.
async fn describe_vpcs(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeVpcs", params).await
}

/// Every `VpcId` a `DescribeVpcs` of the whole account answers with, requiring success.
async fn described_vpc_ids(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = describe_vpcs(router, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpcs, got {status} with body {body}"
    );
    elements(&body, "vpcId")
}

/// `CreateVpc` with `params`, requiring success, and the `VpcId` it minted.
async fn create_vpc(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = super::ec2_query::call(router, REGION, "CreateVpc", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVpc, got {status} with body {body}"
    );
    element(&body, "vpcId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateVpc must report the VPC it minted: {body}"))
}

/// Every `GroupId` `DescribeSecurityGroups` reports for `vpc_id`, requiring success.
///
/// A set, not a list: a group's own id appears a second time inside the rule that
/// references the group itself, which the default group of every VPC carries.
async fn groups_of_vpc(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> BTreeSet<String> {
    let params = [
        param("Filter.1.Name", "vpc-id"),
        param("Filter.1.Value.1", vpc_id),
    ];
    let (status, body) =
        super::ec2_query::call(router, REGION, "DescribeSecurityGroups", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSecurityGroups, got {status} \
         with body {body}"
    );
    elements(&body, "groupId").into_iter().collect()
}

/// The id of the account's default VPC, which a fresh store already holds.
fn default_vpc_id(engine: &engine::InMemoryEngine) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .default_vpc_id()
        .expect("a fresh account has a default VPC")
}

// ---------------------------------------------------------------------------
// What the delete takes with the VPC
// ---------------------------------------------------------------------------

/// The rule: `DeleteVpc` removes the VPC the request names together with every record that
/// names it — its default security group, its attributes and the transition the clock owed
/// it — leaves every other VPC alone, and leaves the id naming nothing, so a reader and a
/// second delete both refuse it.
///
/// The rows differ in the shape of the VPC being deleted: its block size, its tenancy, how
/// many tags it carries, the form of its IPv6 association, and whether the clock has already
/// moved it from `pending` to `available`.
#[tokio::test]
async fn delete_vpc_removes_the_named_vpc_and_every_record_that_names_it() {
    let (router, engine) = super::ec2_query::emulator();
    let default_vpc = default_vpc_id(&engine);
    // A bystander VPC that no row deletes: every delete must leave it, and the account's
    // default VPC, exactly where they were.
    let bystander = create_vpc("bystander", &router, &[param("CidrBlock", "10.200.0.0/16")]).await;

    // (case, the members of the CreateVpc request, whether to wait the transition out)
    let cases: [(&str, Vec<Param>, bool); 6] = [
        (
            "the plainest VPC",
            vec![param("CidrBlock", "10.1.0.0/16")],
            false,
        ),
        (
            "the smallest allowed block",
            vec![param("CidrBlock", "192.168.100.0/28")],
            false,
        ),
        (
            "dedicated tenancy",
            vec![
                param("CidrBlock", "10.2.0.0/16"),
                param("InstanceTenancy", "dedicated"),
            ],
            false,
        ),
        (
            "three tags",
            vec![
                param("CidrBlock", "10.3.0.0/20"),
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "alpha"),
                param("TagSpecification.1.Tag.2.Key", "Tier"),
                param("TagSpecification.1.Tag.2.Value", "web"),
                param("TagSpecification.1.Tag.3.Key", "Owner"),
                param("TagSpecification.1.Tag.3.Value", "platform"),
            ],
            false,
        ),
        (
            "a VPC that brought its own IPv6 block",
            vec![
                param("CidrBlock", "10.4.0.0/16"),
                param("Ipv6CidrBlock", "2001:db8:1234:1a00::/56"),
                param("Ipv6Pool", "ipv6pool-ec2-0123456789abcdef0"),
            ],
            false,
        ),
        (
            "a VPC past its available transition",
            vec![
                param("CidrBlock", "10.5.0.0/16"),
                param("AmazonProvidedIpv6CidrBlock", "true"),
            ],
            true,
        ),
    ];

    for (case, create_params, wait_for_available) in cases {
        let vpc_id = create_vpc(case, &router, &create_params).await;
        if wait_for_available {
            tokio::time::sleep(past_the_transition()).await;
        }
        let before = described_vpc_ids(case, &router).await;
        assert!(
            before.contains(&vpc_id),
            "case {case:?}: the VPC must be reported before it is deleted, got {before:?}"
        );
        let groups_before = groups_of_vpc(case, &router, &vpc_id).await;
        assert_eq!(
            groups_before.len(),
            1,
            "case {case:?}: a new VPC comes with exactly its default security group, \
             got {groups_before:?}"
        );

        let (status, body) = delete_vpc(&router, &[param("VpcId", &vpc_id)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteVpc, got {status} with body {body}"
        );

        let after = described_vpc_ids(case, &router).await;
        assert!(
            !after.contains(&vpc_id),
            "case {case:?}: the deleted VPC {vpc_id} must no longer be reported, got {after:?}"
        );
        for survivor in [&default_vpc, &bystander] {
            assert!(
                after.contains(survivor),
                "case {case:?}: deleting {vpc_id} must leave {survivor}, got {after:?}"
            );
        }
        let groups_after = groups_of_vpc(case, &router, &vpc_id).await;
        assert!(
            groups_after.is_empty(),
            "case {case:?}: deleting the VPC deletes its default security group, \
             got {groups_after:?}"
        );
        // No record of the VPC may outlive it: neither the attributes the create wrote for
        // it nor the transition the clock owed it.
        {
            let store = engine.state::<Ec2State>();
            let store = store.lock();
            assert!(
                !store.vpc_attributes.contains_key(&vpc_id),
                "case {case:?}: the attributes of the deleted VPC {vpc_id} must be gone too"
            );
            assert!(
                !store.vpc_available_at.contains_key(&vpc_id),
                "case {case:?}: nothing may still be owed to the deleted VPC {vpc_id}"
            );
        }

        // The id now names nothing, so both the reader and a second delete refuse it.
        let (read_status, read_body) = describe_vpcs(&router, &[param("VpcId.1", &vpc_id)]).await;
        assert_eq!(
            read_status, 400,
            "case {case:?}: expected HTTP 400 reading the deleted VPC, got {read_status} \
             with body {read_body}"
        );
        assert_eq!(
            error_code(&read_body),
            "InvalidVpcID.NotFound",
            "case {case:?}: expected InvalidVpcID.NotFound from DescribeVpcs, got {read_body}"
        );
        let (again_status, again_body) = delete_vpc(&router, &[param("VpcId", &vpc_id)]).await;
        assert_eq!(
            again_status, 400,
            "case {case:?}: a delete is not idempotent; expected HTTP 400, got {again_status} \
             with body {again_body}"
        );
        assert_eq!(
            error_code(&again_body),
            "InvalidVpcID.NotFound",
            "case {case:?}: expected InvalidVpcID.NotFound from the second DeleteVpc, \
             got {again_body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The ids a delete refuses
// ---------------------------------------------------------------------------

/// The rule: a request whose `VpcId` names no VPC of the account deletes nothing and is
/// refused with `InvalidVpcID.NotFound`.
///
/// The rows differ in the form of the value: both published id lengths (8 and 17 hex
/// digits), another resource kind's id, text that is no id at all, and an empty value, which
/// names no VPC like any other value the account holds none for.
#[tokio::test]
async fn an_id_that_names_no_vpc_deletes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let kept = create_vpc("kept", &router, &[param("CidrBlock", "10.6.0.0/16")]).await;
    let before = described_vpc_ids("before", &router).await;

    let cases: [RefusalCase; 5] = [
        (
            "a long id no VPC has",
            "vpc-0123456789abcdef0",
            "InvalidVpcID.NotFound",
        ),
        (
            "a short id no VPC has",
            "vpc-1a2b3c4d",
            "InvalidVpcID.NotFound",
        ),
        (
            "another resource kind's id",
            "subnet-0123456789abcdef0",
            "InvalidVpcID.NotFound",
        ),
        ("not an id at all", "not-an-id", "InvalidVpcID.NotFound"),
        ("an empty vpcId", "", "InvalidVpcID.NotFound"),
    ];

    for (case, vpc_id, expected_code) in cases {
        let (status, body) = delete_vpc(&router, &[param("VpcId", vpc_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        let after = described_vpc_ids(case, &router).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused delete must change nothing, {before:?} became {after:?}"
        );
    }

    // A request that omits the member entirely never reaches the operation: `VpcId` is
    // required, so the boundary validator refuses it first.
    let (status, body) = delete_vpc(&router, &[]).await;
    assert_eq!(
        status, 400,
        "a request with no VpcId must be refused with HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "MalformedInput",
        "a request with no VpcId is refused by the boundary validator, got {body}"
    );
    assert_eq!(
        described_vpc_ids("no vpcId at all", &router).await,
        before,
        "a refused delete must change nothing"
    );
    assert!(
        engine.state::<Ec2State>().lock().vpcs.contains_key(&kept),
        "the VPC no row named must still be stored"
    );
}
