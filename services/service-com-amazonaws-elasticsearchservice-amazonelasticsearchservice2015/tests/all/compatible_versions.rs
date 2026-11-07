// SPDX-License-Identifier: CC0-1.0

//! `GetCompatibleElasticsearchVersions`, driven as real restJson1 requests
//! through the router.
//!
//! The rule under test: the operation reports the upgrade paths out of the
//! service's supported engine versions — a request naming a domain is answered
//! with ONE entry whose `SourceVersion` is the version that domain stores, and
//! a request naming none with every supported source version. From a version a
//! domain reaches every later supported version of its own major, and crosses a
//! major boundary only from the last supported version of a major, into the
//! versions of the major that follows it.
//!
//! The rows below span that class: the first and last supported version, a
//! version in the middle of a major, the last version of each Elasticsearch
//! major, the last Elasticsearch version (whose next major is the first
//! OpenSearch one), an OpenSearch version, and the model's abbreviated `OS_X.Y`
//! spelling. The modelled FORM of a version string, and so which spellings a
//! row may use, is this service's own model:
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

/// One row per way a source version can sit in the supported set, with the
/// versions the upgrade-path rule reaches from it. The supported set these are
/// drawn from is the service's own published list of engine versions, named once
/// in `state::SUPPORTED_ENGINE_VERSIONS`; `7.10` is one row of the set, not the
/// rule.
#[allow(clippy::type_complexity)]
const UPGRADE_PATHS: [(&str, &[&str]); 10] = [
    // The oldest supported version: the last (and only) one of its major, so it
    // crosses into the next major.
    ("1.5", &["2.3"]),
    ("2.3", &["5.1", "5.3", "5.5", "5.6"]),
    // Mid-major: later minors of the same major only, no boundary crossing.
    ("5.3", &["5.5", "5.6"]),
    (
        "5.6",
        &["6.0", "6.2", "6.3", "6.4", "6.5", "6.7", "6.8"],
    ),
    ("6.8", &["7.1", "7.4", "7.7", "7.8", "7.9", "7.10"]),
    ("7.9", &["7.10"]),
    // The last Elasticsearch version: the major after it is the first OpenSearch
    // one.
    (
        "7.10",
        &[
            "OpenSearch_1.0",
            "OpenSearch_1.1",
            "OpenSearch_1.2",
            "OpenSearch_1.3",
        ],
    ),
    (
        "OpenSearch_1.3",
        &[
            "OpenSearch_2.3",
            "OpenSearch_2.5",
            "OpenSearch_2.7",
            "OpenSearch_2.9",
            "OpenSearch_2.11",
            "OpenSearch_2.13",
            "OpenSearch_2.15",
            "OpenSearch_2.17",
        ],
    ),
    // The abbreviated spelling the model's pattern also admits, mid-major.
    (
        "OS_2.3",
        &[
            "OpenSearch_2.5",
            "OpenSearch_2.7",
            "OpenSearch_2.9",
            "OpenSearch_2.11",
            "OpenSearch_2.13",
            "OpenSearch_2.15",
            "OpenSearch_2.17",
        ],
    ),
    // The newest supported version: nothing to upgrade to.
    ("OpenSearch_2.17", &[]),
];

/// A domain's answer is ONE entry, whose source version is the version the
/// domain stores and whose targets are the versions the rule reaches from it.
#[tokio::test]
async fn a_domains_answer_reports_its_stored_version_and_its_upgrade_targets() {
    for (index, (version, expected_targets)) in UPGRADE_PATHS.iter().enumerate() {
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
            found[0].0, *version,
            "case {version:?}: SourceVersion must be the domain's stored version, got {:?}",
            found[0].0
        );
        assert_eq!(
            found[0].1,
            expected_targets.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
            "case {version:?}: unexpected TargetVersions, got {:?}",
            found[0].1
        );

        // Every target is strictly later than the source, and sits either in the
        // source's own major or in exactly one major after it.
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
            "case {version:?}: targets must stay in the source's major or the next one, \
             got majors {majors:?} for source {source:?}"
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

    // Each row of the table is a source the catalog carries, with the same
    // targets the per-domain answer gives — except the newest version, which is
    // the source of no upgrade. A row written in the abbreviated `OS_X.Y`
    // spelling is the same version as its `OpenSearch_X.Y` catalog entry.
    for (version, expected_targets) in UPGRADE_PATHS {
        let canonical = match version.strip_prefix("OS_") {
            Some(number) => format!("OpenSearch_{number}"),
            None => version.to_string(),
        };
        let entry = found.iter().find(|(source, _)| *source == canonical);
        match (entry, expected_targets.is_empty()) {
            (Some((_, targets)), false) => assert_eq!(
                *targets,
                expected_targets
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>(),
                "case {version:?}: the catalog must give the same targets as the \
                 per-domain answer, got {targets:?}"
            ),
            (None, true) => {}
            (entry, expected_empty) => panic!(
                "case {version:?}: expected catalog entry present={}, got {entry:?}",
                !expected_empty
            ),
        }
    }
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
