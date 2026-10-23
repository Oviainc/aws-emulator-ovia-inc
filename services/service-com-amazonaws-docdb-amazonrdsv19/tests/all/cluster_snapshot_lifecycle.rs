// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon DocumentDB cluster-snapshot operations.
//!
//! The rule these pin: EVERY cluster-snapshot operation that names a snapshot answers the
//! modelled `DBClusterSnapshotNotFoundFault` — HTTP 404, `text/xml`, an awsQuery
//! `<ErrorResponse>` whose `Error/Code` is `DBClusterSnapshotNotFoundFault` — when the
//! identifier it was given names no snapshot in state, and computes everything else from
//! the snapshots that ARE in state. It is asserted over a TABLE spanning the five
//! operations and the identifier forms each accepts, with the store both empty and
//! holding other snapshots, so a 404 can never come from "the store is empty".
//!
//! Every call goes over the real wire protocol (awsQuery form bodies in, XML out) through
//! the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) the
//! production router uses, over the shared harness in
//! [`crate::cluster_lifecycle`]: a snapshot is always read back through a describe, and a
//! delete is always followed by the describe that must no longer find it.

use crate::cluster_lifecycle::{
    count, error_code, let_the_clock_catch_up, opt_text, text, texts, Docdb, REGION,
};

use service_com_amazonaws_docdb_amazonrdsv19::state;

/// The ARN form a cluster snapshot carries, as `DbClusterSnapshotRecord::arn` builds it.
fn snapshot_arn(identifier: &str) -> String {
    format!("arn:aws:rds:{REGION}:000000000000:cluster-snapshot:{identifier}")
}

/// One row of [`SNAPSHOT_OPERATIONS`]: the `Action`, the member that names the snapshot
/// it works on, and the other members the model requires.
type SnapshotOperation = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

/// Every operation that names a cluster snapshot, as `(action, the member that names the
/// snapshot, the other members the model requires)`.
///
/// The naming member and the extra required members come from the model
/// (`smithy_query(query="operation", operation="CopyDBClusterSnapshot,…")`):
/// `CopyDBClusterSnapshot` names its source in `SourceDBClusterSnapshotIdentifier` and
/// also requires `TargetDBClusterSnapshotIdentifier`, and
/// `ModifyDBClusterSnapshotAttribute` also requires `AttributeName`.
const SNAPSHOT_OPERATIONS: [SnapshotOperation; 5] = [
    ("DescribeDBClusterSnapshots", "DBClusterSnapshotIdentifier", &[]),
    ("DeleteDBClusterSnapshot", "DBClusterSnapshotIdentifier", &[]),
    (
        "CopyDBClusterSnapshot",
        "SourceDBClusterSnapshotIdentifier",
        &[("TargetDBClusterSnapshotIdentifier", "the-copy")],
    ),
    (
        "DescribeDBClusterSnapshotAttributes",
        "DBClusterSnapshotIdentifier",
        &[],
    ),
    (
        "ModifyDBClusterSnapshotAttribute",
        "DBClusterSnapshotIdentifier",
        &[("AttributeName", "restore")],
    ),
];

/// Create `cluster` and take a final snapshot of it called `snapshot`, which is the one
/// path that writes a cluster snapshot without using a snapshot operation — so the
/// snapshot operations are always read against state something else put there.
async fn snapshot_a_new_cluster(docdb: &Docdb, cluster: &str, snapshot: &str) {
    docdb
        .ok(
            cluster,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", cluster),
                ("Engine", "docdb"),
                ("MasterUsername", "admin"),
                ("MasterUserPassword", "secret99"),
            ],
        )
        .await;
    // A cluster AWS returns `creating` can only be deleted once the clock has made it
    // `available`; the emulator never sleeps for that, so the test lets it pass.
    let_the_clock_catch_up();
    docdb
        .ok(
            cluster,
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", cluster),
                ("SkipFinalSnapshot", "false"),
                ("FinalDBSnapshotIdentifier", snapshot),
            ],
        )
        .await;
    // The final snapshot is taken in the `creating` status and the clock owes it
    // `available`; the tests that work with a finished snapshot let that window pass, and
    // the one that asserts the transient status reads it back before calling this.
    let_the_clock_catch_up();
}

/// An identifier that names no stored snapshot is the modelled 404 on EVERY operation
/// that names one — whatever form the identifier takes, and whether or not other
/// snapshots exist.
#[tokio::test]
async fn every_snapshot_operation_refuses_an_identifier_that_names_no_snapshot() {
    // `DeleteDBClusterMessage$FinalDBSnapshotIdentifier` bounds a snapshot identifier at
    // 255 characters; an identifier at that published bound still names nothing.
    let longest = format!("s{}", "x".repeat(state::MAX_SNAPSHOT_IDENTIFIER_LEN - 1));

    // (case, the identifier to ask for)
    let missing: [(&str, &str); 5] = [
        ("a plain name", "no-such-snapshot"),
        // Identifiers are stored lowercased, so case cannot turn a miss into a hit.
        ("the same name upper-cased", "NO-SUCH-SNAPSHOT"),
        ("one character past a stored name", "alpha-snapshotx"),
        ("an ARN form", "arn:aws:rds:us-east-1:000000000000:cluster-snapshot:no-such-snapshot"),
        ("the longest allowed identifier", longest.as_str()),
    ];

    // Every request id the refusals carried: an awsQuery error envelope names ONE request,
    // so no two of them may be the same.
    let mut request_ids = std::collections::BTreeSet::new();

    // Both store states: nothing stored at all, and two snapshots stored that simply are
    // not the one asked for.
    for seeded in [false, true] {
        let docdb = Docdb::new();
        if seeded {
            snapshot_a_new_cluster(&docdb, "alpha", "alpha-snapshot").await;
            snapshot_a_new_cluster(&docdb, "beta", "beta-snapshot").await;
        }
        for (action, naming_member, extra) in SNAPSHOT_OPERATIONS {
            for (form, identifier) in missing {
                let case = format!("{action} with {form} (store seeded: {seeded})");
                let mut params = vec![(naming_member, identifier)];
                params.extend_from_slice(extra);
                let reply = docdb.call(action, &params).await;
                assert_eq!(
                    reply.status, 404,
                    "case {case:?}: expected the modelled DBClusterSnapshotNotFoundFault's \
                     404, got {} with {}",
                    reply.status, reply.xml
                );
                assert!(
                    reply.content_type.starts_with("text/xml"),
                    "case {case:?}: an awsQuery error is text/xml, got {:?} with {}",
                    reply.content_type,
                    reply.xml
                );
                assert_eq!(
                    error_code(&reply.xml),
                    "DBClusterSnapshotNotFoundFault",
                    "case {case:?}: got {}",
                    reply.xml
                );
                assert_eq!(
                    text(&reply.xml, "Type"),
                    "Sender",
                    "case {case:?}: a client error is a Sender fault, got {}",
                    reply.xml
                );
                assert!(
                    text(&reply.xml, "Message").contains(identifier),
                    "case {case:?}: the message must name the identifier the request \
                     asked for, got {}",
                    reply.xml
                );
                // The envelope element an SDK reads back as the error's request id, the
                // counterpart of a successful response's `ResponseMetadata/RequestId`.
                let request_id = text(&reply.xml, "RequestId");
                assert_eq!(
                    request_id.split('-').map(str::len).collect::<Vec<_>>(),
                    vec![8, 4, 4, 4, 12],
                    "case {case:?}: expected an 8-4-4-4-12 request id, got {}",
                    reply.xml
                );
                assert!(
                    request_ids.insert(request_id.clone()),
                    "case {case:?}: every response names its own request, but \
                     {request_id:?} was already seen"
                );
            }
        }
    }
}

/// `CopyDBClusterSnapshot` names the region it looked for a missing source in: the one
/// the source ARN names, or the one the request was signed for.
#[tokio::test]
async fn a_copy_reports_the_region_it_looked_for_a_missing_source_in() {
    let docdb = Docdb::new();

    // (case, the source the request names, the region the message must name)
    let rows: [(&str, &str, &str); 4] = [
        // A plain identifier is looked for in the request's own region.
        ("a plain identifier", "no-such-snapshot", REGION),
        (
            "an ARN in another region",
            "arn:aws:rds:us-west-2:000000000000:cluster-snapshot:no-such-snapshot",
            "us-west-2",
        ),
        (
            "an ARN in a third region",
            "arn:aws:rds:eu-central-1:000000000000:cluster-snapshot:no-such-snapshot",
            "eu-central-1",
        ),
        (
            "an ARN in the request's own region",
            "arn:aws:rds:us-east-1:000000000000:cluster-snapshot:no-such-snapshot",
            REGION,
        ),
    ];

    for (case, source, expected_region) in rows {
        let reply = docdb
            .call(
                "CopyDBClusterSnapshot",
                &[
                    ("SourceDBClusterSnapshotIdentifier", source),
                    ("TargetDBClusterSnapshotIdentifier", "the-copy"),
                ],
            )
            .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404, got {} with {}",
            reply.status, reply.xml
        );
        assert_eq!(
            text(&reply.xml, "Message"),
            format!("Requested snapshot {source} not found in source region {expected_region}"),
            "case {case:?}: got {}",
            reply.xml
        );
    }
}

/// A snapshot the clock has not finished yet is reported in its transient status, so a
/// caller that reads one straight back sees what AWS shows it.
#[tokio::test]
async fn a_final_snapshot_is_reported_creating_until_the_clock_makes_it_available() {
    let docdb = Docdb::new();
    docdb
        .ok(
            "create",
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "alpha"),
                ("Engine", "docdb"),
                ("MasterUsername", "admin"),
                ("MasterUserPassword", "secret99"),
            ],
        )
        .await;
    let_the_clock_catch_up();
    docdb
        .ok(
            "delete with a final snapshot",
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "alpha"),
                ("SkipFinalSnapshot", "false"),
                ("FinalDBSnapshotIdentifier", "alpha-snapshot"),
            ],
        )
        .await;

    // Read it back at once: the clock still owes the transition.
    let taking = docdb
        .ok(
            "describe while still being taken",
            "DescribeDBClusterSnapshots",
            &[("DBClusterSnapshotIdentifier", "alpha-snapshot")],
        )
        .await;
    assert_eq!(
        text(&taking, "Status"),
        "creating",
        "case \"describe while still being taken\": a snapshot AWS returns being taken \
         must not be reported finished, got {taking}"
    );
    assert_eq!(
        text(&taking, "PercentProgress"),
        "0",
        "case \"describe while still being taken\": got {taking}"
    );

    let_the_clock_catch_up();
    let finished = docdb
        .ok(
            "describe once the clock has passed",
            "DescribeDBClusterSnapshots",
            &[("DBClusterSnapshotIdentifier", "alpha-snapshot")],
        )
        .await;
    assert_eq!(
        text(&finished, "Status"),
        "available",
        "case \"describe once the clock has passed\": the clock owed this snapshot \
         `available`, got {finished}"
    );
    assert_eq!(
        text(&finished, "PercentProgress"),
        "100",
        "case \"describe once the clock has passed\": got {finished}"
    );
}

/// The whole snapshot lifecycle, computed from state: a stored snapshot is described,
/// copied, shared, unshared and deleted, and every answer is read back.
#[tokio::test]
async fn the_snapshot_lifecycle_is_computed_from_persisted_state() {
    let docdb = Docdb::new();
    snapshot_a_new_cluster(&docdb, "alpha", "alpha-snapshot").await;
    snapshot_a_new_cluster(&docdb, "beta", "beta-snapshot").await;

    // 1. Both snapshots are reported, in identifier order.
    let all = docdb.ok("describe all", "DescribeDBClusterSnapshots", &[]).await;
    assert_eq!(
        texts(&all, "DBClusterSnapshotIdentifier"),
        vec!["alpha-snapshot".to_string(), "beta-snapshot".to_string()],
        "case \"describe all\": expected both stored snapshots in order, got {all}"
    );

    // 2. One snapshot, named case-insensitively, projects the record's own facts.
    let one = docdb
        .ok(
            "describe one",
            "DescribeDBClusterSnapshots",
            &[("DBClusterSnapshotIdentifier", "ALPHA-SNAPSHOT")],
        )
        .await;
    assert_eq!(
        count(&one, "DBClusterSnapshot"),
        1,
        "case \"describe one\": expected exactly the named snapshot, got {one}"
    );
    assert_eq!(
        text(&one, "DBClusterSnapshotArn"),
        snapshot_arn("alpha-snapshot"),
        "case \"describe one\": expected the regional snapshot ARN, got {one}"
    );
    assert_eq!(
        text(&one, "DBClusterIdentifier"),
        "alpha",
        "case \"describe one\": the snapshot names the cluster it was taken from, got {one}"
    );
    assert_eq!(
        text(&one, "SnapshotType"),
        "manual",
        "case \"describe one\": a snapshot the caller asked for is manual, got {one}"
    );
    assert_eq!(
        text(&one, "Status"),
        "available",
        "case \"describe one\": got {one}"
    );
    assert_eq!(
        text(&one, "PercentProgress"),
        "100",
        "case \"describe one\": a stored snapshot is complete, got {one}"
    );
    assert_eq!(
        text(&one, "Engine"),
        "docdb",
        "case \"describe one\": got {one}"
    );
    assert_eq!(
        text(&one, "Port"),
        "27017",
        "case \"describe one\": expected DocumentDB's default port, got {one}"
    );
    assert_eq!(
        opt_text(&one, "SourceDBClusterSnapshotArn"),
        None,
        "case \"describe one\": a snapshot taken from a cluster was copied from nothing, \
         got {one}"
    );

    // The same snapshot by ARN.
    let by_arn = docdb
        .ok(
            "describe by arn",
            "DescribeDBClusterSnapshots",
            &[("DBClusterSnapshotIdentifier", &snapshot_arn("alpha-snapshot"))],
        )
        .await;
    assert_eq!(
        texts(&by_arn, "DBClusterSnapshotIdentifier"),
        vec!["alpha-snapshot".to_string()],
        "case \"describe by arn\": an ARN names the same snapshot, got {by_arn}"
    );

    // 3. `DBClusterIdentifier` narrows to one cluster's snapshots; a cluster that holds
    // none is an empty page, because this operation models no cluster-not-found error.
    let by_cluster = docdb
        .ok(
            "describe by cluster",
            "DescribeDBClusterSnapshots",
            &[("DBClusterIdentifier", "ALPHA")],
        )
        .await;
    assert_eq!(
        texts(&by_cluster, "DBClusterSnapshotIdentifier"),
        vec!["alpha-snapshot".to_string()],
        "case \"describe by cluster\": got {by_cluster}"
    );
    let unknown_cluster = docdb
        .ok(
            "describe by unknown cluster",
            "DescribeDBClusterSnapshots",
            &[("DBClusterIdentifier", "no-such-cluster")],
        )
        .await;
    assert_eq!(
        count(&unknown_cluster, "DBClusterSnapshot"),
        0,
        "case \"describe by unknown cluster\": expected an empty page, got {unknown_cluster}"
    );

    // The two selectors are documented as mutually exclusive.
    let both = docdb
        .call(
            "DescribeDBClusterSnapshots",
            &[
                ("DBClusterIdentifier", "alpha"),
                ("DBClusterSnapshotIdentifier", "alpha-snapshot"),
            ],
        )
        .await;
    assert_eq!(
        both.status, 400,
        "case \"both selectors\": expected a refusal, got {} with {}",
        both.status, both.xml
    );

    // 4. A copy is a new record that points back at its source.
    let copy = docdb
        .ok(
            "copy",
            "CopyDBClusterSnapshot",
            &[
                ("SourceDBClusterSnapshotIdentifier", "alpha-snapshot"),
                ("TargetDBClusterSnapshotIdentifier", "alpha-copy"),
            ],
        )
        .await;
    assert_eq!(
        text(&copy, "DBClusterSnapshotIdentifier"),
        "alpha-copy",
        "case \"copy\": got {copy}"
    );
    assert_eq!(
        text(&copy, "SourceDBClusterSnapshotArn"),
        snapshot_arn("alpha-snapshot"),
        "case \"copy\": the copy must name the snapshot it came from, got {copy}"
    );
    assert_eq!(
        text(&copy, "DBClusterIdentifier"),
        "alpha",
        "case \"copy\": a copy is still a snapshot of the source's cluster, got {copy}"
    );
    assert_eq!(
        text(&copy, "Status"),
        "copying",
        "case \"copy\": a copy AWS returns in progress must not be reported finished, \
         got {copy}"
    );
    assert_eq!(
        text(&copy, "PercentProgress"),
        "0",
        "case \"copy\": a copy still being made has made no progress, got {copy}"
    );

    // While it is copying, the state errors that go with that status hold.
    for (case, action, extra) in [
        ("delete a copy still copying", "DeleteDBClusterSnapshot", &[][..]),
        (
            "share a copy still copying",
            "ModifyDBClusterSnapshotAttribute",
            &[("AttributeName", "restore")][..],
        ),
    ] {
        let mut params = vec![("DBClusterSnapshotIdentifier", "alpha-copy")];
        params.extend_from_slice(extra);
        let reply = docdb.call(action, &params).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} with {}",
            reply.status, reply.xml
        );
        assert_eq!(
            error_code(&reply.xml),
            "InvalidDBClusterSnapshotStateFault",
            "case {case:?}: got {}",
            reply.xml
        );
    }

    // The clock makes the copy available; a describe materialises the transition.
    let_the_clock_catch_up();
    assert_eq!(
        text(&copy, "DBClusterSnapshotArn"),
        snapshot_arn("alpha-copy"),
        "case \"copy\": got {copy}"
    );
    let after_copy = docdb
        .ok("describe after copy", "DescribeDBClusterSnapshots", &[])
        .await;
    assert_eq!(
        texts(&after_copy, "DBClusterSnapshotIdentifier"),
        vec![
            "alpha-copy".to_string(),
            "alpha-snapshot".to_string(),
            "beta-snapshot".to_string()
        ],
        "case \"describe after copy\": the copy must be in state, got {after_copy}"
    );
    assert_eq!(
        texts(&after_copy, "Status"),
        vec![
            "available".to_string(),
            "available".to_string(),
            "available".to_string()
        ],
        "case \"describe after copy\": the clock owed the copy `available`, got {after_copy}"
    );

    // Copying onto an identifier already in state is the modeled already-exists fault.
    let duplicate = docdb
        .call(
            "CopyDBClusterSnapshot",
            &[
                ("SourceDBClusterSnapshotIdentifier", "alpha-snapshot"),
                ("TargetDBClusterSnapshotIdentifier", "ALPHA-COPY"),
            ],
        )
        .await;
    assert_eq!(
        duplicate.status, 400,
        "case \"duplicate copy\": expected 400, got {} with {}",
        duplicate.status, duplicate.xml
    );
    assert_eq!(
        error_code(&duplicate.xml),
        "DBClusterSnapshotAlreadyExistsFault",
        "case \"duplicate copy\": got {}",
        duplicate.xml
    );

    // 5. The `restore` attribute starts empty, takes the accounts a caller authorizes,
    // and is read back by the describe.
    let attributes = docdb
        .ok(
            "attributes before sharing",
            "DescribeDBClusterSnapshotAttributes",
            &[("DBClusterSnapshotIdentifier", "alpha-snapshot")],
        )
        .await;
    assert_eq!(
        texts(&attributes, "AttributeName"),
        vec!["restore".to_string()],
        "case \"attributes before sharing\": `restore` is always reported, got {attributes}"
    );
    assert_eq!(
        count(&attributes, "AttributeValue"),
        0,
        "case \"attributes before sharing\": a snapshot nobody is authorized for carries \
         no values, got {attributes}"
    );

    let shared = docdb
        .ok(
            "share",
            "ModifyDBClusterSnapshotAttribute",
            &[
                ("DBClusterSnapshotIdentifier", "alpha-snapshot"),
                ("AttributeName", "restore"),
                ("ValuesToAdd.AttributeValue.1", "111122223333"),
                ("ValuesToAdd.AttributeValue.2", "all"),
            ],
        )
        .await;
    assert_eq!(
        texts(&shared, "AttributeValue"),
        vec!["111122223333".to_string(), "all".to_string()],
        "case \"share\": the answer must report the attribute as it now stands, got {shared}"
    );
    let read_back = docdb
        .ok(
            "attributes after sharing",
            "DescribeDBClusterSnapshotAttributes",
            &[("DBClusterSnapshotIdentifier", "alpha-snapshot")],
        )
        .await;
    assert_eq!(
        texts(&read_back, "AttributeValue"),
        vec!["111122223333".to_string(), "all".to_string()],
        "case \"attributes after sharing\": the write must be in state, got {read_back}"
    );
    // The copy does not inherit the source's authorizations.
    let copy_attributes = docdb
        .ok(
            "attributes of the copy",
            "DescribeDBClusterSnapshotAttributes",
            &[("DBClusterSnapshotIdentifier", "alpha-copy")],
        )
        .await;
    assert_eq!(
        count(&copy_attributes, "AttributeValue"),
        0,
        "case \"attributes of the copy\": `restore` is not copied, got {copy_attributes}"
    );

    // Authorizing the same account twice changes nothing, and a removal takes it away.
    let again = docdb
        .ok(
            "share the same account again",
            "ModifyDBClusterSnapshotAttribute",
            &[
                ("DBClusterSnapshotIdentifier", "alpha-snapshot"),
                ("AttributeName", "restore"),
                ("ValuesToAdd.AttributeValue.1", "111122223333"),
            ],
        )
        .await;
    assert_eq!(
        texts(&again, "AttributeValue"),
        vec!["111122223333".to_string(), "all".to_string()],
        "case \"share the same account again\": `restore` is a set, got {again}"
    );
    let unshared = docdb
        .ok(
            "unshare",
            "ModifyDBClusterSnapshotAttribute",
            &[
                ("DBClusterSnapshotIdentifier", "alpha-snapshot"),
                ("AttributeName", "restore"),
                ("ValuesToRemove.AttributeValue.1", "111122223333"),
            ],
        )
        .await;
    assert_eq!(
        texts(&unshared, "AttributeValue"),
        vec!["all".to_string()],
        "case \"unshare\": got {unshared}"
    );

    // 6. `SnapshotType` selects over the documented values; `public` is now the one
    // snapshot whose `restore` attribute carries `all`.
    // (case, SnapshotType, the snapshots it must select)
    let type_rows: [(&str, &str, &[&str]); 4] = [
        (
            "manual",
            "manual",
            &["alpha-copy", "alpha-snapshot", "beta-snapshot"],
        ),
        ("public", "public", &["alpha-snapshot"]),
        ("automated", "automated", &[]),
        ("shared", "shared", &[]),
    ];
    for (case, snapshot_type, expected) in type_rows {
        let page = docdb
            .ok(
                case,
                "DescribeDBClusterSnapshots",
                &[("SnapshotType", snapshot_type)],
            )
            .await;
        let reported = texts(&page, "DBClusterSnapshotIdentifier");
        assert_eq!(
            reported,
            expected.iter().map(|id| (*id).to_string()).collect::<Vec<_>>(),
            "case {case:?}: expected {expected:?}, got {reported:?} in {page}"
        );
    }
    let bad_type = docdb
        .call(
            "DescribeDBClusterSnapshots",
            &[("SnapshotType", "semi-automated")],
        )
        .await;
    assert_eq!(
        bad_type.status, 400,
        "case \"an unmodelled SnapshotType\": expected a refusal, got {} with {}",
        bad_type.status, bad_type.xml
    );

    // 7. Deleting a snapshot answers with it and removes it from state; a second delete
    // finds nothing.
    let deleted = docdb
        .ok(
            "delete the copy",
            "DeleteDBClusterSnapshot",
            &[("DBClusterSnapshotIdentifier", "alpha-copy")],
        )
        .await;
    assert_eq!(
        text(&deleted, "DBClusterSnapshotIdentifier"),
        "alpha-copy",
        "case \"delete the copy\": got {deleted}"
    );
    let after_delete = docdb
        .ok("describe after delete", "DescribeDBClusterSnapshots", &[])
        .await;
    assert_eq!(
        texts(&after_delete, "DBClusterSnapshotIdentifier"),
        vec!["alpha-snapshot".to_string(), "beta-snapshot".to_string()],
        "case \"describe after delete\": the deleted snapshot must be gone, got {after_delete}"
    );
    let twice = docdb
        .call(
            "DeleteDBClusterSnapshot",
            &[("DBClusterSnapshotIdentifier", "alpha-copy")],
        )
        .await;
    assert_eq!(
        twice.status, 404,
        "case \"delete twice\": expected 404, got {} with {}",
        twice.status, twice.xml
    );
    assert_eq!(
        error_code(&twice.xml),
        "DBClusterSnapshotNotFoundFault",
        "case \"delete twice\": got {}",
        twice.xml
    );
}

/// `ModifyDBClusterSnapshotAttribute` refuses what the model and its documentation do not
/// allow, over the class of each rule.
#[tokio::test]
async fn sharing_a_snapshot_refuses_values_outside_the_documented_rule() {
    let docdb = Docdb::new();
    snapshot_a_new_cluster(&docdb, "alpha", "alpha-snapshot").await;

    // (case, the members to send besides the snapshot identifier)
    let refused: [(&str, &[(&str, &str)]); 6] = [
        (
            // `AttributeName`: `restore` is the only attribute AWS defines.
            "an attribute other than restore",
            &[("AttributeName", "share")],
        ),
        (
            "the right attribute name in the wrong case",
            &[("AttributeName", "RESTORE")],
        ),
        (
            // A value must be an account id or `all`.
            "a value that is not an account id",
            &[
                ("AttributeName", "restore"),
                ("ValuesToAdd.AttributeValue.1", "not-an-account"),
            ],
        ),
        (
            "an account id one digit short",
            &[
                ("AttributeName", "restore"),
                ("ValuesToAdd.AttributeValue.1", "11112222333"),
            ],
        ),
        (
            "an account id one digit long",
            &[
                ("AttributeName", "restore"),
                ("ValuesToAdd.AttributeValue.1", "1111222233334"),
            ],
        ),
        (
            // The result would depend on which list were applied first.
            "the same value added and removed",
            &[
                ("AttributeName", "restore"),
                ("ValuesToAdd.AttributeValue.1", "111122223333"),
                ("ValuesToRemove.AttributeValue.1", "111122223333"),
            ],
        ),
    ];

    for (case, members) in refused {
        let mut params = vec![("DBClusterSnapshotIdentifier", "alpha-snapshot")];
        params.extend_from_slice(members);
        let reply = docdb
            .call("ModifyDBClusterSnapshotAttribute", &params)
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected a refusal, got {} with {}",
            reply.status, reply.xml
        );
    }

    // Nothing above reached the store: the attribute is still empty.
    let attributes = docdb
        .ok(
            "attributes after the refusals",
            "DescribeDBClusterSnapshotAttributes",
            &[("DBClusterSnapshotIdentifier", "alpha-snapshot")],
        )
        .await;
    assert_eq!(
        count(&attributes, "AttributeValue"),
        0,
        "case \"attributes after the refusals\": a refused call must store nothing, \
         got {attributes}"
    );

    // One account per published slot is authorized, and the one past the limit is the
    // modeled quota fault; see `state::MAX_SNAPSHOT_SHARED_ACCOUNTS`.
    for index in 0..state::MAX_SNAPSHOT_SHARED_ACCOUNTS {
        let account = format!("{:012}", 100_000_000_000_u64 + index as u64);
        docdb
            .ok(
                "fill the sharing quota",
                "ModifyDBClusterSnapshotAttribute",
                &[
                    ("DBClusterSnapshotIdentifier", "alpha-snapshot"),
                    ("AttributeName", "restore"),
                    ("ValuesToAdd.AttributeValue.1", &account),
                ],
            )
            .await;
    }
    let over_quota = docdb
        .call(
            "ModifyDBClusterSnapshotAttribute",
            &[
                ("DBClusterSnapshotIdentifier", "alpha-snapshot"),
                ("AttributeName", "restore"),
                ("ValuesToAdd.AttributeValue.1", "999988887777"),
            ],
        )
        .await;
    assert_eq!(
        over_quota.status, 400,
        "case \"one account past the quota\": expected 400, got {} with {}",
        over_quota.status, over_quota.xml
    );
    assert_eq!(
        error_code(&over_quota.xml),
        // The wire code is the shape's `awsQueryError` code, which drops the `Fault`
        // suffix here (`smithy_query(query="operation",
        // operation="ModifyDBClusterSnapshotAttribute")`: awsQueryError code
        // `SharedSnapshotQuotaExceeded`).
        "SharedSnapshotQuotaExceeded",
        "case \"one account past the quota\": got {}",
        over_quota.xml
    );
    let still = docdb
        .ok(
            "attributes after the over-quota call",
            "DescribeDBClusterSnapshotAttributes",
            &[("DBClusterSnapshotIdentifier", "alpha-snapshot")],
        )
        .await;
    assert_eq!(
        count(&still, "AttributeValue"),
        state::MAX_SNAPSHOT_SHARED_ACCOUNTS,
        "case \"attributes after the over-quota call\": the refused account must not have \
         been stored, got {still}"
    );
}

/// `ValuesToAdd` and `ValuesToRemove` publish no `@length`, so their entry COUNTS are the
/// caller's to choose, and every check this operation runs is per entry. A list past the
/// emulator's collection bound is refused before anything is checked or stored; a list AT
/// the bound is well formed, so it is the documented sharing quota that answers it.
#[tokio::test]
async fn an_over_size_attribute_value_list_is_refused_before_anything_is_applied() {
    let docdb = Docdb::new();
    snapshot_a_new_cluster(&docdb, "alpha", "alpha-snapshot").await;

    /// Distinct, well-formed 12-digit account ids, so a row at the bound and a row past it
    /// differ only in the entry COUNT.
    fn accounts(count: usize) -> Vec<String> {
        (0..count)
            .map(|index| format!("{:012}", 100_000_000_000_u64 + index as u64))
            .collect()
    }

    /// The request members for `member` carrying `values`, in the awsQuery list form.
    fn list_members(member: &str, values: &[String]) -> Vec<(String, String)> {
        values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                (
                    format!("{member}.AttributeValue.{}", index + 1),
                    value.clone(),
                )
            })
            .collect()
    }

    // (case, the member carrying the list, how many entries, the wire error code)
    let rows: [(&str, &str, usize, &str); 4] = [
        (
            "ValuesToAdd one entry past the collection bound",
            "ValuesToAdd",
            state::MAX_COLLECTION_ENTRIES + 1,
            "ValidationException",
        ),
        (
            "ValuesToRemove one entry past the collection bound",
            "ValuesToRemove",
            state::MAX_COLLECTION_ENTRIES + 1,
            "ValidationException",
        ),
        (
            // At the bound the request is well formed, so the DOCUMENTED quota on the
            // accounts a snapshot ends up shared with is what refuses it.
            "ValuesToAdd at the collection bound",
            "ValuesToAdd",
            state::MAX_COLLECTION_ENTRIES,
            "SharedSnapshotQuotaExceeded",
        ),
        (
            // Ten entries is inside both bounds: adding them is accepted, which is what
            // makes the two refusals above about the counts and not about the values.
            "ValuesToAdd inside both bounds",
            "ValuesToAdd",
            10,
            "",
        ),
    ];

    for (case, member, count_of, expected_code) in rows {
        let values = accounts(count_of);
        let mut owned = vec![(
            "DBClusterSnapshotIdentifier".to_string(),
            "alpha-snapshot".to_string(),
        )];
        owned.push(("AttributeName".to_string(), "restore".to_string()));
        owned.extend(list_members(member, &values));
        let params: Vec<(&str, &str)> = owned
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let reply = docdb
            .call("ModifyDBClusterSnapshotAttribute", &params)
            .await;
        if expected_code.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the call to be served, got {} with {}",
                reply.status, reply.xml
            );
            assert_eq!(
                count(&reply.xml, "AttributeValue"),
                count_of,
                "case {case:?}: every value sent must be authorized, got {}",
                reply.xml
            );
            continue;
        }
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected a refusal, got {} with {}",
            reply.status, reply.xml
        );
        assert_eq!(
            error_code(&reply.xml),
            expected_code,
            "case {case:?}: got {}",
            reply.xml
        );
    }
}

/// An identifier carrying XML markup comes back ESCAPED inside one well-formed
/// `<ErrorResponse>`: a message is request data, so it can never close the envelope early
/// or add a second `<RequestId>` to it.
#[tokio::test]
async fn a_refusal_escapes_an_identifier_that_carries_xml_markup() {
    let docdb = Docdb::new();

    // (case, the identifier to ask for)
    let rows: [(&str, &str); 4] = [
        ("angle brackets and an ampersand", "no<such>&snapshot"),
        ("a quote and an apostrophe", "no\"such'snapshot"),
        ("the envelope's own closing tag", "x</ErrorResponse>"),
        (
            "the envelope's request-id element",
            "x<RequestId>spoofed</RequestId>",
        ),
    ];

    for (case, identifier) in rows {
        let reply = docdb
            .call(
                "DescribeDBClusterSnapshots",
                &[("DBClusterSnapshotIdentifier", identifier)],
            )
            .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected the modelled 404, got {} with {}",
            reply.status, reply.xml
        );
        assert_eq!(
            error_code(&reply.xml),
            "DBClusterSnapshotNotFoundFault",
            "case {case:?}: got {}",
            reply.xml
        );
        // Read back through the unescaping reader: the message is exactly what was asked
        // for, so the markup survived as DATA.
        assert_eq!(
            text(&reply.xml, "Message"),
            format!("DBClusterSnapshot not found: {identifier}"),
            "case {case:?}: got {}",
            reply.xml
        );
        // And exactly one of each envelope element, so nothing in the message was read as
        // markup on the way out.
        for element in ["<ErrorResponse", "<RequestId>", "</ErrorResponse>"] {
            assert_eq!(
                reply.xml.matches(element).count(),
                1,
                "case {case:?}: expected exactly one {element}, got {}",
                reply.xml
            );
        }
    }
}

/// `MaxRecords` holds its published bounds and the `Marker` round-trips over the stored
/// snapshots.
#[tokio::test]
async fn max_records_holds_its_bounds_and_the_marker_pages_the_snapshots() {
    let docdb = Docdb::new();
    snapshot_a_new_cluster(&docdb, "alpha", "snapshot-000").await;
    // One more snapshot than the smallest page, so a first page cannot be the last.
    let total = state::MIN_MAX_RECORDS as usize + 2;
    for index in 1..total {
        let target = format!("snapshot-{index:03}");
        docdb
            .ok(
                "seed a page",
                "CopyDBClusterSnapshot",
                &[
                    ("SourceDBClusterSnapshotIdentifier", "snapshot-000"),
                    ("TargetDBClusterSnapshotIdentifier", &target),
                ],
            )
            .await;
    }

    // (case, MaxRecords, accepted)
    let bounds: [(&str, i32, bool); 4] = [
        ("one below the minimum", state::MIN_MAX_RECORDS - 1, false),
        ("the published minimum", state::MIN_MAX_RECORDS, true),
        ("the published maximum", state::MAX_MAX_RECORDS, true),
        ("one above the maximum", state::MAX_MAX_RECORDS + 1, false),
    ];
    for (case, max_records, accepted) in bounds {
        let value = max_records.to_string();
        let reply = docdb
            .call("DescribeDBClusterSnapshots", &[("MaxRecords", &value)])
            .await;
        let ok = reply.status == 200;
        assert_eq!(
            ok, accepted,
            "case {case:?}: expected accepted == {accepted}, got {} with {}",
            reply.status, reply.xml
        );
    }

    // The first page carries the smallest allowed number of records and a marker; the
    // second starts strictly beyond it and carries none.
    let page_size = state::MIN_MAX_RECORDS.to_string();
    let first = docdb
        .ok(
            "first page",
            "DescribeDBClusterSnapshots",
            &[("MaxRecords", &page_size)],
        )
        .await;
    let first_ids = texts(&first, "DBClusterSnapshotIdentifier");
    assert_eq!(
        first_ids.len(),
        state::MIN_MAX_RECORDS as usize,
        "case \"first page\": expected a full page, got {first}"
    );
    let marker = text(&first, "Marker");
    assert_eq!(
        marker,
        *first_ids.last().expect("a full page has a last record"),
        "case \"first page\": the marker is the last record of the page, got {first}"
    );
    let second = docdb
        .ok(
            "second page",
            "DescribeDBClusterSnapshots",
            &[("MaxRecords", &page_size), ("Marker", &marker)],
        )
        .await;
    let second_ids = texts(&second, "DBClusterSnapshotIdentifier");
    assert_eq!(
        second_ids.len(),
        total - state::MIN_MAX_RECORDS as usize,
        "case \"second page\": expected the rest of the snapshots, got {second}"
    );
    assert!(
        second_ids.iter().all(|id| *id > marker),
        "case \"second page\": every record must lie beyond the marker {marker:?}, \
         got {second_ids:?}"
    );
    assert_eq!(
        opt_text(&second, "Marker"),
        None,
        "case \"second page\": the last page carries no marker, got {second}"
    );
}
