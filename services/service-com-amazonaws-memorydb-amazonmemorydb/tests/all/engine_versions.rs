// SPDX-License-Identifier: CC0-1.0

//! `DescribeEngineVersions`: the engine-version catalog, end to end over the wire.
//!
//! Every case drives the same path a client does (an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.DescribeEngineVersions`) through the router harness the
//! lifecycle suite builds, so what is asserted is the emulator's HTTP behaviour.
//!
//! The rules under test, each over the WHOLE catalog rather than one recorded request:
//!
//! * every reported version derives its own `ParameterGroupFamily`
//!   (`memorydb_<engine><major>`) and its `EnginePatchVersion` (`<version>.<patch>`),
//!   so a family follows the engine version it belongs to;
//! * `Engine`, `EngineVersion` and `ParameterGroupFamily` are filters that COMPOSE:
//!   a version is reported only when it matches all of them;
//! * a combination naming no version the service offers \u2014 a version only ANOTHER
//!   engine has, an unknown version, a family belonging to another engine \u2014 is the
//!   modelled `InvalidParameterValueException`, never an empty page, while an `Engine`
//!   the service does not run at all is the modelled
//!   `InvalidParameterCombinationException` naming it;
//! * `DefaultOnly` reports the greatest minor version of each parameter group family;
//! * the listing pages over the whole catalog in catalog order for any `MaxResults` the
//!   service's page-size window admits;
//! * describing engine versions is a READ: it changes nothing a later caller sees.

use serde_json::{json, Value};
use service_com_amazonaws_memorydb_amazonmemorydb::state::CLUSTER_PAGE_SIZE_WINDOW;
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{call, error_code, ok, router};

/// The page sizes this listing serves: `DescribeEngineVersions` reads the cluster-side
/// surface, so its window is that surface's own.
const WINDOW_MIN: i32 = CLUSTER_PAGE_SIZE_WINDOW.min;
const WINDOW_MAX: i32 = CLUSTER_PAGE_SIZE_WINDOW.max;

/// The engines MemoryDB offers, from the AWS MemoryDB *Supported engine versions*
/// documentation: it runs Redis OSS and Valkey. The per-version numbers are NOT pinned
/// here \u2014 every rule below is checked against whatever the catalog reports, so a
/// catalog that gains a version keeps passing.
const DOCUMENTED_ENGINES: [&str; 2] = ["redis", "valkey"];

/// One reported engine version, reduced to the members the rules talk about.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    engine: String,
    version: String,
    patch_version: String,
    family: String,
}

/// A string member of a reported entry, or a panic naming the case.
fn field(entry: &Value, name: &str, case: &str) -> String {
    entry
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {name} in {entry}"))
        .to_string()
}

/// `EngineVersions` of a response, as [`Reported`] rows in the order they arrived.
fn reported(body: &Value, case: &str) -> Vec<Reported> {
    body.get("EngineVersions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected EngineVersions in {body}"))
        .iter()
        .map(|entry| Reported {
            engine: field(entry, "Engine", case),
            version: field(entry, "EngineVersion", case),
            patch_version: field(entry, "EnginePatchVersion", case),
            family: field(entry, "ParameterGroupFamily", case),
        })
        .collect()
}

/// Describe engine versions with `request`, which must succeed, as [`Reported`] rows.
async fn describe(router: &MultiServiceRouter, request: &Value, case: &str) -> Vec<Reported> {
    let body = ok(router, "DescribeEngineVersions", request, case).await;
    reported(&body, case)
}

/// The whole catalog, unfiltered \u2014 the set every other case is derived from.
async fn catalog(router: &MultiServiceRouter) -> Vec<Reported> {
    let catalog = describe(router, &json!({}), "unfiltered").await;
    assert!(
        !catalog.is_empty(),
        "case {:?}: the service must offer at least one engine version, got {catalog:?}",
        "unfiltered"
    );
    catalog
}

/// The `(major, minor)` numbers of a reported `EngineVersion`.
fn version_parts(version: &str, case: &str) -> (u32, u32) {
    let (major, minor) = version
        .split_once('.')
        .unwrap_or_else(|| panic!("case {case:?}: expected <major>.<minor>, got {version:?}"));
    let parse = |part: &str| {
        part.parse::<u32>()
            .unwrap_or_else(|_| panic!("case {case:?}: expected a number, got {part:?}"))
    };
    (parse(major), parse(minor))
}

/// Every reported version derives the family its engine defaults belong to from its own
/// engine and MAJOR version, and its patch version from its own version \u2014 so a
/// parameter group family follows the engine version rather than being an unrelated
/// token. Checked on EVERY catalog entry, including the ones no issue cited.
#[tokio::test]
async fn every_version_derives_its_family_and_patch_version() {
    let router = router();
    let catalog = catalog(&router).await;

    for entry in &catalog {
        let case = format!("{} {}", entry.engine, entry.version);
        let (major, _) = version_parts(&entry.version, &case);
        assert_eq!(
            entry.family,
            format!("memorydb_{}{major}", entry.engine),
            "case {case:?}: the family must be memorydb_<engine><major>, got {entry:?}"
        );
        assert!(
            entry.patch_version.starts_with(&format!("{}.", entry.version)),
            "case {case:?}: the patch version must extend the engine version, got {entry:?}"
        );
        let patch_parts: Vec<&str> = entry.patch_version.split('.').collect();
        assert_eq!(
            patch_parts.len(),
            3,
            "case {case:?}: the patch version must be <major>.<minor>.<patch>, got {entry:?}"
        );
        assert!(
            patch_parts
                .iter()
                .all(|part| part.parse::<u32>().is_ok()),
            "case {case:?}: every patch version part must be a number, got {entry:?}"
        );
    }

    // Two versions of one engine never collide, and the same version is never reported
    // twice for one engine.
    let mut seen: Vec<(String, String)> = catalog
        .iter()
        .map(|entry| (entry.engine.clone(), entry.version.clone()))
        .collect();
    let before = seen.len();
    seen.sort();
    seen.dedup();
    assert_eq!(
        seen.len(),
        before,
        "case {:?}: (engine, version) must be unique, got {catalog:?}",
        "uniqueness"
    );

    // The catalog offers the engines MemoryDB documents.
    for engine in DOCUMENTED_ENGINES {
        assert!(
            catalog.iter().any(|entry| entry.engine == engine),
            "case {engine:?}: the catalog must offer this engine, got {catalog:?}"
        );
    }
}

/// `Engine`, `EngineVersion` and `ParameterGroupFamily` select on values the catalog
/// publishes, and compose: the reported set is exactly the catalog entries matching
/// every filter the request carried. The table's rows are DERIVED from the catalog, so
/// each engine, each version and each family is exercised.
#[tokio::test]
async fn filters_select_exactly_the_matching_catalog_entries() {
    let router = router();
    let catalog = catalog(&router).await;

    let mut rows: Vec<(Value, Vec<Reported>)> = vec![(json!({}), catalog.clone())];
    let mut engines: Vec<String> = catalog.iter().map(|entry| entry.engine.clone()).collect();
    engines.sort();
    engines.dedup();
    for engine in &engines {
        rows.push((
            json!({ "Engine": engine }),
            catalog
                .iter()
                .filter(|entry| &entry.engine == engine)
                .cloned()
                .collect(),
        ));
    }
    for entry in &catalog {
        rows.push((
            json!({ "EngineVersion": entry.version }),
            catalog
                .iter()
                .filter(|other| other.version == entry.version)
                .cloned()
                .collect(),
        ));
        rows.push((
            json!({ "ParameterGroupFamily": entry.family }),
            catalog
                .iter()
                .filter(|other| other.family == entry.family)
                .cloned()
                .collect(),
        ));
        rows.push((
            json!({ "Engine": entry.engine, "EngineVersion": entry.version }),
            vec![entry.clone()],
        ));
        rows.push((
            json!({
                "Engine": entry.engine,
                "EngineVersion": entry.version,
                "ParameterGroupFamily": entry.family,
            }),
            vec![entry.clone()],
        ));
    }

    for (request, want) in rows {
        let case = format!("filter {request}");
        let got = describe(&router, &request, &case).await;
        assert_eq!(
            got, want,
            "case {case:?}: expected {want:?}, got {got:?}"
        );
    }
}

/// A filter naming no version the service offers is the modelled
/// `InvalidParameterValueException`, not an empty page: a version only ANOTHER engine
/// offers, a family belonging to another engine, a version nothing has. The
/// cross-engine rows are derived from the catalog, so the rule is checked for every
/// ordered pair of engines. An `Engine` the catalog does not publish is the OTHER
/// refusal, asserted after the table: the engine is checked on its own, so it answers
/// `InvalidParameterCombinationException` naming the engine asked for.
#[tokio::test]
async fn a_combination_naming_no_offered_version_is_refused() {
    let router = router();
    let catalog = catalog(&router).await;

    let mut rows: Vec<Value> = Vec::new();
    for entry in &catalog {
        for other in &catalog {
            if other.engine == entry.engine {
                continue;
            }
            // `other.engine` does not offer `entry.version` unless some entry says so.
            if !catalog
                .iter()
                .any(|known| known.engine == other.engine && known.version == entry.version)
            {
                rows.push(json!({ "Engine": other.engine, "EngineVersion": entry.version }));
            }
            rows.push(json!({ "Engine": other.engine, "ParameterGroupFamily": entry.family }));
        }
    }
    assert!(
        !rows.is_empty(),
        "case {:?}: the catalog must offer versions that distinguish its engines, \
         got {catalog:?}",
        "cross-engine rows"
    );
    // A version and a family the catalog does not publish at all, in each shape a
    // filter can take.
    rows.push(json!({ "EngineVersion": "99.99" }));
    rows.push(json!({ "ParameterGroupFamily": "memorydb_no-such-family" }));
    rows.push(json!({ "Engine": catalog[0].engine, "EngineVersion": "99.99" }));

    for request in rows {
        let case = format!("refused {request}");
        let (status, body) = call(&router, "DescribeEngineVersions", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected 400 InvalidParameterValueException, \
             got {status} with {body}"
        );
    }

    // An `Engine` this service does not run: refused on the engine alone, whatever else
    // the request filters on, and in a letter case no published engine uses \u2014 `Engine`
    // is an enum-like value rather than a resource name, so it is not folded.
    let unknown_engine: [Value; 3] = [
        json!({ "Engine": "no-such-engine" }),
        json!({ "Engine": "no-such-engine", "EngineVersion": catalog[0].version }),
        json!({ "Engine": catalog[0].engine.to_uppercase() }),
    ];
    for request in unknown_engine {
        let case = format!("unknown engine {request}");
        let (status, body) = call(&router, "DescribeEngineVersions", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterCombinationException"),
            "case {case:?}: expected 400 InvalidParameterCombinationException, \
             got {status} with {body}"
        );
    }
}

/// `DefaultOnly` reports the default of each parameter group family: the greatest minor
/// version the catalog offers for that engine and major version. Checked unfiltered and
/// per engine, against the defaults computed from the catalog itself.
#[tokio::test]
async fn default_only_reports_the_latest_version_of_each_family() {
    let router = router();
    let catalog = catalog(&router).await;

    let defaults = |within: &[Reported]| -> Vec<Reported> {
        within
            .iter()
            .filter(|entry| {
                let case = format!("{} {}", entry.engine, entry.version);
                let (major, minor) = version_parts(&entry.version, &case);
                !catalog.iter().any(|other| {
                    let other_case = format!("{} {}", other.engine, other.version);
                    let (other_major, other_minor) = version_parts(&other.version, &other_case);
                    other.engine == entry.engine && other_major == major && other_minor > minor
                })
            })
            .cloned()
            .collect()
    };

    let mut rows: Vec<(Value, Vec<Reported>)> = vec![(
        json!({ "DefaultOnly": true }),
        defaults(&catalog),
    )];
    let mut engines: Vec<String> = catalog.iter().map(|entry| entry.engine.clone()).collect();
    engines.sort();
    engines.dedup();
    for engine in &engines {
        let of_engine: Vec<Reported> = catalog
            .iter()
            .filter(|entry| &entry.engine == engine)
            .cloned()
            .collect();
        rows.push((
            json!({ "Engine": engine, "DefaultOnly": true }),
            defaults(&of_engine),
        ));
        // `DefaultOnly: false` is the default the model declares: it narrows nothing.
        rows.push((json!({ "Engine": engine, "DefaultOnly": false }), of_engine));
    }

    for (request, want) in rows {
        let case = format!("default-only {request}");
        let got = describe(&router, &request, &case).await;
        assert_eq!(
            got, want,
            "case {case:?}: expected {want:?}, got {got:?}"
        );
    }
}

/// The listing pages over the whole catalog in catalog order for any page size the
/// service's window admits, and a page size outside that window, or a token naming no
/// position, is refused.
#[tokio::test]
async fn the_listing_pages_over_the_whole_catalog() {
    let router = router();
    let catalog = catalog(&router).await;

    // At the bottom of the page-size window, one past it, and at the top: the catalog is
    // smaller than the smallest page the service serves, so every one of them is a page
    // that holds it whole — which is the rule under test.
    for max_results in [
        WINDOW_MIN,
        WINDOW_MIN + 1,
        (WINDOW_MIN + WINDOW_MAX) / 2,
        WINDOW_MAX,
    ] {
        let case = format!("MaxResults {max_results}");
        let mut collected: Vec<Reported> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let request = match &token {
                Some(token) => json!({ "MaxResults": max_results, "NextToken": token }),
                None => json!({ "MaxResults": max_results }),
            };
            let body = ok(&router, "DescribeEngineVersions", &request, &case).await;
            let page = reported(&body, &case);
            assert!(
                i32::try_from(page.len()).unwrap_or(i32::MAX) <= max_results,
                "case {case:?}: a page must hold at most {max_results} entries, got {page:?}"
            );
            collected.extend(page);
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= catalog.len() + 1,
                "case {case:?}: paging must terminate, got {collected:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, catalog,
            "case {case:?}: the pages must concatenate to the whole catalog in order, \
             got {collected:?}"
        );
    }

    let refused: [Value; 4] = [
        // Outside the page-size window, on each side of it.
        json!({ "MaxResults": WINDOW_MIN - 1 }),
        json!({ "MaxResults": WINDOW_MAX + 1 }),
        json!({ "MaxResults": -1 }),
        // A token no page of this listing ever handed out.
        json!({ "NextToken": "zzz-no-such-position" }),
    ];
    for request in refused {
        let case = format!("refused {request}");
        let (status, body) = call(&router, "DescribeEngineVersions", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected 400 InvalidParameterValueException, got {status} \
             with {body}"
        );
    }
}

/// Describing engine versions is a READ: it reports the same catalog however often it is
/// called, and it changes nothing another operation reports.
#[tokio::test]
async fn describing_engine_versions_changes_nothing() {
    let router = router();
    let case = "stable read";
    ok(&router, "CreateACL", &json!({ "ACLName": "versions-acl" }), case).await;
    ok(
        &router,
        "CreateCluster",
        &json!({
            "ClusterName": "versions-cluster",
            "NodeType": "db.t4g.small",
            "ACLName": "versions-acl",
        }),
        case,
    )
    .await;

    let first = catalog(&router).await;
    let second = catalog(&router).await;
    assert_eq!(
        first, second,
        "case {case:?}: two reads must agree, got {first:?} then {second:?}"
    );

    // The one cluster and the one ACL are still exactly what they were.
    let clusters = ok(&router, "DescribeClusters", &json!({}), case).await;
    assert_eq!(
        clusters["Clusters"]
            .as_array()
            .expect("Clusters is a list")
            .len(),
        1,
        "case {case:?}: a read must not change the clusters, got {clusters}"
    );
    let acls = ok(&router, "DescribeACLs", &json!({}), case).await;
    assert_eq!(
        acls["ACLs"].as_array().expect("ACLs is a list").len(),
        // The created ACL, beside the built-in `open-access` ACL every MemoryDB
        // account already has.
        2,
        "case {case:?}: a read must not change the ACLs, got {acls}"
    );
}
