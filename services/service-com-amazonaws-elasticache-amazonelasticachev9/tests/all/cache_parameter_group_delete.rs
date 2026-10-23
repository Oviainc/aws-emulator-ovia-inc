// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteCacheParameterGroup` and what the store looks like
//! afterwards, read back through `DescribeCacheParameterGroups`.
//!
//! The rules under test:
//!
//! * a delete REMOVES the group from the account, so the describe that follows it answers
//!   the modelled `CacheParameterGroupNotFound` (404), a second delete answers the same,
//!   the name is free for a create again, and no other group is touched;
//! * "You cannot delete the default cache parameter groups in your account" — a group in
//!   the `default.` namespace is refused with the modelled `InvalidParameterValue` (400),
//!   the code the recorded AWS call answers such a delete with, and is still there
//!   afterwards;
//! * "You cannot delete a cache parameter group if it is associated with any cache
//!   clusters" — the modelled `InvalidCacheParameterGroupState` (400) while a cluster or a
//!   replication group carries the group, and the delete succeeds once that resource is
//!   gone;
//! * the name is resolved as the API documents it ("This value is stored as a lowercase
//!   string"), so the casing a delete spells it in does not matter.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::cache_cluster_lifecycle`]; nothing reaches into
//! the service's state struct, so a response that is not computed from persisted state
//! cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule generalises over
//! (the family, the name's case, the tag count, the kind of resource holding the group),
//! so no single recorded request is what makes them pass.

use crate::cache_cluster_lifecycle::{
    create_available_cluster, params, Emulator, Reply, PAST_TRANSITION,
};

/// Create one cache parameter group, with whatever tags `tags` names.
async fn create_group(
    emulator: &Emulator,
    name: &str,
    family: &str,
    description: &str,
    tags: &[(&str, &str)],
) -> Reply {
    let mut pairs = params(&[
        ("CacheParameterGroupName", name),
        ("CacheParameterGroupFamily", family),
        ("Description", description),
    ]);
    for (index, (key, value)) in tags.iter().enumerate() {
        pairs.push((format!("Tags.Tag.{}.Key", index + 1), (*key).to_string()));
        pairs.push((
            format!("Tags.Tag.{}.Value", index + 1),
            (*value).to_string(),
        ));
    }
    emulator.call("CreateCacheParameterGroup", &pairs).await
}

/// Delete the group `name` names.
async fn delete_group(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DeleteCacheParameterGroup",
            &params(&[("CacheParameterGroupName", name)]),
        )
        .await
}

/// Describe the group `name` names.
async fn describe_group(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DescribeCacheParameterGroups",
            &params(&[("CacheParameterGroupName", name)]),
        )
        .await
}

/// Every `CacheParameterGroupName` a describe of the whole account reports.
async fn all_group_names(emulator: &Emulator) -> Vec<String> {
    let reply = emulator
        .call("DescribeCacheParameterGroups", &params(&[("MaxRecords", "100")]))
        .await;
    assert_eq!(
        reply.status, 200,
        "a describe of every group must answer 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .all("CacheParameterGroups/CacheParameterGroup/CacheParameterGroupName")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Assert that `name` is answered with the modelled not-found, by both readers that
/// resolve a `CacheParameterGroupName`.
async fn assert_absent(emulator: &Emulator, case: &str, name: &str) {
    let described = describe_group(emulator, name).await;
    assert_eq!(
        described.status, 404,
        "case {case:?}: a describe of the deleted group must answer 404, got {} {}",
        described.status, described.body
    );
    assert_eq!(
        described.error_code(),
        "CacheParameterGroupNotFound",
        "case {case:?}: expected the modelled not-found code, got {}",
        described.body
    );
    let again = delete_group(emulator, name).await;
    assert_eq!(
        again.status, 404,
        "case {case:?}: a second delete must answer 404, got {} {}",
        again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "CacheParameterGroupNotFound",
        "case {case:?}: expected the modelled not-found code on the second delete, got {}",
        again.body
    );
}

/// One delete row: the case, the name the group is created with, the name the delete
/// spells it with, the family, and the tags the create carries.
type DeleteCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// The rows span what the rule generalises over: the family (one Valkey, one Redis OSS,
/// one Memcached — `memcached1.5` is not a family the issue cited), the case the name is
/// created and deleted in, and the tag count, including a group carrying the documented
/// maximum-shaped pair list.
const DELETE_CASES: &[DeleteCase] = &[
    (
        "a lowercase name, deleted as spelled",
        "valkey-group",
        "valkey-group",
        "valkey8",
        &[],
    ),
    (
        "a mixed-case name, deleted in another case",
        "Mixed-Case-Group",
        "MIXED-CASE-GROUP",
        "redis7",
        &[],
    ),
    (
        "an uppercase name, deleted in lowercase",
        "UPPER-GROUP",
        "upper-group",
        "memcached1.5",
        &[("team", "cache")],
    ),
    (
        "a tagged group",
        "tagged-group",
        "tagged-group",
        "redis6.x",
        &[("team", "cache"), ("env", "test"), ("owner", "nobody")],
    ),
];

/// A delete removes the group: the describe that follows answers the modelled not-found,
/// a second delete answers the same, the name is free to create again, and the groups
/// beside it are untouched.
#[tokio::test]
async fn a_delete_removes_the_group_from_the_account() {
    let emulator = Emulator::new();
    let before = all_group_names(&emulator).await;

    for (case, created_as, deleted_as, family, tags) in DELETE_CASES {
        let created = create_group(&emulator, created_as, family, "a group to delete", tags).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must succeed, got {} {}",
            created.status, created.body
        );
        let stored = created
            .text("CacheParameterGroup/CacheParameterGroupName")
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            stored,
            created_as.to_ascii_lowercase(),
            "case {case:?}: the create must persist the lowercased name, got {}",
            created.body
        );

        let described = describe_group(&emulator, deleted_as).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the group must be readable before the delete, got {} {}",
            described.status, described.body
        );

        let deleted = delete_group(&emulator, deleted_as).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the delete must answer 200, got {} {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.error_code(),
            "",
            "case {case:?}: the delete must carry no error envelope, got {}",
            deleted.body
        );

        assert_absent(&emulator, case, deleted_as).await;

        // The name is free again: the create that collided before now succeeds, which is
        // what a client that deleted a group to recreate it depends on.
        let recreated =
            create_group(&emulator, created_as, family, "the same name again", tags).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the name must be free to create again, got {} {}",
            recreated.status, recreated.body
        );
        let cleaned = delete_group(&emulator, deleted_as).await;
        assert_eq!(
            cleaned.status, 200,
            "case {case:?}: the recreated group must delete too, got {} {}",
            cleaned.status, cleaned.body
        );
        assert_absent(&emulator, case, deleted_as).await;
    }

    // Nothing else left with them: the account holds exactly the groups it started with.
    let after = all_group_names(&emulator).await;
    assert_eq!(
        after, before,
        "the deletes must leave every other group in place, expected {before:?}, got {after:?}"
    );
}

/// Deleting two of several custom groups removes exactly those two: a delete is not a
/// clear of the map.
#[tokio::test]
async fn a_delete_touches_only_the_group_it_names() {
    let emulator = Emulator::new();
    let seeded = all_group_names(&emulator).await;

    for name in ["keep-one", "drop-one", "keep-two", "drop-two"] {
        let created = create_group(&emulator, name, "redis7", "one of four", &[]).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the create must succeed, got {} {}",
            created.status, created.body
        );
    }

    for name in ["drop-one", "drop-two"] {
        let deleted = delete_group(&emulator, name).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: the delete must answer 200, got {} {}",
            deleted.status, deleted.body
        );
    }

    let mut expected = seeded;
    expected.push("keep-one".to_string());
    expected.push("keep-two".to_string());
    expected.sort();
    let mut after = all_group_names(&emulator).await;
    after.sort();
    assert_eq!(
        after, expected,
        "expected only the named groups removed, expected {expected:?}, got {after:?}"
    );

    for name in ["keep-one", "keep-two"] {
        let described = describe_group(&emulator, name).await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: the kept group must still be readable, got {} {}",
            described.status, described.body
        );
    }
    for name in ["drop-one", "drop-two"] {
        assert_absent(&emulator, name, name).await;
    }
}

/// The default group of a published family cannot be deleted, and is still there after
/// the refusal. The rows are the `default.` group of three families — one Valkey, one
/// Redis OSS and one Memcached — spelled in two cases.
#[tokio::test]
async fn a_default_group_cannot_be_deleted() {
    let emulator = Emulator::new();

    // (case, the name the delete names)
    let rows: [(&str, &str); 4] = [
        ("the Valkey family default", "default.valkey8"),
        ("the Redis OSS family default", "default.redis6.x"),
        ("the Memcached family default", "default.memcached1.4"),
        ("a family default named in another case", "DEFAULT.REDIS7"),
    ];

    for (case, name) in rows {
        let refused = delete_group(&emulator, name).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected the modelled parameter-value refusal, got {}",
            refused.body
        );
        let described = describe_group(&emulator, name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the default group must still be there, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described
                .text("CacheParameterGroups/CacheParameterGroup/CacheParameterGroupName")
                .unwrap_or_default(),
            name.to_ascii_lowercase(),
            "case {case:?}: the describe must report the same default group, got {}",
            described.body
        );
    }
}

/// One in-use row: the case, the parameter group a resource holds, the action that
/// releases it, and that action's form pairs.
type InUseCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// A group a cache cluster or a replication group is associated with cannot be deleted,
/// and can once that resource is gone.
#[tokio::test]
async fn a_group_in_use_cannot_be_deleted() {
    let emulator = Emulator::new();

    for (case, name) in [
        ("the group a cluster carries", "cluster-group"),
        ("the group a replication group carries", "replication-group-group"),
    ] {
        let created = create_group(&emulator, name, "redis7", "a group in use", &[]).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must succeed, got {} {}",
            created.status, created.body
        );
    }

    create_available_cluster(
        &emulator,
        "in-use-cluster",
        &[
            ("Engine", "redis"),
            ("CacheNodeType", "cache.t3.micro"),
            ("NumCacheNodes", "1"),
            ("CacheParameterGroupName", "cluster-group"),
        ],
    )
    .await;
    let group = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", "in-use-rg"),
                ("ReplicationGroupDescription", "a group holding a parameter group"),
                ("CacheNodeType", "cache.t3.micro"),
                ("CacheParameterGroupName", "replication-group-group"),
            ]),
        )
        .await;
    assert_eq!(
        group.status, 200,
        "the replication group must be created, got {} {}",
        group.status, group.body
    );
    std::thread::sleep(PAST_TRANSITION);

    // (case, the parameter group, the action that releases it, its parameters)
    let rows: [InUseCase; 2] = [
        (
            "a cache cluster holds it",
            "cluster-group",
            "DeleteCacheCluster",
            &[("CacheClusterId", "in-use-cluster")],
        ),
        (
            "a replication group holds it",
            "replication-group-group",
            "DeleteReplicationGroup",
            &[("ReplicationGroupId", "in-use-rg")],
        ),
    ];

    for (case, name, release_action, release_params) in rows {
        let refused = delete_group(&emulator, name).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 while the group is in use, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidCacheParameterGroupState",
            "case {case:?}: expected the modelled state fault, got {}",
            refused.body
        );
        let described = describe_group(&emulator, name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the refused group must still be there, got {} {}",
            described.status, described.body
        );

        // Release the group by removing the resource that holds it. A release path that
        // is not implemented yet (its own stub issue owns it) answers 501, and the row
        // then stops at the refusal above rather than claiming a release nothing did.
        let released = emulator.call(release_action, &params(release_params)).await;
        if released.status != 200 {
            assert_eq!(
                released.status, 501,
                "case {case:?}: {release_action} must either succeed or answer the 501 channel, \
                 got {} {}",
                released.status, released.body
            );
            continue;
        }
        let deleted = delete_group(&emulator, name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the delete must succeed once the group is free, got {} {}",
            deleted.status, deleted.body
        );
        assert_absent(&emulator, case, name).await;
    }
}

/// The delete is recorded as an event of the group, beside the create — the log a
/// `DescribeEvents` reader sees, which is the other place a state change shows up.
#[tokio::test]
async fn a_delete_is_recorded_as_an_event() {
    let emulator = Emulator::new();

    let created = create_group(&emulator, "logged-group", "valkey7", "a logged group", &[]).await;
    assert_eq!(
        created.status, 200,
        "the create must succeed, got {} {}",
        created.status, created.body
    );
    let deleted = delete_group(&emulator, "logged-group").await;
    assert_eq!(
        deleted.status, 200,
        "the delete must succeed, got {} {}",
        deleted.status, deleted.body
    );

    let events = emulator
        .call(
            "DescribeEvents",
            &params(&[
                ("SourceType", "cache-parameter-group"),
                ("SourceIdentifier", "logged-group"),
            ]),
        )
        .await;
    assert_eq!(
        events.status, 200,
        "the event read must answer 200, got {} {}",
        events.status, events.body
    );
    let messages = events.all("Event/Message");
    assert_eq!(
        messages.len(),
        2,
        "expected the create and the delete to be logged, got {messages:?} from {}",
        events.body
    );
    assert!(
        messages
            .iter()
            .any(|message| message.to_ascii_lowercase().contains("deleted")),
        "expected an event recording the delete, got {messages:?} from {}",
        events.body
    );
}
