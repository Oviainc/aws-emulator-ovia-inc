// SPDX-License-Identifier: CC0-1.0

//! MemoryDB parameter listings and parameter updates, end to end over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * the parameters a group reports are the engine default parameters of the FAMILY it
//!   was created with — so the listing follows the engine catalog
//!   (`DescribeEngineVersions`), and the families of different engine lines carry
//!   different settings;
//! * every reported parameter publishes its value, data type, allowed values and the
//!   earliest engine version it applies to, and that version is one the service
//!   publishes for the family's own engine line;
//! * `UpdateParameterGroup` persists the values it accepts and a later
//!   `DescribeParameters` reports them, leaving every other parameter at its family
//!   default;
//! * a value outside a parameter's published domain, or a name the family does not
//!   carry, is refused with the modelled error and the group is left EXACTLY as it was
//!   — including when it is one pair of an otherwise valid batch;
//! * every bound the service publishes for a parameter is accepted and one past it is
//!   refused, over the whole published catalog rather than a chosen row;
//! * a read changes nothing a later caller sees: two listings of the same store agree;
//! * a listing pages over the whole set in name order for any `MaxResults`, and a page
//!   size, token or group name that names nothing is the modelled error.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{call, error_code, ok, router};

/// What a family's listing must and must not carry, by the engine line the family
/// belongs to.
///
/// Source: the AWS MemoryDB User Guide's per-family default parameter groups. Redis OSS
/// 7.0 renamed the collection-encoding settings (`*-ziplist-*` → `*-listpack-*`) and
/// replaced `lua-time-limit` with `busy-reply-threshold`, and Valkey 7.2 and 8.0
/// continue that line — which is exactly why a family's parameter list cannot be the
/// same for every family.
struct FamilyExpectation {
    family: &'static str,
    /// Parameters this family must carry.
    carries: &'static [&'static str],
    /// Parameters this family must NOT carry: they belong to another engine line.
    lacks: &'static [&'static str],
}

const FAMILY_EXPECTATIONS: &[FamilyExpectation] = &[
    FamilyExpectation {
        family: "memorydb_redis6",
        carries: &[
            "hash-max-ziplist-entries",
            "hash-max-ziplist-value",
            "lua-time-limit",
            "zset-max-ziplist-entries",
        ],
        lacks: &[
            "busy-reply-threshold",
            "hash-max-listpack-entries",
            "latency-tracking",
            "set-max-listpack-entries",
        ],
    },
    FamilyExpectation {
        family: "memorydb_redis7",
        carries: &[
            "busy-reply-threshold",
            "hash-max-listpack-entries",
            "latency-tracking",
            "set-max-listpack-entries",
        ],
        lacks: &[
            "hash-max-ziplist-entries",
            "hash-max-ziplist-value",
            "lua-time-limit",
            "zset-max-ziplist-entries",
        ],
    },
    FamilyExpectation {
        family: "memorydb_valkey7",
        carries: &[
            "busy-reply-threshold",
            "hash-max-listpack-entries",
            "latency-tracking",
            "set-max-listpack-entries",
        ],
        lacks: &[
            "hash-max-ziplist-entries",
            "hash-max-ziplist-value",
            "lua-time-limit",
            "zset-max-ziplist-entries",
        ],
    },
    FamilyExpectation {
        family: "memorydb_valkey8",
        carries: &[
            "busy-reply-threshold",
            "hash-max-listpack-entries",
            "latency-tracking",
            "set-max-listpack-entries",
        ],
        lacks: &[
            "hash-max-ziplist-entries",
            "hash-max-ziplist-value",
            "lua-time-limit",
            "zset-max-ziplist-entries",
        ],
    },
];

/// The settings every family carries, each with a value from a DIFFERENT kind of
/// domain: a token set, two integer ranges (one open-ended, one closed at its top
/// bound) and free text. The rows are what an update must persist unchanged.
const UPDATES: &[(&str, &str)] = &[
    ("maxmemory-policy", "allkeys-lru"),
    ("activerehashing", "no"),
    ("timeout", "300"),
    ("active-expire-effort", "10"),
    ("notify-keyspace-events", "KEA"),
];

/// The engine versions the service publishes, as `(family, "<major>.<minor>")` pairs
/// read from `DescribeEngineVersions` — the catalog a family's parameters follow.
async fn published_versions(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let body = ok(router, "DescribeEngineVersions", &json!({}), "engine catalog").await;
    let versions = body["EngineVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {:?}: expected EngineVersions in {body}", "catalog"));
    versions
        .iter()
        .map(|version| {
            let family = version["ParameterGroupFamily"]
                .as_str()
                .unwrap_or_else(|| panic!("expected ParameterGroupFamily in {version}"));
            let engine_version = version["EngineVersion"]
                .as_str()
                .unwrap_or_else(|| panic!("expected EngineVersion in {version}"));
            (family.to_string(), engine_version.to_string())
        })
        .collect()
}

/// `"<major>.<minor>"` as a comparable pair.
fn version_parts(version: &str, case: &str) -> (u32, u32) {
    let (major, minor) = version
        .split_once('.')
        .unwrap_or_else(|| panic!("case {case:?}: expected <major>.<minor>, got {version:?}"));
    (
        major
            .parse()
            .unwrap_or_else(|_| panic!("case {case:?}: expected a numeric major in {version:?}")),
        minor
            .parse()
            .unwrap_or_else(|_| panic!("case {case:?}: expected a numeric minor in {version:?}")),
    )
}

/// Create one parameter group of `family`, named after the case.
async fn create_group(router: &MultiServiceRouter, name: &str, family: &str) {
    ok(
        router,
        "CreateParameterGroup",
        &json!({ "ParameterGroupName": name, "Family": family }),
        name,
    )
    .await;
}

/// Every parameter one group reports, by name, from an unpaged `DescribeParameters`.
async fn parameters(router: &MultiServiceRouter, group: &str) -> BTreeMap<String, Value> {
    let body = ok(
        router,
        "DescribeParameters",
        &json!({ "ParameterGroupName": group }),
        group,
    )
    .await;
    assert!(
        body.get("NextToken").is_none(),
        "case {group:?}: an unpaged listing must end, got {body}"
    );
    parameter_map(&body, group)
}

/// `Parameters` of a `DescribeParameters` body, by name.
fn parameter_map(body: &Value, case: &str) -> BTreeMap<String, Value> {
    parameter_list(body, case)
        .iter()
        .map(|parameter| {
            let name = parameter["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected Name in {parameter}"));
            (name.to_string(), parameter.clone())
        })
        .collect()
}

/// `Parameters` of a `DescribeParameters` body, in answer order.
fn parameter_list(body: &Value, case: &str) -> Vec<Value> {
    body.get("Parameters")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected Parameters in {body}"))
        .clone()
}

/// `Parameters[*].Name` of a `DescribeParameters` body, in answer order.
fn parameter_names(body: &Value, case: &str) -> Vec<String> {
    parameter_list(body, case)
        .iter()
        .map(|parameter| {
            parameter["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected Name in {parameter}"))
                .to_string()
        })
        .collect()
}

/// One `UpdateParameterGroup` call carrying `pairs`.
async fn update(
    router: &MultiServiceRouter,
    group: &str,
    pairs: &[(&str, &str)],
) -> (u16, Value) {
    let body = json!({
        "ParameterGroupName": group,
        "ParameterNameValues": pairs
            .iter()
            .map(|(name, value)| json!({ "ParameterName": name, "ParameterValue": value }))
            .collect::<Vec<Value>>(),
    });
    call(router, "UpdateParameterGroup", &body).await
}

/// A string field of a reported parameter.
fn field(parameter: &Value, member: &str, case: &str) -> String {
    parameter
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected {member} in {parameter}"))
        .to_string()
}

/// The domain a reported parameter publishes, parsed from its `DataType` and
/// `AllowedValues`.
///
/// The catalog publishes no negative bound, so a range is `"<min>-<max>"` or the
/// open-ended `"<min>-"`; a token set is comma-separated; free text publishes no
/// `AllowedValues` member at all.
#[derive(Debug, PartialEq, Eq)]
enum Domain {
    Range { min: i64, max: Option<i64> },
    Tokens(Vec<String>),
    Text,
}

fn domain_of(parameter: &Value, case: &str) -> Domain {
    let data_type = field(parameter, "DataType", case);
    let allowed = parameter
        .get("AllowedValues")
        .map(|allowed| {
            allowed
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected AllowedValues to be a string, got {allowed}")
                })
                .to_string()
        })
        .unwrap_or_default();
    assert!(
        data_type == "integer" || data_type == "string",
        "case {case:?}: expected DataType integer or string, got {data_type:?}"
    );
    if data_type == "integer" {
        let bounds = allowed
            .split_once('-')
            .unwrap_or_else(|| panic!("case {case:?}: expected a range, got {allowed:?}"));
        let min: i64 = bounds
            .0
            .parse()
            .unwrap_or_else(|_| panic!("case {case:?}: expected a numeric min in {allowed:?}"));
        let max = if bounds.1.is_empty() {
            None
        } else {
            Some(bounds.1.parse().unwrap_or_else(|_| {
                panic!("case {case:?}: expected a numeric max in {allowed:?}")
            }))
        };
        return Domain::Range { min, max };
    }
    if allowed.is_empty() {
        Domain::Text
    } else {
        Domain::Tokens(allowed.split(',').map(ToString::to_string).collect())
    }
}

/// A group's parameters are its FAMILY's engine defaults: the families the engine
/// catalog publishes carry the settings of their own engine line, every reported
/// parameter publishes a complete record, and the earliest engine version it names is
/// one the service publishes and is not newer than the family itself.
#[tokio::test]
async fn a_groups_parameters_are_the_engine_defaults_of_its_family() {
    let router = router();
    let catalog = published_versions(&router).await;
    let families: BTreeSet<String> = catalog.iter().map(|(family, _)| family.clone()).collect();
    assert!(
        !families.is_empty(),
        "case {:?}: the engine catalog must publish at least one family",
        "catalog"
    );
    let published: BTreeSet<String> = catalog
        .iter()
        .map(|(_, version)| version.clone())
        .collect();

    for family in &families {
        let expectation = FAMILY_EXPECTATIONS
            .iter()
            .find(|row| row.family == family.as_str())
            .unwrap_or_else(|| {
                panic!("case {family:?}: the published family has no expectation row")
            });
        let group = format!("group-{family}");
        create_group(&router, &group, family).await;
        let reported = parameters(&router, &group).await;
        assert!(
            !reported.is_empty(),
            "case {family:?}: a family the catalog publishes must carry engine defaults"
        );

        for name in expectation.carries {
            assert!(
                reported.contains_key(*name),
                "case {family:?}: expected parameter {name:?}, got {:?}",
                reported.keys().collect::<Vec<_>>()
            );
        }
        for name in expectation.lacks {
            assert!(
                !reported.contains_key(*name),
                "case {family:?}: parameter {name:?} belongs to another engine line, got {:?}",
                reported.keys().collect::<Vec<_>>()
            );
        }

        // The newest version the catalog publishes for this family: no parameter of the
        // family may require a version newer than that.
        let family_top = catalog
            .iter()
            .filter(|(candidate, _)| candidate == family)
            .map(|(_, version)| version_parts(version, family))
            .max()
            .unwrap_or_else(|| panic!("case {family:?}: the family has no published version"));

        for (name, parameter) in &reported {
            let case = format!("{family}/{name}");
            let value = field(parameter, "Value", &case);
            assert!(
                !field(parameter, "Description", &case).is_empty(),
                "case {case:?}: expected a description, got {parameter}"
            );
            let minimum = field(parameter, "MinimumEngineVersion", &case);
            assert!(
                published.contains(&minimum),
                "case {case:?}: MinimumEngineVersion {minimum:?} must be a published \
                 engine version, published {published:?}"
            );
            assert!(
                version_parts(&minimum, &case) <= family_top,
                "case {case:?}: MinimumEngineVersion {minimum:?} must not be newer than \
                 the family's own {family_top:?}"
            );
            // The default value must itself satisfy the domain the parameter publishes.
            match domain_of(parameter, &case) {
                Domain::Range { min, max } => {
                    let number: i64 = value.parse().unwrap_or_else(|_| {
                        panic!("case {case:?}: expected an integer value, got {value:?}")
                    });
                    assert!(
                        number >= min && max.is_none_or(|max| number <= max),
                        "case {case:?}: the default {value:?} must lie in {min}-{max:?}"
                    );
                }
                Domain::Tokens(tokens) => assert!(
                    tokens.contains(&value),
                    "case {case:?}: the default {value:?} must be one of {tokens:?}"
                ),
                Domain::Text => {}
            }
        }
    }

    // Two families of different engine lines do not report the same set of settings.
    let redis6 = parameters(&router, "group-memorydb_redis6").await;
    let redis7 = parameters(&router, "group-memorydb_redis7").await;
    assert_ne!(
        redis6.keys().collect::<Vec<_>>(),
        redis7.keys().collect::<Vec<_>>(),
        "case {:?}: families of different engine lines must differ in their settings",
        "redis6 vs redis7"
    );
}

/// An update persists exactly the values it accepted: a later describe reports them,
/// every other parameter still reports its family default, and the answer is the stored
/// group.
#[tokio::test]
async fn an_update_persists_its_values_and_the_describe_reports_them() {
    let router = router();
    let catalog = published_versions(&router).await;
    let families: BTreeSet<String> = catalog.into_iter().map(|(family, _)| family).collect();

    for family in &families {
        // A second group of the SAME family, never updated, is the reference for "every
        // other parameter is untouched".
        let group = format!("updated-{family}");
        let reference = format!("pristine-{family}");
        create_group(&router, &group, family).await;
        create_group(&router, &reference, family).await;

        let (status, answered) = update(&router, &group, UPDATES).await;
        assert_eq!(
            status, 200,
            "case {family:?}: expected the update to answer 200, got {status} with {answered}"
        );
        assert_eq!(
            answered["ParameterGroup"]["Name"],
            json!(group),
            "case {family:?}: the update must answer the stored group, got {answered}"
        );
        assert_eq!(
            answered["ParameterGroup"]["Family"],
            json!(family),
            "case {family:?}: the update must not move the group's family, got {answered}"
        );

        let updated = parameters(&router, &group).await;
        let pristine = parameters(&router, &reference).await;
        for (name, value) in UPDATES {
            let reported = updated
                .get(*name)
                .unwrap_or_else(|| panic!("case {family:?}: expected parameter {name:?}"));
            assert_eq!(
                reported["Value"],
                json!(value),
                "case {:?}: expected the persisted value, got {reported}",
                format!("{family}/{name}")
            );
        }
        for (name, parameter) in &pristine {
            if UPDATES.iter().any(|(updated_name, _)| updated_name == name) {
                continue;
            }
            assert_eq!(
                updated.get(name),
                Some(parameter),
                "case {:?}: an untouched parameter must still report its family default",
                format!("{family}/{name}")
            );
        }
    }
}

/// A refused update changes nothing: for every shape of refusal — an unknown name, the
/// other engine line's spelling, a value of the wrong type, a value outside the
/// published range or token set, an empty or over-long batch, and a bad pair inside an
/// otherwise valid batch — the group reports exactly what it reported before.
#[tokio::test]
async fn a_refused_update_leaves_the_group_unchanged() {
    let router = router();
    const GROUP: &str = "rejections";
    const FAMILY: &str = "memorydb_redis7";
    create_group(&router, GROUP, FAMILY).await;
    // One accepted update first, so the snapshot the refusals must preserve holds
    // persisted values and not only defaults.
    let (status, answered) = update(&router, GROUP, UPDATES).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the setup update to answer 200, got {status} with {answered}",
        "setup"
    );
    let before = parameters(&router, GROUP).await;

    // Each row is one refusal class; the expected error code is the modelled one for
    // that class (a value the service does not accept, or members it cannot combine).
    struct Rejection {
        case: &'static str,
        pairs: Vec<(&'static str, &'static str)>,
        expected_code: &'static str,
    }
    let rejections: Vec<Rejection> = vec![
        Rejection {
            case: "unknown parameter name",
            pairs: vec![("no-such-parameter", "1")],
            expected_code: "InvalidParameterValueException",
        },
        Rejection {
            case: "the other engine line's spelling",
            pairs: vec![("hash-max-ziplist-entries", "256")],
            expected_code: "InvalidParameterValueException",
        },
        Rejection {
            case: "a non-integer for an integer parameter",
            pairs: vec![("timeout", "soon")],
            expected_code: "InvalidParameterValueException",
        },
        Rejection {
            case: "below an integer parameter's minimum",
            pairs: vec![("active-expire-effort", "0")],
            expected_code: "InvalidParameterValueException",
        },
        Rejection {
            case: "above an integer parameter's maximum",
            pairs: vec![("active-expire-effort", "11")],
            expected_code: "InvalidParameterValueException",
        },
        Rejection {
            case: "a token outside the published set",
            pairs: vec![("maxmemory-policy", "evict-everything")],
            expected_code: "InvalidParameterValueException",
        },
        Rejection {
            case: "a bad pair inside an otherwise valid batch",
            pairs: vec![("timeout", "30"), ("maxmemory-policy", "nonsense")],
            expected_code: "InvalidParameterValueException",
        },
        Rejection {
            case: "no pairs at all",
            pairs: vec![],
            expected_code: "InvalidParameterCombinationException",
        },
    ];

    for Rejection {
        case,
        pairs,
        expected_code,
    } in &rejections
    {
        let (status, refused) = update(&router, GROUP, pairs).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal with 400, got {status} with {refused}"
        );
        assert_eq!(
            &error_code(&refused),
            expected_code,
            "case {case:?}: expected the modelled {expected_code}, got {refused}"
        );
        assert_eq!(
            parameters(&router, GROUP).await,
            before,
            "case {case:?}: a refused update must leave the group unchanged"
        );
    }

    // More pairs than one request may carry: 21 copies of one valid pair.
    let many: Vec<(&str, &str)> = std::iter::repeat_n(("timeout", "30"), 21).collect();
    let (status, refused) = update(&router, GROUP, &many).await;
    assert_eq!(
        status, 400,
        "case {:?}: expected a refusal with 400, got {status} with {refused}",
        "21 pairs"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParameterCombinationException",
        "case {:?}: expected the modelled combination error, got {refused}",
        "21 pairs"
    );
    assert_eq!(
        parameters(&router, GROUP).await,
        before,
        "case {:?}: a refused update must leave the group unchanged",
        "21 pairs"
    );

    // A group that is not in state is the modelled not-found fault, for both the read
    // and the write.
    for name in ["no-such-group", "REJECTIONS", "rejection"] {
        let (status, refused) = update(&router, name, &[("timeout", "30")]).await;
        assert_eq!(
            status, 404,
            "case {name:?}: an unknown group must answer 404, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ParameterGroupNotFoundFault",
            "case {name:?}: expected the modelled not-found fault, got {refused}"
        );
        let (status, refused) = call(
            &router,
            "DescribeParameters",
            &json!({ "ParameterGroupName": name }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {name:?}: an unknown group must answer 404, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ParameterGroupNotFoundFault",
            "case {name:?}: expected the modelled not-found fault, got {refused}"
        );
    }
}

/// Every bound the service PUBLISHES for a parameter is accepted and one past it is
/// refused, and every token it publishes is accepted while one outside the set is not —
/// over the whole published catalog of a family, not a chosen row.
#[tokio::test]
async fn every_published_bound_is_accepted_and_one_past_it_is_refused() {
    let router = router();
    const GROUP: &str = "bounds";
    const FAMILY: &str = "memorydb_valkey8";
    create_group(&router, GROUP, FAMILY).await;

    for (name, parameter) in parameters(&router, GROUP).await {
        let case = format!("{FAMILY}/{name}");
        match domain_of(&parameter, &case) {
            Domain::Range { min, max } => {
                // At each published bound: accepted, and the value is what a later read
                // reports.
                for bound in [Some(min), max].into_iter().flatten() {
                    let value = bound.to_string();
                    let (status, answered) =
                        update(&router, GROUP, &[(&name, value.as_str())]).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: the published bound {value} must be accepted, got \
                         {status} with {answered}"
                    );
                    let reported = parameters(&router, GROUP).await;
                    assert_eq!(
                        reported[&name]["Value"],
                        json!(value),
                        "case {case:?}: expected the bound {value} to persist, got {:?}",
                        reported[&name]
                    );
                }
                // One past each published bound: refused, and the accepted value stands.
                let accepted = parameters(&router, GROUP).await[&name]["Value"].clone();
                let past: Vec<i64> = [Some(min - 1), max.map(|max| max + 1)]
                    .into_iter()
                    .flatten()
                    .collect();
                for outside in past {
                    let value = outside.to_string();
                    let (status, refused) =
                        update(&router, GROUP, &[(&name, value.as_str())]).await;
                    assert_eq!(
                        status, 400,
                        "case {case:?}: {value} is outside {min}-{max:?} and must be \
                         refused, got {status} with {refused}"
                    );
                    assert_eq!(
                        error_code(&refused),
                        "InvalidParameterValueException",
                        "case {case:?}: expected the modelled invalid-value error, got \
                         {refused}"
                    );
                    assert_eq!(
                        parameters(&router, GROUP).await[&name]["Value"],
                        accepted,
                        "case {case:?}: the refused value must not be persisted"
                    );
                }
            }
            Domain::Tokens(tokens) => {
                for token in &tokens {
                    let (status, answered) = update(&router, GROUP, &[(&name, token)]).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: the published token {token:?} must be accepted, got \
                         {status} with {answered}"
                    );
                    assert_eq!(
                        parameters(&router, GROUP).await[&name]["Value"],
                        json!(token),
                        "case {case:?}: expected the token {token:?} to persist"
                    );
                }
                let outside = format!("{}-not-a-token", tokens.join("+"));
                let (status, refused) =
                    update(&router, GROUP, &[(&name, outside.as_str())]).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: {outside:?} is outside {tokens:?} and must be refused, \
                     got {status} with {refused}"
                );
                assert_eq!(
                    error_code(&refused),
                    "InvalidParameterValueException",
                    "case {case:?}: expected the modelled invalid-value error, got {refused}"
                );
            }
            // Free text: the engine parses the value, so the service publishes no set to
            // hold it to and every modelled string is accepted.
            Domain::Text => {
                for value in ["", "KEA", "5%"] {
                    let (status, answered) = update(&router, GROUP, &[(&name, value)]).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: free text {value:?} must be accepted, got {status} \
                         with {answered}"
                    );
                }
            }
        }
    }
}

/// The listing pages over the whole set in name order for ANY page size, a read changes
/// nothing a later caller sees, and a page size or token that names no page is the
/// modelled invalid-value error.
#[tokio::test]
async fn the_parameter_listing_pages_in_name_order() {
    let router = router();
    const GROUP: &str = "paged";
    create_group(&router, GROUP, "memorydb_redis7").await;

    let unpaged = ok(
        &router,
        "DescribeParameters",
        &json!({ "ParameterGroupName": GROUP }),
        GROUP,
    )
    .await;
    let expected = parameter_names(&unpaged, GROUP);
    let mut sorted = expected.clone();
    sorted.sort();
    assert_eq!(
        expected, sorted,
        "case {:?}: the listing must be in name order",
        "unpaged"
    );

    // A read must not change what the next read sees.
    let again = ok(
        &router,
        "DescribeParameters",
        &json!({ "ParameterGroupName": GROUP }),
        GROUP,
    )
    .await;
    assert_eq!(
        unpaged, again,
        "case {:?}: a read must not change what the next read sees",
        "repeat read"
    );

    // Page sizes at 1, in the middle, at the set's size and one past it.
    let total = expected.len();
    for page_size in [1, 2, 7, total, total + 1] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // A page size of at least 1 always makes progress, so this cannot loop forever.
        for _ in 0..=total {
            let mut body = json!({ "ParameterGroupName": GROUP, "MaxResults": page_size });
            if let Some(token) = token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let page = ok(&router, "DescribeParameters", &body, &case).await;
            let names = parameter_names(&page, &case);
            assert!(
                names.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} parameters, got {page}"
            );
            seen.extend(names);
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the listing must end, still holding token {token:?}"
        );
        assert_eq!(
            seen, expected,
            "case {case:?}: the pages together must be the whole listing in name order"
        );
    }

    // A page size below 1 names no page.
    for max_results in [0, -1, -9] {
        let (status, refused) = call(
            &router,
            "DescribeParameters",
            &json!({ "ParameterGroupName": GROUP, "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {max_results:?}: a page size below 1 must answer 400, got {status} with \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParameterValueException",
            "case {max_results:?}: expected the modelled invalid-value error, got {refused}"
        );
    }

    // A token this listing never minted names no position in it.
    for token in ["zzzz-not-a-position", "~"] {
        let (status, refused) = call(
            &router,
            "DescribeParameters",
            &json!({ "ParameterGroupName": GROUP, "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: a token past the listing must answer 400, got {status} with \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParameterValueException",
            "case {token:?}: expected the modelled invalid-value error, got {refused}"
        );
    }
}
