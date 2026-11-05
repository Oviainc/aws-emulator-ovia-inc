// SPDX-License-Identifier: CC0-1.0

//! Redshift-managed VPC endpoints (endpoint access) and the endpoint
//! authorizations that grant them, end to end over the awsQuery wire.
//!
//! The RULE the suite asserts: every operation in this family resolves each name
//! its request carries against this service's store — the endpoint first, then
//! the subnet group, then the cluster — and a name that resolves to nothing is
//! that resource's OWN modelled fault with the status the model gives it, raised
//! before any record is written; a name that resolves is served from the stored
//! record. So the tables span the operations, the kind of name that is
//! unresolvable, and the spelling, length and case of that name, rather than one
//! recorded request.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.

use std::time::Duration;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator, REGION};

/// An account id other than the emulator's own, for a grant to somebody else.
///
/// `pub(crate)` so the sibling suite about this family's refusal WORDING grants
/// to the same third-party account rather than repeating the literal.
pub(crate) const OTHER_ACCOUNT: &str = "111122223333";

/// A name of exactly `len` characters that satisfies every other part of the
/// documented endpoint-name syntax: a leading letter and then alphanumerics.
///
/// `pub(crate)` so the sibling suite about this family's refusal WORDING builds
/// its length rows the same way rather than writing a second copy.
pub(crate) fn name_of_length(len: usize) -> String {
    std::iter::once('g')
        .chain(std::iter::repeat_n('x', len - 1))
        .collect()
}

/// `call` takes borrowed pairs, so a case that builds owned parameters borrows
/// them through this.
pub(crate) fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// Assert one refusal: the answer's status and awsQuery `<Code>`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// Assert one served answer.
pub(crate) fn assert_served(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected status 200, got {} with body {}",
        answer.status, answer.body
    );
}

/// Create one cluster and leave it in the `available` state, which is the state
/// an endpoint may be created on it in.
///
/// The clock-owed `creating` -> `available` transition falls due
/// [`state::PROVISIONING_SPAN`] after the create, so this waits that span out
/// (plus a margin for a loaded runner) rather than reaching into the store.
pub(crate) async fn available_cluster(emulator: &Emulator, identifier: &str) {
    let created = emulator.create_minimal(identifier).await;
    assert_served(&format!("seed cluster {identifier}"), &created);
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
}

/// Create one cluster subnet group holding `subnet_ids`.
pub(crate) async fn create_subnet_group(
    emulator: &Emulator,
    name: &str,
    subnet_ids: &[&str],
) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        ("ClusterSubnetGroupName".to_string(), name.to_string()),
        ("Description".to_string(), format!("subnets for {name}")),
    ];
    for (index, subnet_id) in subnet_ids.iter().enumerate() {
        params.push((
            format!("SubnetIds.SubnetIdentifier.{}", index + 1),
            (*subnet_id).to_string(),
        ));
    }
    emulator
        .call("CreateClusterSubnetGroup", &borrowed(&params))
        .await
}

/// `CreateEndpointAccess` for one cluster and subnet group, with the security
/// groups `security_group_ids` names (none when it is empty).
pub(crate) async fn create_endpoint(
    emulator: &Emulator,
    endpoint_name: &str,
    cluster_identifier: &str,
    subnet_group_name: &str,
    security_group_ids: &[&str],
) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        ("EndpointName".to_string(), endpoint_name.to_string()),
        (
            "ClusterIdentifier".to_string(),
            cluster_identifier.to_string(),
        ),
        (
            "SubnetGroupName".to_string(),
            subnet_group_name.to_string(),
        ),
    ];
    for (index, id) in security_group_ids.iter().enumerate() {
        params.push((
            format!("VpcSecurityGroupIds.VpcSecurityGroupId.{}", index + 1),
            (*id).to_string(),
        ));
    }
    emulator
        .call("CreateEndpointAccess", &borrowed(&params))
        .await
}

/// `AuthorizeEndpointAccess` of one cluster to one account, for the VPCs
/// `vpc_ids` names (every VPC of that account when it is empty).
async fn authorize(
    emulator: &Emulator,
    cluster_identifier: &str,
    account: &str,
    vpc_ids: &[&str],
) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        (
            "ClusterIdentifier".to_string(),
            cluster_identifier.to_string(),
        ),
        ("Account".to_string(), account.to_string()),
    ];
    for (index, vpc) in vpc_ids.iter().enumerate() {
        params.push((
            format!("VpcIds.VpcIdentifier.{}", index + 1),
            (*vpc).to_string(),
        ));
    }
    emulator
        .call("AuthorizeEndpointAccess", &borrowed(&params))
        .await
}

/// Every endpoint name a `DescribeEndpointAccess` body lists, in page order.
fn listed_endpoints(answer: &Answer) -> Vec<String> {
    all_text(&answer.body, "EndpointName")
}

/// Each operation of the family resolves the names its request carries and
/// answers the modelled fault of whichever one resolves to nothing.
///
/// The table spans the OPERATIONS and the KIND of name that is unresolvable (an
/// endpoint, a subnet group, a cluster), and the endpoint-name rows span the
/// spelling, both documented length bounds and the case of the name — so no row
/// is the recorded request and none of the refusals can be an "everything is
/// refused" artefact (the served rows at the end rule that out).
#[tokio::test]
async fn an_unresolvable_name_is_that_resources_own_modelled_fault_in_every_operation() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "endpoint-host").await;
    assert_served(
        "seed subnet group",
        &create_subnet_group(&emulator, "endpoint-subnets", &["subnet-a", "subnet-b"]).await,
    );
    assert_served(
        "seed endpoint",
        &create_endpoint(
            &emulator,
            "present-endpoint",
            "endpoint-host",
            "endpoint-subnets",
            &[],
        )
        .await,
    );

    // The endpoint name is what `DescribeEndpointAccess`, `DeleteEndpointAccess`
    // and `ModifyEndpointAccess` resolve, so an unresolvable one is
    // `EndpointNotFound` (404) whichever of them named it, and whatever its
    // shape.
    let unknown_endpoints: Vec<(&str, String)> = vec![
        ("a plain unknown endpoint name", "no-such-endpoint".to_string()),
        ("the shortest unknown endpoint name", name_of_length(1)),
        // "An endpoint name must contain 1-30 characters."
        ("the longest unknown endpoint name", name_of_length(30)),
        (
            "an unknown endpoint name in mixed case",
            "No-Such-Endpoint".to_string(),
        ),
        (
            "an endpoint name one character off a stored one",
            "present-endpoin".to_string(),
        ),
        (
            // Uppercase is valid in an endpoint name and nothing folds it, so
            // another spelling of a stored name names no endpoint.
            "a stored endpoint name in another case",
            "Present-Endpoint".to_string(),
        ),
    ];
    for (case, endpoint_name) in &unknown_endpoints {
        for action in [
            "DescribeEndpointAccess",
            "DeleteEndpointAccess",
            "ModifyEndpointAccess",
        ] {
            let answer = emulator
                .call(action, &[("EndpointName", endpoint_name.as_str())])
                .await;
            assert_refused(
                &format!("{action} with {case}"),
                &answer,
                404,
                "EndpointNotFound",
            );
        }
        // The endpoint is resolved BEFORE the cluster, so naming neither an
        // existing endpoint nor an existing cluster is refused on the endpoint.
        let both = emulator
            .call(
                "DescribeEndpointAccess",
                &[
                    ("EndpointName", endpoint_name.as_str()),
                    ("ClusterIdentifier", "no-such-cluster"),
                ],
            )
            .await;
        assert_refused(
            &format!("DescribeEndpointAccess with {case} and an unknown cluster"),
            &both,
            404,
            "EndpointNotFound",
        );
    }

    // The cluster identifier is what the authorization operations resolve, and
    // `DescribeEndpointAccess` resolves it too once the endpoint name resolves
    // (or is absent): an unresolvable one is `ClusterNotFound` (404).
    let unknown_clusters: Vec<(&str, String)> = vec![
        ("a plain unknown cluster", "no-such-cluster".to_string()),
        ("the shortest unknown cluster", name_of_length(1)),
        // "Must contain from 1 to 63 alphanumeric characters or hyphens."
        ("the longest unknown cluster", name_of_length(63)),
        (
            "a cluster one character off a stored one",
            "endpoint-hos".to_string(),
        ),
    ];
    for (case, cluster_identifier) in &unknown_clusters {
        let rows: Vec<(&str, Vec<(String, String)>)> = vec![
            (
                "DescribeEndpointAccess",
                vec![(
                    "ClusterIdentifier".to_string(),
                    cluster_identifier.clone(),
                )],
            ),
            (
                "DescribeEndpointAuthorization",
                vec![(
                    "ClusterIdentifier".to_string(),
                    cluster_identifier.clone(),
                )],
            ),
            (
                "AuthorizeEndpointAccess",
                vec![
                    ("ClusterIdentifier".to_string(), cluster_identifier.clone()),
                    ("Account".to_string(), OTHER_ACCOUNT.to_string()),
                ],
            ),
            (
                "RevokeEndpointAccess",
                vec![
                    ("ClusterIdentifier".to_string(), cluster_identifier.clone()),
                    ("Account".to_string(), OTHER_ACCOUNT.to_string()),
                ],
            ),
            (
                "CreateEndpointAccess",
                vec![
                    ("EndpointName".to_string(), "wanted-endpoint".to_string()),
                    ("ClusterIdentifier".to_string(), cluster_identifier.clone()),
                    // A subnet group that DOES resolve, so the cluster is what
                    // this row is about.
                    (
                        "SubnetGroupName".to_string(),
                        "endpoint-subnets".to_string(),
                    ),
                ],
            ),
        ];
        for (action, params) in rows {
            let answer = emulator.call(action, &borrowed(&params)).await;
            assert_refused(
                &format!("{action} with {case}"),
                &answer,
                404,
                "ClusterNotFound",
            );
        }
    }

    // `CreateEndpointAccess` resolves the SUBNET GROUP before the cluster, so a
    // request naming neither is refused on the subnet group — with that
    // resource's own fault and its own 400 status.
    let unknown_groups: Vec<(&str, String)> = vec![
        ("a plain unknown subnet group", "no-such-subnets".to_string()),
        ("the shortest unknown subnet group", name_of_length(1)),
        // "Must contain no more than 255 alphanumeric characters or hyphens."
        ("the longest unknown subnet group", name_of_length(255)),
        (
            "a subnet group one character off a stored one",
            "endpoint-subnet".to_string(),
        ),
    ];
    for (case, subnet_group) in &unknown_groups {
        let answer = create_endpoint(
            &emulator,
            "wanted-endpoint",
            "no-such-cluster",
            subnet_group,
            &[],
        )
        .await;
        assert_refused(
            &format!("CreateEndpointAccess with {case} and an unknown cluster"),
            &answer,
            400,
            "ClusterSubnetGroupNotFoundFault",
        );
        // And the refused create allocated nothing: the endpoint it named is
        // still absent.
        let described = emulator
            .call(
                "DescribeEndpointAccess",
                &[("EndpointName", "wanted-endpoint")],
            )
            .await;
        assert_refused(
            &format!("after CreateEndpointAccess with {case}"),
            &described,
            404,
            "EndpointNotFound",
        );
    }

    // The names are resolved BEFORE the request's own shape is judged, so a
    // request that is wrong in both ways is still answered with the modelled
    // fault of the name that resolves to nothing.
    let shape_and_name = create_endpoint(
        &emulator,
        // One character past the documented maximum endpoint-name length.
        &name_of_length(31),
        "no-such-cluster",
        "no-such-subnets",
        &[],
    )
    .await;
    assert_refused(
        "CreateEndpointAccess with an unresolvable subnet group and a malformed endpoint name",
        &shape_and_name,
        400,
        "ClusterSubnetGroupNotFoundFault",
    );
    let unresolvable_and_shape = emulator
        .call(
            "AuthorizeEndpointAccess",
            &[
                ("ClusterIdentifier", "no-such-cluster"),
                ("Account", "not-an-account"),
            ],
        )
        .await;
    assert_refused(
        "AuthorizeEndpointAccess with an unresolvable cluster and a malformed account",
        &unresolvable_and_shape,
        404,
        "ClusterNotFound",
    );

    // The same operations, with names that DO resolve, are served: the refusals
    // above are the preconditions, not a blanket refusal.
    let served = emulator
        .call(
            "DescribeEndpointAccess",
            &[
                ("EndpointName", "present-endpoint"),
                ("ClusterIdentifier", "endpoint-host"),
            ],
        )
        .await;
    assert_served("a stored endpoint and cluster are served", &served);
    assert_eq!(
        listed_endpoints(&served),
        vec!["present-endpoint".to_string()],
        "case a stored endpoint and cluster are served: expected exactly that endpoint, got body {}",
        served.body
    );
    assert_served(
        "a stored cluster takes an authorization",
        &authorize(&emulator, "endpoint-host", OTHER_ACCOUNT, &[]).await,
    );
    assert_served(
        "a stored cluster's authorizations are listed",
        &emulator
            .call(
                "DescribeEndpointAuthorization",
                &[("ClusterIdentifier", "endpoint-host")],
            )
            .await,
    );
}

/// What a created endpoint holds is exactly what every later read reports, over a
/// table spanning the subnet group it is deployed from, the number of security
/// groups the request names, and how many endpoints one cluster carries.
#[tokio::test]
async fn a_created_endpoint_is_reported_exactly_as_stored_and_round_trips() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "round-trip-host").await;
    let group = create_subnet_group(&emulator, "Round-Trip-Subnets", &["subnet-a", "subnet-b"]).await;
    assert_served("seed subnet group", &group);

    // The cluster's own connection port, which the endpoint reports rather than
    // choosing one.
    let cluster = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", "round-trip-host")])
        .await;
    assert_served("read the host cluster", &cluster);
    let cluster_port = cluster
        .text("Port")
        .expect("the available cluster publishes its endpoint port");

    // (case, endpoint name, subnet group as REQUESTED, the group's STORED name,
    //  security group ids)
    let rows: Vec<(&str, &str, &str, &str, Vec<&str>)> = vec![
        (
            "no security group named, so the VPC's default one",
            "rt-default-sg",
            "Round-Trip-Subnets",
            "round-trip-subnets",
            vec![],
        ),
        (
            "one named security group",
            "rt-one-sg",
            "round-trip-subnets",
            "round-trip-subnets",
            vec!["sg-11111111111111111"],
        ),
        (
            "several named security groups",
            "rt-many-sg",
            "ROUND-TRIP-SUBNETS",
            "round-trip-subnets",
            vec![
                "sg-11111111111111111",
                "sg-22222222222222222",
                "sg-33333333333333333",
            ],
        ),
        (
            // The account HAS a default subnet group before anybody creates one.
            "the account's default subnet group",
            "rt-default-group",
            state::DEFAULT_CLUSTER_SUBNET_GROUP,
            state::DEFAULT_CLUSTER_SUBNET_GROUP,
            vec![],
        ),
        (
            // Both documented length bounds of an endpoint name.
            "the shortest endpoint name",
            "r",
            "round-trip-subnets",
            "round-trip-subnets",
            vec![],
        ),
    ];

    let longest = name_of_length(30);
    let mut rows = rows;
    rows.push((
        "the longest endpoint name",
        longest.as_str(),
        "round-trip-subnets",
        "round-trip-subnets",
        vec![],
    ));

    let mut addresses: Vec<String> = Vec::new();
    for (case, endpoint_name, requested_group, stored_group, security_groups) in rows {
        let created = create_endpoint(
            &emulator,
            endpoint_name,
            "round-trip-host",
            requested_group,
            &security_groups,
        )
        .await;
        assert_served(case, &created);

        // A new endpoint is coming up, not serving: the transient state is
        // observable to a caller that acts straight after the create.
        assert_eq!(
            created.text("EndpointStatus").as_deref(),
            Some(state::ENDPOINT_STATUS_CREATING),
            "case {case}: expected EndpointStatus={}, got {:?}",
            state::ENDPOINT_STATUS_CREATING,
            created.text("EndpointStatus")
        );
        assert_eq!(
            created.text("SubnetGroupName").as_deref(),
            Some(stored_group),
            "case {case}: expected the subnet group's stored name {stored_group:?}, got {:?}",
            created.text("SubnetGroupName")
        );
        assert_eq!(
            created.text("Port").as_deref(),
            Some(cluster_port.as_str()),
            "case {case}: expected the cluster's own port {cluster_port:?}, got {:?}",
            created.text("Port")
        );
        assert_eq!(
            created.text("ResourceOwner").as_deref(),
            Some(engine::emulator_account_id()),
            "case {case}: expected the cluster owner {:?}, got {:?}",
            engine::emulator_account_id(),
            created.text("ResourceOwner")
        );
        assert_eq!(
            created.text("Address"),
            Some(state::endpoint_access_address(endpoint_name, REGION)),
            "case {case}: expected the endpoint's own derived address, got {:?}",
            created.text("Address")
        );
        let address = created.text("Address").unwrap_or_default();
        assert!(
            !addresses.contains(&address),
            "case {case}: expected an address of its own, got {address:?} again among {addresses:?}"
        );
        addresses.push(address);

        // The security groups: the ones named, or the VPC's default one when the
        // request named none.
        let served_groups = all_text(&created.body, "VpcSecurityGroupId");
        let expected_groups: Vec<String> = if security_groups.is_empty() {
            let vpc = created
                .text("VpcId")
                .expect("the endpoint reports the VPC its interface is in");
            vec![state::default_vpc_security_group_id(&vpc)]
        } else {
            security_groups.iter().map(|id| (*id).to_string()).collect()
        };
        assert_eq!(
            served_groups, expected_groups,
            "case {case}: expected the security groups {expected_groups:?}, got {served_groups:?}"
        );

        // The interface is in the subnet group's VPC, in one of that group's own
        // subnets.
        let group_subnets = emulator
            .call(
                "DescribeClusterSubnetGroups",
                &[("ClusterSubnetGroupName", stored_group)],
            )
            .await;
        assert_served(&format!("{case}: read the subnet group"), &group_subnets);
        assert_eq!(
            created.text("VpcId"),
            group_subnets.text("VpcId"),
            "case {case}: expected the subnet group's own VpcId {:?}, got {:?}",
            group_subnets.text("VpcId"),
            created.text("VpcId")
        );
        let group_subnet_ids = all_text(&group_subnets.body, "SubnetIdentifier");
        let endpoint_subnet = created
            .text("SubnetId")
            .expect("the endpoint's interface reports the subnet it was deployed into");
        assert!(
            group_subnet_ids.contains(&endpoint_subnet),
            "case {case}: expected a subnet of the group {group_subnet_ids:?}, got \
             {endpoint_subnet:?}"
        );

        // Read it straight back: every member comes back as the create reported
        // it, and the clock-owed transition to `active` has fallen due.
        tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
        let described = emulator
            .call("DescribeEndpointAccess", &[("EndpointName", endpoint_name)])
            .await;
        assert_served(&format!("{case}: read the endpoint back"), &described);
        assert_eq!(
            listed_endpoints(&described),
            vec![endpoint_name.to_string()],
            "case {case}: expected exactly that endpoint listed, got body {}",
            described.body
        );
        assert_eq!(
            described.text("EndpointStatus").as_deref(),
            Some(state::ENDPOINT_STATUS_ACTIVE),
            "case {case}: expected EndpointStatus={} once the span passed, got {:?}",
            state::ENDPOINT_STATUS_ACTIVE,
            described.text("EndpointStatus")
        );
        for member in [
            "ClusterIdentifier",
            "ResourceOwner",
            "SubnetGroupName",
            "Address",
            "Port",
            "VpcId",
            "EndpointCreateTime",
        ] {
            assert_eq!(
                described.text(member),
                created.text(member),
                "case {case} member {member}: expected {:?} persisted, got {:?}",
                created.text(member),
                described.text(member)
            );
        }
    }

    // Each locator narrows the set, and an unfiltered read reports every
    // endpoint the account holds, in creation order.
    let all = emulator.call("DescribeEndpointAccess", &[]).await;
    assert_served("an unfiltered read", &all);
    assert_eq!(
        listed_endpoints(&all),
        vec![
            "rt-default-sg".to_string(),
            "rt-one-sg".to_string(),
            "rt-many-sg".to_string(),
            "rt-default-group".to_string(),
            "r".to_string(),
            longest.clone(),
        ],
        "case an unfiltered read: expected every endpoint in creation order, got body {}",
        all.body
    );
    let by_owner = emulator
        .call(
            "DescribeEndpointAccess",
            &[("ResourceOwner", engine::emulator_account_id())],
        )
        .await;
    assert_served("a read by the cluster owner", &by_owner);
    assert_eq!(
        listed_endpoints(&by_owner),
        listed_endpoints(&all),
        "case a read by the cluster owner: expected every endpoint of this account, got body {}",
        by_owner.body
    );
    let by_other_owner = emulator
        .call("DescribeEndpointAccess", &[("ResourceOwner", OTHER_ACCOUNT)])
        .await;
    assert_served("a read by another owner", &by_other_owner);
    assert!(
        listed_endpoints(&by_other_owner).is_empty(),
        "case a read by another owner: expected no endpoint, got body {}",
        by_other_owner.body
    );
    // The VPC filter: the VPC the endpoints are in reports them, another does
    // not.
    let vpc = all
        .text("VpcId")
        .expect("a listed endpoint reports its VPC");
    let by_vpc = emulator
        .call("DescribeEndpointAccess", &[("VpcId", vpc.as_str())])
        .await;
    assert_served("a read by VPC", &by_vpc);
    assert!(
        !listed_endpoints(&by_vpc).is_empty()
            && listed_endpoints(&by_vpc).len() < listed_endpoints(&all).len(),
        "case a read by VPC: expected the endpoints of that VPC alone, got body {}",
        by_vpc.body
    );
    let by_absent_vpc = emulator
        .call("DescribeEndpointAccess", &[("VpcId", "vpc-00000000000000000")])
        .await;
    assert_served("a read by an unrelated VPC", &by_absent_vpc);
    assert!(
        listed_endpoints(&by_absent_vpc).is_empty(),
        "case a read by an unrelated VPC: expected no endpoint, got body {}",
        by_absent_vpc.body
    );

    // A second create under a stored name is the modelled already-exists
    // refusal, not a second record.
    let repeat = create_endpoint(
        &emulator,
        "rt-one-sg",
        "round-trip-host",
        "round-trip-subnets",
        &[],
    )
    .await;
    assert_refused(
        "a repeat create of a stored endpoint",
        &repeat,
        400,
        "EndpointAlreadyExists",
    );
}

/// An endpoint name outside the documented syntax is refused, and one inside it
/// is taken: the table spans each clause of the rule and both of its bounds.
#[tokio::test]
async fn an_endpoint_name_outside_the_documented_syntax_is_refused() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "syntax-host").await;

    // "An endpoint name must contain 1-30 characters. Valid characters are A-Z,
    // a-z, 0-9, and hyphen(-). The first character must be a letter. The name
    // can't contain two consecutive hyphens or end with a hyphen."
    let too_long = name_of_length(31);
    let longest = name_of_length(30);
    let rows: Vec<(&str, &str, bool)> = vec![
        ("one character past the documented maximum", too_long.as_str(), false),
        ("a leading digit", "9-endpoint", false),
        ("a character outside the set", "end_point", false),
        ("two consecutive hyphens", "end--point", false),
        ("a trailing hyphen", "endpoint-", false),
        ("the longest admitted name", longest.as_str(), true),
        ("the shortest admitted name", "e", true),
        ("uppercase, which the rule admits", "EndPoint-One", true),
        ("digits and hyphens", "ep-2024-07", true),
    ];

    for (case, endpoint_name, admitted) in rows {
        let answer = create_endpoint(
            &emulator,
            endpoint_name,
            "syntax-host",
            state::DEFAULT_CLUSTER_SUBNET_GROUP,
            &[],
        )
        .await;
        if admitted {
            assert_served(case, &answer);
            assert_eq!(
                answer.text("EndpointName").as_deref(),
                Some(endpoint_name),
                "case {case}: expected the name stored verbatim, got {:?}",
                answer.text("EndpointName")
            );
        } else {
            assert_refused(case, &answer, 400, "InvalidParameterValue");
            // Nothing was allocated: the name the refused create carried names no
            // endpoint.
            let described = emulator
                .call("DescribeEndpointAccess", &[("EndpointName", endpoint_name)])
                .await;
            assert_refused(
                &format!("after {case}"),
                &described,
                404,
                "EndpointNotFound",
            );
        }
    }
}

/// A delete reports the endpoint `deleting` and the record leaves the store once
/// the span has passed, so the name is absent from every later read — and a
/// delete or a modify of an endpoint that is not serving is the modelled state
/// fault.
#[tokio::test]
async fn a_deleted_endpoint_is_absent_from_every_later_read() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "delete-host").await;

    // The rows differ in the spelling and length of the name, so the rule is not
    // pinned to one.
    let longest = name_of_length(30);
    let rows: Vec<(&str, &str)> = vec![
        ("a plain name", "doomed-endpoint"),
        ("an uppercase name", "Doomed-Endpoint-Two"),
        ("the shortest name", "d"),
        ("the longest name", longest.as_str()),
    ];

    for (case, endpoint_name) in rows {
        let created = create_endpoint(
            &emulator,
            endpoint_name,
            "delete-host",
            state::DEFAULT_CLUSTER_SUBNET_GROUP,
            &[],
        )
        .await;
        assert_served(case, &created);

        // While it is still coming up it can take neither a delete nor a modify.
        for action in ["DeleteEndpointAccess", "ModifyEndpointAccess"] {
            let early = emulator
                .call(action, &[("EndpointName", endpoint_name)])
                .await;
            assert_refused(
                &format!("{action} on an endpoint that is still {case}"),
                &early,
                400,
                "InvalidEndpointState",
            );
        }

        tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
        let deleted = emulator
            .call("DeleteEndpointAccess", &[("EndpointName", endpoint_name)])
            .await;
        assert_served(&format!("{case}: the delete"), &deleted);
        assert_eq!(
            deleted.text("EndpointStatus").as_deref(),
            Some(state::ENDPOINT_STATUS_DELETING),
            "case {case}: expected EndpointStatus={} from the delete, got {:?}",
            state::ENDPOINT_STATUS_DELETING,
            deleted.text("EndpointStatus")
        );
        // The tear-down is observable: the endpoint is still reported, as
        // `deleting`, and a second delete is refused on its state.
        let during = emulator
            .call("DescribeEndpointAccess", &[("EndpointName", endpoint_name)])
            .await;
        assert_served(&format!("{case}: the read during the tear-down"), &during);
        assert_eq!(
            during.text("EndpointStatus").as_deref(),
            Some(state::ENDPOINT_STATUS_DELETING),
            "case {case}: expected the endpoint reported {} during the tear-down, got {:?}",
            state::ENDPOINT_STATUS_DELETING,
            during.text("EndpointStatus")
        );
        let again = emulator
            .call("DeleteEndpointAccess", &[("EndpointName", endpoint_name)])
            .await;
        assert_refused(&format!("{case}: a second delete"), &again, 400, "InvalidEndpointState");

        // Once the span has passed the record is gone from every read, and the
        // name is free again.
        tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
        let after = emulator
            .call("DescribeEndpointAccess", &[("EndpointName", endpoint_name)])
            .await;
        assert_refused(&format!("{case}: the read after"), &after, 404, "EndpointNotFound");
        let listed = emulator.call("DescribeEndpointAccess", &[]).await;
        assert!(
            !listed_endpoints(&listed).contains(&endpoint_name.to_string()),
            "case {case}: expected {endpoint_name:?} gone from the list, got body {}",
            listed.body
        );
        let remade = create_endpoint(
            &emulator,
            endpoint_name,
            "delete-host",
            state::DEFAULT_CLUSTER_SUBNET_GROUP,
            &[],
        )
        .await;
        assert_served(&format!("{case}: the name is free again"), &remade);
    }
}

/// `ModifyEndpointAccess` replaces the endpoint's complete security-group list
/// and leaves every other member of the record alone; a request that names no
/// list changes nothing.
#[tokio::test]
async fn a_modify_replaces_the_complete_security_group_list() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "modify-host").await;
    let created = create_endpoint(
        &emulator,
        "modified-endpoint",
        "modify-host",
        state::DEFAULT_CLUSTER_SUBNET_GROUP,
        &["sg-11111111111111111"],
    )
    .await;
    assert_served("seed endpoint", &created);
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;

    // The rows differ along the SIZE and the CONTENT of the replacement list,
    // including the empty request that names none.
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "one group for another",
            vec!["sg-22222222222222222"],
            vec!["sg-22222222222222222"],
        ),
        (
            "one group for several",
            vec!["sg-33333333333333333", "sg-44444444444444444"],
            vec!["sg-33333333333333333", "sg-44444444444444444"],
        ),
        (
            "several groups back to one",
            vec!["sg-55555555555555555"],
            vec!["sg-55555555555555555"],
        ),
        (
            "no list named, so the membership stands",
            vec![],
            vec!["sg-55555555555555555"],
        ),
    ];

    for (case, requested, expected) in rows {
        let mut params: Vec<(String, String)> = vec![(
            "EndpointName".to_string(),
            "modified-endpoint".to_string(),
        )];
        for (index, id) in requested.iter().enumerate() {
            params.push((
                format!("VpcSecurityGroupIds.VpcSecurityGroupId.{}", index + 1),
                (*id).to_string(),
            ));
        }
        let modified = emulator
            .call("ModifyEndpointAccess", &borrowed(&params))
            .await;
        assert_served(case, &modified);
        let expected: Vec<String> = expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            all_text(&modified.body, "VpcSecurityGroupId"),
            expected,
            "case {case}: expected the security groups {expected:?}, got body {}",
            modified.body
        );
        // Nothing else moved, and the change is what a later read reports.
        let described = emulator
            .call(
                "DescribeEndpointAccess",
                &[("EndpointName", "modified-endpoint")],
            )
            .await;
        assert_served(&format!("{case}: read back"), &described);
        assert_eq!(
            all_text(&described.body, "VpcSecurityGroupId"),
            expected,
            "case {case}: expected the change persisted, got body {}",
            described.body
        );
        for member in [
            "ClusterIdentifier",
            "SubnetGroupName",
            "Address",
            "Port",
            "EndpointCreateTime",
            "VpcId",
        ] {
            assert_eq!(
                described.text(member),
                created.text(member),
                "case {case} member {member}: expected {:?} untouched, got {:?}",
                created.text(member),
                described.text(member)
            );
        }
    }
}

/// An endpoint authorization is identified by its cluster-and-account pair: a
/// second grant for the same pair is refused, each grant is reported from the
/// point of view the read asks for, and a revoke withdraws exactly what it names.
#[tokio::test]
async fn authorizations_are_scoped_to_their_cluster_and_account() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "grant-host").await;
    available_cluster(&emulator, "grant-host-two").await;

    // The rows differ along the VPC scope of the grant (every VPC of the account,
    // one VPC, several) and along the account and cluster granted.
    let rows: Vec<(&str, &str, &str, Vec<&str>, bool)> = vec![
        (
            "every VPC of the grantee account",
            "grant-host",
            OTHER_ACCOUNT,
            vec![],
            true,
        ),
        (
            "one named VPC",
            "grant-host",
            "444455556666",
            vec!["vpc-11111111111111111"],
            false,
        ),
        (
            "several named VPCs",
            "grant-host",
            "777788889999",
            vec!["vpc-22222222222222222", "vpc-33333333333333333"],
            false,
        ),
        (
            "the same account on another cluster",
            "grant-host-two",
            OTHER_ACCOUNT,
            vec![],
            true,
        ),
    ];

    for (case, cluster, account, vpcs, allowed_all) in &rows {
        let granted = authorize(&emulator, cluster, account, vpcs).await;
        assert_served(case, &granted);
        assert_eq!(
            granted.text("Grantor").as_deref(),
            Some(engine::emulator_account_id()),
            "case {case}: expected the cluster owner as grantor, got {:?}",
            granted.text("Grantor")
        );
        assert_eq!(
            granted.text("Grantee").as_deref(),
            Some(*account),
            "case {case}: expected the grantee {account:?}, got {:?}",
            granted.text("Grantee")
        );
        assert_eq!(
            granted.text("Status").as_deref(),
            Some(state::AUTHORIZATION_STATUS_AUTHORIZED),
            "case {case}: expected Status={}, got {:?}",
            state::AUTHORIZATION_STATUS_AUTHORIZED,
            granted.text("Status")
        );
        assert_eq!(
            granted.text("AllowedAllVPCs").as_deref(),
            Some(if *allowed_all { "true" } else { "false" }),
            "case {case}: expected AllowedAllVPCs={allowed_all}, got {:?}",
            granted.text("AllowedAllVPCs")
        );
        assert_eq!(
            all_text(&granted.body, "VpcIdentifier"),
            vpcs.iter().map(|vpc| (*vpc).to_string()).collect::<Vec<String>>(),
            "case {case}: expected the VPCs the request named, got body {}",
            granted.body
        );
        // The cluster's status is read off the cluster, not stored on the grant.
        assert_eq!(
            granted.text("ClusterStatus").as_deref(),
            Some(state::STATUS_AVAILABLE),
            "case {case}: expected the cluster's own status, got {:?}",
            granted.text("ClusterStatus")
        );
        assert_eq!(
            granted.text("EndpointCount").as_deref(),
            Some("0"),
            "case {case}: expected no endpoint created for the grant yet, got {:?}",
            granted.text("EndpointCount")
        );

        // A second grant for the same pair is the modelled already-exists refusal.
        let repeat = authorize(&emulator, cluster, account, vpcs).await;
        assert_refused(
            &format!("a repeat grant of {case}"),
            &repeat,
            400,
            "EndpointAuthorizationAlreadyExists",
        );
    }

    // The grantor's point of view is the default, and each filter narrows it.
    let from_grantor = emulator.call("DescribeEndpointAuthorization", &[]).await;
    assert_served("a read from the grantor's point of view", &from_grantor);
    assert_eq!(
        all_text(&from_grantor.body, "Grantee"),
        vec![
            OTHER_ACCOUNT.to_string(),
            "444455556666".to_string(),
            "777788889999".to_string(),
            OTHER_ACCOUNT.to_string(),
        ],
        "case a read from the grantor's point of view: expected every grant in creation order, \
         got body {}",
        from_grantor.body
    );
    let by_cluster = emulator
        .call(
            "DescribeEndpointAuthorization",
            &[("ClusterIdentifier", "grant-host-two")],
        )
        .await;
    assert_served("a read filtered by cluster", &by_cluster);
    assert_eq!(
        all_text(&by_cluster.body, "Grantee"),
        vec![OTHER_ACCOUNT.to_string()],
        "case a read filtered by cluster: expected that cluster's one grant, got body {}",
        by_cluster.body
    );
    let by_account = emulator
        .call(
            "DescribeEndpointAuthorization",
            &[("Account", "444455556666")],
        )
        .await;
    assert_served("a read filtered by grantee account", &by_account);
    assert_eq!(
        all_text(&by_account.body, "Grantee"),
        vec!["444455556666".to_string()],
        "case a read filtered by grantee account: expected that account's grant, got body {}",
        by_account.body
    );
    // "If true, Amazon Redshift returns endpoint authorizations that you've been
    // granted": this account granted all of these, and was granted none.
    let as_grantee = emulator
        .call("DescribeEndpointAuthorization", &[("Grantee", "true")])
        .await;
    assert_served("a read from the grantee's point of view", &as_grantee);
    assert!(
        all_text(&as_grantee.body, "Grantee").is_empty(),
        "case a read from the grantee's point of view: expected no grant to this account, got \
         body {}",
        as_grantee.body
    );

    // A revoke naming VPCs withdraws exactly those, and the grant stands with
    // the rest.
    let partial = emulator
        .call(
            "RevokeEndpointAccess",
            &[
                ("ClusterIdentifier", "grant-host"),
                ("Account", "777788889999"),
                ("VpcIds.VpcIdentifier.1", "vpc-22222222222222222"),
            ],
        )
        .await;
    assert_served("a revoke of one of several VPCs", &partial);
    assert_eq!(
        partial.text("Status").as_deref(),
        Some(state::AUTHORIZATION_STATUS_AUTHORIZED),
        "case a revoke of one of several VPCs: expected the grant still {}, got {:?}",
        state::AUTHORIZATION_STATUS_AUTHORIZED,
        partial.text("Status")
    );
    assert_eq!(
        all_text(&partial.body, "VpcIdentifier"),
        vec!["vpc-33333333333333333".to_string()],
        "case a revoke of one of several VPCs: expected the remaining VPC, got body {}",
        partial.body
    );
    // A VPC the grant does not allow is nothing this call can withdraw, and
    // neither is a single VPC of a grant that covers every VPC.
    let unknown_vpc = emulator
        .call(
            "RevokeEndpointAccess",
            &[
                ("ClusterIdentifier", "grant-host"),
                ("Account", "777788889999"),
                ("VpcIds.VpcIdentifier.1", "vpc-22222222222222222"),
            ],
        )
        .await;
    assert_refused(
        "a revoke of a VPC the grant does not allow",
        &unknown_vpc,
        400,
        "InvalidAuthorizationState",
    );
    let all_vpcs = emulator
        .call(
            "RevokeEndpointAccess",
            &[
                ("ClusterIdentifier", "grant-host"),
                ("Account", OTHER_ACCOUNT),
                ("VpcIds.VpcIdentifier.1", "vpc-22222222222222222"),
            ],
        )
        .await;
    assert_refused(
        "a revoke of one VPC from an every-VPC grant",
        &all_vpcs,
        400,
        "InvalidAuthorizationState",
    );

    // A revoke naming no VPC withdraws the grant itself: it reports `Revoking`
    // and is gone from every read once the span has passed.
    let revoked = emulator
        .call(
            "RevokeEndpointAccess",
            &[
                ("ClusterIdentifier", "grant-host"),
                ("Account", "444455556666"),
            ],
        )
        .await;
    assert_served("a revoke of the whole grant", &revoked);
    assert_eq!(
        revoked.text("Status").as_deref(),
        Some(state::AUTHORIZATION_STATUS_REVOKING),
        "case a revoke of the whole grant: expected Status={}, got {:?}",
        state::AUTHORIZATION_STATUS_REVOKING,
        revoked.text("Status")
    );
    let second = emulator
        .call(
            "RevokeEndpointAccess",
            &[
                ("ClusterIdentifier", "grant-host"),
                ("Account", "444455556666"),
            ],
        )
        .await;
    assert_refused(
        "a second revoke of the same grant",
        &second,
        400,
        "InvalidAuthorizationState",
    );
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    let after = emulator
        .call(
            "DescribeEndpointAuthorization",
            &[("Account", "444455556666")],
        )
        .await;
    assert_served("a read after the grant was revoked", &after);
    assert!(
        all_text(&after.body, "Grantee").is_empty(),
        "case a read after the grant was revoked: expected the grant gone, got body {}",
        after.body
    );
    // A grant for an account that holds none is nothing to revoke.
    let absent = emulator
        .call(
            "RevokeEndpointAccess",
            &[
                ("ClusterIdentifier", "grant-host"),
                ("Account", "444455556666"),
            ],
        )
        .await;
    assert_refused(
        "a revoke of a grant that was never made",
        &absent,
        404,
        "EndpointAuthorizationNotFound",
    );
}

/// A grant that still admits Redshift-managed VPC endpoints can only be revoked
/// with `Force`, which deletes those endpoints with it — and the grant's
/// `EndpointCount` is read off the endpoints that stand.
#[tokio::test]
async fn a_grant_that_admits_endpoints_is_revoked_only_with_force() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "force-host").await;
    let granted = authorize(&emulator, "force-host", OTHER_ACCOUNT, &[]).await;
    assert_served("seed grant", &granted);

    // Two endpoints on the cluster, both in the VPC the grant admits.
    for endpoint_name in ["forced-one", "forced-two"] {
        let created = create_endpoint(
            &emulator,
            endpoint_name,
            "force-host",
            state::DEFAULT_CLUSTER_SUBNET_GROUP,
            &[],
        )
        .await;
        assert_served(&format!("seed endpoint {endpoint_name}"), &created);
    }
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;

    // The count is derived: it moved with the endpoints, without the grant being
    // touched.
    let described = emulator
        .call(
            "DescribeEndpointAuthorization",
            &[("ClusterIdentifier", "force-host")],
        )
        .await;
    assert_served("read the grant", &described);
    assert_eq!(
        described.text("EndpointCount").as_deref(),
        Some("2"),
        "case read the grant: expected EndpointCount=2, got {:?} with body {}",
        described.text("EndpointCount"),
        described.body
    );

    // Without `Force` the grant is left alone …
    let refused = emulator
        .call(
            "RevokeEndpointAccess",
            &[
                ("ClusterIdentifier", "force-host"),
                ("Account", OTHER_ACCOUNT),
            ],
        )
        .await;
    assert_refused(
        "a revoke of a grant that still admits endpoints",
        &refused,
        400,
        "InvalidAuthorizationState",
    );
    let still = emulator
        .call(
            "DescribeEndpointAuthorization",
            &[("ClusterIdentifier", "force-host")],
        )
        .await;
    assert_served("the grant after the refused revoke", &still);
    assert_eq!(
        still.text("Status").as_deref(),
        Some(state::AUTHORIZATION_STATUS_AUTHORIZED),
        "case the grant after the refused revoke: expected it still {}, got {:?}",
        state::AUTHORIZATION_STATUS_AUTHORIZED,
        still.text("Status")
    );
    assert_eq!(
        listed_endpoints(&emulator.call("DescribeEndpointAccess", &[]).await),
        vec!["forced-one".to_string(), "forced-two".to_string()],
        "case the endpoints after the refused revoke: expected both still there"
    );

    // … and with it, the endpoints go with the grant.
    let forced = emulator
        .call(
            "RevokeEndpointAccess",
            &[
                ("ClusterIdentifier", "force-host"),
                ("Account", OTHER_ACCOUNT),
                ("Force", "true"),
            ],
        )
        .await;
    assert_served("a forced revoke", &forced);
    assert_eq!(
        forced.text("Status").as_deref(),
        Some(state::AUTHORIZATION_STATUS_REVOKING),
        "case a forced revoke: expected Status={}, got {:?}",
        state::AUTHORIZATION_STATUS_REVOKING,
        forced.text("Status")
    );
    let during = emulator.call("DescribeEndpointAccess", &[]).await;
    assert_served("the endpoints during the forced tear-down", &during);
    assert_eq!(
        all_text(&during.body, "EndpointStatus"),
        vec![
            state::ENDPOINT_STATUS_DELETING.to_string(),
            state::ENDPOINT_STATUS_DELETING.to_string(),
        ],
        "case the endpoints during the forced tear-down: expected both {}, got body {}",
        state::ENDPOINT_STATUS_DELETING,
        during.body
    );
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    let after = emulator.call("DescribeEndpointAccess", &[]).await;
    assert_served("the endpoints after the forced tear-down", &after);
    assert!(
        listed_endpoints(&after).is_empty(),
        "case the endpoints after the forced tear-down: expected none left, got body {}",
        after.body
    );
}

/// `DescribeEndpointAccess` pages its report under the documented `MaxRecords`
/// window and the `Marker` cursor it hands out, over a store filled past one
/// page.
///
/// The records are seeded straight into the same shared store the handlers read,
/// because the window's own minimum is larger than a readable table of creates
/// and the paging is a property of the COLLECTION rather than of one create.
#[tokio::test]
async fn a_read_pages_under_the_documented_max_records_window() {
    let emulator = Emulator::new();
    let total = state::MAX_RECORDS_MIN as usize + 5;
    {
        let store = state::service_state(&*emulator.engine).expect("the Redshift store");
        let mut store = store.lock();
        for index in 0..total {
            let endpoint_name = format!("seeded-{index:04}");
            store
                .endpoint_accesses
                .push(state::StoredEndpointAccess {
                    endpoint: service_com_amazonaws_redshift_redshiftserviceversion20121201::ops::types::CreateEndpointAccessOutput {
                        endpoint_name: Some(endpoint_name),
                        cluster_identifier: Some("seeded-host".to_string()),
                        endpoint_status: Some(state::ENDPOINT_STATUS_ACTIVE.to_string()),
                        ..Default::default()
                    },
                    // Already past, so the record needs no clock-owed transition.
                    active_at: std::time::Instant::now(),
                    removed_at: None,
                });
        }
    }

    // (case, MaxRecords, the page size it must serve)
    let rows: Vec<(&str, Option<i32>, usize)> = vec![
        ("no MaxRecords, so the documented default", None, total),
        (
            "the documented minimum",
            Some(state::MAX_RECORDS_MIN),
            state::MAX_RECORDS_MIN as usize,
        ),
        (
            "the documented maximum",
            Some(state::MAX_RECORDS_MAX),
            total,
        ),
    ];
    for (case, max_records, expected) in rows {
        let value = max_records.map(|value| value.to_string());
        let params: Vec<(&str, &str)> = value
            .as_deref()
            .map(|value| vec![("MaxRecords", value)])
            .unwrap_or_default();
        let answer = emulator.call("DescribeEndpointAccess", &params).await;
        assert_served(case, &answer);
        assert_eq!(
            listed_endpoints(&answer).len(),
            expected,
            "case {case}: expected {expected} records on the page, got body {}",
            answer.body
        );
    }

    // One past each published bound is refused, and the cursor walks the whole
    // collection exactly once.
    for (case, max_records) in [
        ("one below the published minimum", state::MAX_RECORDS_MIN - 1),
        ("one above the published maximum", state::MAX_RECORDS_MAX + 1),
    ] {
        let value = max_records.to_string();
        let answer = emulator
            .call("DescribeEndpointAccess", &[("MaxRecords", &value)])
            .await;
        assert_refused(case, &answer, 400, "ValidationException");
    }

    let page_size = state::MAX_RECORDS_MIN.to_string();
    let first = emulator
        .call("DescribeEndpointAccess", &[("MaxRecords", &page_size)])
        .await;
    assert_served("the first page", &first);
    let marker = first
        .text("Marker")
        .expect("a page with records after it hands out a Marker");
    let second = emulator
        .call(
            "DescribeEndpointAccess",
            &[("MaxRecords", &page_size), ("Marker", &marker)],
        )
        .await;
    assert_served("the second page", &second);
    assert!(
        second.text("Marker").is_none(),
        "case the second page: expected no Marker after the last record, got body {}",
        second.body
    );
    let mut walked = listed_endpoints(&first);
    walked.extend(listed_endpoints(&second));
    let expected: Vec<String> = (0..total).map(|index| format!("seeded-{index:04}")).collect();
    assert_eq!(
        walked, expected,
        "case the cursor walk: expected every seeded endpoint exactly once in order"
    );

    // A cursor this service did not hand out is refused rather than served as a
    // mid-page offset.
    let forged = emulator
        .call("DescribeEndpointAccess", &[("Marker", "7.deadbeef")])
        .await;
    assert_refused("a forged Marker", &forged, 400, "ValidationException");
}
