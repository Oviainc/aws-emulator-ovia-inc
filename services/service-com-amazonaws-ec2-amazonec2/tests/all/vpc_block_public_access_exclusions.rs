// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the VPC Block Public Access exclusion slice —
//! `CreateVpcBlockPublicAccessExclusion` and `DescribeVpcBlockPublicAccessExclusions` —
//! driven over the wire the service actually speaks: `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `CreateVpcBlockPublicAccessExclusion` APPLIES ONE EXCLUSION TO THE ONE VPC OR SUBNET
//!    THE REQUEST NAMES and `DescribeVpcBlockPublicAccessExclusions` REPORTS THAT STORE:
//!    the mode and the tags the request sets round-trip, and the members the service owns
//!    (the exclusion id, its ARN, its state and its timestamps) are derived rather than
//!    taken;
//! 2. the CLOCK, not a caller, moves an exclusion `create-in-progress` ->
//!    `create-complete`;
//! 3. a request that names no target, both targets, a target the account does not hold, a
//!    mode outside the modelled catalog, or a target that already carries an exclusion is
//!    refused with EC2's own error code, and nothing is stored;
//! 4. `ExclusionIds`, `Filters` and `MaxResults` narrow and page the report, in
//!    `ExclusionId` order.
//!
//! Each table row is a distinct input along what the rule generalises over — which member
//! names the target, which mode (every member of the modelled catalog), how many tags,
//! which filter, which values, which page size at each published bound and one past it — so
//! no row is the specification.

use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, RESOURCE_TYPE_VPC_BLOCK_PUBLIC_ACCESS_EXCLUSION,
    TRANSITION_DELAY, VPC_BPA_EXCLUSION_STATE_CREATE_COMPLETE,
    VPC_BPA_EXCLUSION_STATE_CREATE_IN_PROGRESS,
};
use ec2::ops::types::INTERNET_GATEWAY_EXCLUSION_MODE_VALUES;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, filters, indexed};
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for. An exclusion's ARN names the Region the
/// request was signed for, so it is read back out of that ARN below.
const REGION: &str = "us-west-2";

/// A well-formed VPC id no VPC of the account is stored under.
const UNKNOWN_VPC_ID: &str = "vpc-0ffffffffffffffff";

/// A well-formed subnet id no subnet of the account is stored under.
const UNKNOWN_SUBNET_ID: &str = "subnet-0ffffffffffffffff";

/// How long a clock-owed transition takes to fall due: the service's own span, plus a
/// margin so a slow machine cannot read the state before it is due.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(400)).await;
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn api(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One `DescribeVpcBlockPublicAccessExclusions` call.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    api(router, "DescribeVpcBlockPublicAccessExclusions", params).await
}

/// A VPC with `cidr_block`, as its id — one of the two kinds of target an exclusion covers.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {body}"))
}

/// A subnet of `vpc_id` with `cidr_block`, as its id — the other kind of target.
async fn create_subnet(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[param("VpcId", vpc_id), param("CidrBlock", cidr_block)],
    )
    .await;
    element(&body, "subnetId").unwrap_or_else(|| panic!("case {case:?}: no <subnetId> in {body}"))
}

/// How many exclusions the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .vpc_block_public_access_exclusions
        .len()
}

/// One exclusion as a response reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Exclusion {
    exclusion_id: String,
    internet_gateway_exclusion_mode: String,
    resource_arn: String,
    state: String,
    creation_timestamp: String,
    last_update_timestamp: String,
    deletion_timestamp: Option<String>,
    tags: Vec<(String, String)>,
}

/// Every exclusion a response carries, in document order: the elements that have an
/// `exclusionId` child, which is both `<vpcBlockPublicAccessExclusion>` in a create's answer
/// and each `<item>` of the describe's `<vpcBlockPublicAccessExclusionSet>`.
fn reported(case: &str, body: &str) -> Vec<Exclusion> {
    let doc = document(case, body);
    doc.descendants()
        .filter(|node| node.is_element() && child(*node, "exclusionId").is_some())
        .map(|node| Exclusion {
            exclusion_id: child_text(node, "exclusionId").unwrap_or_default(),
            internet_gateway_exclusion_mode: child_text(node, "internetGatewayExclusionMode")
                .unwrap_or_default(),
            resource_arn: child_text(node, "resourceArn").unwrap_or_default(),
            state: child_text(node, "state").unwrap_or_default(),
            creation_timestamp: child_text(node, "creationTimestamp").unwrap_or_default(),
            last_update_timestamp: child_text(node, "lastUpdateTimestamp").unwrap_or_default(),
            deletion_timestamp: child_text(node, "deletionTimestamp"),
            tags: child(node, "tagSet")
                .map(|set| {
                    set.children()
                        .filter(|item| item.is_element() && item.tag_name().name() == "item")
                        .map(|item| {
                            (
                                child_text(item, "key").unwrap_or_default(),
                                child_text(item, "value").unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// The one exclusion a response carries, or a panic naming the case.
fn only(case: &str, body: &str) -> Exclusion {
    let found = reported(case, body);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one exclusion, got {found:?} from {body}"
    );
    found.into_iter().next().expect("one exclusion")
}

/// The form fields for one `TagSpecification` of `tags` on the exclusion being created.
fn tag_specification(tags: &[(&str, &str)]) -> Vec<Param> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        RESOURCE_TYPE_VPC_BLOCK_PUBLIC_ACCESS_EXCLUSION,
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

/// Which member of the request names the resource an exclusion is applied to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// `VpcId` — "a VPC ID".
    Vpc,
    /// `SubnetId` — "a subnet ID".
    Subnet,
}

impl Target {
    /// The request member that carries this kind of id.
    fn member(self) -> &'static str {
        match self {
            Self::Vpc => "VpcId",
            Self::Subnet => "SubnetId",
        }
    }
}

/// One row of the create table: the case name, which member names the target, the mode, and
/// the tags the request assigns.
type CreateCase<'a> = (&'a str, Target, &'a str, &'a [(&'a str, &'a str)]);

/// A create call for `target_id`, with `mode` and `tags`.
fn create_params(
    target: Target,
    target_id: &str,
    mode: &str,
    tags: &[(&str, &str)],
) -> Vec<Param> {
    let mut params = vec![
        param(target.member(), target_id),
        param("InternetGatewayExclusionMode", mode),
    ];
    params.extend(tag_specification(tags));
    params
}

/// One exclusion created for `target_id`, as the create reported it.
async fn create_exclusion(
    case: &str,
    router: &MultiServiceRouter,
    target: Target,
    target_id: &str,
    mode: &str,
    tags: &[(&str, &str)],
) -> Exclusion {
    let body = ok(
        case,
        router,
        "CreateVpcBlockPublicAccessExclusion",
        &create_params(target, target_id, mode, tags),
    )
    .await;
    only(case, &body)
}

// ---------------------------------------------------------------------------
// Rule 1: a create stores the exclusion, and the describe reports the store
// ---------------------------------------------------------------------------

/// RULE 1: the exclusion a create answers is the one the describe reports, every member the
/// request set round-trips, and the members the service owns are derived.
///
/// The rows differ in which member names the target (a VPC or a subnet), in the mode, and in
/// how many tags the request assigns — so no one shape of request is the specification.
#[tokio::test]
async fn a_create_stores_the_exclusion_and_the_describe_reports_it() {
    let (router, _engine) = super::ec2_query::emulator();

    // (case, which member names the target, the mode, the tags)
    let cases: [CreateCase; 5] = [
        ("a VPC, bidirectional, untagged", Target::Vpc, "allow-bidirectional", &[]),
        ("a VPC, egress-only, one tag", Target::Vpc, "allow-egress", &[("Name", "vpc-exclusion")]),
        (
            "a subnet, bidirectional, two tags",
            Target::Subnet,
            "allow-bidirectional",
            &[("Name", "subnet-exclusion"), ("Tier", "web")],
        ),
        ("a subnet, egress-only, untagged", Target::Subnet, "allow-egress", &[]),
        (
            "a VPC, bidirectional, a tag whose value is empty",
            Target::Vpc,
            "allow-bidirectional",
            &[("Owner", "")],
        ),
    ];

    for (index, (case, target, mode, tags)) in cases.into_iter().enumerate() {
        // A VPC per row, so no row's target carries another row's exclusion.
        let vpc_cidr = format!("10.{index}.0.0/16");
        let vpc_id = create_vpc(case, &router, &vpc_cidr).await;
        let target_id = match target {
            Target::Vpc => vpc_id,
            Target::Subnet => {
                create_subnet(case, &router, &vpc_id, &format!("10.{index}.1.0/24")).await
            }
        };

        let created = create_exclusion(case, &router, target, &target_id, mode, tags).await;

        assert!(
            created
                .exclusion_id
                .starts_with(&format!("{}-", ec2::state::VPC_BPA_EXCLUSION_ID_PREFIX)),
            "case {case:?}: expected an exclusion id the service minted, got {:?}",
            created.exclusion_id
        );
        assert_eq!(
            created.internet_gateway_exclusion_mode, mode,
            "case {case:?}: expected the requested mode, got {:?}",
            created.internet_gateway_exclusion_mode
        );
        // "The ARN of the exclusion" (`VpcBlockPublicAccessExclusion$ResourceArn`): an EC2
        // ARN of the Region the request was signed for, under the resource type the modelled
        // `ResourceType` catalog publishes for this resource.
        let expected_arn = format!(
            "arn:aws:ec2:{REGION}:{account}:{RESOURCE_TYPE_VPC_BLOCK_PUBLIC_ACCESS_EXCLUSION}/{id}",
            account = engine::emulator_account_id(),
            id = created.exclusion_id,
        );
        assert_eq!(
            created.resource_arn, expected_arn,
            "case {case:?}: expected the exclusion's own ARN {expected_arn:?}, got {:?}",
            created.resource_arn
        );
        assert_eq!(
            created.state, VPC_BPA_EXCLUSION_STATE_CREATE_IN_PROGRESS,
            "case {case:?}: expected an exclusion the clock still owes `create-complete`, \
             got {:?}",
            created.state
        );
        assert!(
            !created.creation_timestamp.is_empty() && !created.last_update_timestamp.is_empty(),
            "case {case:?}: expected both timestamps to be reported, got {created:?}"
        );
        assert_eq!(
            created.deletion_timestamp, None,
            "case {case:?}: an exclusion nothing deleted carries no deletion timestamp, \
             got {created:?}"
        );
        let expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            created.tags, expected_tags,
            "case {case:?}: expected the requested tags {expected_tags:?}, got {:?}",
            created.tags
        );

        // The describe reports the same record, member for member.
        let body = ok(
            case,
            &router,
            "DescribeVpcBlockPublicAccessExclusions",
            &indexed("ExclusionId", &[created.exclusion_id.as_str()]),
        )
        .await;
        let described = only(case, &body);
        assert_eq!(
            described, created,
            "case {case:?}: the describe must report the exclusion the create stored, \
             got {described:?}"
        );
    }
}

/// RULE 1: every member of the modelled `InternetGatewayExclusionMode` catalog is accepted
/// and reported back.
///
/// The catalog is the generated
/// [`ec2::ops::types::INTERNET_GATEWAY_EXCLUSION_MODE_VALUES`], so a member added to the
/// model is covered here without this test being edited.
#[tokio::test]
async fn every_documented_exclusion_mode_is_accepted_and_reported() {
    let (router, _engine) = super::ec2_query::emulator();

    for (index, mode) in INTERNET_GATEWAY_EXCLUSION_MODE_VALUES.iter().enumerate() {
        let case = format!("mode {mode}");
        let vpc_id = create_vpc(&case, &router, &format!("10.{}.0.0/16", index + 100)).await;
        let created = create_exclusion(&case, &router, Target::Vpc, &vpc_id, mode, &[]).await;
        assert_eq!(
            created.internet_gateway_exclusion_mode, *mode,
            "case {case:?}: expected the mode to round-trip, got {created:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the clock moves a new exclusion to `create-complete`
// ---------------------------------------------------------------------------

/// RULE 2: a new exclusion is reported `create-in-progress` straight after the create — the
/// span is never zero — and `create-complete` once the clock has passed the due instant,
/// without anything else being called.
#[tokio::test]
async fn the_clock_moves_a_new_exclusion_to_create_complete() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "the clock owes `create-complete`";
    let vpc_id = create_vpc(case, &router, "10.200.0.0/16").await;
    let created = create_exclusion(
        case,
        &router,
        Target::Vpc,
        &vpc_id,
        INTERNET_GATEWAY_EXCLUSION_MODE_VALUES[0],
        &[],
    )
    .await;

    let ids = indexed("ExclusionId", &[created.exclusion_id.as_str()]);
    let body = ok(
        case,
        &router,
        "DescribeVpcBlockPublicAccessExclusions",
        &ids,
    )
    .await;
    let before = only(case, &body);
    assert_eq!(
        before.state, VPC_BPA_EXCLUSION_STATE_CREATE_IN_PROGRESS,
        "case {case:?}: a caller reading straight after the create still sees the transient \
         state, got {before:?}"
    );

    past_the_transition().await;

    let body = ok(
        case,
        &router,
        "DescribeVpcBlockPublicAccessExclusions",
        &ids,
    )
    .await;
    let after = only(case, &body);
    assert_eq!(
        after.state, VPC_BPA_EXCLUSION_STATE_CREATE_COMPLETE,
        "case {case:?}: the clock must move the exclusion to `create-complete`, got {after:?}"
    );
    assert_eq!(
        after.creation_timestamp, before.creation_timestamp,
        "case {case:?}: when the exclusion was created cannot change, got {after:?}"
    );
    assert!(
        !after.last_update_timestamp.is_empty(),
        "case {case:?}: an exclusion the clock moved still reports when it was last \
         updated, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a create the service cannot satisfy is refused, and stores nothing
// ---------------------------------------------------------------------------

/// RULE 3: a create the service cannot satisfy is refused with EC2's own code for what is
/// wrong with it, and nothing is stored.
///
/// The rows are the ways one request can fail: no target at all, both targets at once, each
/// kind of target the account does not hold, a mode outside the modelled catalog, a tag
/// specification naming another resource type, and a second exclusion for a resource that
/// already carries one.
#[tokio::test]
async fn a_create_the_service_cannot_satisfy_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let setup = "the refusal suite's targets";
    let vpc_id = create_vpc(setup, &router, "10.50.0.0/16").await;
    let subnet_id = create_subnet(setup, &router, &vpc_id, "10.50.1.0/24").await;
    // The one exclusion the duplicate row below collides with.
    let held = create_exclusion(
        setup,
        &router,
        Target::Subnet,
        &subnet_id,
        "allow-egress",
        &[],
    )
    .await;
    let stored_before = stored_count(&engine);
    assert_eq!(
        stored_before, 1,
        "the refusal suite starts from the one exclusion it created, got {stored_before}"
    );

    let bidirectional = "allow-bidirectional";
    // (case, the request's fields, the expected error code)
    let cases: [(&str, Vec<Param>, &str); 8] = [
        (
            "no target at all",
            vec![param("InternetGatewayExclusionMode", bidirectional)],
            "MissingParameter",
        ),
        (
            "both targets at once",
            vec![
                param("VpcId", &vpc_id),
                param("SubnetId", &subnet_id),
                param("InternetGatewayExclusionMode", bidirectional),
            ],
            "InvalidParameterCombination",
        ),
        (
            // A member that is present but empty names no resource, which is a bad value
            // for that member rather than an absent one.
            "an empty VPC id",
            vec![
                param("VpcId", ""),
                param("InternetGatewayExclusionMode", bidirectional),
            ],
            "InvalidParameterValue",
        ),
        (
            "a VPC the account does not hold",
            create_params(Target::Vpc, UNKNOWN_VPC_ID, bidirectional, &[]),
            "InvalidVpcID.NotFound",
        ),
        (
            "a subnet the account does not hold",
            create_params(Target::Subnet, UNKNOWN_SUBNET_ID, bidirectional, &[]),
            "InvalidSubnetID.NotFound",
        ),
        (
            // The modelled `InternetGatewayExclusionMode` enum is refused at the boundary,
            // before the handler runs, so the code is the validator's rather than EC2's
            // own `InvalidParameterValue`.
            "a mode outside the modelled catalog",
            create_params(Target::Vpc, &vpc_id, "allow-nothing", &[]),
            "ValidationError",
        ),
        (
            "a tag specification for another resource type",
            {
                let mut params = create_params(Target::Vpc, &vpc_id, bidirectional, &[]);
                params.push(param("TagSpecification.1.ResourceType", "vpc"));
                params.push(param("TagSpecification.1.Tag.1.Key", "Name"));
                params.push(param("TagSpecification.1.Tag.1.Value", "nope"));
                params
            },
            "InvalidParameterValue",
        ),
        (
            "a second exclusion for a resource that already carries one",
            create_params(Target::Subnet, &subnet_id, bidirectional, &[]),
            "InvalidParameterValue",
        ),
    ];

    for (case, params, expected_code) in cases {
        let (status, body) = api(&router, "CreateVpcBlockPublicAccessExclusion", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            stored_before,
            "case {case:?}: a refused create must store nothing"
        );
    }

    // The exclusion that was already there is untouched by every refusal above.
    let body = ok(
        "the held exclusion",
        &router,
        "DescribeVpcBlockPublicAccessExclusions",
        &[],
    )
    .await;
    let reported_ids: Vec<String> = reported("the held exclusion", &body)
        .into_iter()
        .map(|exclusion| exclusion.exclusion_id)
        .collect();
    assert_eq!(
        reported_ids,
        vec![held.exclusion_id],
        "the refusals must leave the one stored exclusion alone, got {reported_ids:?}"
    );
}

/// RULE 3: `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and
/// creates nothing; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_creating() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "a dry run";
    let vpc_id = create_vpc(case, &router, "10.60.0.0/16").await;

    let mut params = create_params(Target::Vpc, &vpc_id, "allow-bidirectional", &[]);
    params.push(param("DryRun", "true"));
    let (status, body) = api(&router, "CreateVpcBlockPublicAccessExclusion", &params).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got {body}"
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "case {case:?}: a dry run must store nothing"
    );

    // The same request without the dry run is an ordinary create.
    let created = create_exclusion(case, &router, Target::Vpc, &vpc_id, "allow-egress", &[]).await;
    assert_eq!(
        stored_count(&engine),
        1,
        "case {case:?}: the real create must store the exclusion {created:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: ExclusionIds, Filters and MaxResults narrow and page the report
// ---------------------------------------------------------------------------

/// The exclusions the narrowing tests below work against: one per subnet of one VPC, with a
/// mode and a tag per row.
struct Seeded {
    /// The exclusions, in the order their ids sort in (which is creation order).
    exclusions: Vec<Exclusion>,
}

impl Seeded {
    /// The ids of the exclusions at `positions`.
    fn ids_at(&self, positions: &[usize]) -> Vec<String> {
        positions
            .iter()
            .map(|position| self.exclusions[*position].exclusion_id.clone())
            .collect()
    }

    /// Every id, in order.
    fn ids(&self) -> Vec<String> {
        self.exclusions
            .iter()
            .map(|exclusion| exclusion.exclusion_id.clone())
            .collect()
    }
}

/// Create `count` exclusions, each on a subnet of its own, alternating the mode and giving
/// each a `Stage` tag so the filter rows have something to select on.
async fn seed(router: &MultiServiceRouter, count: usize) -> Seeded {
    let case = "the narrowing suite's exclusions";
    let vpc_id = create_vpc(case, router, "10.70.0.0/16").await;
    let mut exclusions = Vec::new();
    for index in 0..count {
        let subnet_id = create_subnet(case, router, &vpc_id, &format!("10.70.{index}.0/24")).await;
        let mode = INTERNET_GATEWAY_EXCLUSION_MODE_VALUES
            [index % INTERNET_GATEWAY_EXCLUSION_MODE_VALUES.len()];
        let stage = if index % 2 == 0 { "prod" } else { "dev" };
        exclusions
            .push(create_exclusion(case, router, Target::Subnet, &subnet_id, mode, &[("Stage", stage)]).await);
    }
    Seeded { exclusions }
}

/// The ids a describe reports for `params`.
async fn described_ids(case: &str, router: &MultiServiceRouter, params: &[Param]) -> Vec<String> {
    let body = ok(case, router, "DescribeVpcBlockPublicAccessExclusions", params).await;
    reported(case, &body)
        .into_iter()
        .map(|exclusion| exclusion.exclusion_id)
        .collect()
}

/// RULE 4: a describe with no `ExclusionIds` reports every exclusion in `ExclusionId` order,
/// and `ExclusionIds` narrows it to the ids named — in that same order, whatever order the
/// request listed them in.
#[tokio::test]
async fn exclusion_ids_narrow_the_report_and_the_order_is_the_id_order() {
    let (router, _engine) = super::ec2_query::emulator();
    let seeded = seed(&router, 4).await;

    let every = described_ids("no ExclusionIds", &router, &[]).await;
    assert_eq!(
        every,
        seeded.ids(),
        "no ExclusionIds: expected every exclusion in id order, got {every:?}"
    );

    // (case, the positions the request names, the positions expected back)
    let cases: [(&str, &[usize], &[usize]); 4] = [
        ("one id", &[2], &[2]),
        ("two ids", &[0, 3], &[0, 3]),
        ("the same id twice", &[1, 1], &[1]),
        ("ids out of order", &[3, 1, 0], &[0, 1, 3]),
    ];
    for (case, named, expected) in cases {
        let ids = seeded.ids_at(named);
        let params = indexed(
            "ExclusionId",
            &ids.iter().map(String::as_str).collect::<Vec<&str>>(),
        );
        let reported_ids = described_ids(case, &router, &params).await;
        assert_eq!(
            reported_ids,
            seeded.ids_at(expected),
            "case {case:?}: expected the exclusions named, got {reported_ids:?}"
        );
    }

    // An id the account holds no exclusion under is an error, not an absent row.
    let (status, body) = describe(
        &router,
        &indexed("ExclusionId", &["vpcbpa-exclude-0ffffffffffffffff"]),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unknown exclusion id must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidVpcBlockPublicAccessExclusionId.NotFound",
        "an unknown exclusion id must be refused as not found, got {body}"
    );
}

/// RULE 4: each filter the operation documents selects on the exclusion's own member, an
/// undocumented name is refused, and multiple filters are joined with an AND.
#[tokio::test]
async fn documented_filters_select_and_an_undocumented_one_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let seeded = seed(&router, 4).await;
    // Rows 0 and 2 carry `Stage=prod` and the first catalog mode; rows 1 and 3 carry
    // `Stage=dev` and the second.
    let first_mode = INTERNET_GATEWAY_EXCLUSION_MODE_VALUES[0];
    let second_mode = INTERNET_GATEWAY_EXCLUSION_MODE_VALUES[1];
    let one_arn = seeded.exclusions[2].resource_arn.clone();

    // (case, the filters, the positions expected back)
    let cases: [(&str, Vec<Param>, &[usize]); 8] = [
        (
            "internet-gateway-exclusion-mode, first member",
            filters(&[("internet-gateway-exclusion-mode", &[first_mode])]),
            &[0, 2],
        ),
        (
            "internet-gateway-exclusion-mode, second member",
            filters(&[("internet-gateway-exclusion-mode", &[second_mode])]),
            &[1, 3],
        ),
        (
            "internet-gateway-exclusion-mode, both members",
            filters(&[(
                "internet-gateway-exclusion-mode",
                &[first_mode, second_mode],
            )]),
            &[0, 1, 2, 3],
        ),
        (
            "resource-arn, one exclusion's own ARN",
            filters(&[("resource-arn", &[one_arn.as_str()])]),
            &[2],
        ),
        (
            "state, the state every new exclusion is in",
            filters(&[("state", &[VPC_BPA_EXCLUSION_STATE_CREATE_IN_PROGRESS])]),
            &[0, 1, 2, 3],
        ),
        (
            "state, a state none of them is in",
            filters(&[("state", &[VPC_BPA_EXCLUSION_STATE_CREATE_COMPLETE])]),
            &[],
        ),
        ("tag:Stage", filters(&[("tag:Stage", &["dev"])]), &[1, 3]),
        (
            "tag-key and tag-value, joined with an AND",
            filters(&[("tag-key", &["Stage"]), ("tag-value", &["prod"])]),
            &[0, 2],
        ),
    ];
    for (case, params, expected) in cases {
        let reported_ids = described_ids(case, &router, &params).await;
        assert_eq!(
            reported_ids,
            seeded.ids_at(expected),
            "case {case:?}: expected the exclusions the filter selects, got {reported_ids:?}"
        );
    }

    // A name this operation does not document is refused rather than read as "no results",
    // and so is a `tag:` prefix with no key.
    for name in ["exclusion-id", "vpc-id", "State", "tag:"] {
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: an undocumented filter must be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

/// RULE 4: `MaxResults` bounds each page and `NextToken` resumes from where the last page
/// ended, so walking the pages reports every exclusion exactly once in id order; a request
/// with no `MaxResults` is not paged at all.
#[tokio::test]
async fn max_results_pages_the_report_and_the_token_resumes_it() {
    let (router, _engine) = super::ec2_query::emulator();
    let seeded = seed(&router, 12).await;

    // The model publishes `range 5..1000` for `MaxResults`, so the smallest page size the
    // service serves is the interesting one for a walk of 12 exclusions.
    for page_size in [5usize, 12, 1000] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(next) = token.as_deref() {
                params.push(param("NextToken", next));
            }
            let body = ok(
                &case,
                &router,
                "DescribeVpcBlockPublicAccessExclusions",
                &params,
            )
            .await;
            let page: Vec<String> = reported(&case, &body)
                .into_iter()
                .map(|exclusion| exclusion.exclusion_id)
                .collect();
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults exclusions, got {page:?}"
            );
            walked.extend(page);
            token = element(&body, "nextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked,
            seeded.ids(),
            "case {case:?}: the walk must report every exclusion exactly once in id order, \
             got {walked:?}"
        );
    }

    let unpaged = described_ids("no MaxResults", &router, &[]).await;
    assert_eq!(
        unpaged,
        seeded.ids(),
        "no MaxResults: expected every exclusion in one answer, got {unpaged:?}"
    );
}

/// RULE 4: `MaxResults` outside the range the model publishes is refused at the boundary;
/// each published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, _engine) = super::ec2_query::emulator();
    seed(&router, 6).await;

    // The published bounds (`DescribeVpcBlockPublicAccessExclusionsMaxResults`,
    // `range 5..1000`) and one past each.
    for (max_results, expected_status) in [(4, 400), (5, 200), (1000, 200), (1001, 400)] {
        let (status, body) = describe(&router, &[param("MaxResults", &max_results.to_string())])
            .await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// RULE 4: a `NextToken` this service did not issue is refused with `InvalidPaginationToken`
/// rather than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let seeded = seed(&router, 2).await;

    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded.exclusions[0].exclusion_id.clone(),
        ::hex::encode(&seeded.exclusions[0].exclusion_id),
    ];
    for token in cases {
        let (status, body) = describe(&router, &[param("NextToken", &token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// RULE 4: the request-controlled factors of the filter scan are capped before any matching
/// runs: more filters than [`ec2::state::MAX_FILTERS`], more values than
/// [`ec2::state::MAX_FILTER_VALUES`] in one filter, a value longer than
/// [`ec2::state::MAX_FILTER_VALUE_BYTES`], or more ids than
/// [`ec2::state::MAX_RESOURCE_IDS`] are refused, and each cap is accepted AT its bound.
#[tokio::test]
async fn the_request_controlled_factors_of_the_scan_are_capped() {
    use ec2::state::{
        MAX_FILTERS, MAX_FILTER_VALUES, MAX_FILTER_VALUE_BYTES, MAX_RESOURCE_IDS,
    };

    let (router, _engine) = super::ec2_query::emulator();
    let seeded = seed(&router, 1).await;
    let held = seeded.exclusions[0].exclusion_id.clone();

    /// One documented filter repeated `count` times, each with one value.
    fn many_filters(count: usize) -> Vec<Param> {
        (1..=count)
            .flat_map(|index| {
                vec![
                    param(&format!("Filter.{index}.Name"), "state"),
                    param(&format!("Filter.{index}.Value.1"), "create-in-progress"),
                ]
            })
            .collect()
    }

    /// One documented filter carrying `count` values.
    fn many_values(count: usize) -> Vec<Param> {
        let mut params = vec![param("Filter.1.Name", "state")];
        params.extend(
            (1..=count).map(|index| param(&format!("Filter.1.Value.{index}"), "no-such-state")),
        );
        params
    }

    /// `ExclusionIds` carrying `count` copies of `id`.
    fn many_ids(id: &str, count: usize) -> Vec<Param> {
        (1..=count)
            .map(|index| param(&format!("ExclusionId.{index}"), id))
            .collect()
    }

    // (case, the request's fields, the expected HTTP status)
    let cases: [(&str, Vec<Param>, u16); 8] = [
        ("filters at the cap", many_filters(MAX_FILTERS), 200),
        ("one filter past the cap", many_filters(MAX_FILTERS + 1), 400),
        ("values at the cap", many_values(MAX_FILTER_VALUES), 200),
        ("one value past the cap", many_values(MAX_FILTER_VALUES + 1), 400),
        (
            "a value at the byte cap",
            filters(&[("state", &["v".repeat(MAX_FILTER_VALUE_BYTES).as_str()])]),
            200,
        ),
        (
            "a value one byte past the cap",
            filters(&[("state", &["v".repeat(MAX_FILTER_VALUE_BYTES + 1).as_str()])]),
            400,
        ),
        ("ids at the cap", many_ids(&held, MAX_RESOURCE_IDS), 200),
        (
            "one id past the cap",
            many_ids(&held, MAX_RESOURCE_IDS + 1),
            400,
        ),
    ];
    for (case, params, expected_status) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
        }
    }
}

/// RULE 4: `DryRun` on the describe reports the permitted outcome and reads nothing.
#[tokio::test]
async fn dry_run_on_the_describe_reports_the_permitted_outcome() {
    let (router, _engine) = super::ec2_query::emulator();
    seed(&router, 1).await;

    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );

    // A dry run asks whether the request WOULD have succeeded, so one that would have been
    // refused is refused with that reason rather than reported as permitted.
    let cases: [(&str, Vec<Param>, &str); 3] = [
        (
            "a forged token",
            vec![param("DryRun", "true"), param("NextToken", "not-a-token")],
            "InvalidPaginationToken",
        ),
        (
            "an undocumented filter",
            {
                let mut params = vec![param("DryRun", "true")];
                params.extend(filters(&[("exclusion-id", &["anything"])]));
                params
            },
            "InvalidParameterValue",
        ),
        (
            "too many ids",
            {
                let mut params = vec![param("DryRun", "true")];
                params.extend(
                    (1..=ec2::state::MAX_RESOURCE_IDS + 1).map(|index| {
                        param(
                            &format!("ExclusionId.{index}"),
                            "vpcbpa-exclude-00000000000000001",
                        )
                    }),
                );
                params
            },
            "InvalidParameterValue",
        ),
    ];
    for (case, params, expected_code) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
}
