// SPDX-License-Identifier: CC0-1.0

//! What this service's refusals SAY and what its parameter catalog PUBLISHES, end to end
//! over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! A message is part of the contract: one error code covers many rules, so the sentence
//! is how a caller learns WHICH rule it broke, and a catalog value is what a client
//! validates against before it writes. The rules under test, each over a SET of inputs
//! rather than one request:
//!
//! * a refusal's message is a TEMPLATE of the service: its fixed text is fixed, and its
//!   variable parts come from the request or from the service's own metadata — never a
//!   request value spliced into a sentence that has none;
//! * the cluster update states an absent cluster, and the parameter-group reads state an
//!   absent group, in fixed text that names no resource, while the paths recorded
//!   reporting the name they could not resolve still report it;
//! * a cluster's shard and replica counts are refused by the service's own sentence about
//!   the number of shards and the number of replicas per shard, whichever write path and
//!   whichever member carried the count;
//! * a value outside a modelled enum, and a list longer than its modelled maximum, are
//!   refused in the service's own wording for that type rather than the framework's
//!   generic constraint clause;
//! * a parameter-group DELETE validates the identifier's SYNTAX before it resolves the
//!   group, so a malformed name is answered with the identifier rule rather than with
//!   whatever the store holds under it;
//! * a default parameter group of a search baseline describes itself as a customized
//!   default group of its LINE's family with the vector-search setting on;
//! * the published parameter catalog is service data: every family carrying a setting
//!   reports the same text and the same value set for it, a group a caller creates
//!   reports it too, and an update of another setting leaves it alone;
//! * what a parameter PUBLISHES in `AllowedValues` is exactly what an update accepts for
//!   it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_memorydb_amazonmemorydb::ops::types::SOURCE_TYPE_VALUES;
use service_com_amazonaws_memorydb_amazonmemorydb::state::MAX_RESOURCE_NAME_LEN;
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{call, error_code, ok, router};
use crate::parameters::{create_group, field, parameters, update};

/// A node type the service publishes, for the cluster writes below.
const NODE_TYPE: &str = "db.t4g.small";

/// The ACL every MemoryDB account already has, so a cluster write under test carries no
/// ACL of its own.
const BUILTIN_ACL: &str = "open-access";

/// A parameter group family the engine catalog publishes, and the one whose line carries
/// MemoryDB's own vector-search settings.
const VALKEY_FAMILY: &str = "memorydb_valkey7";

/// What the service answers a cluster it holds no record of on the UPDATE path.
const CLUSTER_NOT_FOUND: &str = "Cluster not found.";

/// What the service answers a parameter group it holds no record of on the paths that
/// state the condition alone.
const PARAMETER_GROUP_NOT_FOUND: &str = "Parameter Group not found.";

/// What the service answers a shard count below the one shard a cluster must have.
const TOO_FEW_SHARDS: &str = "Number of shards cannot be less than 1.";

/// What the service answers a replica count outside the bounds it publishes for
/// `NumReplicasPerShard` ("The maximum is 5").
const REPLICA_COUNT_OUT_OF_RANGE: &str = "The number of replicas per shard must be within 0 and 5.";

/// What the service answers a `SourceType` no `DescribeEvents` serves. The token list is
/// the service's own order; [`the_event_source_type_refusal_lists_the_modelled_types`]
/// pins it to the modelled value set.
const INVALID_EVENT_SOURCE_TYPE: &str =
    "Invalid event source type. Valid types are node,cluster,parameter-group,subnet-group,user,acl";

/// The clauses the service's identifier rule ends with, whichever subject a kind of
/// resource states them about.
const IDENTIFIER_RULE_CLAUSES: &str = "must begin with a letter; must contain only ASCII \
                                       letters, digits, and hyphens; and must not end \
                                       with a hyphen or contain two consecutive hyphens.";

/// The `message` of a failure body, under either spelling of the member.
fn message(body: &Value, case: &str) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {body}"))
        .to_string()
}

/// Call an operation that must be refused, and assert its status, code and message.
async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    case: &str,
    code: &str,
    wanted: &str,
) {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, code),
        "case {case:?}: expected 400 {code}, got {status} with {answered}"
    );
    assert_eq!(
        message(&answered, case),
        wanted,
        "case {case:?}: expected the service's sentence, got {answered}"
    );
}

/// Create one cluster the update cases can act on.
async fn cluster(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateCluster",
        &json!({ "ClusterName": name, "NodeType": NODE_TYPE, "ACLName": BUILTIN_ACL }),
        name,
    )
    .await;
}

/// The names of the default parameter groups the account holds, each with the family and
/// the description it reports.
async fn default_groups(router: &MultiServiceRouter) -> Vec<(String, String, String)> {
    let listed = ok(
        router,
        "DescribeParameterGroups",
        &json!({ "MaxResults": 100 }),
        "default groups",
    )
    .await;
    listed["ParameterGroups"]
        .as_array()
        .unwrap_or_else(|| panic!("case {:?}: expected ParameterGroups in {listed}", "defaults"))
        .iter()
        .map(|group| {
            (
                field(group, "Name", "default group"),
                field(group, "Family", "default group"),
                field(group, "Description", "default group"),
            )
        })
        .filter(|(name, _, _)| name.starts_with("default."))
        .collect()
}

/// A cluster the update path cannot resolve is stated in fixed text that names no
/// cluster, for every shape an unresolved name takes — and the same call against a
/// cluster that IS in state is served, so the refusal is the lookup and not a blanket
/// one.
#[tokio::test]
async fn the_cluster_update_states_an_absent_cluster_without_naming_it() {
    let router = router();
    cluster(&router, "present-cluster").await;

    // The rows differ in the shape of a name that resolves to nothing: a plain one, one
    // written in mixed case, a one-character one and one at the length bound.
    let at_bound: String = std::iter::once('a')
        .chain(std::iter::repeat_n('b', MAX_RESOURCE_NAME_LEN - 1))
        .collect();
    let absent = [
        "no-such-cluster".to_string(),
        "No-Such-Cluster".to_string(),
        "c".to_string(),
        at_bound,
    ];
    for name in &absent {
        let case = format!("UpdateCluster names {name:?}");
        refused(
            &router,
            "UpdateCluster",
            &json!({ "ClusterName": name, "Description": "moved" }),
            &case,
            "ClusterNotFoundFault",
            CLUSTER_NOT_FOUND,
        )
        .await;
        // The sentence has no variable part, so no spelling of the request's name can
        // appear in it.
        assert!(
            !CLUSTER_NOT_FOUND.contains(name.as_str())
                && !CLUSTER_NOT_FOUND.contains(&name.to_ascii_lowercase()),
            "case {case:?}: the sentence must not carry the requested name"
        );
    }

    let served = ok(
        &router,
        "UpdateCluster",
        &json!({ "ClusterName": "present-cluster", "Description": "moved" }),
        "a cluster in state",
    )
    .await;
    assert_eq!(
        served["Cluster"]["Description"],
        json!("moved"),
        "case {:?}: the update must be applied, got {served}",
        "a cluster in state"
    );
}

/// A shard count below one, and a replica count outside the published bounds, are refused
/// with the service's own sentence about that count — whichever write path carried it and
/// whichever member it arrived in — and a count the bound admits is accepted by the same
/// path.
#[tokio::test]
async fn a_layout_count_outside_its_bounds_is_refused_with_the_services_sentence() {
    let router = router();
    cluster(&router, "layout-cluster").await;

    // The rows span the class below the bound: the first value under it, a negative one
    // and the extreme the member's type admits.
    for count in [0, -1, i32::MIN] {
        let case = format!("CreateCluster NumShards={count}");
        refused(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": "shardless",
                "NodeType": NODE_TYPE,
                "ACLName": BUILTIN_ACL,
                "NumShards": count,
            }),
            &case,
            "InvalidParameterValueException",
            TOO_FEW_SHARDS,
        )
        .await;
        let case = format!("UpdateCluster ShardCount={count}");
        refused(
            &router,
            "UpdateCluster",
            &json!({
                "ClusterName": "layout-cluster",
                "ShardConfiguration": { "ShardCount": count },
            }),
            &case,
            "InvalidParameterValueException",
            TOO_FEW_SHARDS,
        )
        .await;
    }

    // The rows span the replica class on both sides: below the floor, above the ceiling,
    // and the extreme the member's type admits.
    for count in [-1, 6, i32::MAX] {
        let case = format!("CreateCluster NumReplicasPerShard={count}");
        refused(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": "replicaless",
                "NodeType": NODE_TYPE,
                "ACLName": BUILTIN_ACL,
                "NumReplicasPerShard": count,
            }),
            &case,
            "InvalidParameterValueException",
            REPLICA_COUNT_OUT_OF_RANGE,
        )
        .await;
        let case = format!("UpdateCluster ReplicaCount={count}");
        refused(
            &router,
            "UpdateCluster",
            &json!({
                "ClusterName": "layout-cluster",
                "ReplicaConfiguration": { "ReplicaCount": count },
            }),
            &case,
            "InvalidParameterValueException",
            REPLICA_COUNT_OUT_OF_RANGE,
        )
        .await;
    }

    // AT each bound the write is served, so the sentences state bounds the service
    // really enforces.
    for (index, (shards, replicas)) in [(1, 0), (2, 5)].iter().enumerate() {
        let name = format!("at-the-bounds-{index}");
        let created = ok(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": &name,
                "NodeType": NODE_TYPE,
                "ACLName": BUILTIN_ACL,
                "NumShards": shards,
                "NumReplicasPerShard": replicas,
            }),
            &name,
        )
        .await;
        assert_eq!(
            created["Cluster"]["NumberOfShards"],
            json!(shards),
            "case {name:?}: expected the layout the request asked for, got {created}"
        );
    }
}

/// `DescribeEvents` refuses a `SourceType` it does not serve with the service's own
/// sentence — the same one whatever the rejected value is, and never carrying that value
/// — and serves every type the model declares.
#[tokio::test]
async fn the_event_source_type_refusal_lists_the_modelled_types() {
    let router = router();

    // The rows differ in HOW the value misses the value set: a word of its own, a
    // modelled token in the wrong case, a near-miss plural, one with trailing space, and
    // the empty string.
    for value in ["bogus", "Cluster", "clusters", "node ", ""] {
        let case = format!("DescribeEvents SourceType={value:?}");
        refused(
            &router,
            "DescribeEvents",
            &json!({ "SourceType": value }),
            &case,
            "InvalidParameterValueException",
            INVALID_EVENT_SOURCE_TYPE,
        )
        .await;
    }

    // The sentence's list is the modelled value set, in the service's own order: it is
    // compared against the GENERATED `SourceType` catalog the handler and the boundary
    // match on, so neither the sentence nor the check can drift from the model.
    let listed: BTreeSet<&str> = INVALID_EVENT_SOURCE_TYPE
        .rsplit(' ')
        .next()
        .unwrap_or_default()
        .split(',')
        .collect();
    let modelled: BTreeSet<&str> = SOURCE_TYPE_VALUES.iter().copied().collect();
    assert_eq!(
        listed, modelled,
        "case {:?}: the sentence must list exactly the modelled SourceType values, got \
         {listed:?}",
        "source type list"
    );

    // And every modelled type is served rather than refused.
    for value in &modelled {
        let case = format!("DescribeEvents SourceType={value:?}");
        let body = ok(
            &router,
            "DescribeEvents",
            &json!({ "SourceType": value }),
            &case,
        )
        .await;
        assert!(
            body.get("Events").is_some(),
            "case {case:?}: a modelled source type must be served, got {body}"
        );
    }
}

/// `DescribeServiceUpdates` refuses a `Status` entry the model does not declare by
/// reporting that entry back in the service's own sentence, wherever in the list it sat,
/// and refuses a `ClusterNames` list longer than the model's maximum with the service's
/// own sentence about that maximum — not the framework's constraint clause.
#[tokio::test]
async fn the_service_update_filters_are_refused_in_the_services_own_wording() {
    let router = router();

    // The rows differ in the rejected value AND in its position in the list, which the
    // sentence says nothing about.
    let rows: &[(&str, Value)] = &[
        ("bogus", json!(["bogus"])),
        ("BOGUS", json!(["BOGUS"])),
        ("done", json!(["available", "done"])),
        ("in progress", json!(["complete", "in progress"])),
    ];
    for (rejected, status) in rows {
        let case = format!("DescribeServiceUpdates Status={status}");
        refused(
            &router,
            "DescribeServiceUpdates",
            &json!({ "Status": status }),
            &case,
            "InvalidParameterValueException",
            &format!("The input service update status {rejected} is not valid."),
        )
        .await;
    }

    // A list of statuses the model DOES declare gets past validation, so the refusals
    // above are the value set and not a blanket one.
    let (_, answered) = call(
        &router,
        "DescribeServiceUpdates",
        &json!({ "Status": ["available", "complete"] }),
    )
    .await;
    assert_ne!(
        error_code(&answered).as_str(),
        "InvalidParameterValueException",
        "case {:?}: modelled statuses must not be refused as values, got {answered}",
        "modelled statuses"
    );

    // The `ClusterNames` maximum is the model's own, so the sentence names it; the rows
    // are one past it and well past it.
    for count in [21, 40] {
        let names: Vec<String> = (0..count).map(|index| format!("cluster-{index}")).collect();
        let case = format!("DescribeServiceUpdates with {count} cluster names");
        refused(
            &router,
            "DescribeServiceUpdates",
            &json!({ "ClusterNames": names }),
            &case,
            "InvalidParameterValueException",
            "Too many cluster names provided. Maximum is 20.",
        )
        .await;
    }
    let at_bound: Vec<String> = (0..20).map(|index| format!("cluster-{index}")).collect();
    let (_, answered) = call(
        &router,
        "DescribeServiceUpdates",
        &json!({ "ClusterNames": at_bound }),
    )
    .await;
    assert_ne!(
        error_code(&answered).as_str(),
        "InvalidParameterValueException",
        "case {:?}: a list AT the maximum must not be refused, got {answered}",
        "20 cluster names"
    );
}

/// `DeleteParameterGroup` checks the identifier's SYNTAX before it resolves the group:
/// every malformed name is answered with the rule it broke — whatever the store holds
/// under that name — and the group it could have named is still there afterwards. A
/// well-formed name reaches the lookup, and one that resolves is deleted.
#[tokio::test]
async fn the_parameter_group_delete_validates_the_identifier_before_it_resolves_the_group() {
    let router = router();
    let identifier_rule = format!(
        "The parameter ParameterGroupName is not a valid identifier. Identifiers \
         {IDENTIFIER_RULE_CLAUSES}"
    );

    // The rows break the spelling clause one part at a time, and include the name of a
    // group MemoryDB itself owns — which is dotted, so it is not a legal identifier and
    // never reaches the default-group protection.
    let malformed = [
        "default.memorydb-valkey7",
        "default.memorydb-redis7.search",
        "9lives",
        "-leading",
        "under_score",
        "two words",
        "trailing-",
        "double--hyphen",
    ];
    for name in malformed {
        let case = format!("DeleteParameterGroup names {name:?}");
        refused(
            &router,
            "DeleteParameterGroup",
            &json!({ "ParameterGroupName": name }),
            &case,
            "InvalidParameterValueException",
            &identifier_rule,
        )
        .await;
    }
    // A name past the length bound breaks the OTHER clause of the same rule, and is
    // refused by it rather than by the lookup.
    let past_bound: String = std::iter::once('a')
        .chain(std::iter::repeat_n('b', MAX_RESOURCE_NAME_LEN))
        .collect();
    refused(
        &router,
        "DeleteParameterGroup",
        &json!({ "ParameterGroupName": &past_bound }),
        "DeleteParameterGroup names a name past the length bound",
        "InvalidParameterValueException",
        &format!("Parameter group name length must be less than {MAX_RESOURCE_NAME_LEN}."),
    )
    .await;

    // Nothing was deleted: the groups MemoryDB owns are still reported.
    let still_there: Vec<String> = default_groups(&router)
        .await
        .into_iter()
        .map(|(name, _, _)| name)
        .collect();
    for name in ["default.memorydb-valkey7", "default.memorydb-redis7.search"] {
        assert!(
            still_there.iter().any(|held| held == name),
            "case {name:?}: a refused delete must leave the group in place, got \
             {still_there:?}"
        );
    }

    // A WELL-FORMED name reaches the lookup, and the lookup answers for it.
    refused(
        &router,
        "DeleteParameterGroup",
        &json!({ "ParameterGroupName": "no-such-group" }),
        "DeleteParameterGroup names a well-formed absent group",
        "ParameterGroupNotFoundFault",
        PARAMETER_GROUP_NOT_FOUND,
    )
    .await;
    create_group(&router, "deletable-group", VALKEY_FAMILY).await;
    let deleted = ok(
        &router,
        "DeleteParameterGroup",
        &json!({ "ParameterGroupName": "deletable-group" }),
        "a group the caller owns",
    )
    .await;
    assert_eq!(
        deleted["ParameterGroup"]["Name"],
        json!("deletable-group"),
        "case {:?}: the delete must report the group it removed, got {deleted}",
        "a group the caller owns"
    );
    refused(
        &router,
        "DescribeParameterGroups",
        &json!({ "ParameterGroupName": "deletable-group" }),
        "a deleted group",
        "ParameterGroupNotFoundFault",
        PARAMETER_GROUP_NOT_FOUND,
    )
    .await;
}

/// An absent parameter group is answered with the template of the operation that
/// resolved it: the paths whose subject is the group itself state the condition and name
/// no group, and the paths recorded reporting the name they could not resolve report it.
/// Checked over a set of absent names, including ones the store folds differently from
/// how the request spelled them.
#[tokio::test]
async fn the_parameter_group_lookup_states_the_template_of_the_operation_that_resolved_it() {
    let router = router();
    cluster(&router, "repointed-cluster").await;

    for name in ["no-such-group", "No-Such-Group", "absent-group-2"] {
        let folded = name.to_ascii_lowercase();
        // The paths that state the condition alone.
        for (operation, body) in [
            (
                "DescribeParameterGroups",
                json!({ "ParameterGroupName": name }),
            ),
            ("DescribeParameters", json!({ "ParameterGroupName": name })),
            (
                "ResetParameterGroup",
                json!({ "ParameterGroupName": name, "AllParameters": true }),
            ),
            (
                "DeleteParameterGroup",
                json!({ "ParameterGroupName": name }),
            ),
        ] {
            let case = format!("{operation} names {name:?}");
            refused(
                &router,
                operation,
                &body,
                &case,
                "ParameterGroupNotFoundFault",
                PARAMETER_GROUP_NOT_FOUND,
            )
            .await;
        }
        // The paths that report the name they could not resolve.
        for (operation, body) in [
            (
                "UpdateParameterGroup",
                json!({
                    "ParameterGroupName": name,
                    "ParameterNameValues": [{ "ParameterName": "timeout", "ParameterValue": "30" }],
                }),
            ),
            (
                "CreateCluster",
                json!({
                    "ClusterName": format!("pointing-at-{folded}"),
                    "NodeType": NODE_TYPE,
                    "ACLName": BUILTIN_ACL,
                    "ParameterGroupName": name,
                }),
            ),
            (
                "UpdateCluster",
                json!({ "ClusterName": "repointed-cluster", "ParameterGroupName": name }),
            ),
        ] {
            let case = format!("{operation} names {name:?}");
            refused(
                &router,
                operation,
                &body,
                &case,
                "ParameterGroupNotFoundFault",
                &format!("ParameterGroup not found: {folded}"),
            )
            .await;
        }
    }
}

/// A default parameter group of a SEARCH baseline describes itself as a customized
/// default group of its line's family with the vector-search setting on — the family
/// without the `.search` qualifier its own name carries — and every other default group
/// describes the baseline it is of. Checked over every default group the account holds,
/// and against the value set that setting publishes.
#[tokio::test]
async fn a_search_baseline_default_group_describes_itself_by_its_lines_family() {
    let router = router();
    let groups = default_groups(&router).await;
    assert!(
        groups.len() > 1,
        "case {:?}: the account must hold the default groups, got {groups:?}",
        "default groups"
    );

    // The token the search setting takes to be ON is the one the description names, read
    // from the catalog rather than assumed.
    let search_tokens = parameters(&router, "default.memorydb-valkey7")
        .await
        .get("search-enabled")
        .map(|parameter| field(parameter, "AllowedValues", "search-enabled"))
        .unwrap_or_else(|| panic!("case {:?}: the Valkey line publishes search-enabled", "tokens"));
    let on_token = search_tokens
        .split(',')
        .next()
        .unwrap_or_default()
        .to_string();

    let mut baselines = 0_usize;
    for (name, family, description) in &groups {
        let case = format!("default group {name}");
        let is_search_baseline = name.ends_with(".search");
        let wanted = if is_search_baseline {
            baselines += 1;
            assert!(
                !family.contains(".search"),
                "case {case:?}: a search baseline reports its line's family, got {family:?}"
            );
            format!("Customized default parameter group for {family} with search-enabled {on_token}")
        } else {
            // The baseline a group is of is its own name read as a family token.
            let baseline = name
                .strip_prefix("default.")
                .unwrap_or_else(|| panic!("case {case:?}: a default group is named default.<baseline>"))
                .replacen("memorydb-", "memorydb_", 1);
            format!("Default parameter group for {baseline}")
        };
        assert_eq!(
            description, &wanted,
            "case {case:?}: expected the description the service gives this baseline, got \
             {description:?}"
        );
    }
    assert!(
        baselines >= 2,
        "case {:?}: the account holds a search baseline per line that has one, got \
         {baselines}",
        "search baselines"
    );
}

/// One row of the published catalog: a setting, the description and the value set the
/// service publishes for it, and whether only the Valkey-backed families carry it.
///
/// Source: the `DescribeParameters` listing of the recorded service's default parameter
/// groups. The rows include settings this issue did not cite (`activedefrag`,
/// `maxmemory-policy`, `timeout`, `stream-node-max-entries`), so the rule is checked
/// beyond the cited names. `description` is `None` for a setting whose published wording
/// no recorded listing pins, so only the value set it publishes is asserted for that row.
struct CatalogRow {
    name: &'static str,
    description: Option<&'static str>,
    allowed_values: &'static str,
    valkey_only: bool,
}

const CATALOG: &[CatalogRow] = &[
    CatalogRow {
        name: "acllog-max-len",
        description: Some("The maximum length of the ACL Log"),
        allowed_values: "1-10000",
        valkey_only: false,
    },
    CatalogRow {
        name: "activedefrag",
        description: Some("Enabled active memory defragmentation"),
        allowed_values: "yes,no",
        valkey_only: false,
    },
    CatalogRow {
        name: "active-expire-effort",
        description: Some(
            "The amount of effort that redis uses to expire items in the active \
             expiration job",
        ),
        allowed_values: "1-10",
        valkey_only: false,
    },
    CatalogRow {
        name: "hll-sparse-max-bytes",
        description: Some("HyperLogLog sparse representation bytes limit"),
        allowed_values: "1-16000",
        valkey_only: false,
    },
    CatalogRow {
        name: "lfu-decay-time",
        description: Some(
            "The amount of time in minutes to decrement the key counter for LFU \
             eviction policyd",
        ),
        allowed_values: "0-",
        valkey_only: false,
    },
    CatalogRow {
        name: "lfu-log-factor",
        description: Some(
            "The log factor for incrementing key counter for LFU eviction policy",
        ),
        allowed_values: "1-",
        valkey_only: false,
    },
    CatalogRow {
        name: "list-compress-depth",
        description: Some(
            "Number of quicklist ziplist nodes from each side of the list to exclude \
             from compression. The head and tail of the list are always uncompressed \
             for fast push/pop operations",
        ),
        allowed_values: "0-",
        valkey_only: false,
    },
    CatalogRow {
        name: "maxmemory-policy",
        description: Some("Max memory policy"),
        allowed_values: "volatile-lru,allkeys-lru,volatile-lfu,allkeys-lfu,volatile-random,\
                         allkeys-random,volatile-ttl,noeviction",
        valkey_only: false,
    },
    CatalogRow {
        name: "maxmemory-samples",
        description: Some("Max memory samples"),
        allowed_values: "1-",
        valkey_only: false,
    },
    CatalogRow {
        name: "stream-node-max-entries",
        description: Some("The maximum number of items a single node in a stream can contain"),
        allowed_values: "0-",
        valkey_only: false,
    },
    CatalogRow {
        name: "tcp-keepalive",
        description: Some("If non-zero, send ACKs every given number of seconds"),
        allowed_values: "0-",
        valkey_only: false,
    },
    CatalogRow {
        name: "timeout",
        description: Some(
            "Close connection if client is idle for a given number of seconds, or never \
             if 0",
        ),
        allowed_values: "0,20-",
        valkey_only: false,
    },
    CatalogRow {
        name: "search-enabled",
        description: None,
        allowed_values: "yes,no",
        valkey_only: true,
    },
];

/// The published catalog is SERVICE data: every family that carries a setting reports the
/// same description and the same value set for it, a group a caller creates in that
/// family reports them too, and an update of ANOTHER setting leaves them alone.
#[tokio::test]
async fn the_published_catalog_reports_the_services_own_text_for_every_carrying_family() {
    let router = router();
    let families = ["memorydb_redis6", "memorydb_redis7", VALKEY_FAMILY];

    for (index, family) in families.iter().enumerate() {
        let group = format!("catalog-text-{index}");
        create_group(&router, &group, family).await;
        let valkey = family.starts_with("memorydb_valkey");
        let default_group = format!("default.{}", family.replace('_', "-"));

        for source in [default_group.as_str(), group.as_str()] {
            let reported = parameters(&router, source).await;
            for row in CATALOG {
                let case = format!("{source}/{}", row.name);
                let Some(parameter) = reported.get(row.name) else {
                    assert!(
                        row.valkey_only && !valkey,
                        "case {case:?}: the family must carry this setting, got the \
                         listing {:?}",
                        reported.keys().collect::<Vec<_>>()
                    );
                    continue;
                };
                if let Some(description) = row.description {
                    assert_eq!(
                        field(parameter, "Description", &case),
                        description,
                        "case {case:?}: expected the published description, got {parameter}"
                    );
                }
                assert_eq!(
                    field(parameter, "AllowedValues", &case),
                    row.allowed_values,
                    "case {case:?}: expected the published value set, got {parameter}"
                );
            }
        }

        // An update of one setting rewrites no other setting's published record.
        let before = parameters(&router, &group).await;
        let (status, answered) = update(&router, &group, &[("timeout", "300")]).await;
        assert_eq!(
            status, 200,
            "case {family:?}: expected the update to be served, got {status} with {answered}"
        );
        let after = parameters(&router, &group).await;
        for row in CATALOG {
            if row.name == "timeout" {
                continue;
            }
            let case = format!("{family}/{} after an unrelated update", row.name);
            assert_eq!(
                after.get(row.name),
                before.get(row.name),
                "case {case:?}: an update must leave another setting's record alone"
            );
        }
    }
}

/// What a parameter PUBLISHES in `AllowedValues` is what an update accepts for it: every
/// token of a published token set round-trips onto the group, and a token outside the set
/// is refused with the sentence built from that same set. Checked over every token-valued
/// setting a Valkey family carries, which is the whole published set rather than a chosen
/// parameter.
#[tokio::test]
async fn a_token_setting_accepts_exactly_the_tokens_it_publishes() {
    let router = router();
    create_group(&router, "token-domains", VALKEY_FAMILY).await;
    let reported = parameters(&router, "token-domains").await;

    let mut checked = 0_usize;
    for (name, parameter) in &reported {
        let case = format!("token-domains/{name}");
        if field(parameter, "DataType", &case) != "string" {
            continue;
        }
        let Some(allowed) = parameter.get("AllowedValues").and_then(Value::as_str) else {
            // Free text: the engine parses the value itself, so the service publishes no
            // value set for it.
            continue;
        };
        let tokens: Vec<&str> = allowed.split(',').collect();
        checked += 1;
        for token in &tokens {
            let (status, answered) = update(&router, "token-domains", &[(name, token)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a published token {token:?} must be accepted, got {status} \
                 with {answered}"
            );
            let read_back = parameters(&router, "token-domains").await;
            assert_eq!(
                field(&read_back[name], "Value", &case),
                *token,
                "case {case:?}: expected the persisted token {token:?}"
            );
        }
        // A value outside the published set is refused, and the refusal quotes that set.
        let outside = format!("{}-not-a-token", tokens.first().unwrap_or(&"x"));
        let (status, answered) = update(&router, "token-domains", &[(name, outside.as_str())]).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: a token outside the published set must be refused, got {answered}"
        );
        assert_eq!(
            message(&answered, &case),
            format!("invalid parameter value, allowed values are:{allowed}"),
            "case {case:?}: the refusal must quote the published set"
        );
    }
    assert!(
        checked >= 2,
        "case {:?}: the family must publish token settings to check, got {checked}",
        "token domains"
    );

    // The service's own vector-search switch takes the engine's boolean tokens, and the
    // boolean spellings it does NOT publish are refused.
    let search = &reported["search-enabled"];
    assert_eq!(
        field(search, "AllowedValues", "search-enabled"),
        "yes,no",
        "case {:?}: expected the published token set, got {search}",
        "search-enabled"
    );
    for value in ["true", "false", "YES", "1"] {
        let case = format!("search-enabled={value:?}");
        let (status, answered) = update(&router, "token-domains", &[("search-enabled", value)]).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: a value outside the published set must be refused, got {answered}"
        );
    }
}
