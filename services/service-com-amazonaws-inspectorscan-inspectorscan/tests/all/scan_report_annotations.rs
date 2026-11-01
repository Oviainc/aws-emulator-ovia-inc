// SPDX-License-Identifier: CC0-1.0

//! What a `ScanSbom` report says about the scan itself, end-to-end over HTTP.
//!
//! The rules under test:
//!
//! * a scan reports WHAT IT FOUND as a summary over the matched findings — the
//!   Inspector forms as the per-severity `vulnerability_count` (every bucket
//!   present, zeros included), the CycloneDX form as the same totals in the
//!   namespaced `metadata.properties` — so a scan that matched nothing still
//!   reports zeros rather than nothing;
//! * a scan reports a component it holds NO RULE for as skipped: in the
//!   Inspector forms as a `messages[]` entry, in the CycloneDX form as that
//!   component's own `…:info` property. A component whose package the store has
//!   rules for was ruled on, so it is never reported as skipped — whether or not
//!   its version fell inside an affected range;
//! * a report IDENTIFIES its own components: every entry of the component tree
//!   carries a document-local `comp-<n>` ref, assigned in document order,
//!   whatever ref the submitted document carried, and every `affects[].ref`
//!   names one of them;
//! * the CycloneDX report names the scanning SERVICE in
//!   `metadata.tools.services`, beside whatever the submitted document said
//!   about the tools that produced it;
//! * `outputFormat` is optional, and a request naming none is answered the
//!   Inspector-native report.
//!
//! The advisory catalog the matching rows are written against is
//! `src/state.rs::seed_vulnerability_db`; the rows that need a severity or a
//! classification the catalog does not carry inject their own advisory into the
//! same store the handler reads.

use super::scan_sbom::{bom, component, emulator, post_scan};
use serde_json::{json, Value};
use service_com_amazonaws_inspectorscan_inspectorscan::state::{
    build_index, ComAmazonawsInspectorscanInspectorscanState, ReleaseVersion, VersionRange,
    VulnRecord,
};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The namespace the scan's own annotations live under in a CycloneDX report.
const NAMESPACE: &str = "amazon:inspector:sbom_scanner";

/// What a scan reports for a component it held no rule for.
const SKIPPED: &str = "Component skipped: no supported rules found.";

/// The service entry the scan reports itself under.
const SERVICE_NAME: &str = "Amazon Inspector Scan SBOM API";

/// The severity buckets a scan summary is reported over, and the
/// `metadata.properties` name each is reported under in the CycloneDX form.
/// `other` is the bucket for a finding rated outside the four named severities.
const BUCKETS: &[(&str, &str)] = &[
    ("critical", "critical_vulnerabilities"),
    ("high", "high_vulnerabilities"),
    ("medium", "medium_vulnerabilities"),
    ("low", "low_vulnerabilities"),
    ("other", "other_vulnerabilities"),
];

/// The value of the CycloneDX property named `name`, if the array carries it.
fn property(properties: &Value, name: &str) -> Option<String> {
    properties
        .as_array()?
        .iter()
        .find(|entry| entry["name"].as_str() == Some(name))
        .and_then(|entry| entry["value"].as_str().map(str::to_string))
}

/// Every `bom-ref` of a report's component tree, in document order, parents
/// before children.
fn refs_in_document_order(components: &Value) -> Vec<String> {
    let mut refs = Vec::new();
    if let Some(entries) = components.as_array() {
        for entry in entries {
            refs.push(entry["bom-ref"].as_str().unwrap_or_default().to_string());
            refs.extend(refs_in_document_order(&entry["components"]));
        }
    }
    refs
}

/// The `info_message` a report's `messages[]` carries for `purl`, if any.
fn message_for(report: &Value, purl: &str) -> Option<String> {
    report["messages"]
        .as_array()?
        .iter()
        .find(|entry| entry["purl"].as_str() == Some(purl))
        .and_then(|entry| entry["info_message"].as_str().map(str::to_string))
}

/// An advisory the seeded catalog does not carry, pushed into the SAME store the
/// handler reads so a scan matches it like any other.
fn inject_advisory(
    engine: &Arc<engine::InMemoryEngine>,
    id: &str,
    package_type: &str,
    package_name: &str,
    severity: &str,
    cwes: Vec<u32>,
) {
    let state = engine.state::<ComAmazonawsInspectorscanInspectorscanState>();
    let mut state = state.lock();
    state.vulnerability_db.push(VulnRecord {
        cve_id: id.to_string(),
        source: "NVD".to_string(),
        source_url: None,
        package_name: package_name.to_string(),
        package_type: package_type.to_string(),
        affected: VersionRange::before(ReleaseVersion::new(9, 0, 0)),
        fixed_in_version: Some("9.0.0".to_string()),
        cvss_v3_score: 5.0,
        cvss_v3_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:L/I:N/A:N".to_string(),
        nvd_severity: severity.to_string(),
        inspector_score: 5.0,
        inspector_cvss_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:L/I:N/A:N".to_string(),
        inspector_severity: severity.to_string(),
        cwes,
        description: format!("{id} applies to {package_name}."),
        advisories: Vec::new(),
        published: "2024-01-01T00:00:00Z".to_string(),
        modified: "2024-01-02T00:00:00Z".to_string(),
    });
    let index = build_index(&state.vulnerability_db);
    state.index = index;
}

/// One row of the summary table: a document, and the per-severity totals the
/// scan of it must report.
#[derive(Debug)]
struct SummaryCase {
    what: &'static str,
    sbom: Value,
    /// critical, high, medium, low, other — in `BUCKETS` order.
    totals: [u64; 5],
}

/// Documents spanning the summary's whole range: no components at all, an empty
/// list, a component no advisory applies to, a version past an advisory's fixed
/// release, one finding of each severity the catalog carries, and several
/// findings in several ecosystems.
fn summary_cases() -> Vec<SummaryCase> {
    vec![
        SummaryCase {
            what: "no components member at all",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.5" }),
            totals: [0, 0, 0, 0, 0],
        },
        SummaryCase {
            what: "an empty components array",
            sbom: bom(Vec::new()),
            totals: [0, 0, 0, 0, 0],
        },
        SummaryCase {
            what: "a package no advisory applies to",
            sbom: bom(vec![component(
                "ref-clean",
                "left-pad",
                "1.0.0",
                "pkg:npm/left-pad@1.0.0",
            )]),
            totals: [0, 0, 0, 0, 0],
        },
        SummaryCase {
            what: "a version at the advisory's fixed release",
            sbom: bom(vec![component(
                "ref-fixed",
                "lodash",
                "4.17.21",
                "pkg:npm/lodash@4.17.21",
            )]),
            totals: [0, 0, 0, 0, 0],
        },
        SummaryCase {
            what: "one critical finding",
            sbom: bom(vec![component(
                "ref-minimist",
                "minimist",
                "1.2.5",
                "pkg:npm/minimist@1.2.5",
            )]),
            totals: [1, 0, 0, 0, 0],
        },
        SummaryCase {
            what: "one high finding",
            sbom: bom(vec![component(
                "ref-lodash",
                "lodash",
                "4.17.20",
                "pkg:npm/lodash@4.17.20",
            )]),
            totals: [0, 1, 0, 0, 0],
        },
        SummaryCase {
            what: "a critical and a high finding",
            sbom: bom(vec![
                component("ref-minimist", "minimist", "1.2.5", "pkg:npm/minimist@1.2.5"),
                component("ref-lodash", "lodash", "4.17.20", "pkg:npm/lodash@4.17.20"),
            ]),
            totals: [1, 1, 0, 0, 0],
        },
        SummaryCase {
            what: "two critical findings in two ecosystems",
            sbom: bom(vec![
                component("ref-minimist", "minimist", "1.2.5", "pkg:npm/minimist@1.2.5"),
                component(
                    "ref-log4j",
                    "log4j-core",
                    "2.14.1",
                    "pkg:maven/org.apache.logging.log4j/log4j-core@2.14.1",
                ),
            ]),
            totals: [2, 0, 0, 0, 0],
        },
        SummaryCase {
            what: "three findings and one clean component across four ecosystems",
            sbom: bom(vec![
                component("ref-urllib3", "urllib3", "1.26.4", "pkg:pypi/urllib3@1.26.4"),
                component("ref-pyyaml", "PyYAML", "5.3", "pkg:pypi/pyyaml@5.3"),
                component(
                    "ref-gin",
                    "gin",
                    "v1.7.0",
                    "pkg:golang/github.com/gin-gonic/gin@v1.7.0",
                ),
                component("ref-clean", "left-pad", "1.0.0", "pkg:npm/left-pad@1.0.0"),
            ]),
            totals: [1, 2, 0, 0, 0],
        },
    ]
}

#[tokio::test]
async fn the_scan_summary_is_reported_in_every_output_format() {
    let (_engine, router) = emulator();
    for case in summary_cases() {
        // The Inspector forms carry the summary as `vulnerability_count`.
        for format in ["INSPECTOR", "INSPECTOR_ALT"] {
            let request = json!({ "sbom": case.sbom, "outputFormat": format });
            let (status, response) = post_scan(&router, &request).await;
            assert_eq!(
                status, 200,
                "case {:?} in {format}: expected HTTP 200, got {status}: {response}",
                case.what
            );
            let counts = &response["sbom"]["vulnerability_count"];
            for (position, (bucket, _)) in BUCKETS.iter().enumerate() {
                assert_eq!(
                    counts[bucket].as_u64(),
                    Some(case.totals[position]),
                    "case {:?} in {format}: expected {} {bucket} finding(s), got {counts}",
                    case.what,
                    case.totals[position]
                );
            }
            assert_eq!(
                counts.as_object().map(serde_json::Map::len),
                Some(BUCKETS.len()),
                "case {:?} in {format}: the count object carries one member per severity \
                 bucket, got {counts}",
                case.what
            );
        }

        // The CycloneDX form carries the same totals as namespaced properties,
        // each value the decimal count.
        let request = json!({ "sbom": case.sbom, "outputFormat": "CYCLONE_DX_1_5" });
        let (status, response) = post_scan(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {:?} in CYCLONE_DX_1_5: expected HTTP 200, got {status}: {response}",
            case.what
        );
        let properties = &response["sbom"]["metadata"]["properties"];
        for (position, (_, name)) in BUCKETS.iter().enumerate() {
            let qualified = format!("{NAMESPACE}:{name}");
            assert_eq!(
                property(properties, &qualified).as_deref(),
                Some(case.totals[position].to_string().as_str()),
                "case {:?}: expected {qualified} to be {}, got {properties}",
                case.what,
                case.totals[position]
            );
        }
        // No malicious-package detection applies to any of these documents, so
        // that total is reported as zero rather than left out.
        assert_eq!(
            property(properties, &format!("{NAMESPACE}:malicious_packages")).as_deref(),
            Some("0"),
            "case {:?}: expected no malicious packages, got {properties}",
            case.what
        );
    }
}

/// Severities and classifications the seeded catalog does not carry: the
/// summary must bucket a finding by the severity it is rated at, and count a
/// malicious-package detection (CWE-506, "Embedded Malicious Code") under the
/// malicious total as well.
#[tokio::test]
async fn an_advisory_of_any_severity_lands_in_its_own_bucket() {
    // CWE-506 is the "Embedded Malicious Code" classification a
    // malicious-package detection carries; the other rows carry a weakness
    // that is not that one.
    for (case, severity, cwes, bucket, malicious) in [
        ("a medium advisory", "MEDIUM", vec![79_u32], "medium", "0"),
        ("a low advisory", "LOW", vec![200], "low", "0"),
        ("an unrated advisory", "UNTRIAGED", vec![400], "other", "0"),
        (
            "a malicious-package detection",
            "CRITICAL",
            vec![506],
            "critical",
            "1",
        ),
    ] {
        let (engine, router) = emulator();
        inject_advisory(
            &engine,
            "CVE-2024-99999",
            "npm",
            "under-test-package",
            severity,
            cwes,
        );
        let sbom = bom(vec![component(
            "ref-under-test",
            "under-test-package",
            "1.0.0",
            "pkg:npm/under-test-package@1.0.0",
        )]);

        let (status, response) =
            post_scan(&router, &json!({ "sbom": sbom, "outputFormat": "INSPECTOR" })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status}: {response}"
        );
        let counts = &response["sbom"]["vulnerability_count"];
        assert_eq!(
            counts[bucket].as_u64(),
            Some(1),
            "case {case:?}: expected the finding in the {bucket} bucket, got {counts}"
        );
        let total: u64 = BUCKETS
            .iter()
            .filter_map(|(name, _)| counts[name].as_u64())
            .sum();
        assert_eq!(
            total, 1,
            "case {case:?}: the finding is counted once over all buckets, got {counts}"
        );

        let (status, response) = post_scan(
            &router,
            &json!({ "sbom": sbom, "outputFormat": "CYCLONE_DX_1_5" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status}: {response}"
        );
        let properties = &response["sbom"]["metadata"]["properties"];
        assert_eq!(
            property(properties, &format!("{NAMESPACE}:malicious_packages")).as_deref(),
            Some(malicious),
            "case {case:?}: expected {malicious} malicious package(s), got {properties}"
        );
    }
}

/// One row of the skip table: a component, and whether a scan holds a rule for
/// its package at all.
#[derive(Debug)]
struct SkipCase {
    what: &'static str,
    entry: Value,
    purl: Option<&'static str>,
    ruled_on: bool,
}

#[tokio::test]
async fn a_component_no_rule_applies_to_is_reported_as_skipped() {
    let cases = vec![
        SkipCase {
            what: "a package with a matching advisory",
            entry: component("ref-a", "lodash", "4.17.20", "pkg:npm/lodash@4.17.20"),
            purl: Some("pkg:npm/lodash@4.17.20"),
            ruled_on: true,
        },
        SkipCase {
            // The store holds rules for this package, and they cleared this
            // release: the component was ruled on, so nothing was skipped.
            what: "a package whose advisory does not cover this version",
            entry: component("ref-b", "lodash", "4.17.21", "pkg:npm/lodash@4.17.21"),
            purl: Some("pkg:npm/lodash@4.17.21"),
            ruled_on: true,
        },
        SkipCase {
            what: "a maven package below the advisory's lower bound",
            entry: component(
                "ref-c",
                "log4j-core",
                "1.2.17",
                "pkg:maven/org.apache.logging.log4j/log4j-core@1.2.17",
            ),
            purl: Some("pkg:maven/org.apache.logging.log4j/log4j-core@1.2.17"),
            ruled_on: true,
        },
        SkipCase {
            what: "an OS package with a matching advisory",
            entry: component("ref-d", "openssl", "3.0.6", "pkg:deb/debian/openssl@3.0.6"),
            purl: Some("pkg:deb/debian/openssl@3.0.6"),
            ruled_on: true,
        },
        SkipCase {
            what: "a package no advisory mentions",
            entry: component("ref-e", "left-pad", "1.0.0", "pkg:npm/left-pad@1.0.0"),
            purl: Some("pkg:npm/left-pad@1.0.0"),
            ruled_on: false,
        },
        SkipCase {
            // The rules are keyed by (ecosystem, name): the same name in
            // another ecosystem is a package the store knows nothing about.
            what: "a known name in an ecosystem the advisory is not for",
            entry: component("ref-f", "lodash", "4.17.20", "pkg:pypi/lodash@4.17.20"),
            purl: Some("pkg:pypi/lodash@4.17.20"),
            ruled_on: false,
        },
        SkipCase {
            what: "a package in an ecosystem the catalog does not cover",
            entry: component(
                "ref-g",
                "not-in-the-catalog",
                "1.0.0",
                "pkg:cargo/not-in-the-catalog@1.0.0",
            ),
            purl: Some("pkg:cargo/not-in-the-catalog@1.0.0"),
            ruled_on: false,
        },
        SkipCase {
            // No purl: the entry names no coordinate, so there is nothing to
            // look up and nothing to name the message by.
            what: "a component carrying no purl",
            entry: json!({ "type": "library", "name": "unknown-thing", "version": "1.0.0" }),
            purl: None,
            ruled_on: false,
        },
    ];

    let (_engine, router) = emulator();
    for case in &cases {
        // The Inspector form reports a skipped component in `messages`.
        let request = json!({
            "sbom": bom(vec![case.entry.clone()]),
            "outputFormat": "INSPECTOR",
        });
        let (status, response) = post_scan(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status}: {response}",
            case.what
        );
        let messages = response["sbom"]["messages"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if case.ruled_on {
            assert!(
                messages.is_empty(),
                "case {:?}: a component that was ruled on is not reported as skipped, got {:?}",
                case.what,
                messages
            );
        } else {
            assert_eq!(
                messages.len(),
                1,
                "case {:?}: expected one skip message, got {messages:?}",
                case.what
            );
            assert_eq!(
                messages[0]["info_message"].as_str(),
                Some(SKIPPED),
                "case {:?}: expected the skip message {SKIPPED:?}, got {:?}",
                case.what,
                messages[0]
            );
            match case.purl {
                Some(purl) => assert_eq!(
                    message_for(&response["sbom"], purl).as_deref(),
                    Some(SKIPPED),
                    "case {:?}: the message names the component by its purl, got {:?}",
                    case.what,
                    messages[0]
                ),
                None => assert!(
                    messages[0].get("purl").is_none(),
                    "case {:?}: a component with no purl has none to be named by, got {:?}",
                    case.what,
                    messages[0]
                ),
            }
        }

        // The CycloneDX form reports it on the component itself.
        let request = json!({
            "sbom": bom(vec![case.entry.clone()]),
            "outputFormat": "CYCLONE_DX_1_5",
        });
        let (status, response) = post_scan(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status}: {response}",
            case.what
        );
        let properties = &response["sbom"]["components"][0]["properties"];
        let info = property(properties, &format!("{NAMESPACE}:info"));
        if case.ruled_on {
            assert!(
                info.is_none(),
                "case {:?}: a component that was ruled on carries no skip property, got \
                 {properties}",
                case.what
            );
        } else {
            assert_eq!(
                info.as_deref(),
                Some(SKIPPED),
                "case {:?}: expected the skip property on the component, got {properties}",
                case.what
            );
        }
    }
}

/// A submitted component property is PRESERVED beside the scan's own.
#[tokio::test]
async fn the_scans_component_property_is_appended_to_the_submitted_ones() {
    let (_engine, router) = emulator();
    let entry = json!({
        "type": "library",
        "bom-ref": "ref-own",
        "name": "left-pad",
        "version": "1.0.0",
        "purl": "pkg:npm/left-pad@1.0.0",
        "properties": [ { "name": "internal:team", "value": "platform" } ],
    });
    let request = json!({
        "sbom": bom(vec![entry]),
        "outputFormat": "CYCLONE_DX_1_5",
    });
    let (status, response) = post_scan(&router, &request).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {response}");
    let properties = &response["sbom"]["components"][0]["properties"];
    assert_eq!(
        property(properties, "internal:team").as_deref(),
        Some("platform"),
        "case \"a submitted property\": the report preserves it, got {properties}"
    );
    assert_eq!(
        property(properties, &format!("{NAMESPACE}:info")).as_deref(),
        Some(SKIPPED),
        "case \"a submitted property\": the scan's own annotation is added beside it, got \
         {properties}"
    );
}

/// One row of the ref table: a document, and how many components its tree has.
#[derive(Debug)]
struct RefCase {
    what: &'static str,
    components: Vec<Value>,
    total: usize,
}

#[tokio::test]
async fn every_component_of_the_report_carries_the_scans_own_document_local_ref() {
    let cases = vec![
        RefCase {
            what: "one component whose submitted ref is its purl",
            components: vec![component(
                "pkg:npm/lodash@4.17.20",
                "lodash",
                "4.17.20",
                "pkg:npm/lodash@4.17.20",
            )],
            total: 1,
        },
        RefCase {
            what: "three components with submitted refs",
            components: vec![
                component("a", "lodash", "4.17.20", "pkg:npm/lodash@4.17.20"),
                component("b", "minimist", "1.2.5", "pkg:npm/minimist@1.2.5"),
                component("c", "left-pad", "1.0.0", "pkg:npm/left-pad@1.0.0"),
            ],
            total: 3,
        },
        RefCase {
            what: "two components carrying no ref at all",
            components: vec![
                json!({
                    "type": "library",
                    "name": "urllib3",
                    "version": "1.26.4",
                    "purl": "pkg:pypi/urllib3@1.26.4",
                }),
                json!({ "type": "library", "name": "left-pad", "version": "1.0.0" }),
            ],
            total: 2,
        },
        RefCase {
            what: "an assembly of two children, and a sibling",
            components: vec![
                json!({
                    "type": "application",
                    "bom-ref": "pkg:npm/app@1.0.0",
                    "name": "app",
                    "version": "1.0.0",
                    "components": [
                        component("x", "minimist", "1.2.5", "pkg:npm/minimist@1.2.5"),
                        component("y", "left-pad", "1.0.0", "pkg:npm/left-pad@1.0.0"),
                    ],
                }),
                component("z", "lodash", "4.17.20", "pkg:npm/lodash@4.17.20"),
            ],
            total: 4,
        },
    ];

    let (_engine, router) = emulator();
    for case in &cases {
        let request = json!({
            "sbom": bom(case.components.clone()),
            "outputFormat": "CYCLONE_DX_1_5",
        });
        let (status, response) = post_scan(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status}: {response}",
            case.what
        );
        let refs = refs_in_document_order(&response["sbom"]["components"]);
        let expected: Vec<String> = (1..=case.total).map(|n| format!("comp-{n}")).collect();
        assert_eq!(
            refs, expected,
            "case {:?}: expected the refs {expected:?} in document order, got {refs:?}",
            case.what
        );
        let unique: BTreeSet<&String> = refs.iter().collect();
        assert_eq!(
            unique.len(),
            refs.len(),
            "case {:?}: a ref names exactly one component, got {refs:?}",
            case.what
        );
        for entry in response["sbom"]["vulnerabilities"]
            .as_array()
            .unwrap_or(&Vec::new())
        {
            let affected = entry["affects"][0]["ref"].as_str().unwrap_or_default();
            assert!(
                refs.iter().any(|known| known == affected),
                "case {:?}: finding {} affects {affected:?}, which is no component of the \
                 report; its refs are {refs:?}",
                case.what,
                entry["id"]
            );
        }
    }
}

/// One row of the tools table: what the submitted document said about its tools.
#[derive(Debug)]
struct ToolsCase {
    what: &'static str,
    tools: Option<Value>,
    /// A tool the submitted document named, which the report must still carry.
    preserved: Option<&'static str>,
}

#[tokio::test]
async fn the_cyclonedx_report_names_the_scanning_service_in_metadata_tools() {
    let cases = vec![
        ToolsCase {
            what: "a document naming no tools",
            tools: None,
            preserved: None,
        },
        ToolsCase {
            what: "the 1.5 object form carrying tool components",
            tools: Some(json!({
                "components": [ { "type": "application", "name": "sbom-generator" } ]
            })),
            preserved: Some("sbom-generator"),
        },
        ToolsCase {
            what: "the 1.5 object form already naming a service",
            tools: Some(json!({ "services": [ { "name": "another-scanner" } ] })),
            preserved: Some("another-scanner"),
        },
        ToolsCase {
            // CycloneDX 1.5 deprecates the array form; its entries are carried
            // over rather than dropped.
            what: "the deprecated 1.4 array form",
            tools: Some(json!([ { "vendor": "Example", "name": "legacy-generator" } ])),
            preserved: Some("legacy-generator"),
        },
    ];

    let (_engine, router) = emulator();
    for case in &cases {
        let mut submitted = bom(vec![component(
            "ref-clean",
            "left-pad",
            "1.0.0",
            "pkg:npm/left-pad@1.0.0",
        )]);
        match &case.tools {
            Some(tools) => submitted["metadata"]["tools"] = tools.clone(),
            None => {
                submitted["metadata"] = json!({});
            }
        }
        let request = json!({ "sbom": submitted, "outputFormat": "CYCLONE_DX_1_5" });
        let (status, response) = post_scan(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status}: {response}",
            case.what
        );

        let tools = &response["sbom"]["metadata"]["tools"];
        let services = tools["services"].as_array().cloned().unwrap_or_default();
        let scanner = services
            .iter()
            .find(|service| service["name"].as_str() == Some(SERVICE_NAME));
        let scanner = scanner.unwrap_or_else(|| {
            panic!(
                "case {:?}: the report must name {SERVICE_NAME:?} in metadata.tools.services, \
                 got {tools}",
                case.what
            )
        });
        assert!(
            scanner["version"]
                .as_str()
                .is_some_and(|version| !version.is_empty()),
            "case {:?}: the service entry carries the scanning build's version, got {scanner}",
            case.what
        );
        if let Some(preserved) = case.preserved {
            let rendered = tools.to_string();
            assert!(
                rendered.contains(preserved),
                "case {:?}: the report must still name the submitted tool {preserved:?}, got \
                 {tools}",
                case.what
            );
        }
    }
}

/// A submitted `metadata.properties` entry is PRESERVED beside the scan summary.
#[tokio::test]
async fn the_scan_summary_is_appended_to_the_submitted_metadata_properties() {
    let (_engine, router) = emulator();
    let mut submitted = bom(vec![component(
        "ref-clean",
        "left-pad",
        "1.0.0",
        "pkg:npm/left-pad@1.0.0",
    )]);
    submitted["metadata"]["properties"] = json!([ { "name": "internal:build", "value": "42" } ]);
    let request = json!({ "sbom": submitted, "outputFormat": "CYCLONE_DX_1_5" });
    let (status, response) = post_scan(&router, &request).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {response}");
    let properties = &response["sbom"]["metadata"]["properties"];
    assert_eq!(
        property(properties, "internal:build").as_deref(),
        Some("42"),
        "case \"a submitted metadata property\": the report preserves it, got {properties}"
    );
    assert_eq!(
        property(properties, &format!("{NAMESPACE}:critical_vulnerabilities")).as_deref(),
        Some("0"),
        "case \"a submitted metadata property\": the summary is added beside it, got {properties}"
    );
}

#[tokio::test]
async fn a_request_naming_no_output_format_is_answered_the_inspector_report() {
    let (_engine, router) = emulator();
    let submitted = bom(vec![component(
        "ref-log4j",
        "log4j-core",
        "2.14.1",
        "pkg:maven/org.apache.logging.log4j/log4j-core@2.14.1",
    )]);

    let (status, defaulted) = post_scan(&router, &json!({ "sbom": submitted })).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {defaulted}");
    assert!(
        defaulted["sbom"]["vulnerability_count"].is_object(),
        "case \"no outputFormat\": the Inspector report carries the summary, got {}",
        defaulted["sbom"]
    );
    assert_eq!(
        defaulted["sbom"]["vulnerabilities"][0]["id"].as_str(),
        Some("CVE-2021-44228"),
        "case \"no outputFormat\": the matched advisory is reported, got {}",
        defaulted["sbom"]
    );
    assert_eq!(
        defaulted["sbom"]["vulnerabilities"][0]["severity"].as_str(),
        Some("critical"),
        "case \"no outputFormat\": the match is reported at the severity it is rated, got {}",
        defaulted["sbom"]["vulnerabilities"][0]
    );

    let (status, explicit) = post_scan(
        &router,
        &json!({ "sbom": submitted, "outputFormat": "INSPECTOR" }),
    )
    .await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {explicit}");
    assert_eq!(
        defaulted["sbom"]["vulnerabilities"], explicit["sbom"]["vulnerabilities"],
        "case \"no outputFormat\": it answers the same report INSPECTOR does, got {} then {}",
        defaulted["sbom"], explicit["sbom"]
    );

    // And the CycloneDX form is the submitted document augmented, not this one.
    let (status, cyclonedx) = post_scan(
        &router,
        &json!({ "sbom": submitted, "outputFormat": "CYCLONE_DX_1_5" }),
    )
    .await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {cyclonedx}");
    assert_eq!(
        cyclonedx["sbom"]["bomFormat"].as_str(),
        Some("CycloneDX"),
        "case \"CYCLONE_DX_1_5\": expected the submitted document augmented, got {}",
        cyclonedx["sbom"]
    );
    assert!(
        cyclonedx["sbom"]["vulnerability_count"].is_null(),
        "case \"CYCLONE_DX_1_5\": the CycloneDX form reports the summary as metadata \
         properties, got {}",
        cyclonedx["sbom"]
    );
}
