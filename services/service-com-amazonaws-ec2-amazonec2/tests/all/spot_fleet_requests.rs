// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Spot Fleet request slice: `RequestSpotFleet` and
//! `DescribeSpotFleetRequests`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `RequestSpotFleet` MINTS ONE FLEET FROM THE CONFIGURATION and
//!    `DescribeSpotFleetRequests` REPORTS THAT STORE: every member the call sets round-trips
//!    through the describe, and the members the service owns (the fleet id, its state, its
//!    activity status, its create time, the capacity it has fulfilled and the documented
//!    defaults) are DERIVED rather than taken;
//! 2. the fleet PLACES the Spot Instance requests its Spot capacity takes — one per instance,
//!    the count being the pool's share of the target divided by what one of its instances
//!    provides, rounded up — and `diversified` spreads that target across the fleet's pools
//!    while every other strategy places it in one;
//! 3. the CLOCK, not a caller, moves the fleet `submitted` -> `active` and fulfils its
//!    capacity: the capacity reported fulfilled is the weighted capacity of the requests that
//!    have actually launched an instance, so it is 0 while they are still `open`;
//! 4. a launch described by a launch template is resolved against the template the account
//!    holds, with each override one pool of the fleet;
//! 5. a request that cannot be served is refused with EC2's own error code and stores
//!    nothing — including On-Demand target capacity, which an emulator that places capacity
//!    through Spot Instance requests cannot launch — `DryRun` performs nothing, and one
//!    `ClientToken` creates one fleet;
//! 6. `SpotFleetRequestIds` narrows the report and `MaxResults` pages it, in
//!    `SpotFleetRequestId` order.
//!
//! Each table row is a distinct input along what the rule generalises over — which fleet
//! type, which allocation strategy, which interruption behaviour, how much target capacity,
//! how many pools, what one instance of a pool provides, how many tags, which refusal, which
//! page size — so no row is the specification.

use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, FLEET_TYPE_MAINTAIN, FLEET_TYPE_REQUEST,
    INSTANCE_INTERRUPTION_BEHAVIOR_TERMINATE, MAX_PAGE_SIZE, RESOURCE_TYPE_SPOT_FLEET_REQUEST,
    SPOT_FLEET_ACTIVITY_FULFILLED, SPOT_FLEET_ACTIVITY_PENDING_FULFILLMENT,
    SPOT_FLEET_ALLOCATION_STRATEGY_DIVERSIFIED, SPOT_FLEET_ALLOCATION_STRATEGY_LOWEST_PRICE,
    SPOT_FLEET_REQUEST_ID_PREFIX, SPOT_FLEET_STATE_ACTIVE, SPOT_FLEET_STATE_SUBMITTED,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, indexed};
use super::wire::{child, child_text, document};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The ARN of the IAM role a Spot Fleet request grants the fleet. Its account is NOT this
/// emulator's: the role is the caller's, which is what the handler's check allows.
const FLEET_ROLE: &str = "arn:aws:iam::111122223333:role/aws-ec2-spot-fleet-tagging-role";

/// A well-formed Spot Fleet request id no fleet of the account is stored under: the mint
/// counts from one, so a high counter value names nothing.
const UNKNOWN_FLEET_ID: &str = "sfr-0000ffff-0000-0000-0000-00000000ffff";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `SpotFleetRequestConfig.<member>` parameter.
fn c(member: &str, value: impl Into<String>) -> (String, String) {
    p(&format!("SpotFleetRequestConfig.{member}"), value)
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// An AMI of the published catalog a fresh account can already see, by position in that
/// catalog's id order.
///
/// Read out of the service's OWN state rather than written here: a launch specification must
/// name an image the account can resolve, so the test cannot invent an id.
fn catalog_image(index: usize) -> String {
    Ec2State::default()
        .images
        .keys()
        .nth(index)
        .cloned()
        .unwrap_or_else(|| panic!("a fresh account must publish at least {} images", index + 1))
}

/// The AMI the rows that do not care which image they launch from use.
fn image_id() -> String {
    catalog_image(0)
}

/// A subnet of the account's default VPC, read out of the service's own state for the same
/// reason as [`catalog_image`].
fn default_subnet_id() -> String {
    Ec2State::default()
        .subnets
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must hold the default VPC's subnets")
}

/// The id of the account's default security group.
fn default_group_id() -> String {
    Ec2State::default()
        .security_groups
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must hold the default VPC's default security group")
}

async fn request_fleet(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "RequestSpotFleet", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeSpotFleetRequests", params).await
}

async fn describe_spot_requests(router: &MultiServiceRouter) -> String {
    let (status, body) = call(router, REGION, "DescribeSpotInstanceRequests", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeSpotInstanceRequests must succeed, got {status} with body {body}"
    );
    body
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// The minimum a `RequestSpotFleet` call needs: a fleet role, a target capacity and one
/// launch specification naming an AMI.
fn one_fleet() -> Params {
    vec![
        c("IamFleetRole", FLEET_ROLE),
        c("TargetCapacity", "1"),
        c("LaunchSpecifications.1.ImageId", image_id()),
    ]
}

/// Whether `id` is a Spot Fleet request id AWS could have issued: `sfr-` and a UUID, five
/// hyphen-separated groups of 8, 4, 4, 4 and 12 lowercase hexadecimal digits.
fn is_spot_fleet_request_id(id: &str) -> bool {
    let Some(uuid) = id.strip_prefix(SPOT_FLEET_REQUEST_ID_PREFIX) else {
        return false;
    };
    let groups: Vec<&str> = uuid.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, length)| {
                group.len() == length
                    && group
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
}

/// The `spotFleetRequestId` a `RequestSpotFleet` result carries.
fn created_id(case: &str, body: &str) -> String {
    element(body, "spotFleetRequestId")
        .unwrap_or_else(|| panic!("case {case:?}: no spotFleetRequestId in {body}"))
}

/// One Spot Fleet request as a `DescribeSpotFleetRequests` response reports it: the members
/// of the fleet itself, and its configuration as a node to read members off.
struct Reported<'a, 'i> {
    id: String,
    state: String,
    activity_status: String,
    create_time: String,
    config: roxmltree::Node<'a, 'i>,
    tags: Vec<(String, String)>,
}

/// The `<item>` children of `node`'s `set_name` child.
fn set_items<'a, 'i>(
    node: roxmltree::Node<'a, 'i>,
    set_name: &str,
) -> Vec<roxmltree::Node<'a, 'i>> {
    child(node, set_name)
        .map(|set| {
            set.children()
                .filter(|inner| inner.is_element() && inner.tag_name().name() == "item")
                .collect()
        })
        .unwrap_or_default()
}

/// Every Spot Fleet request a describe response carries, in document order.
fn reported<'a, 'i>(case: &str, parsed: &'a roxmltree::Document<'i>) -> Vec<Reported<'a, 'i>> {
    set_items(parsed.root_element(), "spotFleetRequestConfigSet")
        .into_iter()
        .map(|item| {
            let config = child(item, "spotFleetRequestConfig").unwrap_or_else(|| {
                panic!("case {case:?}: a reported fleet carries no spotFleetRequestConfig")
            });
            Reported {
                id: child_text(item, "spotFleetRequestId").unwrap_or_default(),
                state: child_text(item, "spotFleetRequestState").unwrap_or_default(),
                activity_status: child_text(item, "activityStatus").unwrap_or_default(),
                create_time: child_text(item, "createTime").unwrap_or_default(),
                config,
                tags: set_items(item, "tagSet")
                    .into_iter()
                    .map(|tag| {
                        (
                            child_text(tag, "key").unwrap_or_default(),
                            child_text(tag, "value").unwrap_or_default(),
                        )
                    })
                    .collect(),
            }
        })
        .collect()
}

/// The ids a describe reports, in the order it reports them.
async fn described_ids(case: &str, router: &MultiServiceRouter, params: &[(String, String)]) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSpotFleetRequests, got {status}: {body}"
    );
    let parsed = document(case, &body);
    reported(case, &parsed)
        .into_iter()
        .map(|fleet| fleet.id)
        .collect()
}

/// How many Spot Fleet requests the store holds.
fn stored_fleets(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().spot_fleet_requests.len()
}

/// How many Spot Instance requests the store holds.
fn stored_spot_requests(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .spot_instance_requests
        .len()
}

/// A numeric element of a configuration, as an `f64`.
fn number(case: &str, node: roxmltree::Node<'_, '_>, name: &str) -> f64 {
    let text = child_text(node, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in a reported configuration"));
    text.parse()
        .unwrap_or_else(|error| panic!("case {case:?}: <{name}> must be a number: {error}"))
}

/// One row of the round-trip table: a configuration that differs from the others along what
/// rules 1 to 3 generalise over.
struct FleetCase {
    case: &'static str,
    /// `SpotFleetRequestConfig.Type`, or `None` to leave it to the documented default.
    fleet_type: Option<&'static str>,
    target_capacity: i64,
    /// `SpotFleetRequestConfig.AllocationStrategy`, or `None` for the default.
    allocation_strategy: Option<&'static str>,
    /// `SpotFleetRequestConfig.InstanceInterruptionBehavior`, or `None` for the default.
    interruption_behavior: Option<&'static str>,
    /// One launch specification per instance type, in the order the request gives them.
    instance_types: &'static [&'static str],
    /// `WeightedCapacity` of each pool, in the same order; `None` leaves it unset.
    weights: &'static [Option<f64>],
    tags: &'static [(&'static str, &'static str)],
    /// The fleet's own maximum price, if it names one.
    spot_price: Option<&'static str>,
    expected_type: &'static str,
    expected_allocation: &'static str,
    expected_interruption: &'static str,
    /// How many Spot Instances the fleet places in each pool, written out here: the pool's
    /// share of the target capacity divided by what one of its instances provides, rounded
    /// UP, the share being the whole target for every strategy but `diversified`, which
    /// splits it evenly across the pools.
    expected_counts: &'static [usize],
    /// The capacity those placements fulfil once the clock has launched their instances.
    expected_fulfilled: f64,
}

impl FleetCase {
    fn params(&self) -> Params {
        let mut params = vec![
            c("IamFleetRole", FLEET_ROLE),
            c("TargetCapacity", self.target_capacity.to_string()),
        ];
        if let Some(fleet_type) = self.fleet_type {
            params.push(c("Type", fleet_type));
        }
        if let Some(strategy) = self.allocation_strategy {
            params.push(c("AllocationStrategy", strategy));
        }
        if let Some(behavior) = self.interruption_behavior {
            params.push(c("InstanceInterruptionBehavior", behavior));
        }
        if let Some(price) = self.spot_price {
            params.push(c("SpotPrice", price));
        }
        for (index, instance_type) in self.instance_types.iter().enumerate() {
            let position = index + 1;
            params.push(c(
                &format!("LaunchSpecifications.{position}.ImageId"),
                image_id(),
            ));
            params.push(c(
                &format!("LaunchSpecifications.{position}.InstanceType"),
                *instance_type,
            ));
            if let Some(Some(weight)) = self.weights.get(index) {
                params.push(c(
                    &format!("LaunchSpecifications.{position}.WeightedCapacity"),
                    weight.to_string(),
                ));
            }
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            let position = index + 1;
            params.push(c("TagSpecification.1.ResourceType", RESOURCE_TYPE_SPOT_FLEET_REQUEST));
            params.push(c(&format!("TagSpecification.1.Tag.{position}.Key"), *key));
            params.push(c(&format!("TagSpecification.1.Tag.{position}.Value"), *value));
        }
        params
    }
}

/// The rows of the round-trip table. No row repeats another's combination of fleet type,
/// allocation strategy, interruption behaviour, target capacity, pool count, weight or tags.
const FLEET_CASES: &[FleetCase] = &[
    FleetCase {
        case: "one pool, every documented default",
        fleet_type: None,
        target_capacity: 2,
        allocation_strategy: None,
        interruption_behavior: None,
        instance_types: &["t3.micro"],
        weights: &[None],
        tags: &[],
        spot_price: None,
        expected_type: FLEET_TYPE_MAINTAIN,
        expected_allocation: SPOT_FLEET_ALLOCATION_STRATEGY_LOWEST_PRICE,
        expected_interruption: INSTANCE_INTERRUPTION_BEHAVIOR_TERMINATE,
        // Two units of target capacity, one unit per instance.
        expected_counts: &[2],
        expected_fulfilled: 2.0,
    },
    FleetCase {
        case: "a weighted pool, where one instance covers more than one unit",
        fleet_type: Some(FLEET_TYPE_REQUEST),
        target_capacity: 6,
        allocation_strategy: Some(SPOT_FLEET_ALLOCATION_STRATEGY_LOWEST_PRICE),
        interruption_behavior: Some("stop"),
        instance_types: &["m5.large"],
        weights: &[Some(4.0)],
        tags: &[("suite", "spot-fleet")],
        spot_price: Some("0.50"),
        expected_type: FLEET_TYPE_REQUEST,
        expected_allocation: SPOT_FLEET_ALLOCATION_STRATEGY_LOWEST_PRICE,
        expected_interruption: "stop",
        // Six units at four units per instance: two instances, which is eight units.
        expected_counts: &[2],
        expected_fulfilled: 8.0,
    },
    FleetCase {
        case: "two pools, the lowest-priced strategy placing the target in one",
        fleet_type: Some(FLEET_TYPE_MAINTAIN),
        target_capacity: 3,
        allocation_strategy: None,
        interruption_behavior: Some("hibernate"),
        instance_types: &["t3.small", "c5.large"],
        weights: &[None, None],
        tags: &[("suite", "spot-fleet"), ("owner", "tests")],
        spot_price: None,
        expected_type: FLEET_TYPE_MAINTAIN,
        expected_allocation: SPOT_FLEET_ALLOCATION_STRATEGY_LOWEST_PRICE,
        expected_interruption: "hibernate",
        // The lowest-priced strategy places the whole target in the first pool.
        expected_counts: &[3, 0],
        expected_fulfilled: 3.0,
    },
    FleetCase {
        case: "two pools, the diversified strategy spreading the target over both",
        fleet_type: Some(FLEET_TYPE_MAINTAIN),
        target_capacity: 4,
        allocation_strategy: Some(SPOT_FLEET_ALLOCATION_STRATEGY_DIVERSIFIED),
        interruption_behavior: None,
        instance_types: &["t3.medium", "m5.xlarge"],
        weights: &[Some(1.0), Some(2.0)],
        tags: &[],
        spot_price: None,
        expected_type: FLEET_TYPE_MAINTAIN,
        expected_allocation: SPOT_FLEET_ALLOCATION_STRATEGY_DIVERSIFIED,
        expected_interruption: INSTANCE_INTERRUPTION_BEHAVIOR_TERMINATE,
        // Two units per pool: two instances of one unit, one instance of two.
        expected_counts: &[2, 1],
        expected_fulfilled: 4.0,
    },
    FleetCase {
        case: "a maintain fleet of no capacity at all, which may add it later",
        fleet_type: Some(FLEET_TYPE_MAINTAIN),
        target_capacity: 0,
        allocation_strategy: None,
        interruption_behavior: None,
        instance_types: &["t3.nano"],
        weights: &[None],
        tags: &[],
        spot_price: None,
        expected_type: FLEET_TYPE_MAINTAIN,
        expected_allocation: SPOT_FLEET_ALLOCATION_STRATEGY_LOWEST_PRICE,
        expected_interruption: INSTANCE_INTERRUPTION_BEHAVIOR_TERMINATE,
        // No capacity asked for, so nothing is placed.
        expected_counts: &[0],
        expected_fulfilled: 0.0,
    },
];

#[tokio::test]
async fn a_requested_fleet_round_trips_through_the_describe() {
    for row in FLEET_CASES {
        let case = row.case;
        let (router, engine) = emulator();
        let (status, body) = request_fleet(&router, &row.params()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from RequestSpotFleet, got {status}: {body}"
        );
        let fleet_id = created_id(case, &body);
        assert!(
            is_spot_fleet_request_id(&fleet_id),
            "case {case:?}: expected an sfr- id carrying a UUID, got {fleet_id:?}"
        );
        assert_eq!(
            stored_fleets(&engine),
            1,
            "case {case:?}: one call stores one fleet, store holds {}",
            stored_fleets(&engine)
        );

        let expected_counts = row.expected_counts;
        let placed: usize = expected_counts.iter().sum();
        assert_eq!(
            stored_spot_requests(&engine),
            placed,
            "case {case:?}: expected {placed} Spot Instance requests placed, got {}",
            stored_spot_requests(&engine)
        );

        // Before the transition: the fleet is `submitted`, nothing it placed has launched, so
        // no capacity is fulfilled.
        let (status, body) = describe(&router, &indexed("SpotFleetRequestId", &[&fleet_id])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeSpotFleetRequests, got {status}: {body}"
        );
        let parsed = document(case, &body);
        let mut fleets = reported(case, &parsed);
        assert_eq!(
            fleets.len(),
            1,
            "case {case:?}: a describe narrowed to one id reports one fleet, got {}",
            fleets.len()
        );
        let fleet = fleets.remove(0);
        assert_eq!(
            fleet.id, fleet_id,
            "case {case:?}: the describe must report the fleet the request minted"
        );
        assert_eq!(
            fleet.state, SPOT_FLEET_STATE_SUBMITTED,
            "case {case:?}: a fleet AWS has just accepted is {SPOT_FLEET_STATE_SUBMITTED}, got {:?}",
            fleet.state
        );
        assert!(
            !fleet.create_time.is_empty(),
            "case {case:?}: a fleet reports the time it was created, got {:?}",
            fleet.create_time
        );
        assert_eq!(
            number(case, fleet.config, "fulfilledCapacity"),
            0.0,
            "case {case:?}: nothing has launched yet, so no capacity is fulfilled"
        );
        let expected_activity_before = if row.target_capacity == 0 {
            SPOT_FLEET_ACTIVITY_FULFILLED
        } else {
            SPOT_FLEET_ACTIVITY_PENDING_FULFILLMENT
        };
        assert_eq!(
            fleet.activity_status, expected_activity_before,
            "case {case:?}: expected the activity status {expected_activity_before:?}, got {:?}",
            fleet.activity_status
        );

        // What the request SET round-trips, and the defaults it left unset are reported.
        assert_eq!(
            child_text(fleet.config, "iamFleetRole").as_deref(),
            Some(FLEET_ROLE),
            "case {case:?}: the fleet role round-trips"
        );
        assert_eq!(
            number(case, fleet.config, "targetCapacity"),
            row.target_capacity as f64,
            "case {case:?}: the target capacity round-trips"
        );
        assert_eq!(
            child_text(fleet.config, "type").as_deref(),
            Some(row.expected_type),
            "case {case:?}: expected the fleet type {:?}",
            row.expected_type
        );
        assert_eq!(
            child_text(fleet.config, "allocationStrategy").as_deref(),
            Some(row.expected_allocation),
            "case {case:?}: expected the allocation strategy {:?}",
            row.expected_allocation
        );
        assert_eq!(
            child_text(fleet.config, "instanceInterruptionBehavior").as_deref(),
            Some(row.expected_interruption),
            "case {case:?}: expected the interruption behaviour {:?}",
            row.expected_interruption
        );
        assert_eq!(
            child_text(fleet.config, "spotPrice").as_deref(),
            row.spot_price,
            "case {case:?}: the fleet's maximum price round-trips"
        );
        let reported_types: Vec<String> = set_items(fleet.config, "launchSpecifications")
            .into_iter()
            .map(|launch| child_text(launch, "instanceType").unwrap_or_default())
            .collect();
        assert_eq!(
            reported_types,
            row.instance_types
                .iter()
                .map(|instance_type| (*instance_type).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: every launch specification is reported, in the order it was given"
        );
        let expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            fleet.tags, expected_tags,
            "case {case:?}: the fleet's tags round-trip"
        );

        // After the transition: the clock has moved the fleet `active` and launched the
        // instances its requests asked for, so the capacity they provide is fulfilled.
        past_the_transition().await;
        let (status, body) = describe(&router, &indexed("SpotFleetRequestId", &[&fleet_id])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the second describe, got {status}: {body}"
        );
        let parsed = document(case, &body);
        let fleet = reported(case, &parsed).remove(0);
        assert_eq!(
            fleet.state, SPOT_FLEET_STATE_ACTIVE,
            "case {case:?}: the clock moves a fleet to {SPOT_FLEET_STATE_ACTIVE}, got {:?}",
            fleet.state
        );
        let expected_fulfilled = row.expected_fulfilled;
        assert_eq!(
            number(case, fleet.config, "fulfilledCapacity"),
            expected_fulfilled,
            "case {case:?}: expected {expected_fulfilled} units fulfilled"
        );
        assert_eq!(
            fleet.activity_status, SPOT_FLEET_ACTIVITY_FULFILLED,
            "case {case:?}: a fleet at or above its target capacity is \
             {SPOT_FLEET_ACTIVITY_FULFILLED}, got {:?}",
            fleet.activity_status
        );

        // The instances the fleet placed are instances of this account's own store, reported
        // by the Spot Instance request each was launched for.
        let spot = describe_spot_requests(&router).await;
        let launched = elements(&spot, "instanceId");
        assert_eq!(
            launched.len(),
            placed,
            "case {case:?}: expected {placed} launched instances, got {launched:?}"
        );
        // Each pool launched its own share, with its own instance type.
        for (index, instance_type) in row.instance_types.iter().enumerate() {
            let expected = expected_counts.get(index).copied().unwrap_or_default();
            let launched_in_pool = elements(&spot, "instanceType")
                .into_iter()
                .filter(|reported| reported == instance_type)
                .count();
            assert_eq!(
                launched_in_pool, expected,
                "case {case:?}: expected {expected} request(s) in the {instance_type} pool, \
                 got {launched_in_pool}"
            );
        }
    }
}

#[tokio::test]
async fn a_fleet_described_by_a_launch_template_places_the_templates_launch() {
    let case = "a launch template with two overrides";
    let (router, engine) = emulator();
    let (status, body) = call(
        &router,
        REGION,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", "spot-fleet-suite"),
            p("LaunchTemplateData.ImageId", image_id()),
            p("LaunchTemplateData.InstanceType", "t3.micro"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLaunchTemplate must succeed, got {status}: {body}"
    );
    let template_id = element(&body, "launchTemplateId")
        .unwrap_or_else(|| panic!("case {case:?}: no launchTemplateId in {body}"));

    let params = vec![
        c("IamFleetRole", FLEET_ROLE),
        c("TargetCapacity", "2"),
        c("AllocationStrategy", SPOT_FLEET_ALLOCATION_STRATEGY_DIVERSIFIED),
        c(
            "LaunchTemplateConfigs.1.LaunchTemplateSpecification.LaunchTemplateId",
            template_id.clone(),
        ),
        c(
            "LaunchTemplateConfigs.1.LaunchTemplateSpecification.Version",
            "1",
        ),
        c("LaunchTemplateConfigs.1.Overrides.1.InstanceType", "t3.small"),
        c("LaunchTemplateConfigs.1.Overrides.2.InstanceType", "c5.large"),
    ];
    let (status, body) = request_fleet(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from RequestSpotFleet, got {status}: {body}"
    );
    let fleet_id = created_id(case, &body);
    // Two pools, a target of 2 and the diversified strategy: one instance per pool.
    assert_eq!(
        stored_spot_requests(&engine),
        2,
        "case {case:?}: each override is one pool, each placing its share of the target"
    );

    let (status, body) = describe(&router, &indexed("SpotFleetRequestId", &[&fleet_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSpotFleetRequests, got {status}: {body}"
    );
    let parsed = document(case, &body);
    let fleet = reported(case, &parsed).remove(0);
    let configs = set_items(fleet.config, "launchTemplateConfigs");
    assert_eq!(
        configs.len(),
        1,
        "case {case:?}: the launch template configuration round-trips"
    );
    assert_eq!(
        child(configs[0], "launchTemplateSpecification")
            .and_then(|spec| child_text(spec, "launchTemplateId")),
        Some(template_id.clone()),
        "case {case:?}: the template the fleet launches from round-trips"
    );

    past_the_transition().await;
    let spot = describe_spot_requests(&router).await;
    let mut launched_types = elements(&spot, "instanceType");
    launched_types.sort();
    assert_eq!(
        launched_types,
        vec!["c5.large".to_string(), "t3.small".to_string()],
        "case {case:?}: each pool launches its override's instance type"
    );
    // The AMI comes from the template, not from the override.
    for reported_image in elements(&spot, "imageId") {
        assert_eq!(
            reported_image,
            image_id(),
            "case {case:?}: every pool launches the template's AMI"
        );
    }

    // On-Demand capacity is launched by the fleet directly rather than placed through a Spot
    // Instance request, which this emulator does not model — so a fleet that asks for any is
    // refused rather than left reporting capacity nothing would ever launch.
    let case = "a launch template fleet asking for On-Demand capacity";
    let mut on_demand = params.clone();
    on_demand.push(c("OnDemandTargetCapacity", "1"));
    let (status, body) = request_fleet(&router, &on_demand).await;
    assert_eq!(
        status, 501,
        "case {case:?}: expected HTTP 501, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "NotImplemented",
        "case {case:?}: expected NotImplemented, got the body {body}"
    );
}

/// One row of the refusal table: a request that cannot be served, and the error code EC2
/// answers for it.
struct RefusalCase {
    case: &'static str,
    params: fn() -> Params,
    expected_code: &'static str,
}

impl RefusalCase {
    /// The HTTP status EC2 answers this refusal with: 400 for a bad request, and 412 for a
    /// dry run, which reports "a request that would have succeeded" rather than a bad one.
    fn expected_status(&self) -> u16 {
        if self.expected_code == "DryRunOperation" {
            412
        } else {
            400
        }
    }
}

/// The rows of the refusal table: one per rule a configuration has to satisfy.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            case: "an empty fleet role",
            params: || {
                let mut params = one_fleet();
                params[0] = c("IamFleetRole", "");
                params
            },
            expected_code: "MissingParameter",
        },
        RefusalCase {
            case: "a fleet role that is not an ARN",
            params: || {
                let mut params = one_fleet();
                params[0] = c("IamFleetRole", "aws-ec2-spot-fleet-tagging-role");
                params
            },
            expected_code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a fleet role ARN naming another service",
            params: || {
                let mut params = one_fleet();
                params[0] = c(
                    "IamFleetRole",
                    "arn:aws:ec2:us-west-2:111122223333:role/spot-fleet",
                );
                params
            },
            expected_code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "the fleet type that is listed but not used by Spot Fleet",
            params: || {
                let mut params = one_fleet();
                params.push(c("Type", "instant"));
                params
            },
            expected_code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "no capacity at all for a request-type fleet",
            params: || {
                let mut params = one_fleet();
                params[1] = c("TargetCapacity", "0");
                params.push(c("Type", FLEET_TYPE_REQUEST));
                params
            },
            expected_code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a negative target capacity",
            params: || {
                let mut params = one_fleet();
                params[1] = c("TargetCapacity", "-1");
                params
            },
            expected_code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "neither launch specifications nor launch template configurations",
            params: || {
                vec![c("IamFleetRole", FLEET_ROLE), c("TargetCapacity", "1")]
            },
            expected_code: "MissingParameter",
        },
        RefusalCase {
            case: "both ways of describing the launch at once",
            params: || {
                let mut params = one_fleet();
                params.push(c(
                    "LaunchTemplateConfigs.1.LaunchTemplateSpecification.LaunchTemplateName",
                    "whichever",
                ));
                params
            },
            expected_code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "On-Demand capacity above the fleet's total",
            params: || {
                let mut params = one_fleet();
                params.push(c("OnDemandTargetCapacity", "2"));
                params
            },
            expected_code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "On-Demand capacity described by launch specifications",
            params: || {
                let mut params = one_fleet();
                params[1] = c("TargetCapacity", "2");
                params.push(c("OnDemandTargetCapacity", "1"));
                params
            },
            expected_code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "a launch specification naming no AMI",
            params: || {
                vec![
                    c("IamFleetRole", FLEET_ROLE),
                    c("TargetCapacity", "1"),
                    c("LaunchSpecifications.1.InstanceType", "t3.micro"),
                ]
            },
            expected_code: "MissingParameter",
        },
        RefusalCase {
            case: "an AMI the account cannot see",
            params: || {
                let mut params = one_fleet();
                params[2] = c("LaunchSpecifications.1.ImageId", "ami-0eeeeeeeeeeeeeeee");
                params
            },
            expected_code: "InvalidAMIID.NotFound",
        },
        RefusalCase {
            case: "a subnet the account does not hold",
            params: || {
                let mut params = one_fleet();
                params.push(c(
                    "LaunchSpecifications.1.SubnetId",
                    "subnet-0eeeeeeeeeeeeeeee",
                ));
                params
            },
            expected_code: "InvalidSubnetID.NotFound",
        },
        RefusalCase {
            case: "a security group the account does not hold",
            params: || {
                let mut params = one_fleet();
                params.push(c(
                    "LaunchSpecifications.1.GroupSet.1.GroupId",
                    "sg-0eeeeeeeeeeeeeeee",
                ));
                params
            },
            expected_code: "InvalidGroup.NotFound",
        },
        RefusalCase {
            case: "an Availability Zone another Region has",
            params: || {
                let mut params = one_fleet();
                params.push(c(
                    "LaunchSpecifications.1.Placement.AvailabilityZone",
                    "eu-central-1a",
                ));
                params
            },
            expected_code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a maximum price that is not a price",
            params: || {
                let mut params = one_fleet();
                params.push(c("SpotPrice", "as much as it takes"));
                params
            },
            expected_code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a pool whose instances provide nothing",
            params: || {
                let mut params = one_fleet();
                params.push(c("LaunchSpecifications.1.WeightedCapacity", "0"));
                params
            },
            expected_code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a validity window that has already closed",
            params: || {
                let mut params = one_fleet();
                params.push(c("ValidUntil", "2020-01-01T00:00:00Z"));
                params
            },
            expected_code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a launch template the account does not hold",
            params: || {
                vec![
                    c("IamFleetRole", FLEET_ROLE),
                    c("TargetCapacity", "1"),
                    c(
                        "LaunchTemplateConfigs.1.LaunchTemplateSpecification.LaunchTemplateName",
                        "no-such-template",
                    ),
                ]
            },
            expected_code: "InvalidLaunchTemplateName.NotFoundException",
        },
        RefusalCase {
            case: "a tag specification naming another resource type",
            params: || {
                let mut params = one_fleet();
                params.push(c("TagSpecification.1.ResourceType", "instance"));
                params.push(c("TagSpecification.1.Tag.1.Key", "suite"));
                params.push(c("TagSpecification.1.Tag.1.Value", "spot-fleet"));
                params
            },
            expected_code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a dry run that would otherwise have succeeded",
            params: || {
                let mut params = one_fleet();
                params.push(p("DryRun", "true"));
                params
            },
            expected_code: "DryRunOperation",
        },    ]
}

#[tokio::test]
async fn a_request_that_cannot_be_served_is_refused_and_stores_nothing() {
    for row in refusal_cases() {
        let case = row.case;
        let (router, engine) = emulator();
        let (status, body) = request_fleet(&router, &(row.params)()).await;
        assert_eq!(
            status,
            row.expected_status(),
            "case {case:?}: expected HTTP {} from RequestSpotFleet, got {status}: {body}",
            row.expected_status()
        );
        assert_eq!(
            error_code(&body),
            row.expected_code,
            "case {case:?}: expected the error code {:?}, got the body {body}",
            row.expected_code
        );
        assert_eq!(
            stored_fleets(&engine),
            0,
            "case {case:?}: a refused request stores no fleet"
        );
        assert_eq!(
            stored_spot_requests(&engine),
            0,
            "case {case:?}: a refused request places no Spot Instance request"
        );
    }
}

#[tokio::test]
async fn one_client_token_creates_one_fleet() {
    let case = "a replayed client token";
    let (router, engine) = emulator();
    let mut params = one_fleet();
    params.push(c("ClientToken", "one-fleet-only"));
    let (status, body) = request_fleet(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the first call must succeed, got {status}: {body}"
    );
    let first = created_id(case, &body);
    let (status, body) = request_fleet(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the replay must succeed, got {status}: {body}"
    );
    assert_eq!(
        created_id(case, &body),
        first,
        "case {case:?}: one token names one fleet"
    );
    assert_eq!(
        stored_fleets(&engine),
        1,
        "case {case:?}: a replay creates no second fleet"
    );
    assert_eq!(
        stored_spot_requests(&engine),
        1,
        "case {case:?}: a replay places no further capacity"
    );

    // A different token is a different fleet.
    let mut other = one_fleet();
    other.push(c("ClientToken", "another-fleet"));
    let (status, body) = request_fleet(&router, &other).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a fresh token must succeed, got {status}: {body}"
    );
    assert_ne!(
        created_id(case, &body),
        first,
        "case {case:?}: a different token names a different fleet"
    );
    assert_eq!(
        stored_fleets(&engine),
        2,
        "case {case:?}: two tokens name two fleets"
    );
}

#[tokio::test]
async fn the_report_is_narrowed_by_id_and_paged_by_max_results() {
    let case = "three fleets";
    let (router, _engine) = emulator();
    let mut ids: Vec<String> = Vec::new();
    for index in 0..3 {
        let mut params = one_fleet();
        params.push(c("ClientToken", format!("fleet-{index}")));
        let (status, body) = request_fleet(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: RequestSpotFleet must succeed, got {status}: {body}"
        );
        ids.push(created_id(case, &body));
    }
    let mut in_id_order = ids.clone();
    in_id_order.sort();

    assert_eq!(
        described_ids(case, &router, &[]).await,
        in_id_order,
        "case {case:?}: an unnarrowed describe reports every fleet in SpotFleetRequestId order"
    );
    assert_eq!(
        described_ids(case, &router, &indexed("SpotFleetRequestId", &[&in_id_order[1]])).await,
        vec![in_id_order[1].clone()],
        "case {case:?}: SpotFleetRequestIds narrows the report to the fleets it names"
    );

    // One fleet per page, following the token to the next.
    let (status, body) = describe(&router, &[p("MaxResults", "1")]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a paged describe must succeed, got {status}: {body}"
    );
    let parsed = document(case, &body);
    let page = reported(case, &parsed);
    assert_eq!(
        page.len(),
        1,
        "case {case:?}: MaxResults=1 reports one fleet, got {}",
        page.len()
    );
    assert_eq!(
        page[0].id, in_id_order[0],
        "case {case:?}: the first page starts at the first id"
    );
    let token = element(&body, "nextToken")
        .unwrap_or_else(|| panic!("case {case:?}: a truncated page carries a nextToken: {body}"));
    assert_eq!(
        described_ids(
            case,
            &router,
            &[p("MaxResults", "1"), p("NextToken", token.clone())]
        )
        .await,
        vec![in_id_order[1].clone()],
        "case {case:?}: the token names the fleet the next page starts at"
    );

    // A page that holds every fleet carries no token.
    let (status, body) = describe(&router, &[p("MaxResults", "3")]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a full page must succeed, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "nextToken"),
        None,
        "case {case:?}: a page with nothing after it carries no token: {body}"
    );

    // The page size's published bounds, and one past each: the model declares no `@range`
    // on this operation's `MaxResults`, so only a page of none is a refusal.
    for (page_size, expected_status) in [
        ("1", 200),
        (MAX_PAGE_SIZE.to_string().as_str(), 200),
        ("0", 400),
        ("1001", 200),
    ] {
        let (status, body) = describe(&router, &[p("MaxResults", page_size)]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: MaxResults={page_size} must answer HTTP {expected_status}, \
             got {status}: {body}"
        );
    }

    // An id of the wrong shape is refused before any lookup; a well-formed id of no fleet is
    // not found.
    for (named_id, expected_code) in [
        ("sfr-not-a-uuid", "InvalidSpotFleetRequestId.Malformed"),
        ("fleet-0000ffff-0000-0000-0000-00000000ffff", "InvalidSpotFleetRequestId.Malformed"),
        ("sfr-00000000000000001", "InvalidSpotFleetRequestId.Malformed"),
        (UNKNOWN_FLEET_ID, "InvalidSpotFleetRequestId.NotFound"),
    ] {
        let (status, body) = describe(&router, &indexed("SpotFleetRequestId", &[named_id])).await;
        assert_eq!(
            status, 400,
            "case {named_id:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {named_id:?}: expected the error code {expected_code:?}, got the body {body}"
        );
    }

    // A dry run of the describe performs nothing and is refused with the dry-run code.
    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case {case:?}: a dry run is refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got the body {body}"
    );
}

#[tokio::test]
async fn a_fleet_launches_into_the_subnet_and_group_its_specification_names() {
    let case = "a launch specification naming resources of the account";
    let (router, _engine) = emulator();
    let subnet_id = default_subnet_id();
    let group_id = default_group_id();
    let mut params = one_fleet();
    params.push(c("LaunchSpecifications.1.SubnetId", subnet_id.clone()));
    params.push(c(
        "LaunchSpecifications.1.GroupSet.1.GroupId",
        group_id.clone(),
    ));
    let (status, body) = request_fleet(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from RequestSpotFleet, got {status}: {body}"
    );
    let fleet_id = created_id(case, &body);

    let (status, body) = describe(&router, &indexed("SpotFleetRequestId", &[&fleet_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSpotFleetRequests, got {status}: {body}"
    );
    let parsed = document(case, &body);
    let fleet = reported(case, &parsed).remove(0);
    let launches = set_items(fleet.config, "launchSpecifications");
    assert_eq!(
        child_text(launches[0], "subnetId").as_deref(),
        Some(subnet_id.as_str()),
        "case {case:?}: the subnet round-trips"
    );

    past_the_transition().await;
    let spot = describe_spot_requests(&router).await;
    assert_eq!(
        elements(&spot, "subnetId"),
        vec![subnet_id.clone()],
        "case {case:?}: the Spot Instance request the fleet placed names that subnet"
    );
    assert_eq!(
        elements(&spot, "groupId"),
        vec![group_id.clone()],
        "case {case:?}: the Spot Instance request the fleet placed names that security group"
    );
}
