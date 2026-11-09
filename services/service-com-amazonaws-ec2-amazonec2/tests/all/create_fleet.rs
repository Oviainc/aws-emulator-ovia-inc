// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateFleet`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. a fleet PLACES the capacity its target asks for as REAL instances: each pool's share
//!    of the target, divided by what one of its instances provides and rounded up, is
//!    launched into the instance store — Spot capacity through Spot Instance requests of
//!    the account, On-Demand capacity launched directly — and the fleet reports exactly the
//!    capacity it placed;
//! 2. the fleet the store holds is the fleet the request described: its type, its state, its
//!    activity status, its target capacity, its launch template configurations, its options
//!    and its tags, with the members the service owns (the id, the create time, the fulfilled
//!    capacity) DERIVED rather than taken, and the instances read back through
//!    `DescribeFleetInstances`, `DescribeInstances` and `DescribeSpotInstanceRequests`;
//! 3. the CLOCK, not a caller, moves an asynchronous fleet `submitted` -> `active`, while an
//!    `instant` fleet is active when the call answers and reports the instances it launched;
//! 4. a request that cannot be served is refused with EC2's own error code and stores
//!    NOTHING, `DryRun` performs nothing, and one `ClientToken` creates one fleet.
//!
//! Each table row is a distinct input along what the rule generalises over — which fleet
//! type, how much capacity of which type, how many pools, what one instance of a pool
//! provides, which allocation strategy, which refusal — so no row is the specification.

use std::collections::BTreeMap;
use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, FLEET_ACTIVITY_STATUS_FULFILLED,
    FLEET_ACTIVITY_STATUS_PENDING_FULFILLMENT, FLEET_STATE_ACTIVE, FLEET_STATE_SUBMITTED,
    FLEET_TYPE_INSTANT, FLEET_TYPE_MAINTAIN, FLEET_TYPE_REQUEST, INSTANCE_LIFECYCLE_ON_DEMAND,
    INSTANCE_LIFECYCLE_SPOT, SPOT_ALLOCATION_STRATEGY_DIVERSIFIED, TRANSITION_DELAY,
    fleet_id_is_well_formed,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code};
use super::wire::document;

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// An AMI of the published catalog a fresh account can already see.
///
/// Read out of the service's OWN state rather than written here: a launch template must
/// name an image the account can resolve, so the test cannot invent an id.
fn image_id() -> String {
    Ec2State::default()
        .images
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must publish at least one image")
}

/// A subnet of the account's default VPC, read out of the service's own state for the same
/// reason as [`image_id`].
fn default_subnet_id() -> String {
    Ec2State::default()
        .subnets
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must hold the default VPC's subnets")
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// The `instanceIds` a `CreateFleet` result reports, in document order: the `<item>` texts
/// of every `instanceIds` set in the body.
///
/// Read with the XML parser rather than by substring, because the ids are the ITEMS of a
/// set element — `<instanceIds><item>i-…</item></instanceIds>` — as ec2Query renders a list.
fn reported_instance_ids(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "instanceIds")
        .flat_map(|set| {
            set.children()
                .filter(|node| node.is_element() && node.tag_name().name() == "item")
                .map(|node| node.text().unwrap_or_default().to_string())
                .collect::<Vec<String>>()
        })
        .collect()
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreateFleet", params).await
}

/// Create one launch template naming `instance_type` and answer its `LaunchTemplateId`.
async fn launch_template(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    instance_type: &str,
) -> String {
    let (status, body) = call(
        router,
        REGION,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", image_id()),
            p("LaunchTemplateData.InstanceType", instance_type),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLaunchTemplate must succeed, got {status}: {body}"
    );
    element(&body, "launchTemplateId")
        .unwrap_or_else(|| panic!("case {case:?}: no launchTemplateId in {body}"))
}

/// The parameters naming one launch template configuration with no overrides.
fn one_template(template_id: &str) -> Params {
    vec![p(
        "LaunchTemplateConfigs.1.LaunchTemplateSpecification.LaunchTemplateId",
        template_id,
    )]
}

/// What the store holds about one fleet: the members a report of it carries, read through the
/// state the handlers share.
///
/// Read from the STATE rather than through `DescribeFleets` because this suite is about the
/// WRITE: what a create stored has to be assertable without depending on another operation's
/// projection of it. What `DescribeFleets` reports off the same record is that operation's own
/// suite ([`super::describe_fleets`]).
struct Stored {
    fleet_type: String,
    state: String,
    activity_status: String,
    fulfilled_capacity: f64,
    fulfilled_on_demand_capacity: f64,
    total_target_capacity: i32,
    tags: BTreeMap<String, String>,
    instance_ids: Vec<String>,
    spot_request_ids: Vec<String>,
    pool_lifecycles: Vec<String>,
}

/// The fleet `fleet_id` as the store holds it.
fn stored(engine: &engine::InMemoryEngine, case: &str, fleet_id: &str) -> Stored {
    let guard = engine.state::<Ec2State>();
    let store = guard.lock();
    let record = store
        .fleets
        .get(fleet_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store must hold the fleet {fleet_id}"));
    Stored {
        fleet_type: record.fleet.r#type.clone().unwrap_or_default(),
        state: record.fleet.fleet_state.clone().unwrap_or_default(),
        activity_status: record.fleet.activity_status.clone().unwrap_or_default(),
        fulfilled_capacity: record.fleet.fulfilled_capacity.unwrap_or_default(),
        fulfilled_on_demand_capacity: record
            .fleet
            .fulfilled_on_demand_capacity
            .unwrap_or_default(),
        total_target_capacity: record
            .fleet
            .target_capacity_specification
            .as_ref()
            .and_then(|target| target.total_target_capacity)
            .unwrap_or_default(),
        tags: record
            .fleet
            .tags
            .iter()
            .flatten()
            .map(|tag| {
                (
                    tag.key.clone().unwrap_or_default(),
                    tag.value.clone().unwrap_or_default(),
                )
            })
            .collect(),
        instance_ids: record
            .instances
            .iter()
            .map(|instance| instance.instance_id.clone())
            .collect(),
        spot_request_ids: record
            .instances
            .iter()
            .filter_map(|instance| instance.spot_instance_request_id.clone())
            .collect(),
        pool_lifecycles: record
            .pools
            .iter()
            .map(|pool| pool.lifecycle.clone())
            .collect(),
    }
}

/// How many instances, and how many Spot Instance requests, the store holds.
fn store_counts(engine: &engine::InMemoryEngine) -> (usize, usize) {
    let guard = engine.state::<Ec2State>();
    let store = guard.lock();
    (store.instances.len(), store.spot_instance_requests.len())
}

/// How many instances of each INSTANCE TYPE the store holds, which is what says which POOL
/// of a fleet each instance was placed in: a pool is one instance type in one zone, so the
/// distribution of types is the distribution of capacity across the pools.
fn instance_types(engine: &engine::InMemoryEngine) -> BTreeMap<String, usize> {
    let guard = engine.state::<Ec2State>();
    let store = guard.lock();
    let mut counted: BTreeMap<String, usize> = BTreeMap::new();
    for instance in store.instances.values() {
        *counted.entry(instance.instance_type.clone()).or_default() += 1;
    }
    counted
}

/// One row of the placement table: how a fleet describes its capacity, and the instances it
/// must therefore place.
struct Placement {
    case: &'static str,
    fleet_type: &'static str,
    /// Extra parameters beside the launch template configuration and the total capacity.
    extra: fn() -> Params,
    total_target_capacity: i32,
    /// How many instances the fleet places, and how many of them are Spot Instances.
    instances: usize,
    spot_instances: usize,
    /// The capacity the fleet reports fulfilled, and how much of it is On-Demand.
    fulfilled: f64,
    fulfilled_on_demand: f64,
    /// How many instances of each instance type the fleet placed, which is WHICH POOL each
    /// instance was placed in — the fact the allocation strategy and the pool count decide.
    /// Empty for a row whose pools all launch the launch template's own type, where the
    /// counts above already say everything there is to say.
    types: &'static [(&'static str, usize)],
}

#[tokio::test]
async fn a_fleet_places_the_capacity_its_target_asks_for() {
    // The rows differ along what the rule generalises over: the fleet type, which capacity
    // type the target assigns, how many pools the request describes, what one instance of a
    // pool provides and which allocation strategy spreads the capacity.
    let rows = vec![
        Placement {
            case: "a maintain fleet with one pool and a target of two",
            fleet_type: FLEET_TYPE_MAINTAIN,
            extra: Vec::new,
            total_target_capacity: 2,
            instances: 2,
            spot_instances: 2,
            fulfilled: 2.0,
            fulfilled_on_demand: 0.0,
            types: &[("t3.micro", 2)],
        },
        Placement {
            case: "a request fleet spreading four units across two pools",
            fleet_type: FLEET_TYPE_REQUEST,
            extra: || {
                vec![
                    p("SpotOptions.AllocationStrategy", SPOT_ALLOCATION_STRATEGY_DIVERSIFIED),
                    p("LaunchTemplateConfigs.1.Overrides.1.InstanceType", "t3.small"),
                    p("LaunchTemplateConfigs.1.Overrides.2.InstanceType", "c5.large"),
                ]
            },
            total_target_capacity: 4,
            instances: 4,
            spot_instances: 4,
            fulfilled: 4.0,
            fulfilled_on_demand: 0.0,
            // `diversified` draws from EVERY pool, evenly.
            types: &[("c5.large", 2), ("t3.small", 2)],
        },
        Placement {
            case: "a request fleet drawing four units from the first of two pools",
            fleet_type: FLEET_TYPE_REQUEST,
            extra: || {
                vec![
                    p("LaunchTemplateConfigs.1.Overrides.1.InstanceType", "t3.small"),
                    p("LaunchTemplateConfigs.1.Overrides.2.InstanceType", "c5.large"),
                ]
            },
            total_target_capacity: 4,
            instances: 4,
            spot_instances: 4,
            fulfilled: 4.0,
            fulfilled_on_demand: 0.0,
            // The default strategy draws the whole Spot capacity from ONE pool, the first
            // the request gave.
            types: &[("t3.small", 4)],
        },
        Placement {
            case: "a request fleet spreading four units across two of its three pools",
            fleet_type: FLEET_TYPE_REQUEST,
            extra: || {
                vec![
                    p("SpotOptions.InstancePoolsToUseCount", "2"),
                    p("LaunchTemplateConfigs.1.Overrides.1.InstanceType", "t3.small"),
                    p("LaunchTemplateConfigs.1.Overrides.2.InstanceType", "c5.large"),
                    p("LaunchTemplateConfigs.1.Overrides.3.InstanceType", "m5.large"),
                ]
            },
            total_target_capacity: 4,
            instances: 4,
            spot_instances: 4,
            fulfilled: 4.0,
            fulfilled_on_demand: 0.0,
            // "EC2 Fleet … evenly allocates your target Spot capacity across the number of
            // Spot pools that you specify": two of the three pools, two instances each.
            types: &[("c5.large", 2), ("t3.small", 2)],
        },
        Placement {
            case: "a fleet keeping its whole Spot capacity in one instance type",
            fleet_type: FLEET_TYPE_INSTANT,
            extra: || {
                vec![
                    p("SpotOptions.AllocationStrategy", SPOT_ALLOCATION_STRATEGY_DIVERSIFIED),
                    p("SpotOptions.SingleInstanceType", "true"),
                    p("LaunchTemplateConfigs.1.Overrides.1.InstanceType", "t3.small"),
                    p("LaunchTemplateConfigs.1.Overrides.2.InstanceType", "c5.large"),
                ]
            },
            total_target_capacity: 2,
            instances: 2,
            spot_instances: 2,
            fulfilled: 2.0,
            fulfilled_on_demand: 0.0,
            // `SingleInstanceType` keeps the capacity in ONE pool whatever the allocation
            // strategy says.
            types: &[("t3.small", 2)],
        },
        Placement {
            case: "an instant fleet of On-Demand capacity only",
            fleet_type: FLEET_TYPE_INSTANT,
            extra: || vec![p("TargetCapacitySpecification.OnDemandTargetCapacity", "2")],
            total_target_capacity: 2,
            instances: 2,
            spot_instances: 0,
            fulfilled: 2.0,
            fulfilled_on_demand: 2.0,
            types: &[("t3.micro", 2)],
        },
        Placement {
            case: "a maintain fleet of one On-Demand and one Spot unit",
            fleet_type: FLEET_TYPE_MAINTAIN,
            extra: || {
                vec![
                    p("TargetCapacitySpecification.OnDemandTargetCapacity", "1"),
                    p("TargetCapacitySpecification.SpotTargetCapacity", "1"),
                ]
            },
            total_target_capacity: 2,
            instances: 2,
            spot_instances: 1,
            fulfilled: 2.0,
            fulfilled_on_demand: 1.0,
            // One pool per capacity type, both launching the template's own type.
            types: &[("t3.micro", 2)],
        },
        Placement {
            case: "an instant fleet whose pool provides two units per instance",
            fleet_type: FLEET_TYPE_INSTANT,
            extra: || {
                vec![p(
                    "LaunchTemplateConfigs.1.Overrides.1.WeightedCapacity",
                    "2.0",
                )]
            },
            total_target_capacity: 4,
            instances: 2,
            spot_instances: 2,
            fulfilled: 4.0,
            fulfilled_on_demand: 0.0,
            types: &[("t3.micro", 2)],
        },
        Placement {
            case: "an instant fleet whose Spot minimum its capacity cannot reach",
            fleet_type: FLEET_TYPE_INSTANT,
            extra: || {
                vec![
                    p("SpotOptions.SingleInstanceType", "true"),
                    p("SpotOptions.MinTargetCapacity", "4"),
                ]
            },
            // "If this minimum capacity isn't reached, no instances are launched": the
            // fleet's own target is below the minimum it was given, so it launches nothing.
            total_target_capacity: 2,
            instances: 0,
            spot_instances: 0,
            fulfilled: 0.0,
            fulfilled_on_demand: 0.0,
            types: &[],
        },
        Placement {
            case: "a maintain fleet asking for no capacity yet",
            fleet_type: FLEET_TYPE_MAINTAIN,
            extra: Vec::new,
            total_target_capacity: 0,
            instances: 0,
            spot_instances: 0,
            fulfilled: 0.0,
            fulfilled_on_demand: 0.0,
            types: &[],
        },
    ];

    for row in rows {
        let case = row.case;
        let (router, engine) = emulator();
        let template_id = launch_template(&router, case, "create-fleet-suite", "t3.micro").await;
        let mut params = one_template(&template_id);
        params.push(p("Type", row.fleet_type));
        params.push(p(
            "TargetCapacitySpecification.TotalTargetCapacity",
            row.total_target_capacity.to_string(),
        ));
        params.extend((row.extra)());

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateFleet, got {status}: {body}"
        );
        let fleet_id = element(&body, "fleetId")
            .unwrap_or_else(|| panic!("case {case:?}: no fleetId in {body}"));
        assert!(
            fleet_id_is_well_formed(&fleet_id),
            "case {case:?}: expected a fleet id AWS could have issued, got {fleet_id:?}"
        );

        let held = stored(&engine, case, &fleet_id);
        assert_eq!(
            held.fleet_type, row.fleet_type,
            "case {case:?}: expected the stored fleet type {:?}, got {:?}",
            row.fleet_type, held.fleet_type
        );
        assert_eq!(
            held.total_target_capacity, row.total_target_capacity,
            "case {case:?}: expected the stored target capacity {}, got {}",
            row.total_target_capacity, held.total_target_capacity
        );
        assert_eq!(
            held.instance_ids.len(),
            row.instances,
            "case {case:?}: expected the fleet to place {} instance(s), got {:?}",
            row.instances,
            held.instance_ids
        );
        assert_eq!(
            held.spot_request_ids.len(),
            row.spot_instances,
            "case {case:?}: expected {} of them to be placed through a Spot request, got {:?}",
            row.spot_instances,
            held.spot_request_ids
        );
        assert_eq!(
            held.fulfilled_capacity, row.fulfilled,
            "case {case:?}: expected {} units reported fulfilled, got {}",
            row.fulfilled, held.fulfilled_capacity
        );
        assert_eq!(
            held.fulfilled_on_demand_capacity, row.fulfilled_on_demand,
            "case {case:?}: expected {} On-Demand units reported fulfilled, got {}",
            row.fulfilled_on_demand, held.fulfilled_on_demand_capacity
        );
        let expected_activity = if row.fulfilled >= f64::from(row.total_target_capacity) {
            FLEET_ACTIVITY_STATUS_FULFILLED
        } else {
            FLEET_ACTIVITY_STATUS_PENDING_FULFILLMENT
        };
        assert_eq!(
            held.activity_status, expected_activity,
            "case {case:?}: expected the activity status {expected_activity:?}, got {:?}",
            held.activity_status
        );
        for lifecycle in &held.pool_lifecycles {
            assert!(
                lifecycle == INSTANCE_LIFECYCLE_SPOT || lifecycle == INSTANCE_LIFECYCLE_ON_DEMAND,
                "case {case:?}: a pool's lifecycle is a modelled InstanceLifecycle word, \
                 got {lifecycle:?}"
            );
        }

        // Every instance the fleet reports is an instance of the INSTANCE store, and every
        // Spot request it names is a request of the Spot request store: the counts the store
        // holds are exactly the ones the fleet placed.
        let (instances, spot_requests) = store_counts(&engine);
        assert_eq!(
            instances, row.instances,
            "case {case:?}: expected {} instance(s) in the store, got {instances}",
            row.instances
        );
        assert_eq!(
            spot_requests, row.spot_instances,
            "case {case:?}: expected {} Spot Instance request(s) in the store, got \
             {spot_requests}",
            row.spot_instances
        );

        // WHICH pool each instance was placed in: a pool is one instance type in one zone,
        // so the instance types the store holds are the distribution of the fleet's capacity
        // across its pools — what the allocation strategy, the pool count and the
        // single-pool preferences decide.
        let placed_types = instance_types(&engine);
        let expected_types: BTreeMap<String, usize> = row
            .types
            .iter()
            .map(|(instance_type, count)| ((*instance_type).to_string(), *count))
            .collect();
        assert_eq!(
            placed_types, expected_types,
            "case {case:?}: expected the fleet's capacity to land in the pools \
             {expected_types:?}, got {placed_types:?}"
        );

        past_the_transition().await;
        // Each placed instance is reported by `DescribeInstances`, and each Spot request by
        // `DescribeSpotInstanceRequests`, so nothing the fleet names is a value nothing backs.
        let (status, described) = call(&router, REGION, "DescribeInstances", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInstances must succeed, got {status}: {described}"
        );
        for instance_id in &held.instance_ids {
            assert!(
                described.contains(instance_id.as_str()),
                "case {case:?}: DescribeInstances must report the fleet's instance \
                 {instance_id}, got {described}"
            );
        }
        let (status, spot_body) =
            call(&router, REGION, "DescribeSpotInstanceRequests", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSpotInstanceRequests must succeed, got {status}: {spot_body}"
        );
        for request_id in &held.spot_request_ids {
            assert!(
                spot_body.contains(request_id.as_str()),
                "case {case:?}: DescribeSpotInstanceRequests must report the request \
                 {request_id}, got {spot_body}"
            );
        }

        if row.fleet_type == FLEET_TYPE_INSTANT {
            continue;
        }
        // An asynchronous fleet's instances are read back through `DescribeFleetInstances`,
        // which reports the ones that are RUNNING — every instance the fleet placed, now
        // that the clock has run them.
        let (status, reported) = call(
            &router,
            REGION,
            "DescribeFleetInstances",
            &[p("FleetId", fleet_id.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeFleetInstances must succeed, got {status}: {reported}"
        );
        let mut reported_ids = elements(&reported, "instanceId");
        reported_ids.sort();
        let mut expected_ids = held.instance_ids.clone();
        expected_ids.sort();
        assert_eq!(
            reported_ids, expected_ids,
            "case {case:?}: DescribeFleetInstances must report the instances the fleet \
             placed, expected {expected_ids:?}, got {reported_ids:?}"
        );
    }
}

#[tokio::test]
async fn an_instant_fleet_reports_the_instances_it_launched() {
    let case = "an instant fleet of one On-Demand instance in a named subnet";
    let (router, engine) = emulator();
    let template_id = launch_template(&router, case, "instant-fleet", "t3.micro").await;
    let subnet_id = default_subnet_id();
    let mut params = one_template(&template_id);
    params.extend([
        p("Type", FLEET_TYPE_INSTANT),
        p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
        p("TargetCapacitySpecification.OnDemandTargetCapacity", "1"),
        p("LaunchTemplateConfigs.1.Overrides.1.InstanceType", "c5.large"),
        p("LaunchTemplateConfigs.1.Overrides.1.SubnetId", subnet_id.clone()),
        p(
            "LaunchTemplateConfigs.1.Overrides.1.AvailabilityZone",
            format!("{REGION}a"),
        ),
    ]);
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateFleet, got {status}: {body}"
    );
    let fleet_id =
        element(&body, "fleetId").unwrap_or_else(|| panic!("case {case:?}: no fleetId in {body}"));
    let held = stored(&engine, case, &fleet_id);

    // "Information about the instances that were launched by the fleet. Supported only for
    // fleets of type instant" — the ids are the instances the store holds for the fleet.
    let reported_ids = reported_instance_ids(case, &body);
    assert_eq!(
        reported_ids, held.instance_ids,
        "case {case:?}: expected the launched instances {:?}, got {reported_ids:?}",
        held.instance_ids
    );
    assert_eq!(
        element(&body, "lifecycle").as_deref(),
        Some(INSTANCE_LIFECYCLE_ON_DEMAND),
        "case {case:?}: an instance of an On-Demand pool is {INSTANCE_LIFECYCLE_ON_DEMAND}, \
         got {body}"
    );
    assert_eq!(
        element(&body, "instanceType").as_deref(),
        Some("c5.large"),
        "case {case:?}: the instance type is the override's, got {body}"
    );
    assert_eq!(
        element(&body, "availabilityZone").as_deref(),
        Some(format!("{REGION}a").as_str()),
        "case {case:?}: the zone is the one the override named, got {body}"
    );
    assert_eq!(
        element(&body, "subnetId").as_deref(),
        Some(subnet_id.as_str()),
        "case {case:?}: the subnet is the one the override named, got {body}"
    );
    assert!(
        element(&body, "availabilityZoneId").is_some(),
        "case {case:?}: the zone is reported by id as well as by name, got {body}"
    );
    // The fleet launched every instance it asked for, so its error set is empty rather than
    // carrying an entry.
    assert!(
        elements(&body, "errorCode").is_empty(),
        "case {case:?}: a fleet that launched its whole target reports no launch error, \
         got {body}"
    );
    assert_eq!(
        held.state, FLEET_STATE_ACTIVE,
        "case {case:?}: an instant fleet is {FLEET_STATE_ACTIVE} when the call answers, \
         got {:?}",
        held.state
    );

    // "Currently, DescribeFleetInstances does not support fleets of type instant" — the
    // instances of an instant fleet are the ones the create reported.
    let (status, refused) = call(
        &router,
        REGION,
        "DescribeFleetInstances",
        &[p("FleetId", fleet_id.clone())],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: DescribeFleetInstances refuses an instant fleet, got {status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "UnsupportedOperation",
        "case {case:?}: expected UnsupportedOperation, got {refused}"
    );
}

#[tokio::test]
async fn the_clock_moves_an_asynchronous_fleet_from_submitted_to_active() {
    let case = "a maintain fleet right after the call";
    let (router, engine) = emulator();
    let template_id = launch_template(&router, case, "async-fleet", "t3.micro").await;
    let mut params = one_template(&template_id);
    params.extend([
        p("Type", FLEET_TYPE_MAINTAIN),
        p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
    ]);
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateFleet, got {status}: {body}"
    );
    let fleet_id =
        element(&body, "fleetId").unwrap_or_else(|| panic!("case {case:?}: no fleetId in {body}"));
    assert_eq!(
        stored(&engine, case, &fleet_id).state,
        FLEET_STATE_SUBMITTED,
        "case {case:?}: a fleet whose request EC2 has just taken on is \
         {FLEET_STATE_SUBMITTED}"
    );

    past_the_transition().await;
    // A READ is what materialises the move the clock already owed: it was due at that
    // instant whether or not anybody called.
    let (status, reported) = call(
        &router,
        REGION,
        "DescribeFleetInstances",
        &[p("FleetId", fleet_id.clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeFleetInstances must succeed, got {status}: {reported}"
    );
    let case = "the same fleet once its move has fallen due";
    assert_eq!(
        stored(&engine, case, &fleet_id).state,
        FLEET_STATE_ACTIVE,
        "case {case:?}: the clock moves the fleet to {FLEET_STATE_ACTIVE}"
    );
}

#[tokio::test]
async fn the_tags_a_fleet_asks_for_land_on_the_resources_they_name() {
    let case = "an instant fleet tagging itself and its instances";
    let (router, engine) = emulator();
    let template_id = launch_template(&router, case, "tagged-fleet", "t3.micro").await;
    let mut params = one_template(&template_id);
    params.extend([
        p("Type", FLEET_TYPE_INSTANT),
        p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
        p("TagSpecification.1.ResourceType", "fleet"),
        p("TagSpecification.1.Tag.1.Key", "Name"),
        p("TagSpecification.1.Tag.1.Value", "batch"),
        p("TagSpecification.2.ResourceType", "instance"),
        p("TagSpecification.2.Tag.1.Key", "Stage"),
        p("TagSpecification.2.Tag.1.Value", "prod"),
    ]);
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateFleet, got {status}: {body}"
    );
    let fleet_id =
        element(&body, "fleetId").unwrap_or_else(|| panic!("case {case:?}: no fleetId in {body}"));
    let held = stored(&engine, case, &fleet_id);
    assert_eq!(
        held.tags,
        BTreeMap::from([("Name".to_string(), "batch".to_string())]),
        "case {case:?}: the fleet carries the tags its `fleet` specification named, got {:?}",
        held.tags
    );

    // The `instance` tags are on the INSTANCES, read back through `DescribeInstances`.
    let instance_id = held
        .instance_ids
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: the fleet must have placed an instance"));
    let (status, described) = call(
        &router,
        REGION,
        "DescribeInstances",
        &[p("InstanceId.1", instance_id.clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstances must succeed, got {status}: {described}"
    );
    assert!(
        described.contains("Stage") && described.contains("prod"),
        "case {case:?}: the instance carries the tags the `instance` specification named, \
         got {described}"
    );

    // "If the fleet type is maintain or request … you cannot specify a resource type of
    // instance, volume, or network-interface."
    let case = "a maintain fleet tagging the instances it launches";
    let mut refused_params = one_template(&template_id);
    refused_params.extend([
        p("Type", FLEET_TYPE_MAINTAIN),
        p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
        p("TagSpecification.1.ResourceType", "instance"),
        p("TagSpecification.1.Tag.1.Key", "Stage"),
        p("TagSpecification.1.Tag.1.Value", "prod"),
    ]);
    let before = store_counts(&engine);
    let (status, refused) = create(&router, &refused_params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got {refused}"
    );
    assert_eq!(
        store_counts(&engine),
        before,
        "case {case:?}: a refused request places nothing"
    );
}

/// One row of the refusal table: the parameters that make a request unservable, and the
/// status and error code EC2 answers it with.
struct Refusal {
    case: &'static str,
    /// Extra parameters, given the launch template id the row is run with.
    params: fn(&str) -> Params,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn an_unservable_request_is_refused_and_stores_nothing() {
    let rows = vec![
        Refusal {
            case: "no launch template configuration at all",
            params: |_| vec![p("TargetCapacitySpecification.TotalTargetCapacity", "1")],
            status: 400,
            // The member is modelled `@required`, so the boundary validator refuses a
            // request that carries none before the handler runs.
            code: "MalformedInput",
        },
        Refusal {
            case: "a launch template id the service never minted",
            params: |_| {
                vec![
                    p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
                    p(
                        "LaunchTemplateConfigs.1.LaunchTemplateSpecification.LaunchTemplateId",
                        "lt-0000ffff0000ffff0",
                    ),
                ]
            },
            status: 400,
            // A launch template id is one the service itself minted, so an id this account
            // holds no template under is not a launch template id at all.
            code: "InvalidLaunchTemplateId.Malformed",
        },
        Refusal {
            case: "a launch template name the account does not hold",
            params: |_| {
                vec![
                    p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
                    p(
                        "LaunchTemplateConfigs.1.LaunchTemplateSpecification.LaunchTemplateName",
                        "no-such-template",
                    ),
                ]
            },
            status: 400,
            code: "InvalidLaunchTemplateName.NotFoundException",
        },
        Refusal {
            case: "a launch template version the template does not have",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
                    p(
                        "LaunchTemplateConfigs.1.LaunchTemplateSpecification.Version",
                        "99",
                    ),
                ]);
                params
            },
            status: 400,
            code: "InvalidLaunchTemplateId.VersionNotFound",
        },
        Refusal {
            case: "a negative total target capacity",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.push(p(
                    "TargetCapacitySpecification.TotalTargetCapacity",
                    "-1",
                ));
                params
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "no capacity at all for a fleet that is not of type maintain",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("Type", FLEET_TYPE_REQUEST),
                    p("TargetCapacitySpecification.TotalTargetCapacity", "0"),
                ]);
                params
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "On-Demand and Spot capacity above the total",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("TargetCapacitySpecification.TotalTargetCapacity", "2"),
                    p("TargetCapacitySpecification.OnDemandTargetCapacity", "2"),
                    p("TargetCapacitySpecification.SpotTargetCapacity", "1"),
                ]);
                params
            },
            status: 400,
            code: "InvalidParameterCombination",
        },
        Refusal {
            case: "a capacity type drawn from a reservation",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("Type", FLEET_TYPE_INSTANT),
                    p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
                    p(
                        "TargetCapacitySpecification.DefaultTargetCapacityType",
                        "capacity-block",
                    ),
                ]);
                params
            },
            status: 400,
            code: "UnsupportedOperation",
        },
        Refusal {
            case: "an excess capacity policy on a fleet that is not of type maintain",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("Type", FLEET_TYPE_REQUEST),
                    p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
                    p("ExcessCapacityTerminationPolicy", "termination"),
                ]);
                params
            },
            status: 400,
            code: "InvalidParameterCombination",
        },
        Refusal {
            case: "unhealthy-instance replacement on an instant fleet",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("Type", FLEET_TYPE_INSTANT),
                    p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
                    p("ReplaceUnhealthyInstances", "true"),
                ]);
                params
            },
            status: 400,
            code: "InvalidParameterCombination",
        },
        Refusal {
            case: "a single-instance-type preference on a maintain fleet",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("Type", FLEET_TYPE_MAINTAIN),
                    p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
                    p("SpotOptions.SingleInstanceType", "true"),
                ]);
                params
            },
            status: 400,
            code: "InvalidParameterCombination",
        },
        Refusal {
            case: "a Spot pool count beside a strategy that chooses the pools itself",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("TargetCapacitySpecification.TotalTargetCapacity", "2"),
                    p(
                        "SpotOptions.AllocationStrategy",
                        SPOT_ALLOCATION_STRATEGY_DIVERSIFIED,
                    ),
                    p("SpotOptions.InstancePoolsToUseCount", "2"),
                ]);
                params
            },
            status: 400,
            code: "InvalidParameterCombination",
        },
        Refusal {
            case: "a Spot pool count of no pools",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("TargetCapacitySpecification.TotalTargetCapacity", "2"),
                    p("SpotOptions.InstancePoolsToUseCount", "0"),
                ]);
                params
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "a minimum target capacity with neither single-pool preference",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("Type", FLEET_TYPE_INSTANT),
                    p("TargetCapacitySpecification.TotalTargetCapacity", "2"),
                    p("SpotOptions.MinTargetCapacity", "1"),
                ]);
                params
            },
            status: 400,
            code: "InvalidParameterCombination",
        },
        Refusal {
            case: "a validity window that ends before it begins",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
                    p("ValidFrom", "2030-01-02T00:00:00Z"),
                    p("ValidUntil", "2030-01-01T00:00:00Z"),
                ]);
                params
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "a pool whose instances provide no capacity",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
                    p("LaunchTemplateConfigs.1.Overrides.1.WeightedCapacity", "0"),
                ]);
                params
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "a subnet the account does not hold",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
                    p(
                        "LaunchTemplateConfigs.1.Overrides.1.SubnetId",
                        "subnet-0000ffff0000ffff0",
                    ),
                ]);
                params
            },
            status: 400,
            code: "InvalidSubnetID.NotFound",
        },
        Refusal {
            case: "a zone named twice, by name and by id",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
                    p(
                        "LaunchTemplateConfigs.1.Overrides.1.AvailabilityZone",
                        format!("{REGION}a"),
                    ),
                    p(
                        "LaunchTemplateConfigs.1.Overrides.1.AvailabilityZoneId",
                        "usw2-az1",
                    ),
                ]);
                params
            },
            status: 400,
            code: "InvalidParameterCombination",
        },
        Refusal {
            case: "instance types named by their attributes",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
                    p(
                        "LaunchTemplateConfigs.1.Overrides.1.InstanceRequirements.VCpuCount.Min",
                        "2",
                    ),
                    p(
                        "LaunchTemplateConfigs.1.Overrides.1.InstanceRequirements.MemoryMiB.Min",
                        "1024",
                    ),
                ]);
                params
            },
            status: 400,
            code: "UnsupportedOperation",
        },
        Refusal {
            case: "a maximum price that is not a price",
            params: |template_id| {
                let mut params = one_template(template_id);
                params.extend([
                    p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
                    p("LaunchTemplateConfigs.1.Overrides.1.MaxPrice", "cheap"),
                ]);
                params
            },
            status: 400,
            code: "InvalidParameterValue",
        },
    ];

    for row in rows {
        let case = row.case;
        let (router, engine) = emulator();
        let template_id = launch_template(&router, case, "refusal-fleet", "t3.micro").await;
        let before = store_counts(&engine);
        let (status, body) = create(&router, &(row.params)(&template_id)).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status}: {body}",
            row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected the error code {:?}, got the body {body}",
            row.code
        );
        let guard = engine.state::<Ec2State>();
        let fleets = guard.lock().fleets.len();
        assert_eq!(
            fleets, 0,
            "case {case:?}: a refused request stores no fleet, got {fleets}"
        );
        assert_eq!(
            store_counts(&engine),
            before,
            "case {case:?}: a refused request places no instance and no Spot request"
        );
    }
}

#[tokio::test]
async fn a_dry_run_performs_nothing_and_a_client_token_creates_one_fleet() {
    let case = "a dry run of a well-formed request";
    let (router, engine) = emulator();
    let template_id = launch_template(&router, case, "dry-run-fleet", "t3.micro").await;
    let mut params = one_template(&template_id);
    params.extend([
        p("Type", FLEET_TYPE_MAINTAIN),
        p("TargetCapacitySpecification.TotalTargetCapacity", "2"),
    ]);
    let mut dry = params.clone();
    dry.push(p("DryRun", "true"));
    let before = store_counts(&engine);
    let (status, body) = create(&router, &dry).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got {body}"
    );
    assert_eq!(
        store_counts(&engine),
        before,
        "case {case:?}: a dry run places nothing"
    );
    {
        let guard = engine.state::<Ec2State>();
        assert!(
            guard.lock().fleets.is_empty(),
            "case {case:?}: a dry run stores no fleet"
        );
    }

    // A dry run of a request that would have been REFUSED is refused with that reason
    // instead: the flag asks whether the request would have succeeded.
    let case = "a dry run of a request naming an unknown launch template";
    let (status, body) = create(
        &router,
        &[
            p("DryRun", "true"),
            p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
            p(
                "LaunchTemplateConfigs.1.LaunchTemplateSpecification.LaunchTemplateId",
                "lt-0000ffff0000ffff0",
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidLaunchTemplateId.Malformed",
        "case {case:?}: expected the launch template's own refusal, got {body}"
    );

    let case = "the same ClientToken twice";
    let mut idempotent = params.clone();
    idempotent.push(p("ClientToken", "create-fleet-token"));
    let (status, first_body) = create(&router, &idempotent).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status}: {first_body}"
    );
    let first = element(&first_body, "fleetId")
        .unwrap_or_else(|| panic!("case {case:?}: no fleetId in {first_body}"));
    let after_first = store_counts(&engine);
    let (status, second_body) = create(&router, &idempotent).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status}: {second_body}"
    );
    let second = element(&second_body, "fleetId")
        .unwrap_or_else(|| panic!("case {case:?}: no fleetId in {second_body}"));
    assert_eq!(
        second, first,
        "case {case:?}: one token names one fleet, expected {first}, got {second}"
    );
    assert_eq!(
        store_counts(&engine),
        after_first,
        "case {case:?}: the replay places no further capacity"
    );
    {
        let guard = engine.state::<Ec2State>();
        let fleets = guard.lock().fleets.len();
        assert_eq!(
            fleets, 1,
            "case {case:?}: one token creates one fleet, got {fleets}"
        );
    }

    let case = "a different ClientToken";
    let mut other = params.clone();
    other.push(p("ClientToken", "create-fleet-other-token"));
    let (status, body) = create(&router, &other).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status}: {body}"
    );
    let third = element(&body, "fleetId")
        .unwrap_or_else(|| panic!("case {case:?}: no fleetId in {body}"));
    assert_ne!(
        third, first,
        "case {case:?}: a token nothing was created for creates its own fleet, got {third}"
    );
}
