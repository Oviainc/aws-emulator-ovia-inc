// SPDX-License-Identifier: CC0-1.0

//! The two MemoryDB catalogs — the engine releases the service offers, and the engine
//! default parameters of each release's parameter group family — end to end over the
//! wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * the engine-version catalog publishes one entry per engine release the service
//!   offers, each deriving its patch version and its parameter group family from its own
//!   engine and version numbers — and every family it names is one the rest of the
//!   service carries, so the newest release a client picks from this list round-trips
//!   through `CreateParameterGroup`, `DescribeParameters` and `CreateCluster`;
//! * a reported parameter says WHERE its value came from: the engine defaults of the
//!   group's family until an update persists one, the caller from then on — including
//!   when the caller sets the value the family already had;
//! * a reported parameter's earliest applicable engine version is the patch version of
//!   the earliest release of ITS OWN family, so two families of different engine lines
//!   report different ones;
//! * each family carries the settings of its own engine line, and every entry publishes a
//!   complete record.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{call, error_code, ok, router};
use crate::parameters::{create_group, field, parameters, update};

/// One engine release this service publishes.
///
/// Source: the engine releases the recorded MemoryDB endpoint offers, which is what
/// `crate::state::ENGINE_VERSIONS` states — Redis OSS 6.2, 7.0 and 7.1, and Valkey 7.2
/// and 7.3, each at the patch release below, and each belonging to the family of its own
/// engine and major version. Every row is asserted, not only the newest one.
struct Release {
    engine: &'static str,
    version: &'static str,
    patch_version: &'static str,
    family: &'static str,
}

const RELEASES: &[Release] = &[
    Release {
        engine: "redis",
        version: "6.2",
        patch_version: "6.2.6",
        family: "memorydb_redis6",
    },
    Release {
        engine: "redis",
        version: "7.0",
        patch_version: "7.0.7",
        family: "memorydb_redis7",
    },
    Release {
        engine: "redis",
        version: "7.1",
        patch_version: "7.1.1",
        family: "memorydb_redis7",
    },
    Release {
        engine: "valkey",
        version: "7.2",
        patch_version: "7.2.6",
        family: "memorydb_valkey7",
    },
    Release {
        engine: "valkey",
        version: "7.3",
        patch_version: "7.3.0",
        family: "memorydb_valkey7",
    },
];

/// Which engine lines carry one setting.
///
/// Source: the engine default parameters this service publishes
/// (`crate::state::ENGINE_PARAMETERS`), whose `since`/`until` say which line a setting
/// belongs to: the Redis OSS 6 line renamed its collection-encoding settings in Redis OSS
/// 7.0, and the vector-search settings are MemoryDB's own on its Valkey-backed families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lines {
    /// Every family the catalog publishes.
    Every,
    /// Only the Redis OSS 6 line (the setting was renamed or replaced after it).
    Redis6Only,
    /// The Redis OSS 7 line and the Valkey line after it.
    Redis7Onwards,
    /// Only the Valkey-backed families.
    ValkeyOnly,
}

/// One row of the published setting catalog: a setting, and the lines carrying it.
///
/// Rows the issue cited and rows it did not (`maxmemory-samples`, `lfu-log-factor`,
/// `stream-node-max-entries`, the renamed encoding settings, the Valkey-only ones), so
/// the rule is checked beyond the cited names.
const SETTINGS: &[(&str, Lines)] = &[
    ("acllog-max-len", Lines::Every),
    ("activedefrag", Lines::Every),
    ("active-defrag-cycle-max", Lines::Every),
    ("active-defrag-ignore-bytes", Lines::Every),
    ("active-expire-effort", Lines::Every),
    ("client-output-buffer-limit-normal-hard-limit", Lines::Every),
    ("client-output-buffer-limit-pubsub-soft-seconds", Lines::Every),
    ("lfu-log-factor", Lines::Every),
    ("list-compress-depth", Lines::Every),
    ("maxmemory-policy", Lines::Every),
    ("maxmemory-samples", Lines::Every),
    ("notify-keyspace-events", Lines::Every),
    ("stream-node-max-entries", Lines::Every),
    ("tcp-keepalive", Lines::Every),
    ("timeout", Lines::Every),
    ("hash-max-ziplist-entries", Lines::Redis6Only),
    ("lua-time-limit", Lines::Redis6Only),
    ("busy-reply-threshold", Lines::Redis7Onwards),
    ("hash-max-listpack-entries", Lines::Redis7Onwards),
    ("latency-tracking", Lines::Redis7Onwards),
    ("search-enabled", Lines::ValkeyOnly),
    ("search-query-timeout-ms", Lines::ValkeyOnly),
];

/// `Parameter$Source`: the value is the family's engine default.
const SOURCE_SYSTEM: &str = "system";

/// `Parameter$Source`: the value is one a caller persisted on this group.
const SOURCE_USER: &str = "user";

/// Whether `family` is one of the service's Valkey-backed families.
fn is_valkey(family: &str) -> bool {
    family.starts_with("memorydb_valkey")
}

/// Whether a family of `family` carries the setting of `lines`.
fn carries(family: &str, lines: Lines) -> bool {
    match lines {
        Lines::Every => true,
        Lines::Redis6Only => family == "memorydb_redis6",
        Lines::Redis7Onwards => family != "memorydb_redis6",
        Lines::ValkeyOnly => is_valkey(family),
    }
}

/// The families the published releases name, each once.
fn families() -> BTreeSet<&'static str> {
    RELEASES.iter().map(|release| release.family).collect()
}

/// The name of the parameter group MemoryDB itself provides for `family`, derived the way
/// a client derives it: `default.` + the family with underscores written as hyphens.
fn default_group_name(family: &str) -> String {
    format!("default.{}", family.replace('_', "-"))
}

/// The patch version of the earliest release the catalog publishes for `family` — what
/// every parameter of that family names as the earliest engine version it applies to.
fn earliest_patch_version(family: &str) -> &'static str {
    let mut releases: Vec<&Release> = RELEASES
        .iter()
        .filter(|release| release.family == family)
        .collect();
    releases.sort_by_key(|release| version_parts(release.version));
    releases
        .first()
        .unwrap_or_else(|| panic!("case {family:?}: the row set must publish this family"))
        .patch_version
}

/// `"<major>.<minor>"` as a comparable pair.
fn version_parts(version: &str) -> (u32, u32) {
    let (major, minor) = version
        .split_once('.')
        .unwrap_or_else(|| panic!("case {version:?}: expected <major>.<minor>"));
    let number = |part: &str| {
        part.parse::<u32>()
            .unwrap_or_else(|_| panic!("case {version:?}: expected a numeric part, got {part:?}"))
    };
    (number(major), number(minor))
}

/// The updates one family takes, each row a setting of a DIFFERENT kind of published
/// domain: a token set, two disjoint integer ranges, an integer bound, free text, and a
/// value EQUAL to the engine default (which is still the caller's). A Valkey family also
/// takes the setting only its own line carries.
fn updates_for(family: &str) -> Vec<(&'static str, &'static str)> {
    let mut rows = vec![
        ("maxmemory-policy", "allkeys-lru"),
        ("timeout", "300"),
        ("tcp-keepalive", "0"),
        ("notify-keyspace-events", "KEA"),
        ("hll-sparse-max-bytes", "3000"),
    ];
    if is_valkey(family) {
        // The service's own vector-search switch, which takes the engine's boolean
        // tokens like every other boolean setting.
        rows.push(("search-enabled", "yes"));
    }
    rows
}

/// One reported catalog entry, as `(Engine, EngineVersion, EnginePatchVersion,
/// ParameterGroupFamily)` — the four members the rules talk about.
type Reported = (String, String, String, String);

/// The catalog `DescribeEngineVersions` publishes, in catalog order.
async fn published_releases(router: &MultiServiceRouter) -> Vec<Reported> {
    let body = ok(router, "DescribeEngineVersions", &json!({}), "catalog").await;
    body["EngineVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {:?}: expected EngineVersions in {body}", "catalog"))
        .iter()
        .map(|entry| {
            let member = |name: &str| {
                entry
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {:?}: expected {name} in {entry}", "catalog"))
                    .to_string()
            };
            (
                member("Engine"),
                member("EngineVersion"),
                member("EnginePatchVersion"),
                member("ParameterGroupFamily"),
            )
        })
        .collect()
}

/// The releases [`RELEASES`] states, in the same shape a reported entry takes.
fn expected_releases() -> Vec<Reported> {
    RELEASES
        .iter()
        .map(|release| {
            (
                release.engine.to_string(),
                release.version.to_string(),
                release.patch_version.to_string(),
                release.family.to_string(),
            )
        })
        .collect()
}

/// The engine catalog publishes exactly the releases this service offers, in catalog
/// order, and every family it names is carried by the rest of the service: the default
/// parameter group of that family answers parameters, a group a caller creates in it
/// does too, and a cluster can be created on that release WITH such a group — so the
/// newest entry a client picks from this list does not name a family nothing else has.
#[tokio::test]
async fn the_engine_catalog_publishes_releases_whose_families_round_trip() {
    let router = router();
    let published = published_releases(&router).await;
    assert_eq!(
        published,
        expected_releases(),
        "case {:?}: expected the published releases, got {published:?}",
        "catalog"
    );

    // Every release's family: its default group is readable, and a group created in it
    // starts from the same baseline.
    for (index, (engine, version, _, family)) in published.iter().enumerate() {
        let case = format!("{engine} {version}");
        let default_group = default_group_name(family);
        let baseline = parameters(&router, &default_group).await;
        assert!(
            !baseline.is_empty(),
            "case {case:?}: the family {family:?} must carry engine defaults"
        );

        let group = format!("roundtrip-{index}");
        create_group(&router, &group, family).await;
        let fresh = parameters(&router, &group).await;
        assert_eq!(
            fresh.keys().collect::<Vec<_>>(),
            baseline.keys().collect::<Vec<_>>(),
            "case {case:?}: a group of the family must carry the family's settings"
        );

        // A client that picks this release and this group creates a cluster with both,
        // and the cluster reports them back.
        ok(
            &router,
            "CreateACL",
            &json!({ "ACLName": format!("catalog-acl-{index}") }),
            &case,
        )
        .await;
        let cluster = ok(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": format!("catalog-cluster-{index}"),
                "NodeType": "db.t4g.small",
                "ACLName": format!("catalog-acl-{index}"),
                "Engine": engine,
                "EngineVersion": version,
                "ParameterGroupName": group,
            }),
            &case,
        )
        .await;
        for (member, expected) in [
            ("Engine", json!(engine)),
            ("EngineVersion", json!(version)),
            ("ParameterGroupName", json!(group)),
        ] {
            assert_eq!(
                cluster["Cluster"][member], expected,
                "case {case:?}: expected {member} to be {expected}, got {cluster}"
            );
        }
    }
}

/// `Family` is resolved against the published catalog: a family no engine release names
/// carries no engine defaults, so a group cannot be created in it. The rows are families
/// outside the catalog in every shape one can take — an unknown major version, a release
/// line the service does not offer, the family of another engine's major version, a name
/// without the service's prefix, an extended name and an empty one — and after each
/// refusal the name still holds no group.
#[tokio::test]
async fn a_family_the_catalog_does_not_publish_cannot_be_created_in() {
    let router = router();
    let published = families();

    let rows = [
        "memorydb_redis99",
        "memorydb_redis5",
        "memorydb_valkey8",
        "memorydb_valkey6",
        "redis7",
        "memorydb_valkey7-extra",
        "",
    ];
    for (index, family) in rows.iter().enumerate() {
        assert!(
            !published.contains(family),
            "case {family:?}: the row must name a family outside the catalog, \
             published {published:?}"
        );
        let group = format!("outside-{index}");
        let (status, refused) = call(
            &router,
            "CreateParameterGroup",
            &json!({ "ParameterGroupName": group, "Family": family }),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidParameterValueException"),
            "case {family:?}: expected the modelled invalid-value error, got {status} \
             with {refused}"
        );
        // The refusal names the member and the value the request sent for it.
        let message = refused
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains("ParameterGroupFamily") && message.contains(family),
            "case {family:?}: the refusal must name the member and the value, got \
             {refused}"
        );
        // And nothing was stored under the name.
        let (status, answered) = call(
            &router,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": group }),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ParameterGroupNotFoundFault"),
            "case {family:?}: a refused create must store nothing, got {status} with \
             {answered}"
        );
    }

    // Every family the catalog DOES publish is accepted, in any case: the family is
    // stored folded, like every other name in this service.
    for (index, family) in published.iter().enumerate() {
        let group = format!("published-{index}");
        let created = ok(
            &router,
            "CreateParameterGroup",
            &json!({ "ParameterGroupName": group, "Family": family.to_uppercase() }),
            family,
        )
        .await;
        assert_eq!(
            created["ParameterGroup"]["Family"],
            json!(family),
            "case {family:?}: expected the folded family, got {created}"
        );
    }
}

/// until an update persists one for THIS group, the caller from then on — even when the
/// caller persists the value the family already published. Checked for every family, and
/// against a pristine group of the same family which must be untouched.
#[tokio::test]
async fn every_parameter_reports_where_its_value_came_from() {
    let router = router();

    for family in families() {
        // A group name holds no underscore (the service's name rule), so the family it
        // is derived from contributes its hyphenated spelling.
        let group = format!("sourced-{}", family.replace('_', "-"));
        let reference = format!("untouched-{}", family.replace('_', "-"));
        create_group(&router, &group, family).await;
        create_group(&router, &reference, family).await;

        // Before any update, every setting of the group AND of the family's own default
        // group is the engine's.
        let default_group = default_group_name(family);
        for name in [group.as_str(), default_group.as_str()] {
            for (parameter, reported) in parameters(&router, name).await {
                let case = format!("{name}/{parameter}");
                assert_eq!(
                    field(&reported, "Source", &case),
                    SOURCE_SYSTEM,
                    "case {case:?}: an unchanged setting is the engine's, got {reported}"
                );
            }
        }

        let before = parameters(&router, &group).await;
        let rows = updates_for(family);
        let (status, answered) = update(&router, &group, &rows).await;
        assert_eq!(
            status, 200,
            "case {family:?}: expected the update to answer 200, got {status} with \
             {answered}"
        );

        let after = parameters(&router, &group).await;
        for (name, value) in &rows {
            let case = format!("{family}/{name}");
            let reported = after
                .get(*name)
                .unwrap_or_else(|| panic!("case {case:?}: expected the setting to be reported"));
            assert_eq!(
                field(reported, "Value", &case),
                *value,
                "case {case:?}: expected the persisted value, got {reported}"
            );
            assert_eq!(
                field(reported, "Source", &case),
                SOURCE_USER,
                "case {case:?}: a value the caller set is the caller's, got {reported}"
            );
            // Nothing else about the setting moved: the update persists a value, it does
            // not redefine the parameter.
            for member in ["Description", "DataType", "MinimumEngineVersion"] {
                assert_eq!(
                    field(reported, member, &case),
                    field(&before[*name], member, &case),
                    "case {case:?}: an update must not change {member}, got {reported}"
                );
            }
        }
        for (name, reported) in &after {
            if rows.iter().any(|(updated, _)| updated == name) {
                continue;
            }
            assert_eq!(
                Some(reported),
                before.get(name),
                "case {:?}: a setting nobody changed must report exactly what it did",
                format!("{family}/{name}")
            );
        }
        assert_eq!(
            parameters(&router, &reference).await,
            before,
            "case {family:?}: an update must reach only the group it named"
        );
    }
}

/// The earliest engine version a parameter applies to is the patch version of the
/// earliest release of its OWN family: three-part, one the catalog publishes, and
/// different for families of different engine lines.
#[tokio::test]
async fn minimum_engine_version_is_the_earliest_release_of_the_family() {
    let router = router();
    let published: BTreeSet<String> = published_releases(&router)
        .await
        .into_iter()
        .map(|(_, _, patch_version, _)| patch_version)
        .collect();

    let mut by_family: BTreeMap<&str, String> = BTreeMap::new();
    for family in families() {
        let group = default_group_name(family);
        let reported = parameters(&router, &group).await;
        assert!(
            !reported.is_empty(),
            "case {family:?}: the family must carry engine defaults"
        );
        let expected = earliest_patch_version(family);
        for (name, parameter) in &reported {
            let case = format!("{family}/{name}");
            let minimum = field(parameter, "MinimumEngineVersion", &case);
            assert_eq!(
                minimum, expected,
                "case {case:?}: expected the family's earliest release, got {parameter}"
            );
            assert!(
                published.contains(&minimum),
                "case {case:?}: {minimum:?} must be a release the catalog publishes, \
                 published {published:?}"
            );
            assert_eq!(
                minimum.split('.').count(),
                3,
                "case {case:?}: expected <major>.<minor>.<patch>, got {minimum:?}"
            );
        }
        by_family.insert(family, expected.to_string());
    }

    // Two families of different engine lines do not name the same earliest release.
    let distinct: BTreeSet<&String> = by_family.values().collect();
    assert_eq!(
        distinct.len(),
        by_family.len(),
        "case {:?}: each family names its own earliest release, got {by_family:?}",
        "families"
    );
}

/// Each family carries the settings of its own engine line — the ones every line has, the
/// ones the Redis OSS 6 line kept to itself, the ones Redis OSS 7 brought and the ones
/// only MemoryDB's Valkey families have — and every reported setting publishes a complete
/// record: a value, a description, a data type, where the value came from, the earliest
/// release it applies to, and the value set it accepts unless the engine parses the value
/// itself.
#[tokio::test]
async fn each_family_carries_the_settings_of_its_own_engine_line() {
    let router = router();

    for family in families() {
        let group = default_group_name(family);
        let reported = parameters(&router, &group).await;
        for (setting, lines) in SETTINGS {
            let case = format!("{family}/{setting}");
            assert_eq!(
                reported.contains_key(*setting),
                carries(family, *lines),
                "case {case:?}: expected carried={}, got the listing {:?}",
                carries(family, *lines),
                reported.keys().collect::<Vec<_>>()
            );
        }

        for (name, parameter) in &reported {
            let case = format!("{family}/{name}");
            for member in [
                "Name",
                "Value",
                "Description",
                "DataType",
                "MinimumEngineVersion",
                "Source",
            ] {
                let value = field(parameter, member, &case);
                assert!(
                    !value.is_empty() || member == "Value",
                    "case {case:?}: expected a {member}, got {parameter}"
                );
            }
            let data_type = field(parameter, "DataType", &case);
            assert!(
                data_type == "integer" || data_type == "string",
                "case {case:?}: expected DataType integer or string, got {data_type:?}"
            );
            if data_type == "integer" {
                assert!(
                    parameter.get("AllowedValues").is_some(),
                    "case {case:?}: a numeric setting publishes its range, got {parameter}"
                );
            }
        }
    }

    // A setting of ANOTHER line cannot be set on a family that does not carry it: the
    // listing and what an update accepts are the same catalog.
    for family in families() {
        let group = format!("line-{}", family.replace('_', "-"));
        create_group(&router, &group, family).await;
        for (setting, lines) in SETTINGS {
            if carries(family, *lines) {
                continue;
            }
            let case = format!("{family}/{setting}");
            let (status, refused) = update(&router, &group, &[(setting, "1")]).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: a setting of another line must be refused, got {status} \
                 with {refused}"
            );
        }
    }
}
