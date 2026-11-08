// SPDX-License-Identifier: CC0-1.0

//! `GetCompatibleElasticsearchVersions`, driven as real restJson1 requests
//! through the router.
//!
//! The rule under test: the operation reports the upgrade paths out of the
//! service's supported engine versions — a request naming a domain is answered
//! with ONE entry whose `SourceVersion` is the version that domain stores, and
//! a request naming none with every supported source version. From a version a
//! domain reaches every later supported version of its own major, crosses a
//! major boundary only from the last supported version of a major, into the
//! versions of the major that follows it, and from ANY Elasticsearch version
//! reaches the first OpenSearch major the service offers.
//!
//! The rows below span that class: the first and last supported version, a
//! version in the middle of a major, the last version of an Elasticsearch and
//! of an OpenSearch major, a version the service no longer offers, an
//! OpenSearch version, and the model's abbreviated `OS_X.Y` spelling. The
//! modelled FORM of a version string, and so which spellings a row may use, is
//! this service's own model:
//!
//! ```text
//! smithy_query(query="shape",shape="ElasticsearchVersionString",model_path="services/service-com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015/smithy-model/com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015.json")
//! ```

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_in, router, Answer};

/// The region the test signs for.
const REGION: &str = "us-west-2";

async fn create(router: &MultiServiceRouter, name: &str, version: Option<&str>) -> Answer {
    let mut body = json!({
        "DomainName": name,
        "ElasticsearchClusterConfig": {"InstanceType": "t3.small.elasticsearch", "InstanceCount": 1}
    });
    if let Some(version) = version {
        body["ElasticsearchVersion"] = json!(version);
    }
    call_in(router, REGION, "POST", "/2015-01-01/es/domain", Some(&body)).await
}

async fn describe(router: &MultiServiceRouter, name: &str) -> Answer {
    call_in(
        router,
        REGION,
        "GET",
        &format!("/2015-01-01/es/domain/{name}"),
        None,
    )
    .await
}

async fn compatible(router: &MultiServiceRouter, domain_name: Option<&str>) -> Answer {
    let path = match domain_name {
        Some(name) => format!("/2015-01-01/es/compatibleVersions?domainName={name}"),
        None => "/2015-01-01/es/compatibleVersions".to_string(),
    };
    call_in(router, REGION, "GET", &path, None).await
}

/// The entries of one answer, each as its `SourceVersion` and `TargetVersions`.
fn entries(case: &str, answer: &Answer) -> Vec<(String, Vec<String>)> {
    answer
        .at(case, "/CompatibleElasticsearchVersions")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: CompatibleElasticsearchVersions must be a list, got {body}",
                body = answer.body
            )
        })
        .iter()
        .map(|entry| {
            let source = entry
                .get("SourceVersion")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let targets = entry
                .get("TargetVersions")
                .and_then(Value::as_array)
                .map(|targets| {
                    targets
                        .iter()
                        .map(|target| {
                            target
                                .as_str()
                                .unwrap_or_else(|| {
                                    panic!(
                                        "case {case:?}: a target version must be a string, \
                                         got {target}"
                                    )
                                })
                                .to_string()
                        })
                        .collect()
                })
                .unwrap_or_default();
            (source, targets)
        })
        .collect()
}

/// A version's place in the upgrade order — lineage, major, minor — computed
/// here independently of the handler, so the invariants below are checked
/// against the version strings themselves.
fn order(version: &str) -> (u8, u32, u32) {
    let (lineage, number) = match version.split_once('_') {
        Some((prefix, number)) => {
            assert!(
                prefix == "OpenSearch" || prefix == "OS",
                "case {version:?}: unexpected version prefix {prefix:?}"
            );
            (1u8, number)
        }
        None => (0u8, version),
    };
    let (major, minor) = number
        .split_once('.')
        .unwrap_or_else(|| panic!("case {version:?}: version has no minor part"));
    (
        lineage,
        major.parse().expect("major is a number"),
        minor.parse().expect("minor is a number"),
    )
}

/// One row per way a source version can sit in the supported set: the version,
/// whether the service still OFFERS it (and so lists it as a source of the
/// account-wide matrix), and the versions the upgrade-path rule reaches from it.
/// The supported set these are drawn from is the service's own published list of
/// engine versions, named once in `state::SUPPORTED_ENGINE_VERSIONS`; `7.10` is
/// one row of the set, not the rule.
struct UpgradePath {
    version: &'static str,
    offered: bool,
    targets: &'static [&'static str],
}

const UPGRADE_PATHS: [UpgradePath; 10] = [
    // A version the service no longer offers, which an old domain can still be
    // running: the boundary to cross is into the first major offered after it,
    // and the Elasticsearch-to-OpenSearch migration is open from it too.
    UpgradePath {
        version: "5.6",
        offered: false,
        targets: &[
            "6.8",
            "OpenSearch_1.0",
            "OpenSearch_1.1",
            "OpenSearch_1.2",
            "OpenSearch_1.3",
        ],
    },
    // The last supported version of its major, so it crosses into the next one.
    UpgradePath {
        version: "6.8",
        offered: true,
        targets: &[
            "7.1",
            "7.4",
            "7.7",
            "7.8",
            "7.9",
            "7.10",
            "OpenSearch_1.0",
            "OpenSearch_1.1",
            "OpenSearch_1.2",
            "OpenSearch_1.3",
        ],
    },
    // Mid-major: the later minors of its own major, plus the OpenSearch line
    // every Elasticsearch version can migrate onto.
    UpgradePath {
        version: "7.7",
        offered: true,
        targets: &[
            "7.8",
            "7.9",
            "7.10",
            "OpenSearch_1.0",
            "OpenSearch_1.1",
            "OpenSearch_1.2",
            "OpenSearch_1.3",
        ],
    },
    // The last Elasticsearch version: the major after it is the first OpenSearch
    // one.
    UpgradePath {
        version: "7.10",
        offered: true,
        targets: &[
            "OpenSearch_1.0",
            "OpenSearch_1.1",
            "OpenSearch_1.2",
            "OpenSearch_1.3",
        ],
    },
    // Mid-major inside the OpenSearch line: no boundary crossing, and no
    // Elasticsearch-to-OpenSearch migration to make.
    UpgradePath {
        version: "OpenSearch_1.2",
        offered: true,
        targets: &["OpenSearch_1.3"],
    },
    UpgradePath {
        version: "OpenSearch_1.3",
        offered: true,
        targets: &[
            "OpenSearch_2.3",
            "OpenSearch_2.5",
            "OpenSearch_2.7",
            "OpenSearch_2.9",
            "OpenSearch_2.11",
            "OpenSearch_2.13",
            "OpenSearch_2.15",
            "OpenSearch_2.17",
            "OpenSearch_2.19",
        ],
    },
    // The abbreviated spelling the model's pattern also admits, mid-major.
    UpgradePath {
        version: "OS_2.3",
        offered: true,
        targets: &[
            "OpenSearch_2.5",
            "OpenSearch_2.7",
            "OpenSearch_2.9",
            "OpenSearch_2.11",
            "OpenSearch_2.13",
            "OpenSearch_2.15",
            "OpenSearch_2.17",
            "OpenSearch_2.19",
        ],
    },
    UpgradePath {
        version: "OpenSearch_2.19",
        offered: true,
        targets: &["OpenSearch_3.0", "OpenSearch_3.1"],
    },
    UpgradePath {
        version: "OpenSearch_3.0",
        offered: true,
        targets: &["OpenSearch_3.1"],
    },
    // The newest supported version: nothing to upgrade to.
    UpgradePath {
        version: "OpenSearch_3.1",
        offered: true,
        targets: &[],
    },
];

/// A domain's answer is ONE entry, whose source version is the version the
/// domain stores and whose targets are the versions the rule reaches from it.
#[tokio::test]
async fn a_domains_answer_reports_its_stored_version_and_its_upgrade_targets() {
    for (index, path) in UPGRADE_PATHS.iter().enumerate() {
        let version = path.version;
        let expected_targets = path.targets;
        let router = router();
        let name = format!("compat-domain-{index}");
        let created = create(&router, &name, Some(version)).await;
        assert_eq!(
            created.status, 200,
            "case {version:?}: create must answer 200, got {} {}",
            created.status, created.body
        );

        // The stored version a later read reports is the one the answer below
        // must take its source version from.
        let read = describe(&router, &name).await;
        assert_eq!(
            read.at(version, "/DomainStatus/ElasticsearchVersion"),
            &json!(version),
            "case {version:?}: the domain must store the version it was created with, got {:?}",
            read.body.pointer("/DomainStatus/ElasticsearchVersion")
        );

        let answer = compatible(&router, Some(&name)).await;
        assert_eq!(
            answer.status, 200,
            "case {version:?}: the call must answer 200, got {} {}",
            answer.status, answer.body
        );
        let found = entries(version, &answer);
        assert_eq!(
            found.len(),
            1,
            "case {version:?}: a named domain must get exactly one entry, got {found:?}"
        );
        assert_eq!(
            found[0].0, version,
            "case {version:?}: SourceVersion must be the domain's stored version, got {:?}",
            found[0].0
        );
        assert_eq!(
            found[0].1,
            expected_targets.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
            "case {version:?}: unexpected TargetVersions, got {:?}",
            found[0].1
        );

        // Every target is strictly later than the source, and sits in the
        // source's own major, the major after it, or the OpenSearch line an
        // Elasticsearch domain migrates onto.
        let source = order(version);
        let mut majors: Vec<(u8, u32)> = found[0]
            .1
            .iter()
            .map(|target| {
                let target_order = order(target);
                assert!(
                    target_order > source,
                    "case {version:?}: target {target:?} is not later than the source, \
                     got {target_order:?} vs {source:?}"
                );
                (target_order.0, target_order.1)
            })
            .collect();
        majors.dedup();
        assert!(
            majors.len() <= 2 && majors.iter().all(|major| *major >= (source.0, source.1)),
            "case {version:?}: targets must stay in the source's major, the one after it or \
             the OpenSearch line, got majors {majors:?} for source {source:?}"
        );
    }
}

/// With no domain named, every supported version an upgrade can start from gets
/// an entry, in upgrade order, and each entry reaches at least one version.
#[tokio::test]
async fn the_catalog_covers_every_supported_source_version() {
    let case = "no domain named";
    let router = router();
    let answer = compatible(&router, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the call must answer 200, got {} {}",
        answer.status, answer.body
    );
    let found = entries(case, &answer);
    assert!(
        found.len() > 1,
        "case {case:?}: the catalog must carry more than one source version, got {found:?}"
    );

    let mut previous: Option<(u8, u32, u32)> = None;
    for (source, targets) in &found {
        let source_order = order(source);
        if let Some(previous) = previous {
            assert!(
                source_order > previous,
                "case {case:?}: source versions must ascend, {source:?} ({source_order:?}) \
                 follows {previous:?}"
            );
        }
        previous = Some(source_order);
        assert!(
            !targets.is_empty(),
            "case {case:?}: {source:?} is listed as a source but reaches nothing, \
             got {targets:?}"
        );
    }

    // Each row the service still offers is a source the catalog carries, with
    // the same targets the per-domain answer gives — except the newest version,
    // which is the source of no upgrade, and a version the service no longer
    // offers, which is the source of an upgrade but not an engine to start a
    // domain on. A row written in the abbreviated `OS_X.Y` spelling is the same
    // version as its `OpenSearch_X.Y` catalog entry.
    for path in UPGRADE_PATHS {
        let version = path.version;
        let expected_targets = path.targets;
        let canonical = match version.strip_prefix("OS_") {
            Some(number) => format!("OpenSearch_{number}"),
            None => version.to_string(),
        };
        let entry = found.iter().find(|(source, _)| *source == canonical);
        let listed = path.offered && !expected_targets.is_empty();
        match (entry, listed) {
            (Some((_, targets)), true) => assert_eq!(
                *targets,
                expected_targets
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>(),
                "case {version:?}: the catalog must give the same targets as the \
                 per-domain answer, got {targets:?}"
            ),
            (None, false) => {}
            (entry, listed) => panic!(
                "case {version:?}: expected catalog entry present={listed}, got {entry:?}"
            ),
        }
    }
}

/// Every Elasticsearch source the matrix carries reports its OpenSearch upgrade
/// targets, and the matrix carries the OpenSearch line as a source too: this is
/// what an upgrade planner reads to decide a current domain can be moved onto
/// OpenSearch at all.
#[tokio::test]
async fn every_elasticsearch_source_reaches_the_opensearch_line() {
    let case = "the account-wide matrix";
    let router = router();
    let found = entries(case, &compatible(&router, None).await);

    let mut elasticsearch_sources = 0;
    let mut opensearch_sources = 0;
    for (source, targets) in &found {
        if order(source).0 == 0 {
            elasticsearch_sources += 1;
            assert!(
                targets.iter().any(|target| order(target).0 == 1),
                "case {case:?}: Elasticsearch source {source:?} must reach the OpenSearch \
                 line, got {targets:?}"
            );
        } else {
            opensearch_sources += 1;
        }
    }
    assert!(
        elasticsearch_sources > 0 && opensearch_sources > 0,
        "case {case:?}: the matrix must cover both engine lines, got \
         {elasticsearch_sources} Elasticsearch and {opensearch_sources} OpenSearch sources \
         in {found:?}"
    );
}

/// A domain created without a version has no source version stored to narrow
/// on, so it is answered with the same catalog a request naming no domain gets.
#[tokio::test]
async fn a_domain_with_no_stored_version_gets_the_whole_catalog() {
    let case = "domain created without a version";
    let router = router();
    let name = "compat-no-version";
    let created = create(&router, name, None).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: create must answer 200, got {} {}",
        created.status, created.body
    );
    assert!(
        created.missing("/DomainStatus/ElasticsearchVersion"),
        "case {case:?}: the domain must store no version, got {}",
        created.body
    );

    let named = compatible(&router, Some(name)).await;
    let catalog = compatible(&router, None).await;
    assert_eq!(
        entries(case, &named),
        entries(case, &catalog),
        "case {case:?}: the answer must be the whole catalog, got {}",
        named.body
    );
}

/// Asking for a domain this region does not hold is refused with HTTP 404
/// `ResourceNotFoundException` — including a name held in ANOTHER region, since
/// a domain is held per region.
#[tokio::test]
async fn an_unknown_domain_is_refused() {
    let router = router();
    let elsewhere = "compat-other-region";
    let created = call_in(
        &router,
        "eu-west-1",
        "POST",
        "/2015-01-01/es/domain",
        Some(&json!({
            "DomainName": elsewhere,
            "ElasticsearchVersion": "7.10",
            "ElasticsearchClusterConfig": {"InstanceType": "t3.small.elasticsearch", "InstanceCount": 1}
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {elsewhere:?}: create must answer 200, got {} {}",
        created.status, created.body
    );

    for (case, name) in [
        ("never created", "compat-missing"),
        ("created in another region", elsewhere),
    ] {
        let answer = compatible(&router, Some(name)).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: must be refused with 404, got {} {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ResourceNotFoundException"),
            "case {case:?}: the error must be ResourceNotFoundException, got {:?} / {}",
            answer.error_type,
            answer.body
        );
        assert_eq!(
            answer.text(case, "/message"),
            format!("Domain not found: {name}"),
            "case {case:?}: unexpected message, got {}",
            answer.body
        );
    }
}

/// The call reports, it never changes what a later caller sees: two calls in a
/// row agree, and a domain read after one is unchanged.
#[tokio::test]
async fn the_call_changes_nothing() {
    let case = "repeated call";
    let router = router();
    let name = "compat-stable";
    create(&router, name, Some("6.8")).await;
    let before = describe(&router, name).await;

    let first = compatible(&router, Some(name)).await;
    let second = compatible(&router, Some(name)).await;
    assert_eq!(
        entries(case, &first),
        entries(case, &second),
        "case {case:?}: two reads must agree, got {} then {}",
        first.body,
        second.body
    );

    let after = describe(&router, name).await;
    assert_eq!(
        after.at(case, "/DomainStatus/ElasticsearchVersion"),
        before.at(case, "/DomainStatus/ElasticsearchVersion"),
        "case {case:?}: the domain's version must be untouched, got {}",
        after.body
    );
}
