// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ElastiCache update-action slice:
//! `BatchApplyUpdateAction` starts a published service update on the resources
//! a request names and `BatchStopUpdateAction` stops one that is under way.
//!
//! Every assertion goes over the wire through the same [`Emulator`] harness the
//! cluster suite uses, so a response that is not computed from the published
//! catalog and the stored clusters and update actions cannot pass. The service
//! update names are read out of `DescribeServiceUpdates` rather than written
//! here: the catalog is the service's own.
//!
//! The tests are tables: each row is one input along the axis the rule under
//! test generalises over (which update is named, which resource list names it,
//! how the name or the resource misses, how many resources one call names, and
//! what status the action is already in).

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::{create_available_cluster, params, Emulator, Reply};

/// One published service update, reduced to what these tests select on.
#[derive(Debug, Clone)]
struct CatalogUpdate {
    name: String,
    engine: String,
}

/// The whole published catalog, as `DescribeServiceUpdates` reports it.
async fn catalog(emulator: &Emulator) -> Vec<CatalogUpdate> {
    let reply = emulator.call("DescribeServiceUpdates", &[]).await;
    assert_eq!(
        reply.status, 200,
        "the catalog must be reported, got {} {}",
        reply.status, reply.body
    );
    let names = reply.all("ServiceUpdateName");
    let engines = reply.all("Engine");
    assert_eq!(
        names.len(),
        engines.len(),
        "every published update reports its engine, got {}",
        reply.body
    );
    assert!(
        !names.is_empty(),
        "the service publishes a catalog of updates, got {}",
        reply.body
    );
    names
        .into_iter()
        .zip(engines)
        .map(|(name, engine)| CatalogUpdate {
            name: name.to_string(),
            engine: engine.to_string(),
        })
        .collect()
}

/// The first published update that applies to `engine`.
fn update_for(catalog: &[CatalogUpdate], engine: &str) -> CatalogUpdate {
    catalog
        .iter()
        .find(|update| update.engine == engine)
        .unwrap_or_else(|| panic!("the catalog publishes an update for {engine}: {catalog:?}"))
        .clone()
}

/// One batch call: `action` with the service update `name` over the cache
/// clusters and replication groups named.
async fn batch(
    emulator: &Emulator,
    action: &str,
    name: &str,
    cache_clusters: &[&str],
    replication_groups: &[&str],
) -> Reply {
    let mut pairs = params(&[("ServiceUpdateName", name)]);
    for (index, id) in cache_clusters.iter().enumerate() {
        pairs.push((
            format!("CacheClusterIds.member.{}", index + 1),
            (*id).to_string(),
        ));
    }
    for (index, id) in replication_groups.iter().enumerate() {
        pairs.push((
            format!("ReplicationGroupIds.member.{}", index + 1),
            (*id).to_string(),
        ));
    }
    emulator.call(action, &pairs).await
}

/// Every processed update action a reply reports, in document order, as
/// `(member, id, status)` — the member being `CacheClusterId` or
/// `ReplicationGroupId`, which is how the response says WHICH KIND of resource
/// the action belongs to. Read element by element, so a reply carrying both
/// kinds pairs each id with the status reported beside it.
fn processed_actions(reply: &Reply) -> Vec<(String, String, String)> {
    let mut actions: Vec<(String, String, String)> = Vec::new();
    let mut named: Option<(String, String)> = None;
    for (path, text) in &reply.nodes {
        if !path.contains("ProcessedUpdateAction/") {
            continue;
        }
        match path.rsplit('/').next().unwrap_or_default() {
            member @ ("CacheClusterId" | "ReplicationGroupId") => {
                named = Some((member.to_string(), text.clone()));
            }
            "UpdateActionStatus" => {
                let (member, id) = named.take().unwrap_or_else(|| {
                    panic!("a processed update action names its resource: {}", reply.body)
                });
                actions.push((member, id, text.clone()));
            }
            _ => {}
        }
    }
    actions
}

/// The `(CacheClusterId, UpdateActionStatus)` pairs a reply processed, in
/// document order.
fn processed(reply: &Reply) -> Vec<(String, String)> {
    processed_actions(reply)
        .into_iter()
        .filter(|(member, _, _)| member == "CacheClusterId")
        .map(|(_, id, status)| (id, status))
        .collect()
}

/// The `ErrorType`s a reply reported under `UnprocessedUpdateActions`, in
/// document order.
fn unprocessed_error_types(reply: &Reply) -> Vec<&str> {
    reply.all("UnprocessedUpdateAction/ErrorType")
}

/// The `(ReplicationGroupId, UpdateActionStatus)` pairs a reply processed, in
/// document order — the group half of the same accounting [`processed`] reads
/// for cache clusters.
fn processed_groups(reply: &Reply) -> Vec<(String, String)> {
    processed_actions(reply)
        .into_iter()
        .filter(|(member, _, _)| member == "ReplicationGroupId")
        .map(|(_, id, status)| (id, status))
        .collect()
}

/// Create one replication group running `engine`, named `id`.
async fn create_group(emulator: &Emulator, id: &str, engine: &str) -> Reply {
    emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", id),
                ("ReplicationGroupDescription", "an emulated replication group"),
                ("Engine", engine),
                ("CacheNodeType", "cache.t3.micro"),
            ]),
        )
        .await
}

#[tokio::test]
async fn an_update_action_on_a_replication_group_is_kept_apart_from_one_on_a_cluster() {
    let emulator = Emulator::new();
    let published = catalog(&emulator).await;
    let redis_update = update_for(&published, "redis");
    let memcached_update = update_for(&published, "memcached");

    // The axis: the KIND of resource the update action is named for. A cache
    // cluster and a replication group may carry the same name while being
    // different resources, so the two actions must be kept apart.
    create_available_cluster(&emulator, "shared-name", &[("Engine", "redis")]).await;
    let group = create_group(&emulator, "shared-name", "redis").await;
    assert_eq!(
        group.status, 200,
        "case group create: a replication group may carry a cluster's name, got {} {}",
        group.status, group.body
    );

    // One call naming both: each is processed under its own member, and each
    // gets its own stored action.
    let applied = batch(
        &emulator,
        "BatchApplyUpdateAction",
        &redis_update.name,
        &["shared-name"],
        &["shared-name"],
    )
    .await;
    assert_eq!(
        applied.status, 200,
        "case both kinds: the call must answer the results message, got {} {}",
        applied.status, applied.body
    );
    let waiting = state::UPDATE_ACTION_WAITING_TO_START.to_string();
    assert_eq!(
        processed(&applied),
        vec![("shared-name".to_string(), waiting.clone())],
        "case both kinds: the cluster's action must be processed, got {}",
        applied.body
    );
    assert_eq!(
        processed_groups(&applied),
        vec![("shared-name".to_string(), waiting.clone())],
        "case both kinds: the group's action must be processed, got {}",
        applied.body
    );
    assert!(
        unprocessed_error_types(&applied).is_empty(),
        "case both kinds: nothing may be unprocessed, got {}",
        applied.body
    );

    // Stopping the GROUP's action moves only that one: the cluster's is still
    // under way, so applying it again is still reported unprocessed.
    let stopped = batch(
        &emulator,
        "BatchStopUpdateAction",
        &redis_update.name,
        &[],
        &["shared-name"],
    )
    .await;
    assert_eq!(
        processed_groups(&stopped),
        vec![(
            "shared-name".to_string(),
            state::UPDATE_ACTION_STOPPING.to_string()
        )],
        "case stop the group: only the group's action is stopping, got {}",
        stopped.body
    );
    let cluster_again = batch(
        &emulator,
        "BatchApplyUpdateAction",
        &redis_update.name,
        &["shared-name"],
        &[],
    )
    .await;
    assert!(
        processed(&cluster_again).is_empty()
            && unprocessed_error_types(&cluster_again)
                == vec![state::UPDATE_ACTION_INVALID_ERROR_TYPE],
        "case stop the group: the cluster's own action must be untouched and still under way, \
         got {}",
        cluster_again.body
    );

    // A group is also checked against the update's engine: the Memcached
    // update applies to neither resource of this Valkey/Redis OSS pair.
    let crossed = batch(
        &emulator,
        "BatchApplyUpdateAction",
        &memcached_update.name,
        &[],
        &["shared-name"],
    )
    .await;
    assert_eq!(
        crossed.status, 200,
        "case group engine mismatch: the call must answer the results message, got {} {}",
        crossed.status, crossed.body
    );
    assert!(
        processed_groups(&crossed).is_empty(),
        "case group engine mismatch: nothing may be processed, got {}",
        crossed.body
    );
    assert_eq!(
        unprocessed_error_types(&crossed),
        vec![state::UPDATE_ACTION_INVALID_ERROR_TYPE],
        "case group engine mismatch: the group must be reported unprocessed, got {}",
        crossed.body
    );
}

#[tokio::test]
async fn a_service_update_name_the_catalogue_does_not_hold_is_refused_as_not_found() {
    let emulator = Emulator::new();
    let published = catalog(&emulator).await;
    let real = update_for(&published, "redis");
    create_available_cluster(&emulator, "batch-target", &[("Engine", "redis")]).await;

    // The axis: how the name misses the catalog — a name of another form, one
    // of the right form released on a date nothing was, the right name in the
    // wrong case, the right name with whitespace, and the empty name. Each
    // over both batch operations, and over both resource lists, because the
    // fault keys on the update rather than on the resource.
    let rows = [
        "no-such-update".to_string(),
        "elasticache-19700101-001".to_string(),
        real.name.to_uppercase(),
        format!("{} ", real.name),
        String::new(),
    ];
    for action in ["BatchApplyUpdateAction", "BatchStopUpdateAction"] {
        for name in &rows {
            for (label, clusters, groups) in [
                ("a cache cluster", &["batch-target"][..], &[][..]),
                ("an unknown cache cluster", &["no-such-cluster"][..], &[][..]),
                ("a replication group", &[][..], &["rg-unknown"][..]),
            ] {
                let case = format!("{action} {name:?} over {label}");
                let reply = batch(&emulator, action, name, clusters, groups).await;
                assert_eq!(
                    reply.status, 404,
                    "case {case}: must be refused with HTTP 404, got {} {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.error_code(),
                    "ServiceUpdateNotFoundFault",
                    "case {case}: must be refused with ServiceUpdateNotFoundFault, got {}",
                    reply.body
                );
            }
        }
    }
}

#[tokio::test]
async fn a_published_update_is_applied_per_resource_and_reports_what_it_could_not_do() {
    let emulator = Emulator::new();
    let published = catalog(&emulator).await;
    let redis_update = update_for(&published, "redis");
    let memcached_update = update_for(&published, "memcached");
    create_available_cluster(&emulator, "applied-one", &[("Engine", "redis")]).await;
    create_available_cluster(&emulator, "applied-two", &[("Engine", "redis")]).await;
    create_available_cluster(&emulator, "applied-memcached", &[("Engine", "memcached")]).await;

    /// One row: the case, the update applied, the cache clusters and
    /// replication groups named, the `(id, status)` pairs that must be
    /// processed and the `ErrorType`s that must be unprocessed.
    type Row = (
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
        Vec<(String, String)>,
        Vec<&'static str>,
    );
    let waiting = state::UPDATE_ACTION_WAITING_TO_START.to_string();
    // The axis: how many resources one call names, which list names them, and
    // whether each one can take the update.
    let rows: [Row; 5] = [
        (
            "one cache cluster",
            &["applied-one"],
            &[],
            vec![("applied-one".to_string(), waiting.clone())],
            vec![],
        ),
        (
            "two cache clusters, in request order",
            &["applied-two", "applied-one"],
            &[],
            vec![
                ("applied-two".to_string(), waiting.clone()),
                ("applied-one".to_string(), waiting.clone()),
            ],
            vec![],
        ),
        (
            "a cache cluster id in another case",
            &["Applied-One"],
            &[],
            vec![("applied-one".to_string(), waiting.clone())],
            vec![],
        ),
        (
            "a cache cluster this account does not hold",
            &["applied-one", "no-such-cluster"],
            &[],
            vec![("applied-one".to_string(), waiting.clone())],
            vec![state::CACHE_CLUSTER_NOT_FOUND_ERROR_TYPE],
        ),
        (
            "a replication group this account does not hold",
            &[],
            &["rg-unknown"],
            vec![],
            vec![state::REPLICATION_GROUP_NOT_FOUND_ERROR_TYPE],
        ),
    ];

    for (case, clusters, groups, want_processed, want_unprocessed) in rows {
        // Each row starts over: an action already under way is reported
        // unprocessed, which the next test is about.
        std::thread::sleep(state::UPDATE_ACTION_TRANSITION * 3);
        let reply = batch(
            &emulator,
            "BatchApplyUpdateAction",
            &redis_update.name,
            clusters,
            groups,
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case}: an update the catalog holds answers the results message, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            processed(&reply),
            want_processed,
            "case {case}: expected {want_processed:?} processed, got {}",
            reply.body
        );
        assert_eq!(
            unprocessed_error_types(&reply),
            want_unprocessed,
            "case {case}: expected {want_unprocessed:?} unprocessed, got {}",
            reply.body
        );
        for unprocessed in unprocessed_error_types(&reply) {
            assert!(
                !unprocessed.is_empty(),
                "case {case}: an unprocessed action must say why, got {}",
                reply.body
            );
        }
    }

    // `ServiceUpdate$Engine` is "the Elasticache engine to which the update
    // applies", so an update released for one engine is no action on a cluster
    // running another: the resource is reported unprocessed rather than the
    // whole call refused.
    std::thread::sleep(state::UPDATE_ACTION_TRANSITION * 3);
    let crossed = batch(
        &emulator,
        "BatchApplyUpdateAction",
        &memcached_update.name,
        &["applied-one", "applied-memcached"],
        &[],
    )
    .await;
    assert_eq!(
        crossed.status, 200,
        "case engine mismatch: the call must answer the results message, got {} {}",
        crossed.status, crossed.body
    );
    assert_eq!(
        processed(&crossed),
        vec![("applied-memcached".to_string(), waiting.clone())],
        "case engine mismatch: only the cluster the update applies to is processed, got {}",
        crossed.body
    );
    assert_eq!(
        unprocessed_error_types(&crossed),
        vec![state::UPDATE_ACTION_INVALID_ERROR_TYPE],
        "case engine mismatch: the other cluster must be reported unprocessed, got {}",
        crossed.body
    );
}

#[tokio::test]
async fn an_update_action_is_stopped_only_while_it_is_under_way() {
    let emulator = Emulator::new();
    let published = catalog(&emulator).await;
    let update = update_for(&published, "redis");
    create_available_cluster(&emulator, "stopped-cluster", &[("Engine", "redis")]).await;

    // Nothing has been applied, so there is no action to stop: the resource is
    // reported unprocessed and the call still answers the results message.
    let nothing = batch(
        &emulator,
        "BatchStopUpdateAction",
        &update.name,
        &["stopped-cluster"],
        &[],
    )
    .await;
    assert_eq!(
        nothing.status, 200,
        "case not applied: the call must answer the results message, got {} {}",
        nothing.status, nothing.body
    );
    assert!(
        processed(&nothing).is_empty(),
        "case not applied: there is nothing to stop, got {}",
        nothing.body
    );
    assert_eq!(
        unprocessed_error_types(&nothing),
        vec![state::UPDATE_ACTION_INVALID_ERROR_TYPE],
        "case not applied: the resource must be reported unprocessed, got {}",
        nothing.body
    );

    // Applied, the action is under way, so applying it again is reported
    // unprocessed rather than restarting it.
    let applied = batch(
        &emulator,
        "BatchApplyUpdateAction",
        &update.name,
        &["stopped-cluster"],
        &[],
    )
    .await;
    assert_eq!(
        processed(&applied),
        vec![(
            "stopped-cluster".to_string(),
            state::UPDATE_ACTION_WAITING_TO_START.to_string()
        )],
        "case applied: the action must start, got {}",
        applied.body
    );
    let again = batch(
        &emulator,
        "BatchApplyUpdateAction",
        &update.name,
        &["stopped-cluster"],
        &[],
    )
    .await;
    assert!(
        processed(&again).is_empty(),
        "case applied twice: an action already under way is not restarted, got {}",
        again.body
    );
    assert_eq!(
        unprocessed_error_types(&again),
        vec![state::UPDATE_ACTION_INVALID_ERROR_TYPE],
        "case applied twice: the resource must be reported unprocessed, got {}",
        again.body
    );

    // Stopping the action it started moves it to `stopping`, read back from
    // the record the apply persisted.
    let stopped = batch(
        &emulator,
        "BatchStopUpdateAction",
        &update.name,
        &["stopped-cluster"],
        &[],
    )
    .await;
    assert_eq!(
        processed(&stopped),
        vec![(
            "stopped-cluster".to_string(),
            state::UPDATE_ACTION_STOPPING.to_string()
        )],
        "case stop: the action must be stopping, got {}",
        stopped.body
    );

    // The clock then carries it to `stopped`, which is where it ends: a stop
    // has nothing left to act on, and the update can be applied again.
    std::thread::sleep(state::UPDATE_ACTION_TRANSITION * 3);
    let twice = batch(
        &emulator,
        "BatchStopUpdateAction",
        &update.name,
        &["stopped-cluster"],
        &[],
    )
    .await;
    assert!(
        processed(&twice).is_empty(),
        "case stopped twice: a stopped action has nothing to stop, got {}",
        twice.body
    );
    let reapplied = batch(
        &emulator,
        "BatchApplyUpdateAction",
        &update.name,
        &["stopped-cluster"],
        &[],
    )
    .await;
    assert_eq!(
        processed(&reapplied),
        vec![(
            "stopped-cluster".to_string(),
            state::UPDATE_ACTION_WAITING_TO_START.to_string()
        )],
        "case reapplied: a stopped action can be started again, got {}",
        reapplied.body
    );

    // Left alone, the action the clock owes runs to completion.
    std::thread::sleep(state::UPDATE_ACTION_TRANSITION * 3);
    let settled = batch(
        &emulator,
        "BatchStopUpdateAction",
        &update.name,
        &["stopped-cluster"],
        &[],
    )
    .await;
    assert!(
        processed(&settled).is_empty(),
        "case completed: a completed action has nothing to stop, got {}",
        settled.body
    );
    assert!(
        settled
            .all("UnprocessedUpdateAction/ErrorMessage")
            .iter()
            .any(|message| message.contains(state::UPDATE_ACTION_COMPLETE)),
        "case completed: the clock must have carried the action to {}, got {}",
        state::UPDATE_ACTION_COMPLETE,
        settled.body
    );
}

#[tokio::test]
async fn a_batch_call_that_names_no_resource_is_refused() {
    let emulator = Emulator::new();
    let published = catalog(&emulator).await;
    let update = update_for(&published, "redis");

    // Both id lists are modelled `@length(min: 0)`, so a request naming no
    // resource reaches the handler — and there is nothing for either command
    // to act on. The axis is the operation.
    for action in ["BatchApplyUpdateAction", "BatchStopUpdateAction"] {
        let reply = batch(&emulator, action, &update.name, &[], &[]).await;
        assert_eq!(
            reply.status, 400,
            "case {action}: must be refused with HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {action}: must be refused with InvalidParameterValue, got {}",
            reply.body
        );
    }
}
