// SPDX-License-Identifier: CC0-1.0

//! A VPC-access domain's placement where the subnets it names cannot be resolved.
//!
//! RULE: `VPCOptions$SubnetIds` names subnets of the service that OWNS subnets, and the
//! two `VPCDerivedInfo` members Amazon VPC derives from them — `VPCId` and
//! `AvailabilityZones` — are what that service answers for them. An emulator with no such
//! service mounted resolves nothing, so a placement is neither completed with a VPC this
//! service invented nor refused for disagreeing with one: every create naming subnets is
//! served, carries the subnets and security groups the request listed, and reports neither
//! derived member. The refusal for subnets that span two VPCs is decided from what the
//! owning service ANSWERED, so silence refuses nothing — the router's own
//! `opensearch_domain_vpc_placement` suite drives the other half of that rule, where the
//! subnets ARE resolvable.
//!
//! The rows differ along what the rule generalises over: how many subnets a placement
//! names, whether it repeats one, whether it names security groups, whether it asks for
//! egress, and whether `VPCOptions` is carried at all.

use serde_json::{json, Value};

use super::domain_lifecycle::{Emulator, SETTLE_WAIT};

/// The engine release every domain below runs.
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// One placement to create.
struct Row {
    case: &'static str,
    name: &'static str,
    /// The `VPCOptions` the create carries, if any.
    vpc_options: Option<Value>,
}

/// The `VPCOptions` members a create echoes back unchanged, as `VPCDerivedInfo` declares
/// them: the three the REQUEST carries. `VPCId` and `AvailabilityZones` are the two the
/// owning service derives, and are asserted absent.
const ECHOED_MEMBERS: &[&str] = &["SubnetIds", "SecurityGroupIds", "EgressEnabled"];

/// The members `VPCDerivedInfo` declares as derived from the subnets rather than carried
/// by the request.
const DERIVED_MEMBERS: &[&str] = &["VPCId", "AvailabilityZones"];

/// The table: one row per way a placement differs, plus the absence of one.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one subnet and one security group",
            name: "unresolved-one",
            vpc_options: Some(json!({
                "SubnetIds": ["subnet-0a0a0000000000001"],
                "SecurityGroupIds": ["sg-0a0a0000000000001"],
            })),
        },
        Row {
            case: "two subnets, no security group",
            name: "unresolved-two",
            vpc_options: Some(json!({
                "SubnetIds": ["subnet-0b0b0000000000001", "subnet-0b0b0000000000002"],
            })),
        },
        Row {
            case: "three subnets, two security groups, egress enabled",
            name: "unresolved-three",
            vpc_options: Some(json!({
                "SubnetIds": [
                    "subnet-0c0c0000000000001",
                    "subnet-0c0c0000000000002",
                    "subnet-0c0c0000000000003",
                ],
                "SecurityGroupIds": ["sg-0c0c0000000000001", "sg-0c0c0000000000002"],
                "EgressEnabled": true,
            })),
        },
        Row {
            case: "one subnet named twice",
            name: "unresolved-repeated",
            vpc_options: Some(json!({
                "SubnetIds": ["subnet-0d0d0000000000001", "subnet-0d0d0000000000001"],
            })),
        },
        Row {
            case: "no VPCOptions at all, so the domain is public",
            name: "unresolved-public",
            vpc_options: None,
        },
    ]
}

/// A `CreateDomain` body for one row.
fn create_body(row: &Row) -> Value {
    let mut body = json!({
        "DomainName": row.name,
        "EngineVersion": ENGINE_VERSION,
        "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
        "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
    });
    if let Some(vpc_options) = &row.vpc_options {
        body["VPCOptions"] = vpc_options.clone();
    }
    body
}

/// Assert the `VPCOptions` of `status` is what the rule says for `row`.
fn assert_placement(status: &Value, row: &Row, source: &str) {
    let case = row.case;
    let reported = &status["VPCOptions"];
    match &row.vpc_options {
        Some(requested) => {
            for member in ECHOED_MEMBERS {
                // A member the request CARRIED comes back as it was given; one it did not
                // carry is a member the service has nothing to report, and it must not
                // invent one.
                if requested.get(member).is_some() {
                    assert_eq!(
                        reported[member], requested[member],
                        "case {case:?}: {source} must report the {member} the request \
                         carried, got {reported:?}"
                    );
                } else {
                    assert_eq!(
                        reported[member],
                        Value::Null,
                        "case {case:?}: {source} must report no {member} for a request that \
                         named none, got {reported:?}"
                    );
                }
            }
            for member in DERIVED_MEMBERS {
                assert_eq!(
                    reported[member],
                    Value::Null,
                    "case {case:?}: {source} must report no {member} for a subnet nothing \
                     resolved, got {reported:?}"
                );
            }
        }
        None => assert_eq!(
            *reported,
            Value::Null,
            "case {case:?}: {source} must report no VPCOptions for a public domain, got \
             {reported:?}"
        ),
    }
}

#[tokio::test]
async fn a_placement_whose_subnets_resolve_nowhere_is_served_whole() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = row.case;
        let created = emulator.create(create_body(&row)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateDomain, got {} with {:?}",
            created.status, created.body
        );
        assert_placement(&created.body["DomainStatus"], &row, "the create");

        // Read the domain back, so what is asserted is the placement the service STORED
        // rather than only what the create echoed.
        let read = emulator.describe(row.name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from DescribeDomain, got {} with {:?}",
            read.status, read.body
        );
        assert_placement(&read.body["DomainStatus"], &row, "DescribeDomain");
    }
}

#[tokio::test]
async fn a_change_of_placement_whose_subnets_resolve_nowhere_is_served_whole() {
    let emulator = Emulator::new();

    // The domain every change below is applied to, created public so each change is the
    // whole of what the placement becomes.
    let public = Row {
        case: "no VPCOptions at all, so the domain is public",
        name: "unresolved-updated",
        vpc_options: None,
    };
    let created = emulator.create(create_body(&public)).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateDomain, got {} with {:?}",
        created.status, created.body
    );

    // The create's own provisioning span falls due first, so the domain is idle and a
    // configuration change is served on it.
    tokio::time::sleep(SETTLE_WAIT).await;

    // A dry run first: it "returns the results of a dry run analysis without actually
    // applying the change", so the domain is still public afterwards.
    let moved = json!({
        "SubnetIds": ["subnet-0e0e0000000000001", "subnet-0e0e0000000000002"],
        "SecurityGroupIds": ["sg-0e0e0000000000001"],
    });
    let dry = emulator
        .update_config(
            public.name,
            json!({ "VPCOptions": moved.clone(), "DryRun": true }),
        )
        .await;
    assert_eq!(
        dry.status, 200,
        "case dry run: expected 200 from UpdateDomainConfig, got {} with {:?}",
        dry.status, dry.body
    );
    let after_dry = emulator.describe(public.name).await;
    assert_placement(
        &after_dry.body["DomainStatus"],
        &public,
        "DescribeDomain after the dry run",
    );

    let row = Row {
        case: "a change into a VPC whose subnets resolve nowhere",
        name: public.name,
        vpc_options: Some(moved.clone()),
    };
    let applied = emulator
        .update_config(public.name, json!({ "VPCOptions": moved }))
        .await;
    assert_eq!(
        applied.status, 200,
        "case {:?}: expected 200 from UpdateDomainConfig, got {} with {:?}",
        row.case, applied.status, applied.body
    );
    let read = emulator.describe(public.name).await;
    assert_placement(&read.body["DomainStatus"], &row, "DescribeDomain");
}
