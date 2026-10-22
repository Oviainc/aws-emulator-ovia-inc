// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the subnet-group and cluster-parameter-group paths, and for the
//! two rules `CreateDBCluster` applies to what a request NAMES.
//!
//! Every test drives the service over its real wire protocol (awsQuery form bodies in, XML
//! out) through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production router uses, with the wire harness this target shares
//! (`tests/all/cluster_lifecycle.rs`): a create is always read back through a describe, and
//! a delete is always followed by the describe that must no longer find the resource, so a
//! response that is not computed from persisted state cannot pass.
//!
//! The rules asserted here, each over a TABLE spanning the class it generalises over:
//!
//! * a describe or delete of a named resource that state does not hold answers that
//!   resource's own modelled not-found fault, never an empty page and never a success;
//! * a create that NAMES another resource (`DBSubnetGroupName`,
//!   `DBClusterParameterGroupName`) is refused with that resource's modelled not-found
//!   fault unless the resource exists, and nothing is stored;
//! * an `EngineVersion` the engine does not offer is refused with awsQuery's documented
//!   `InvalidParameterCombination`, and one it does offer is stored resolved to a published
//!   version (`crate::state::DOCDB_ENGINE_VERSIONS`).

use service_com_amazonaws_docdb_amazonrdsv19::state;

use crate::cluster_lifecycle::{
    count, error_code, let_the_clock_catch_up, opt_text, text, texts, Docdb, REGION,
};

/// The two subnets a well-formed group needs; see
/// `state::MIN_SUBNETS_PER_SUBNET_GROUP` for the rule and why the count is what this
/// emulator can hold it to.
const SUBNET_A: &str = "subnet-0aaaaaaaaaaaaaaa1";
const SUBNET_B: &str = "subnet-0bbbbbbbbbbbbbbb2";
const SUBNET_C: &str = "subnet-0ccccccccccccccc3";

/// Create a well-formed subnet group named `name` over `subnets`, requiring a 200.
async fn create_group(docdb: &Docdb, case: &str, name: &str, subnets: &[&str]) -> String {
    let mut params: Vec<(&str, &str)> = vec![
        ("DBSubnetGroupName", name),
        ("DBSubnetGroupDescription", "subnets for the test cluster"),
    ];
    // The awsQuery list form uses the list member's `xmlName`
    // (`SubnetIdentifierList$member` -> `SubnetIdentifier`), so the subnets arrive as
    // `SubnetIds.SubnetIdentifier.1=...`.
    let members: Vec<String> = (1..=subnets.len())
        .map(|index| format!("SubnetIds.SubnetIdentifier.{index}"))
        .collect();
    for (member, subnet) in members.iter().zip(subnets) {
        params.push((member.as_str(), subnet));
    }
    docdb.ok(case, "CreateDBSubnetGroup", &params).await
}

// ---------------------------------------------------------------------------
// The subnet-group lifecycle, end to end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_subnet_group_lifecycle_is_computed_from_persisted_state() {
    let docdb = Docdb::new();

    // The rows differ along what the rule generalises over: how many subnets the group
    // carries, the case of its name (stored lowercased), and its description.
    let rows: [(&str, &str, &[&str]); 3] = [
        ("two subnets", "test-subnets", &[SUBNET_A, SUBNET_B]),
        ("three subnets", "wider-subnets", &[SUBNET_A, SUBNET_B, SUBNET_C]),
        ("mixed-case name", "Mixed.Case_Name", &[SUBNET_B, SUBNET_C]),
    ];

    for (case, name, subnets) in rows {
        let created = create_group(&docdb, case, name, subnets).await;
        let stored_name = name.to_ascii_lowercase();
        assert_eq!(
            text(&created, "DBSubnetGroupName"),
            stored_name,
            "case {case:?}: `DBSubnetGroupName` is stored as a lowercase string, got {created}"
        );
        assert_eq!(
            text(&created, "DBSubnetGroupArn"),
            format!("arn:aws:rds:{REGION}:000000000000:subgrp:{stored_name}"),
            "case {case:?}: expected the regional subnet-group ARN, got {created}"
        );
        assert_eq!(
            texts(&created, "SubnetIdentifier"),
            subnets
                .iter()
                .map(|subnet| (*subnet).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the subnets the request named, got {created}"
        );

        // Read the create back through the describe: same name, same subnets, same ARN.
        let described = docdb
            .ok(
                case,
                "DescribeDBSubnetGroups",
                &[("DBSubnetGroupName", name)],
            )
            .await;
        assert_eq!(
            count(&described, "DBSubnetGroup"),
            1,
            "case {case:?}: a named describe must report exactly that group, got {described}"
        );
        assert_eq!(
            text(&described, "DBSubnetGroupName"),
            stored_name,
            "case {case:?}: expected the stored group, got {described}"
        );
        assert_eq!(
            texts(&described, "SubnetIdentifier").len(),
            subnets.len(),
            "case {case:?}: expected {} subnets read back, got {described}",
            subnets.len()
        );
        assert_eq!(
            text(&described, "DBSubnetGroupDescription"),
            "subnets for the test cluster",
            "case {case:?}: expected the stored description, got {described}"
        );
    }

    // The unnamed describe reports every stored group, in one page.
    let all = docdb.ok("list all", "DescribeDBSubnetGroups", &[]).await;
    assert_eq!(
        count(&all, "DBSubnetGroup"),
        rows.len(),
        "case \"list all\": expected every created group, got {all}"
    );
    assert_eq!(
        opt_text(&all, "Marker"),
        None,
        "case \"list all\": a single page carries no Marker, got {all}"
    );

    // Deleting a group removes it: the delete answers 200 and the describe that named it
    // now answers the modelled 404, which is what a waiter polls for.
    for (case, name, _) in rows {
        let deleted = docdb
            .ok(case, "DeleteDBSubnetGroup", &[("DBSubnetGroupName", name)])
            .await;
        assert!(
            deleted.contains("DeleteDBSubnetGroupResponse"),
            "case {case:?}: expected the DeleteDBSubnetGroupResponse envelope, got {deleted}"
        );

        let gone = docdb
            .call("DescribeDBSubnetGroups", &[("DBSubnetGroupName", name)])
            .await;
        assert_eq!(
            (gone.status, error_code(&gone.xml).as_str()),
            (404, "DBSubnetGroupNotFoundFault"),
            "case {case:?}: a deleted group must report the modelled not-found fault, got {} \
             with {}",
            gone.status,
            gone.xml
        );
    }

    let empty = docdb
        .ok("list after deletes", "DescribeDBSubnetGroups", &[])
        .await;
    assert_eq!(
        count(&empty, "DBSubnetGroup"),
        0,
        "case \"list after deletes\": every group was deleted, got {empty}"
    );
}

/// A named resource state does not hold answers that resource's own modelled not-found
/// fault — on the read, on the delete, and whatever shape the name takes.
#[tokio::test]
async fn a_subnet_group_that_does_not_exist_is_the_modelled_not_found_fault() {
    let docdb = Docdb::new();
    create_group(&docdb, "seed", "present-group", &[SUBNET_A, SUBNET_B]).await;

    // (case, the name the request gives) — spanning a plain name, a name that differs only
    // in case from nothing stored, an ARN form, and the reserved `default` group this
    // emulator seeds no record for.
    let missing: [(&str, &str); 4] = [
        ("a name never created", "no-such-group"),
        ("a near miss of a stored name", "present-group-2"),
        (
            "an ARN of a group that does not exist",
            "arn:aws:rds:us-east-1:000000000000:subgrp:no-such-group",
        ),
        ("the reserved default group", "default"),
    ];

    for (case, name) in missing {
        for action in ["DescribeDBSubnetGroups", "DeleteDBSubnetGroup"] {
            let reply = docdb.call(action, &[("DBSubnetGroupName", name)]).await;
            assert_eq!(
                (reply.status, error_code(&reply.xml).as_str()),
                (404, "DBSubnetGroupNotFoundFault"),
                "case {case:?} on {action}: expected 404 DBSubnetGroupNotFoundFault, got {} \
                 with {}",
                reply.status,
                reply.xml
            );
        }
    }

    // The group that DOES exist is untouched by every refusal above.
    let still_there = docdb
        .ok(
            "the stored group survives",
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName", "present-group")],
        )
        .await;
    assert_eq!(
        text(&still_there, "DBSubnetGroupName"),
        "present-group",
        "case \"the stored group survives\": expected the stored group, got {still_there}"
    );
}

/// `CreateDBSubnetGroup`'s own documented refusals, each with the modelled fault.
#[tokio::test]
async fn creating_a_subnet_group_holds_its_documented_rules() {
    let docdb = Docdb::new();
    create_group(&docdb, "seed", "taken-name", &[SUBNET_A, SUBNET_B]).await;

    // (case, name, subnets, expected status, expected wire code)
    let rows: [(&str, &str, &[&str], u16, &str); 5] = [
        (
            // "Must not be default" — the model states it on the member itself, and the
            // model declares no error shape for it, so it is the boundary refusal.
            "the reserved name",
            "default",
            &[SUBNET_A, SUBNET_B],
            400,
            "ValidationException",
        ),
        (
            "a name outside the documented character set",
            "bad/name",
            &[SUBNET_A, SUBNET_B],
            400,
            "ValidationException",
        ),
        (
            // "subnet groups must contain at least one subnet in at least two
            // Availability Zones".
            "one subnet",
            "too-narrow",
            &[SUBNET_A],
            400,
            "DBSubnetGroupDoesNotCoverEnoughAZs",
        ),
        (
            // The same subnet twice is one subnet, so it cannot cover two zones either.
            "the same subnet twice",
            "still-too-narrow",
            &[SUBNET_A, SUBNET_A],
            400,
            "DBSubnetGroupDoesNotCoverEnoughAZs",
        ),
        (
            "a name already in use",
            "taken-name",
            &[SUBNET_A, SUBNET_B],
            400,
            // The model's `awsQueryError` code for `DBSubnetGroupAlreadyExistsFault`.
            "DBSubnetGroupAlreadyExists",
        ),
    ];

    for (case, name, subnets, expected_status, expected_code) in rows {
        let mut params: Vec<(&str, &str)> = vec![
            ("DBSubnetGroupName", name),
            ("DBSubnetGroupDescription", "a group the service refuses"),
        ];
        let members: Vec<String> = (1..=subnets.len())
            .map(|index| format!("SubnetIds.SubnetIdentifier.{index}"))
            .collect();
        for (member, subnet) in members.iter().zip(subnets) {
            params.push((member.as_str(), subnet));
        }
        let reply = docdb.call("CreateDBSubnetGroup", &params).await;
        assert_eq!(
            (reply.status, error_code(&reply.xml).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {} with {}",
            reply.status,
            reply.xml
        );
    }

    // Nothing a refusal named reached the store: only the seeded group is there.
    let all = docdb
        .ok("only the seeded group is stored", "DescribeDBSubnetGroups", &[])
        .await;
    assert_eq!(
        texts(&all, "DBSubnetGroupName"),
        vec!["taken-name".to_string()],
        "case \"only the seeded group is stored\": a refused create must store nothing, got \
         {all}"
    );
}

/// The two published quotas this store enforces, each with the modelled fault: more subnets
/// in one group than the per-group quota, and more groups than the per-Region quota.
#[tokio::test]
async fn the_subnet_group_quotas_refuse_a_store_that_would_grow_past_them() {
    let docdb = Docdb::new();

    // `state::MAX_SUBNETS_PER_SUBNET_GROUP` subnets is the published bound and is accepted;
    // one past it is the modelled fault. The ids are generated so the row follows the
    // constant rather than a literal count.
    let within: Vec<String> = (0..state::MAX_SUBNETS_PER_SUBNET_GROUP)
        .map(|index| format!("subnet-{index:017x}"))
        .collect();
    let past: Vec<String> = (0..=state::MAX_SUBNETS_PER_SUBNET_GROUP)
        .map(|index| format!("subnet-{index:017x}"))
        .collect();

    for (case, subnets, expected) in [
        ("at the published per-group bound", &within, None),
        (
            "one subnet past the per-group bound",
            &past,
            // This fault carries no `awsQueryError` override, so the wire code is the
            // shape name.
            Some("DBSubnetQuotaExceededFault"),
        ),
    ] {
        let name = format!("quota-{}", subnets.len());
        let borrowed: Vec<&str> = subnets.iter().map(String::as_str).collect();
        let mut params: Vec<(&str, &str)> = vec![
            ("DBSubnetGroupName", name.as_str()),
            ("DBSubnetGroupDescription", "a group at the quota"),
        ];
        let members: Vec<String> = (1..=borrowed.len())
            .map(|index| format!("SubnetIds.SubnetIdentifier.{index}"))
            .collect();
        for (member, subnet) in members.iter().zip(&borrowed) {
            params.push((member.as_str(), subnet));
        }
        let reply = docdb.call("CreateDBSubnetGroup", &params).await;
        match expected {
            None => assert_eq!(
                reply.status, 200,
                "case {case:?}: the published bound must be accepted, got {} with {}",
                reply.status, reply.xml
            ),
            Some(code) => assert_eq!(
                (reply.status, error_code(&reply.xml).as_str()),
                (400, code),
                "case {case:?}: expected 400 {code}, got {} with {}",
                reply.status,
                reply.xml
            ),
        }
    }

    // The per-Region quota: the store fills to `state::MAX_DB_SUBNET_GROUPS` and the next
    // create is the modelled fault rather than a stored record.
    let seeded = count(
        &docdb
            .ok("count the stored groups", "DescribeDBSubnetGroups", &[])
            .await,
        "DBSubnetGroup",
    );
    for index in seeded..state::MAX_DB_SUBNET_GROUPS {
        let name = format!("filler-{index}");
        create_group(&docdb, "fill the Region", &name, &[SUBNET_A, SUBNET_B]).await;
    }
    let over = docdb
        .call(
            "CreateDBSubnetGroup",
            &[
                ("DBSubnetGroupName", "one-too-many"),
                ("DBSubnetGroupDescription", "past the Region quota"),
                ("SubnetIds.SubnetIdentifier.1", SUBNET_A),
                ("SubnetIds.SubnetIdentifier.2", SUBNET_B),
            ],
        )
        .await;
    assert_eq!(
        (over.status, error_code(&over.xml).as_str()),
        // The model's `awsQueryError` code for `DBSubnetGroupQuotaExceededFault`.
        (400, "DBSubnetGroupQuotaExceeded"),
        "case \"one group past the Region quota\": expected 400 DBSubnetGroupQuotaExceeded, \
         got {} with {}",
        over.status,
        over.xml
    );
    let stored = count(
        &docdb
            .ok("the store stops at the quota", "DescribeDBSubnetGroups", &[])
            .await,
        "DBSubnetGroup",
    );
    assert_eq!(
        stored,
        state::MAX_DB_SUBNET_GROUPS,
        "case \"the store stops at the quota\": expected the store to hold exactly the \
         quota, got {stored}"
    );
}

/// A subnet group a cluster is placed in is in use: the delete is refused with the modelled
/// state fault and the record stays, until the cluster is gone.
///
/// The cluster names the group by its ARN, which the member accepts: the record must keep
/// the RESOLVED name, or the reference is invisible to the in-use rule and the cluster
/// reports a name the group is not stored under.
#[tokio::test]
async fn a_subnet_group_in_use_by_a_cluster_cannot_be_deleted() {
    let docdb = Docdb::new();
    let created = create_group(&docdb, "seed", "in-use-group", &[SUBNET_A, SUBNET_B]).await;
    let group_arn = text(&created, "DBSubnetGroupArn");
    let placed = docdb
        .ok(
            "place a cluster in the group, naming it by ARN",
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "placed-cluster"),
                ("Engine", state::DOCDB_ENGINE),
                ("MasterUsername", "admin"),
                ("MasterUserPassword", "secret99"),
                ("DBSubnetGroupName", &group_arn),
            ],
        )
        .await;
    assert_eq!(
        text(&placed, "DBSubnetGroup"),
        "in-use-group",
        "case \"place a cluster in the group, naming it by ARN\": the cluster must report the \
         group's stored name, not the form the request used, got {placed}"
    );

    let refused = docdb
        .call(
            "DeleteDBSubnetGroup",
            &[("DBSubnetGroupName", "in-use-group")],
        )
        .await;
    assert_eq!(
        (refused.status, error_code(&refused.xml).as_str()),
        (400, "InvalidDBSubnetGroupStateFault"),
        "case \"in use\": expected 400 InvalidDBSubnetGroupStateFault, got {} with {}",
        refused.status,
        refused.xml
    );

    let still_there = docdb
        .ok(
            "the group survives the refusal",
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName", "in-use-group")],
        )
        .await;
    assert_eq!(
        text(&still_there, "DBSubnetGroupName"),
        "in-use-group",
        "case \"the group survives the refusal\": expected the stored group, got {still_there}"
    );

    // With the cluster gone the same delete succeeds, so the refusal really was the
    // reference and not a blanket refusal.
    let_the_clock_catch_up();
    docdb
        .ok(
            "drop the cluster",
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "placed-cluster"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await;
    docdb
        .ok(
            "delete the freed group",
            "DeleteDBSubnetGroup",
            &[("DBSubnetGroupName", "in-use-group")],
        )
        .await;
}

// ---------------------------------------------------------------------------
// What a create NAMES, and the engine-version catalogue
// ---------------------------------------------------------------------------

/// `CreateDBCluster` names two other resources by name. Each must exist, or the create is
/// refused with THAT resource's modelled not-found fault and nothing is stored.
#[tokio::test]
async fn creating_a_cluster_requires_every_resource_it_names_to_exist() {
    let docdb = Docdb::new();
    create_group(&docdb, "seed", "cluster-subnets", &[SUBNET_A, SUBNET_B]).await;
    // The default cluster parameter group of the engine's newest family, which exists in
    // every account and Region; see `state::default_parameter_groups`.
    let default_group = state::default_parameter_group_name(&state::parameter_group_family(
        state::DEFAULT_ENGINE_VERSION,
    ));

    // One row of the refusal table: the case, the extra members the create carries, and
    // the status and wire code the reference it names must be refused with.
    type Refusal = (&'static str, Vec<(&'static str, String)>, u16, &'static str);

    // (case, extra members, expected status, expected wire code)
    let refusals: [Refusal; 4] = [
        (
            "a subnet group that does not exist",
            vec![("DBSubnetGroupName", "no-such-group".to_string())],
            404,
            "DBSubnetGroupNotFoundFault",
        ),
        (
            "a subnet group named by an ARN that does not exist",
            vec![(
                "DBSubnetGroupName",
                "arn:aws:rds:us-east-1:000000000000:subgrp:no-such-group".to_string(),
            )],
            404,
            "DBSubnetGroupNotFoundFault",
        ),
        (
            "a cluster parameter group that does not exist",
            vec![(
                "DBClusterParameterGroupName",
                "no-such-parameter-group".to_string(),
            )],
            404,
            // The model's `awsQueryError` code for
            // `DBClusterParameterGroupNotFoundFault`.
            "DBClusterParameterGroupNotFound",
        ),
        (
            "an existing subnet group and a parameter group that does not exist",
            vec![
                ("DBSubnetGroupName", "cluster-subnets".to_string()),
                ("DBClusterParameterGroupName", "default.docdb9.9".to_string()),
            ],
            404,
            "DBClusterParameterGroupNotFound",
        ),
    ];

    for (case, extra, expected_status, expected_code) in refusals {
        let mut params: Vec<(&str, &str)> = vec![
            ("DBClusterIdentifier", "refused-cluster"),
            ("Engine", state::DOCDB_ENGINE),
            ("MasterUsername", "admin"),
            ("MasterUserPassword", "secret99"),
        ];
        for (member, value) in &extra {
            params.push((member, value.as_str()));
        }
        let reply = docdb.call("CreateDBCluster", &params).await;
        assert_eq!(
            (reply.status, error_code(&reply.xml).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {} with {}",
            reply.status,
            reply.xml
        );

        let stored = docdb
            .call(
                "DescribeDBClusters",
                &[("DBClusterIdentifier", "refused-cluster")],
            )
            .await;
        assert_eq!(
            (stored.status, error_code(&stored.xml).as_str()),
            (404, "DBClusterNotFoundFault"),
            "case {case:?}: a refused create must store nothing, got {} with {}",
            stored.status,
            stored.xml
        );
    }

    // Both references resolved: the create succeeds and reports what it was given.
    let created = docdb
        .ok(
            "both references exist",
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "placed-cluster"),
                ("Engine", state::DOCDB_ENGINE),
                ("MasterUsername", "admin"),
                ("MasterUserPassword", "secret99"),
                ("DBSubnetGroupName", "cluster-subnets"),
                ("DBClusterParameterGroupName", &default_group),
            ],
        )
        .await;
    assert_eq!(
        text(&created, "DBSubnetGroup"),
        "cluster-subnets",
        "case \"both references exist\": expected the subnet group it was placed in, got \
         {created}"
    );
    assert_eq!(
        text(&created, "DBClusterParameterGroup"),
        default_group,
        "case \"both references exist\": expected the parameter group it was given, got \
         {created}"
    );
}

/// The engine-version rule: a version the engine offers is accepted and stored resolved to
/// a published version; any other version is refused with awsQuery's documented
/// `InvalidParameterCombination`.
#[tokio::test]
async fn an_engine_version_outside_the_catalogue_is_refused() {
    let docdb = Docdb::new();

    // Every published version, named in full and as a line, plus the absent case. The
    // catalogue is `state::DOCDB_ENGINE_VERSIONS`; the rows below are built from it so a
    // version added there is covered here too, and `4.0.0` — a version this issue did not
    // cite — is among them.
    for (index, published) in state::DOCDB_ENGINE_VERSIONS.iter().enumerate() {
        let line = published
            .rsplit_once('.')
            .map(|(line, _)| line.to_string())
            .expect("every published version carries a major.minor line");
        for (shape, requested) in [("in full", (*published).to_string()), ("as a line", line)] {
            let case = format!("{published} {shape}");
            let identifier = format!("version-{index}-{}", shape.replace(' ', "-"));
            let created = docdb
                .ok(
                    &case,
                    "CreateDBCluster",
                    &[
                        ("DBClusterIdentifier", &identifier),
                        ("Engine", state::DOCDB_ENGINE),
                        ("MasterUsername", "admin"),
                        ("MasterUserPassword", "secret99"),
                        ("EngineVersion", &requested),
                    ],
                )
                .await;
            assert_eq!(
                text(&created, "EngineVersion"),
                *published,
                "case {case:?}: a version named as a line resolves to the published version, \
                 got {created}"
            );
            // And it is the STORED version, not just the create's answer.
            let described = docdb
                .ok(
                    &case,
                    "DescribeDBClusters",
                    &[("DBClusterIdentifier", &identifier)],
                )
                .await;
            assert_eq!(
                text(&described, "EngineVersion"),
                *published,
                "case {case:?}: expected the stored version, got {described}"
            );
        }
    }

    let defaulted = docdb
        .ok(
            "no version at all",
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "version-default"),
                ("Engine", state::DOCDB_ENGINE),
                ("MasterUsername", "admin"),
                ("MasterUserPassword", "secret99"),
            ],
        )
        .await;
    assert_eq!(
        text(&defaulted, "EngineVersion"),
        state::DEFAULT_ENGINE_VERSION,
        "case \"no version at all\": expected the documented default, got {defaulted}"
    );

    // (case, the version the request names) — one past each published bound, a line that
    // does not exist, a patch that does not exist, and values that are not versions.
    let refused: [(&str, &str); 6] = [
        ("far past the newest major", "99.9.9"),
        ("one past the newest major", "6.0.0"),
        ("one before the oldest major", "2.6.0"),
        ("a minor line the engine does not offer", "5.1"),
        ("a patch the engine does not publish", "5.0.9"),
        ("not a version at all", "latest"),
    ];

    for (case, version) in refused {
        let reply = docdb
            .call(
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", "refused-version"),
                    ("Engine", state::DOCDB_ENGINE),
                    ("MasterUsername", "admin"),
                    ("MasterUserPassword", "secret99"),
                    ("EngineVersion", version),
                ],
            )
            .await;
        assert_eq!(
            (reply.status, error_code(&reply.xml).as_str()),
            (400, "InvalidParameterCombination"),
            "case {case:?}: expected 400 InvalidParameterCombination, got {} with {}",
            reply.status,
            reply.xml
        );
        assert!(
            reply.xml.contains(version),
            "case {case:?}: the message is built from the request, so it must name the \
             version asked for, got {}",
            reply.xml
        );

        let stored = docdb
            .call(
                "DescribeDBClusters",
                &[("DBClusterIdentifier", "refused-version")],
            )
            .await;
        assert_eq!(
            (stored.status, error_code(&stored.xml).as_str()),
            (404, "DBClusterNotFoundFault"),
            "case {case:?}: a refused create must store nothing, got {} with {}",
            stored.status,
            stored.xml
        );
    }

    // The same rule on the modify path, against a cluster that exists.
    let_the_clock_catch_up();
    let modified = docdb
        .call(
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "version-default"),
                ("EngineVersion", "99.9.9"),
                ("AllowMajorVersionUpgrade", "true"),
            ],
        )
        .await;
    assert_eq!(
        (modified.status, error_code(&modified.xml).as_str()),
        (400, "InvalidParameterCombination"),
        "case \"modify to a version outside the catalogue\": expected 400 \
         InvalidParameterCombination, got {} with {}",
        modified.status,
        modified.xml
    );
}

// ---------------------------------------------------------------------------
// Cluster parameter groups
// ---------------------------------------------------------------------------

/// The published engine-version catalogue is ordered oldest first, which is what makes the
/// documented default ("the latest major engine version") the last entry, and what
/// `resolve_engine_version` relies on to answer the newest version in a named line.
#[test]
fn the_engine_version_catalogue_is_ordered_oldest_first() {
    let mut sorted = state::DOCDB_ENGINE_VERSIONS;
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        state::DOCDB_ENGINE_VERSIONS,
        "case \"catalogue order\": the published versions must be listed oldest first, got \
         {:?}",
        state::DOCDB_ENGINE_VERSIONS
    );
    assert_eq!(
        Some(state::DEFAULT_ENGINE_VERSION),
        state::DOCDB_ENGINE_VERSIONS.last().copied(),
        "case \"catalogue order\": the documented default is the latest published version, \
         got {:?}",
        state::DEFAULT_ENGINE_VERSION
    );
}

/// Which parameter-group names and families THIS model may be asked about, which is what
/// the shared `rds` front door reads to decide whether to serve a parameter-group request
/// from it rather than from the RDS model's own handler.
#[test]
fn the_model_answers_only_for_its_own_parameter_group_families() {
    // (case, value, served by this model)
    let names: [(&str, &str, bool); 6] = [
        ("a default group of this engine", "default.docdb5.0", true),
        ("the oldest published family", "default.docdb3.6", true),
        ("a family this engine does not have", "default.docdb9.9", false),
        ("another engine's family", "default.aurora-mysql8.0", false),
        ("a caller-created group", "team-cluster-params", true),
        ("a name that merely starts like a default", "defaults.docdb5.0", true),
    ];
    for (case, name, served) in names {
        assert_eq!(
            state::serves_parameter_group_name(name),
            served,
            "case {case:?} ({name}): expected served == {served}"
        );
    }

    let families: [(&str, &str, bool); 4] = [
        ("this engine's newest family", "docdb5.0", true),
        ("this engine's oldest family", "docdb3.6", true),
        ("a family this engine does not have", "docdb9.9", false),
        ("another engine's family", "aurora-mysql8.0", false),
    ];
    for (case, family, served) in families {
        assert_eq!(
            state::serves_parameter_group_family(family),
            served,
            "case {case:?} ({family}): expected served == {served}"
        );
    }
}

/// `DescribeDBClusterParameterGroups` reports the default group of every engine-version
/// family, and a name that matches none of them is the modelled 404 — the answer a waiter
/// polls a deleted group for.
#[tokio::test]
async fn cluster_parameter_groups_report_the_engine_families_and_404_anything_else() {
    let docdb = Docdb::new();

    let all = docdb
        .ok("list all", "DescribeDBClusterParameterGroups", &[])
        .await;
    // One default group per engine-version family AWS publishes for this engine, named
    // literally rather than through the same function the handler answers from, and sorted
    // as the describe reports them.
    let expected = vec![
        "default.docdb3.6".to_string(),
        "default.docdb4.0".to_string(),
        "default.docdb5.0".to_string(),
    ];
    assert_eq!(
        texts(&all, "DBClusterParameterGroupName"),
        expected,
        "case \"list all\": expected one default group per engine-version family, got {all}"
    );

    // Each one reads back on its own, with the family and ARN the name implies.
    for name in &expected {
        let described = docdb
            .ok(
                name,
                "DescribeDBClusterParameterGroups",
                &[("DBClusterParameterGroupName", name)],
            )
            .await;
        assert_eq!(
            count(&described, "DBClusterParameterGroup"),
            1,
            "case {name:?}: a named describe must report exactly that group, got {described}"
        );
        let family = text(&described, "DBParameterGroupFamily");
        assert_eq!(
            text(&described, "DBClusterParameterGroupName"),
            state::default_parameter_group_name(&family),
            "case {name:?}: the default group of a family is named after it, got {described}"
        );
        assert_eq!(
            text(&described, "DBClusterParameterGroupArn"),
            format!("arn:aws:rds:{REGION}:000000000000:cluster-pg:{name}"),
            "case {name:?}: expected the regional parameter-group ARN, got {described}"
        );
    }

    // (case, the name the request gives) — a name never created, a near miss of a default
    // group's name, a family that does not exist, and an engine that is not this one.
    let missing: [(&str, &str); 4] = [
        ("a name never created", "no-such-parameter-group"),
        ("a near miss of a default group", "default.docdb5"),
        ("a family the engine does not have", "default.docdb9.9"),
        ("another engine's family", "default.aurora5.6"),
    ];

    for (case, name) in missing {
        let reply = docdb
            .call(
                "DescribeDBClusterParameterGroups",
                &[("DBClusterParameterGroupName", name)],
            )
            .await;
        assert_eq!(
            (reply.status, error_code(&reply.xml).as_str()),
            (404, "DBParameterGroupNotFound"),
            "case {case:?}: expected 404 DBParameterGroupNotFound (the model's awsQueryError \
             code for DBParameterGroupNotFoundFault), got {} with {}",
            reply.status,
            reply.xml
        );
    }
}
