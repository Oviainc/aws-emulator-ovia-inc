// SPDX-License-Identifier: CC0-1.0

//! Existence preconditions across the Redshift control plane, end to end over
//! the awsQuery wire.
//!
//! The RULE every case here asserts: an operation that names another
//! control-plane resource by name resolves that name against this service's
//! store, and when it resolves to nothing answers that resource's own modelled
//! NotFound fault with the status the model gives it — before any record is
//! written. A resource that was deleted resolves to nothing like one that never
//! existed, so a delete is observable through every later read of its name.
//!
//! The suite drives real HTTP requests through the same `MultiServiceRouter` the
//! production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize. Every table spans
//! what the rule generalises over — which operation, which referenced resource,
//! and the name's case, length and spelling — rather than one recorded request.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// A name of exactly `len` characters that satisfies every other part of the
/// documented identifier syntax: a leading letter and then alphanumerics.
fn name_of_length(len: usize) -> String {
    std::iter::once('g')
        .chain(std::iter::repeat_n('x', len - 1))
        .collect()
}

/// `call` takes borrowed pairs, so a case that builds owned parameters borrows
/// them through this.
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// One row of a precondition table: the case name, the request parameters, and
/// the `(status, error code)` the service must answer with.
type PreconditionCase = (String, Vec<(String, String)>, u16, &'static str);

/// Create one cluster subnet group holding `subnet_ids`.
async fn create_subnet_group(emulator: &Emulator, name: &str, subnet_ids: &[&str]) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        ("ClusterSubnetGroupName".to_string(), name.to_string()),
        (
            "Description".to_string(),
            format!("subnets for {name}"),
        ),
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

/// Create one cluster parameter group of the engine version's family.
async fn create_parameter_group(emulator: &Emulator, name: &str) -> Answer {
    let family = state::parameter_group_family(state::DEFAULT_CLUSTER_VERSION);
    emulator
        .call(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", name),
                ("ParameterGroupFamily", &family),
                ("Description", "a group a precondition case names"),
            ],
        )
        .await
}

/// `CreateCluster` with the three required members plus `extra`.
async fn create_cluster(
    emulator: &Emulator,
    identifier: &str,
    extra: &[(&str, &str)],
) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![
        ("ClusterIdentifier", identifier),
        ("NodeType", "ra3.xlplus"),
        ("MasterUsername", "admin"),
    ];
    params.extend_from_slice(extra);
    emulator.call("CreateCluster", &params).await
}

/// Assert one precondition row: the answer's status and `<Code>`.
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

/// A `CreateCluster` naming a subnet group no record holds is refused with the
/// modelled fault, and leaves NO cluster behind: the identifier it named is
/// still absent, which a later read reports as `ClusterNotFound`.
#[tokio::test]
async fn a_create_naming_an_absent_subnet_group_is_refused_and_allocates_nothing() {
    let emulator = Emulator::new();
    // One group that DOES exist, so the table's refusals cannot be an "every
    // name is refused" artefact.
    let created = create_subnet_group(&emulator, "present-subnets", &["subnet-a"]).await;
    assert_eq!(
        created.status, 200,
        "case seed present-subnets: expected 200, got {} with body {}",
        created.status, created.body
    );

    // The rows differ along what the rule generalises over: the spelling, the
    // length (both documented bounds) and the case of the name that resolves to
    // nothing — and the last row names a group that exists, which must be taken.
    let rows: Vec<(&str, String, bool)> = vec![
        ("a plain unknown name", "no-such-subnet-group".to_string(), false),
        ("the shortest unknown name", name_of_length(1), false),
        ("the longest unknown name", name_of_length(255), false),
        (
            "an unknown name in mixed case",
            "No-Such-Subnet-Group".to_string(),
            false,
        ),
        (
            "a name one character off a stored one",
            "present-subnet".to_string(),
            false,
        ),
        ("a stored name", "present-subnets".to_string(), true),
        (
            "a stored name in mixed case",
            "Present-Subnets".to_string(),
            true,
        ),
    ];

    for (index, (case, subnet_group, admitted)) in rows.into_iter().enumerate() {
        let identifier = format!("precondition-cluster-{index}");
        let answer = create_cluster(
            &emulator,
            &identifier,
            &[("ClusterSubnetGroupName", &subnet_group)],
        )
        .await;
        let described = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", &identifier)])
            .await;

        if admitted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the create admitted with 200, got {} with body {}",
                answer.status, answer.body
            );
            // The cluster reports the group's STORED name and the VPC that group
            // holds, so the two records cannot disagree.
            assert_eq!(
                answer.text("ClusterSubnetGroupName").as_deref(),
                Some("present-subnets"),
                "case {case}: expected the stored subnet group name, got {:?}",
                answer.text("ClusterSubnetGroupName")
            );
            assert_eq!(
                answer.text("VpcId"),
                created.text("VpcId"),
                "case {case}: expected the subnet group's own VpcId {:?}, got {:?}",
                created.text("VpcId"),
                answer.text("VpcId")
            );
            assert_eq!(
                described.status, 200,
                "case {case}: expected the admitted cluster readable, got {} with body {}",
                described.status, described.body
            );
        } else {
            assert_refused(case, &answer, 400, "ClusterSubnetGroupNotFoundFault");
            // Nothing was allocated: the identifier the refused create named is
            // still absent.
            assert_refused(case, &described, 404, "ClusterNotFound");
        }
    }
}

/// Every operation that names a referenced resource resolves it the same way, so
/// the table spans the OPERATIONS and the referenced resources rather than one
/// call: an absent name is that resource's own modelled fault, whichever
/// operation named it.
#[tokio::test]
async fn an_absent_reference_is_that_resources_own_fault_in_every_operation() {
    let emulator = Emulator::new();
    let created = create_subnet_group(&emulator, "shared-subnets", &["subnet-a", "subnet-b"]).await;
    assert_eq!(
        created.status, 200,
        "case seed shared-subnets: expected 200, got {} with body {}",
        created.status, created.body
    );

    let rows: Vec<PreconditionCase> = vec![
        (
            "CreateCluster names an absent subnet group".to_string(),
            vec![
                ("Action".to_string(), "CreateCluster".to_string()),
                ("ClusterIdentifier".to_string(), "absent-sng".to_string()),
                ("NodeType".to_string(), "ra3.xlplus".to_string()),
                ("MasterUsername".to_string(), "admin".to_string()),
                (
                    "ClusterSubnetGroupName".to_string(),
                    "nowhere-subnets".to_string(),
                ),
            ],
            400,
            "ClusterSubnetGroupNotFoundFault",
        ),
        (
            "CreateCluster names an absent parameter group".to_string(),
            vec![
                ("Action".to_string(), "CreateCluster".to_string()),
                ("ClusterIdentifier".to_string(), "absent-cpg".to_string()),
                ("NodeType".to_string(), "ra3.xlplus".to_string()),
                ("MasterUsername".to_string(), "admin".to_string()),
                (
                    "ClusterParameterGroupName".to_string(),
                    "nowhere-group".to_string(),
                ),
            ],
            404,
            "ClusterParameterGroupNotFound",
        ),
        (
            "DescribeClusters names an absent cluster".to_string(),
            vec![
                ("Action".to_string(), "DescribeClusters".to_string()),
                (
                    "ClusterIdentifier".to_string(),
                    "nowhere-cluster".to_string(),
                ),
            ],
            404,
            "ClusterNotFound",
        ),
        (
            "DescribeClusterSubnetGroups names an absent subnet group".to_string(),
            vec![
                (
                    "Action".to_string(),
                    "DescribeClusterSubnetGroups".to_string(),
                ),
                (
                    "ClusterSubnetGroupName".to_string(),
                    "nowhere-subnets".to_string(),
                ),
            ],
            400,
            "ClusterSubnetGroupNotFoundFault",
        ),
        (
            "DescribeClusterParameterGroups names an absent parameter group".to_string(),
            vec![
                (
                    "Action".to_string(),
                    "DescribeClusterParameterGroups".to_string(),
                ),
                (
                    "ParameterGroupName".to_string(),
                    "nowhere-group".to_string(),
                ),
            ],
            404,
            "ClusterParameterGroupNotFound",
        ),
        (
            "DeleteClusterParameterGroup names an absent parameter group".to_string(),
            vec![
                (
                    "Action".to_string(),
                    "DeleteClusterParameterGroup".to_string(),
                ),
                (
                    "ParameterGroupName".to_string(),
                    "nowhere-group".to_string(),
                ),
            ],
            404,
            "ClusterParameterGroupNotFound",
        ),
    ];

    for (case, params, status, code) in rows {
        let mut params = params;
        let action = params.remove(0).1;
        let answer = emulator.call(&action, &borrowed(&params)).await;
        assert_refused(&case, &answer, status, code);
    }

    // The same reads, with a name that DOES resolve, are served: the refusals
    // above are the precondition, not a blanket refusal.
    let listed = emulator
        .call(
            "DescribeClusterSubnetGroups",
            &[("ClusterSubnetGroupName", "shared-subnets")],
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case a stored subnet group is served: expected 200, got {} with body {}",
        listed.status, listed.body
    );
    assert_eq!(
        all_text(&listed.body, "ClusterSubnetGroupName"),
        vec!["shared-subnets".to_string()],
        "case a stored subnet group is served: expected exactly that group, got body {}",
        listed.body
    );
}

/// What a created subnet group holds is exactly what a later read reports, for a
/// table spanning the name's case and the number of subnets (both documented
/// bounds of `SubnetIds`).
#[tokio::test]
async fn a_created_subnet_group_is_reported_exactly_as_stored() {
    let emulator = Emulator::new();
    // The documented maximum: "A maximum of 20 subnets can be modified in a
    // single request" (`CreateClusterSubnetGroupMessage$SubnetIds`).
    let many: Vec<String> = (0..state::SUBNETS_PER_SUBNET_GROUP_QUOTA)
        .map(|index| format!("subnet-{index:04}"))
        .collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();

    // (case, requested name, subnet ids, the name the group must be STORED under)
    let rows: Vec<(&str, &str, Vec<&str>, &str)> = vec![
        ("one subnet", "one-subnet-group", vec!["subnet-a"], "one-subnet-group"),
        (
            "several subnets",
            "three-subnet-group",
            vec!["subnet-a", "subnet-b", "subnet-c"],
            "three-subnet-group",
        ),
        (
            "the documented maximum number of subnets",
            "max-subnet-group",
            many.clone(),
            "max-subnet-group",
        ),
        (
            // "Amazon Redshift stores the value as a lowercase string."
            "mixed case is folded",
            "Mixed-Case-Group",
            vec!["subnet-a"],
            "mixed-case-group",
        ),
    ];

    for (case, requested, subnet_ids, stored) in rows {
        let created = create_subnet_group(&emulator, requested, &subnet_ids).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("ClusterSubnetGroupName").as_deref(),
            Some(stored),
            "case {case}: expected the stored name {stored:?}, got {:?}",
            created.text("ClusterSubnetGroupName")
        );
        assert_eq!(
            created.text("SubnetGroupStatus").as_deref(),
            Some(state::SUBNET_GROUP_STATUS_COMPLETE),
            "case {case}: expected SubnetGroupStatus={}, got {:?}",
            state::SUBNET_GROUP_STATUS_COMPLETE,
            created.text("SubnetGroupStatus")
        );

        // Read it straight back by its stored name: every member comes back as
        // the create reported it.
        let described = emulator
            .call(
                "DescribeClusterSubnetGroups",
                &[("ClusterSubnetGroupName", stored)],
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected the group readable, got {} with body {}",
            described.status, described.body
        );
        assert_eq!(
            all_text(&described.body, "SubnetIdentifier"),
            subnet_ids
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<String>>(),
            "case {case}: expected the subnets in request order, got body {}",
            described.body
        );
        for member in ["Description", "VpcId", "SubnetGroupStatus"] {
            assert_eq!(
                described.text(member),
                created.text(member),
                "case {case} member {member}: expected {:?} persisted, got {:?}",
                created.text(member),
                described.text(member)
            );
        }
        // Every subnet reports an Availability Zone of the Region the request
        // was signed for. The report also carries each zone's supported
        // platforms, whose own element is `<Name>` too, so the names are
        // partitioned rather than counted blindly.
        let zones = state::region_availability_zones("us-west-2");
        let names = all_text(&described.body, "Name");
        let (zone_names, platform_names): (Vec<String>, Vec<String>) =
            names.into_iter().partition(|name| zones.contains(name));
        assert_eq!(
            zone_names.len(),
            subnet_ids.len(),
            "case {case}: expected one Availability Zone of us-west-2 per subnet, got \
             {zone_names:?} beside {platform_names:?}"
        );
        for platform in &platform_names {
            assert_eq!(
                platform, state::PLATFORM_VPC,
                "case {case}: expected every other <Name> to be a supported platform, got \
                 {platform:?}"
            );
        }
    }

    // A second create under the stored name is the group's own "already exists"
    // fault, not a second record.
    let repeat = create_subnet_group(&emulator, "ONE-SUBNET-GROUP", &["subnet-a"]).await;
    assert_refused(
        "a repeat create of a stored subnet group",
        &repeat,
        400,
        "ClusterSubnetGroupAlreadyExists",
    );
}

/// A deleted parameter group resolves to nothing: the by-name read, the
/// unfiltered list and a second delete all report it absent. The table spans the
/// name's case and length, so the rule is not pinned to one spelling.
#[tokio::test]
async fn a_deleted_parameter_group_is_absent_from_every_later_read() {
    let emulator = Emulator::new();
    // (case, requested name, the name it is stored under)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a plain name",
            "doomed-group".to_string(),
            "doomed-group".to_string(),
        ),
        (
            "mixed case, folded on the way in and out",
            "Doomed-Group-Two".to_string(),
            "doomed-group-two".to_string(),
        ),
        (
            "the shortest documented name",
            name_of_length(1),
            name_of_length(1),
        ),
        (
            "the longest documented name",
            name_of_length(255),
            name_of_length(255),
        ),
    ];

    for (case, requested, stored) in rows {
        let created = create_parameter_group(&emulator, &requested).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected the group created, got {} with body {}",
            created.status, created.body
        );
        let described = emulator
            .call(
                "DescribeClusterParameterGroups",
                &[("ParameterGroupName", &stored)],
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected the group readable before the delete, got {} with body {}",
            described.status, described.body
        );

        let deleted = emulator
            .call(
                "DeleteClusterParameterGroup",
                &[("ParameterGroupName", &requested)],
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: expected the delete to succeed, got {} with body {}",
            deleted.status, deleted.body
        );

        // The by-name read now answers the modelled fault …
        let after = emulator
            .call(
                "DescribeClusterParameterGroups",
                &[("ParameterGroupName", &stored)],
            )
            .await;
        assert_refused(case, &after, 404, "ClusterParameterGroupNotFound");
        // … a second delete does too …
        let again = emulator
            .call(
                "DeleteClusterParameterGroup",
                &[("ParameterGroupName", &stored)],
            )
            .await;
        assert_refused(case, &again, 404, "ClusterParameterGroupNotFound");
        // … and the unfiltered list no longer carries it, while the default
        // group every account has is still there.
        let listed = emulator.call("DescribeClusterParameterGroups", &[]).await;
        let names = all_text(&listed.body, "ParameterGroupName");
        assert!(
            !names.contains(&stored),
            "case {case}: expected {stored:?} gone from the list, got {names:?}"
        );
        assert!(
            names.contains(&state::default_parameter_group(state::DEFAULT_CLUSTER_VERSION)),
            "case {case}: expected the default group still listed, got {names:?}"
        );
    }
}

/// A parameter group the account cannot lose is refused and the group stays
/// readable — each with the refusal its own condition earns: a default group
/// Redshift owns is the service's, not the account's, so the value the request
/// named is refused (`InvalidParameterValue`), while a group a cluster is
/// configured from is refused on the association's state
/// (`InvalidClusterParameterGroupState`).
#[tokio::test]
async fn a_parameter_group_nothing_can_lose_is_refused_and_stays() {
    let emulator = Emulator::new();
    let default_group = state::default_parameter_group(state::DEFAULT_CLUSTER_VERSION);
    let attached_group = "attached-group";

    let created = create_parameter_group(&emulator, attached_group).await;
    assert_eq!(
        created.status, 200,
        "case seed {attached_group}: expected 200, got {} with body {}",
        created.status, created.body
    );
    let cluster = create_cluster(
        &emulator,
        "group-holder",
        &[("ClusterParameterGroupName", attached_group)],
    )
    .await;
    assert_eq!(
        cluster.status, 200,
        "case seed group-holder: expected 200, got {} with body {}",
        cluster.status, cluster.body
    );

    for (case, group, code) in [
        (
            "a default parameter group",
            default_group.as_str(),
            "InvalidParameterValue",
        ),
        (
            "a group a cluster is configured from",
            attached_group,
            "InvalidClusterParameterGroupState",
        ),
    ] {
        let refused = emulator
            .call(
                "DeleteClusterParameterGroup",
                &[("ParameterGroupName", group)],
            )
            .await;
        assert_refused(case, &refused, 400, code);
        let described = emulator
            .call(
                "DescribeClusterParameterGroups",
                &[("ParameterGroupName", group)],
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected the group still readable, got {} with body {}",
            described.status, described.body
        );
    }

    // Once the cluster that held it is gone, the group is the account's to
    // delete: the refusal above is the association, not the group.
    let deleted_cluster = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "group-holder"),
                ("SkipFinalClusterSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        deleted_cluster.status, 200,
        "case release the cluster: expected 200, got {} with body {}",
        deleted_cluster.status, deleted_cluster.body
    );
    let deleted = emulator
        .call(
            "DeleteClusterParameterGroup",
            &[("ParameterGroupName", attached_group)],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the released group: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );
}
