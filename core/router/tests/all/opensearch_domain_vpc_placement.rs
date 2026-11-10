// SPDX-License-Identifier: CC0-1.0

//! A VPC-access Amazon OpenSearch Service domain is placed in ONE VPC.
//!
//! RULE: the subnets a request names in `VPCOptions$SubnetIds` are resolved against the
//! service that owns subnets, and a request whose resolved subnets do not all belong to
//! one VPC describes a network placement the service cannot provision, so it is refused
//! with the modelled `ValidationException` (HTTP 400) and nothing is created or changed;
//! a placement whose subnets agree on one VPC is served, and the domain reports that VPC
//! and the zones of those subnets in the two `VPCDerivedInfo` members Amazon VPC derives
//! from them. Both `CreateDomain` and `UpdateDomainConfig` carry `VPCOptions`, so both
//! are judged by it.
//!
//! A subnet is Amazon EC2's resource, so which VPC one is in is EC2's to say: this suite
//! lives beside the router because every VPC and subnet below is minted by the real EC2
//! operations on the SAME engine that serves OpenSearch Service, so every identifier in a
//! request is one EC2 handed out and every refusal is grounded in what EC2 actually
//! answered. The wire harness is the one
//! [`crate::opensearch_vpc_endpoint_vpc_membership`] publishes, which drives the VPC
//! ENDPOINT half of the same rule.
//!
//! The create rows differ along what the rule generalises over: how many subnets the
//! request names, how many VPCs they span, which position the foreign one sits in, whether
//! a named identifier resolves to a subnet at all, and whether the request names
//! `VPCOptions` in the first place. The update rows differ the same way, and each refusal
//! is followed by a served change, so a refused call is shown to leave the operation
//! working.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::opensearch_vpc_endpoint_vpc_membership::{
    emulator, opensearch, vpc_with_subnets, Reply, PREFIX,
};

/// The engine release every domain below runs — one the service's catalog publishes for
/// the instance type the rows use.
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// Long enough for the provisioning span the clock owes a freshly created domain to have
/// fallen due, so the domain is idle and a configuration change is served on it.
///
/// The span is the one this service's own lifecycle states; the margin covers the test's
/// own scheduling.
fn settle_wait() -> std::time::Duration {
    service_com_amazonaws_opensearch_amazonopensearchservice::state::DOMAIN_WORK
        + std::time::Duration::from_millis(300)
}

/// How AWS words a placement whose subnets do not all belong to one VPC: the sentence the
/// service answers this condition with, pinned here as a literal so a change to the
/// wording moves no expectation with it.
const NOT_IN_ONE_VPC: &str = "The subnets must be in the same VPC.";

/// One subnet a row names.
#[derive(Debug, Clone, Copy)]
enum Pick {
    /// `(network index, subnet index)` of a subnet EC2 minted below.
    Real(usize, usize),
    /// A well-formed identifier the account has no subnet for, so EC2 resolves nothing
    /// for it.
    Unresolvable,
}

/// A well-formed subnet identifier no `CreateSubnet` below handed out.
const UNRESOLVABLE_SUBNET: &str = "subnet-0baadf00d00000001";

/// The security groups every VPC-access row names — not resolved against EC2 by the rule
/// under test, which is about the subnets.
const SECURITY_GROUPS: &[&str] = &["sg-0123456789abcdef0"];

/// One VPC of the estate: the id EC2 minted and, per subnet, the identifier and the zone
/// EC2 placed it in.
struct Network {
    vpc_id: String,
    subnets: Vec<String>,
    zones: Vec<&'static str>,
}

/// One `CreateDomain` to make.
struct CreateRow {
    case: &'static str,
    /// The domain name, which is also what a refused row must leave unresolvable.
    name: &'static str,
    /// The subnets the request names; empty means the request carries no `VPCOptions` at
    /// all, which is a public domain.
    subnets: &'static [Pick],
    /// Whether the create is served.
    accepted: bool,
}

/// The create table: one row per way a placement differs, spanning both sides of the rule.
const CREATE_ROWS: &[CreateRow] = &[
    CreateRow {
        case: "no VPCOptions at all, so the domain is public",
        name: "vpc-place-public",
        subnets: &[],
        accepted: true,
    },
    CreateRow {
        case: "one subnet of the first VPC",
        name: "vpc-place-one",
        subnets: &[Pick::Real(0, 0)],
        accepted: true,
    },
    CreateRow {
        case: "two subnets of the first VPC, one per zone",
        name: "vpc-place-two",
        subnets: &[Pick::Real(0, 0), Pick::Real(0, 1)],
        accepted: true,
    },
    CreateRow {
        case: "three subnets of the first VPC",
        name: "vpc-place-three",
        subnets: &[Pick::Real(0, 0), Pick::Real(0, 1), Pick::Real(0, 2)],
        accepted: true,
    },
    CreateRow {
        case: "two subnets of the second VPC",
        name: "vpc-place-second-vpc",
        subnets: &[Pick::Real(1, 0), Pick::Real(1, 1)],
        accepted: true,
    },
    CreateRow {
        case: "one subnet named twice",
        name: "vpc-place-repeated",
        subnets: &[Pick::Real(0, 1), Pick::Real(0, 1)],
        accepted: true,
    },
    CreateRow {
        case: "an identifier no subnet exists for beside a real subnet",
        name: "vpc-place-unresolvable",
        subnets: &[Pick::Unresolvable, Pick::Real(0, 2)],
        accepted: true,
    },
    CreateRow {
        case: "one subnet of each of two VPCs",
        name: "vpc-place-span-two",
        subnets: &[Pick::Real(0, 0), Pick::Real(1, 0)],
        accepted: false,
    },
    CreateRow {
        case: "the foreign subnet named first",
        name: "vpc-place-foreign-first",
        subnets: &[Pick::Real(1, 1), Pick::Real(0, 1)],
        accepted: false,
    },
    CreateRow {
        case: "two subnets of one VPC and then a foreign one",
        name: "vpc-place-foreign-last",
        subnets: &[Pick::Real(0, 0), Pick::Real(0, 2), Pick::Real(1, 0)],
        accepted: false,
    },
    CreateRow {
        case: "a foreign subnet between two subnets of one VPC",
        name: "vpc-place-foreign-middle",
        subnets: &[Pick::Real(0, 0), Pick::Real(1, 0), Pick::Real(0, 1)],
        accepted: false,
    },
    CreateRow {
        case: "subnets of three VPCs",
        name: "vpc-place-span-three",
        subnets: &[Pick::Real(0, 0), Pick::Real(1, 0), Pick::Real(2, 0)],
        accepted: false,
    },
    CreateRow {
        case: "an unresolvable identifier beside subnets of two VPCs",
        name: "vpc-place-unresolved-span",
        subnets: &[Pick::Unresolvable, Pick::Real(0, 1), Pick::Real(1, 1)],
        accepted: false,
    },
];

/// One `UpdateDomainConfig` to apply to the domain created in the first VPC.
struct UpdateRow {
    case: &'static str,
    /// The subnets the change names.
    subnets: &'static [Pick],
    /// Whether the change is a dry run, which "returns the results of a dry run analysis
    /// without actually applying the change".
    dry_run: bool,
    /// Whether the change is served.
    accepted: bool,
}

/// The update table: the same rule over the operation that CHANGES a placement.
const UPDATE_ROWS: &[UpdateRow] = &[
    UpdateRow {
        case: "a change naming subnets of two VPCs",
        subnets: &[Pick::Real(0, 0), Pick::Real(1, 0)],
        dry_run: false,
        accepted: false,
    },
    UpdateRow {
        case: "a change naming the foreign subnet first",
        subnets: &[Pick::Real(1, 1), Pick::Real(0, 0)],
        dry_run: false,
        accepted: false,
    },
    UpdateRow {
        case: "a dry run naming subnets of two VPCs",
        subnets: &[Pick::Real(0, 2), Pick::Real(2, 0)],
        dry_run: true,
        accepted: false,
    },
    UpdateRow {
        case: "a change naming two subnets of the domain's own VPC",
        subnets: &[Pick::Real(0, 1), Pick::Real(0, 2)],
        dry_run: false,
        accepted: true,
    },
];

/// The subnet identifiers a row names, in the order it names them.
fn named_subnets(networks: &[Network], picks: &[Pick]) -> Vec<String> {
    picks
        .iter()
        .map(|pick| match pick {
            Pick::Real(network, subnet) => networks[*network].subnets[*subnet].clone(),
            Pick::Unresolvable => UNRESOLVABLE_SUBNET.to_string(),
        })
        .collect()
}

/// The VPC the resolved subnets of a row agree on, and the zones they sit in — in the
/// order the row names them, each zone once. `None` where the row names no subnet EC2
/// resolves.
fn expected_placement(networks: &[Network], picks: &[Pick]) -> Option<(String, Vec<String>)> {
    let mut vpc: Option<String> = None;
    let mut zones: Vec<String> = Vec::new();
    for pick in picks {
        if let Pick::Real(network, subnet) = pick {
            let network = &networks[*network];
            vpc = Some(network.vpc_id.clone());
            let zone = network.zones[*subnet].to_string();
            if !zones.contains(&zone) {
                zones.push(zone);
            }
        }
    }
    vpc.map(|vpc| (vpc, zones))
}

/// A `CreateDomain` body naming `subnets`, or a public domain where the row names none.
fn create_body(name: &str, subnets: &[String]) -> Value {
    let mut body = json!({
        "DomainName": name,
        "EngineVersion": ENGINE_VERSION,
        "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
        "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
    });
    if !subnets.is_empty() {
        body["VPCOptions"] = json!({
            "SubnetIds": subnets,
            "SecurityGroupIds": SECURITY_GROUPS,
        });
    }
    body
}

/// `CreateDomain`, as it answered.
async fn create_domain(router: &MultiServiceRouter, body: Value) -> Reply {
    opensearch(
        router,
        "POST",
        &format!("{PREFIX}/opensearch/domain"),
        Some(body),
    )
    .await
}

/// `DescribeDomain`, as it answered.
async fn describe_domain(router: &MultiServiceRouter, name: &str) -> Reply {
    opensearch(
        router,
        "GET",
        &format!("{PREFIX}/opensearch/domain/{name}"),
        None,
    )
    .await
}

/// `UpdateDomainConfig` with the `VPCOptions` a row names, as it answered.
async fn update_config(
    router: &MultiServiceRouter,
    name: &str,
    subnets: &[String],
    dry_run: bool,
) -> Reply {
    opensearch(
        router,
        "POST",
        &format!("{PREFIX}/opensearch/domain/{name}/config"),
        Some(json!({
            "VPCOptions": {
                "SubnetIds": subnets,
                "SecurityGroupIds": SECURITY_GROUPS,
            },
            "DryRun": dry_run,
        })),
    )
    .await
}

/// The domain names `ListDomainNames` reports for this account.
async fn listed_domains(router: &MultiServiceRouter) -> Vec<String> {
    let listed = opensearch(router, "GET", &format!("{PREFIX}/domain"), None).await;
    assert_eq!(
        listed.status, 200,
        "a listing of this account's domains must be served, got {} with {}",
        listed.status, listed.body
    );
    listed.body["DomainNames"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|info| info["DomainName"].as_str().map(str::to_owned))
        .collect()
}

/// The `VPCOptions` a `DomainStatus` reports, as `(subnets, vpc id, zones)`.
fn placement_of(status: &Value) -> (Vec<String>, Option<String>, Vec<String>) {
    let list = |name: &str| {
        status["VPCOptions"][name]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect::<Vec<String>>()
    };
    (
        list("SubnetIds"),
        status["VPCOptions"]["VPCId"].as_str().map(str::to_owned),
        list("AvailabilityZones"),
    )
}

/// Assert a refusal is the modelled one, worded as the service words this condition.
fn assert_refused(answer: &Reply, case: &str) {
    assert_eq!(
        (answer.status, answer.error_code()),
        (400, "ValidationException"),
        "case {case:?}: expected a 400 ValidationException, got {} with {}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.message(),
        NOT_IN_ONE_VPC,
        "case {case:?}: expected the sentence AWS words this condition with, got {}",
        answer.body
    );
}

#[tokio::test]
async fn a_domain_is_placed_in_one_vpc() {
    let router = emulator();

    // THREE VPCs, created through EC2 itself so every identifier below is one EC2 minted
    // and every VPC a refusal is decided from is one it actually answered.
    let plan: [(&str, &[&'static str]); 3] = [
        ("10.130", &["us-west-2a", "us-west-2b", "us-west-2c"]),
        ("10.131", &["us-west-2a", "us-west-2b"]),
        ("10.132", &["us-west-2a"]),
    ];
    let mut networks: Vec<Network> = Vec::new();
    for (prefix, zones) in plan {
        let (vpc_id, subnets) = vpc_with_subnets(&router, prefix, zones).await;
        networks.push(Network {
            vpc_id,
            subnets,
            zones: zones.to_vec(),
        });
    }
    for (left, right) in [(0, 1), (0, 2), (1, 2)] {
        assert_ne!(
            networks[left].vpc_id, networks[right].vpc_id,
            "setup: the VPCs must be distinct, got {} and {}",
            networks[left].vpc_id, networks[right].vpc_id
        );
    }

    for row in CREATE_ROWS {
        let case = row.case;
        let subnets = named_subnets(&networks, row.subnets);
        let answer = create_domain(&router, create_body(row.name, &subnets)).await;

        if !row.accepted {
            assert_refused(&answer, case);

            // A refused create leaves NO domain behind: the name resolves nowhere and the
            // account's listing does not carry it.
            let read = describe_domain(&router, row.name).await;
            assert_eq!(
                read.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: a refused create must leave the name unresolvable, got {} \
                 with {}",
                read.status,
                read.body
            );
            assert!(
                !listed_domains(&router).await.contains(&row.name.to_string()),
                "case {case:?}: a refused create must leave the account's domains as they \
                 were"
            );
            continue;
        }

        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the create to be served, got {} with {}",
            answer.status, answer.body
        );

        // Read the domain back through `DescribeDomain`, so what is asserted is the
        // placement the service STORED and not only what the create echoed.
        let read = describe_domain(&router, row.name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the domain to be readable, got {} with {}",
            read.status, read.body
        );
        for (source, reported) in [
            ("the create", placement_of(&answer.body["DomainStatus"])),
            ("DescribeDomain", placement_of(&read.body["DomainStatus"])),
        ] {
            let (named, vpc_id, zones) = reported;
            assert_eq!(
                named, subnets,
                "case {case:?}: {source} must report the subnets the request named, got \
                 {named:?}"
            );
            match expected_placement(&networks, row.subnets) {
                // The VPC and the zones are DERIVED from the subnets by the service that
                // owns them, so they are what EC2 answered for the subnets that resolved.
                Some((vpc, expected_zones)) => {
                    assert_eq!(
                        vpc_id.as_deref(),
                        Some(vpc.as_str()),
                        "case {case:?}: {source} must report the VPC its subnets are in"
                    );
                    assert_eq!(
                        zones, expected_zones,
                        "case {case:?}: {source} must report the zones its subnets are in, \
                         got {zones:?}"
                    );
                }
                // A request naming no subnet has nothing to derive either member from.
                None => {
                    assert_eq!(
                        vpc_id, None,
                        "case {case:?}: {source} must report no VPC for a public domain"
                    );
                    assert!(
                        zones.is_empty(),
                        "case {case:?}: {source} must report no zones for a public domain, \
                         got {zones:?}"
                    );
                }
            }
        }
    }

    // The CHANGE half of the same rule, on the domain created in the first VPC: the
    // create's own provisioning span falls due first, so the domain is idle and a
    // configuration change is served on it.
    let name = "vpc-place-one";
    tokio::time::sleep(settle_wait()).await;

    for row in UPDATE_ROWS {
        let case = row.case;
        let subnets = named_subnets(&networks, row.subnets);
        let before = describe_domain(&router, name).await;
        let before = placement_of(&before.body["DomainStatus"]);
        let answer = update_config(&router, name, &subnets, row.dry_run).await;

        if row.accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the change to be served, got {} with {}",
                answer.status, answer.body
            );
            let read = describe_domain(&router, name).await;
            let (named, vpc_id, zones) = placement_of(&read.body["DomainStatus"]);
            assert_eq!(
                named, subnets,
                "case {case:?}: the domain must carry the subnets the change named, got \
                 {named:?}"
            );
            let (vpc, expected_zones) = expected_placement(&networks, row.subnets)
                .unwrap_or_else(|| panic!("case {case:?}: the row names a resolvable subnet"));
            assert_eq!(
                vpc_id.as_deref(),
                Some(vpc.as_str()),
                "case {case:?}: the domain must report the VPC its subnets are in"
            );
            assert_eq!(
                zones, expected_zones,
                "case {case:?}: the domain must report the zones its subnets are in, got \
                 {zones:?}"
            );
            tokio::time::sleep(settle_wait()).await;
        } else {
            assert_refused(&answer, case);
            let read = describe_domain(&router, name).await;
            assert_eq!(
                placement_of(&read.body["DomainStatus"]),
                before,
                "case {case:?}: a refused change must leave the placement standing, got {}",
                read.body
            );
        }
    }
}
