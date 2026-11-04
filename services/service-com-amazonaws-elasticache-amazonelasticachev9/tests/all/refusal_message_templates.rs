// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TEMPLATES this service words three of its refusals with —
//! the text a client that logs, branches on or asserts the refusal actually sees.
//!
//! Three rules are under test, each over a SET of inputs rather than the one request that
//! showed it:
//!
//! 1. A `CacheParameterGroupNotFoundFault` is worded with one of TWO sibling templates,
//!    `CacheParameterGroupnot found: <name>` (no separator before `not`) and
//!    `CacheParameterGroup not found: <name>`, each carrying the `CacheParameterGroupName`
//!    the request named and nothing from the store. Which of the two an operation answers
//!    with is that operation's own, so the suite asserts the template PER OPERATION:
//!    `DeleteCacheParameterGroup`, `DescribeCacheParameters` and
//!    `ResetCacheParameterGroup` answer the first, and `ModifyCacheParameterGroup`,
//!    `DescribeCacheParameterGroups` and `CreateCacheCluster`'s resolution of the member
//!    answer the second.
//! 2. `ModifyReplicationGroup` words its `ReplicationGroupNotFoundFault`
//!    `Replication group '<id>' not found` — the id single-quoted, no trailing stop —
//!    while `DescribeReplicationGroups`, asserted beside it on the same id, keeps the
//!    other template the service has for that fault, `ReplicationGroup <id> not found.`
//! 3. A parameter value the row's published `Parameter$AllowedValues` do not cover is
//!    refused `value:<the value> is outside of range:<the published set>`: the only place
//!    the REJECTED value is reported, which is what tells a client which of several
//!    `ParameterNameValues` of one request was refused. A value refused for its DATA TYPE
//!    instead keeps naming that type, so the two classes stay apart.
//!
//! The rows differ along what each rule generalises over: the way a name misses, its case,
//! its length and its form; the operation that resolves it; and, for the parameter rule,
//! the form of the published set (a bottom-bounded range, a bounded range, an enumeration,
//! an enumeration of flags) across both engine catalogs. The published set a message must
//! carry is READ from `DescribeCacheParameters` rather than written out here, so a row
//! cannot pass by agreeing with a literal.
//!
//! Every assertion goes over the wire through [`crate::harness`]. The template text itself
//! is written out here on purpose: a test that built it from the same helper the handlers
//! call would pass whatever that helper said.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::describe_cache_parameters::{create_group, describe, describe_ok, one};
use crate::harness::{params, Emulator, Reply};
use crate::modify_cache_parameter_group::{effective, modify, refused};

/// The family every custom group in this suite is created of, and a second one so the
/// parameter rows span both engine catalogs.
const REDIS_FAMILY: &str = "redis7";
const MEMCACHED_FAMILY: &str = "memcached1.6";

/// The first template: the type name and `not found: ` arrive JOINED, which is the
/// service's own spelling and not a slip here.
fn joined(name: &str) -> String {
    format!("CacheParameterGroupnot found: {name}")
}

/// The second template, the one with the separator.
fn spaced(name: &str) -> String {
    format!("CacheParameterGroup not found: {name}")
}

/// Assert `reply` is the refusal `status` / `code` worded exactly `message`.
fn assert_refusal(case: &str, reply: &Reply, status: u16, code: &str, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (status, code),
        "case {case}: expected {status} {code}, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case}: expected message {message:?}, got {:?}",
        reply.error_message()
    );
}

/// `DescribeCacheParameterGroups` for one group by name.
async fn describe_group(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DescribeCacheParameterGroups",
            &params(&[("CacheParameterGroupName", name)]),
        )
        .await
}

/// `DeleteCacheParameterGroup` for one group by name.
async fn delete_group(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DeleteCacheParameterGroup",
            &params(&[("CacheParameterGroupName", name)]),
        )
        .await
}

/// `ResetCacheParameterGroup` for the whole of one group by name.
async fn reset_group(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "ResetCacheParameterGroup",
            &params(&[
                ("CacheParameterGroupName", name),
                ("ResetAllParameters", "true"),
            ]),
        )
        .await
}

/// `CreateCacheCluster` naming `name` as its `CacheParameterGroupName`, under a cluster id
/// of its own so a refusal cannot be the id being taken.
async fn create_cluster_with_group(emulator: &Emulator, id: &str, name: &str) -> Reply {
    emulator
        .call(
            "CreateCacheCluster",
            &params(&[("CacheClusterId", id), ("CacheParameterGroupName", name)]),
        )
        .await
}

/// Assert every operation that resolves a `CacheParameterGroupName` answers `name` with the
/// modelled 404 and with ITS OWN template for that fault.
async fn assert_both_templates(emulator: &Emulator, index: usize, case: &str, name: &str) {
    const CODE: &str = "CacheParameterGroupNotFound";

    for (operation, reply) in [
        ("DeleteCacheParameterGroup", delete_group(emulator, name).await),
        (
            "DescribeCacheParameters",
            describe(emulator, name, &[]).await,
        ),
        ("ResetCacheParameterGroup", reset_group(emulator, name).await),
    ] {
        assert_refusal(
            &format!("{case} through {operation}"),
            &reply,
            404,
            CODE,
            &joined(name),
        );
    }

    for (operation, reply) in [
        (
            "ModifyCacheParameterGroup",
            modify(emulator, name, &[("timeout", "300")]).await,
        ),
        (
            "DescribeCacheParameterGroups",
            describe_group(emulator, name).await,
        ),
        (
            "CreateCacheCluster",
            create_cluster_with_group(emulator, &format!("group-miss-{index}"), name).await,
        ),
    ] {
        assert_refusal(
            &format!("{case} through {operation}"),
            &reply,
            404,
            CODE,
            &spaced(name),
        );
    }
}

#[tokio::test]
async fn each_operation_words_the_parameter_group_not_found_fault_with_its_own_template() {
    let emulator = Emulator::new();

    // The axis: the way the name misses the store and the FORM of the name — its length
    // (one letter, and one at the documented bound), its case, and the characters it
    // carries. Every row is a name the service's identifier grammar admits, so each
    // operation reaches its not-found rather than refusing the name's form.
    let at_bound = format!("g{}", "a".repeat(state::PARAMETER_GROUP_NAME_MAX_LEN - 1));
    let rows: [(&str, &str); 5] = [
        ("a name nothing ever created", "never-created-group"),
        ("a one-letter name", "g"),
        ("a mixed-case name", "Mixed-Case-Group"),
        ("a name carrying digits and hyphens", "group-7-of-9"),
        ("a name at the documented length bound", at_bound.as_str()),
    ];

    for (index, (case, name)) in rows.iter().enumerate() {
        assert_both_templates(&emulator, index, case, name).await;
    }

    // And the way the recorded instance reaches it: a group that DID exist, deleted, then
    // addressed again — so the refusal is answered from the store having lost the group
    // rather than from a name it never held.
    let deleted = "created-then-deleted";
    create_group(&emulator, deleted, REDIS_FAMILY).await;
    let gone = delete_group(&emulator, deleted).await;
    assert_eq!(
        gone.status, 200,
        "the first delete must be accepted, got {} {}",
        gone.status, gone.body
    );
    assert_both_templates(&emulator, rows.len(), "a deleted name", deleted).await;
}

#[tokio::test]
async fn modify_replication_group_words_its_not_found_fault_its_own_way() {
    let emulator = Emulator::new();

    // The axis: the FORM of the id — its length (one letter, and one at the bound a
    // lookup admits), its case, and the characters it carries.
    let at_bound = format!("r{}", "g".repeat(state::REPLICATION_GROUP_ID_MAX_LEN - 1));
    let ids = [
        "absent-rg",
        "r",
        "Mixed-Case-RG",
        "rg-7-of-9",
        at_bound.as_str(),
    ];

    for id in ids {
        let modified = emulator
            .call(
                "ModifyReplicationGroup",
                &params(&[("ReplicationGroupId", id), ("ApplyImmediately", "true")]),
            )
            .await;
        assert_refusal(
            &format!("{id} through ModifyReplicationGroup"),
            &modified,
            404,
            "ReplicationGroupNotFoundFault",
            &format!("Replication group '{id}' not found"),
        );

        // The same id through the read, which words the fault the OTHER way: the two
        // operations are told apart by their message on the live service, so a single
        // template for the service cannot be right.
        let described = emulator
            .call(
                "DescribeReplicationGroups",
                &params(&[("ReplicationGroupId", id)]),
            )
            .await;
        assert_refusal(
            &format!("{id} through DescribeReplicationGroups"),
            &described,
            404,
            "ReplicationGroupNotFoundFault",
            &format!("ReplicationGroup {id} not found."),
        );
    }
}

/// The published `AllowedValues` of `parameter` of `group`, as a read of the group reports
/// them — the set a refusal must name is the one a client can see.
async fn published_set(emulator: &Emulator, case: &str, group: &str, parameter: &str) -> String {
    let reply = describe_ok(emulator, group, &[]).await;
    let allowed = one(&reply, case, parameter)
        .field("AllowedValues")
        .unwrap_or_default()
        .to_string();
    assert!(
        !allowed.is_empty(),
        "case {case}: this row needs {parameter} to publish a value set to be outside of"
    );
    allowed
}

#[tokio::test]
async fn a_value_outside_a_published_set_is_refused_with_the_value_and_the_set() {
    let emulator = Emulator::new();
    let redis = "outside-redis";
    let memcached = "outside-memcached";
    create_group(&emulator, redis, REDIS_FAMILY).await;
    create_group(&emulator, memcached, MEMCACHED_FAMILY).await;

    // The axis: the form of the published set the value falls outside of — a
    // bottom-bounded range, a bounded range at each end, a range with a literal term
    // beside it, an enumeration of words (including one that misses by case alone) and an
    // enumeration of flags — over both engine catalogs, and the form of the rejected value
    // (negative, zero, past the top, not a member of the enumeration).
    let rows: [(&str, &str, &str); 9] = [
        (redis, "slowlog-max-len", "-5"),
        (redis, "slowlog-log-slower-than", "-1"),
        (redis, "active-defrag-cycle-max", "0"),
        (redis, "active-defrag-cycle-max", "76"),
        (redis, "timeout", "19"),
        (redis, "maxmemory-policy", "not-a-policy"),
        (redis, "maxmemory-policy", "VOLATILE-LRU"),
        (redis, "activedefrag", "true"),
        (memcached, "slab_automove", "3"),
    ];

    for (group, parameter, value) in rows {
        let case = format!("{group}/{parameter}={value:?}");
        let allowed = published_set(&emulator, &case, group, parameter).await;
        // What the group reports for the parameter before the refused request, so the
        // read below shows the refusal changed nothing.
        let before = effective(&emulator, &case, group, parameter).await;

        let reply = modify(&emulator, group, &[(parameter, value)]).await;
        assert_refusal(
            &case,
            &reply,
            400,
            "InvalidParameterValue",
            &format!("value:{value} is outside of range:{allowed}"),
        );

        let after = effective(&emulator, &case, group, parameter).await;
        assert_eq!(
            after, before,
            "case {case}: the refused request must leave the parameter as it was, got {after:?}"
        );
        assert_eq!(
            after.1,
            state::PARAMETER_SOURCE_SYSTEM,
            "case {case}: the parameter must still report the engine's own value, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_value_that_is_not_of_the_parameters_data_type_still_names_that_type() {
    let emulator = Emulator::new();
    let redis = "typed-redis";
    let memcached = "typed-memcached";
    create_group(&emulator, redis, REDIS_FAMILY).await;
    create_group(&emulator, memcached, MEMCACHED_FAMILY).await;

    // The counterpart of the rule above, so a value refused for its TYPE cannot be
    // reported as one outside the published set: the axis is the data type the value is
    // not one of (integer, float, boolean) and the way it is not one (a word, a fraction
    // where only whole numbers count, the empty value, a flag outside the pair).
    let rows: [(&str, &str, &str); 5] = [
        (redis, "active-defrag-cycle-max", "not-a-number"),
        (redis, "active-defrag-cycle-max", "1.5"),
        (redis, "slowlog-max-len", ""),
        (memcached, "chunk_size_growth_factor", "not-a-float"),
        (memcached, "cas_disabled", "2"),
    ];

    for (group, parameter, value) in rows {
        let case = format!("{group}/{parameter}={value:?}");
        let reply = modify(&emulator, group, &[(parameter, value)]).await;
        let message = refused(&case, &reply, 400, "InvalidParameterValue").to_string();
        let published = describe_ok(&emulator, group, &[]).await;
        let reported_type = one(&published, &case, parameter)
            .field("DataType")
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(&reported_type),
            "case {case}: the refusal must name the data type {reported_type} the value is not \
             one of, got {message}"
        );
        assert!(
            !message.starts_with("value:"),
            "case {case}: a value refused for its type must not be reported as one outside the \
             published set, got {message}"
        );
    }
}
