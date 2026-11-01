// SPDX-License-Identifier: CC0-1.0

//! The ordering/idempotence invariants of a scan, over every component multiset
//! this crate's catalog can build up to a bounded size.
//!
//! Split out of `scan_sbom.rs` only for length; it shares that suite's helpers
//! through the `super` path the test crate root gives it.

use super::scan_sbom::{bom, component, emulator, post_scan, CATALOG};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Every component multiset of size 1 and 2 drawn from [`CATALOG`], in index
/// form. Exhaustive rather than sampled: the catalog is small, so the whole
/// space up to pairs is cheaper than a generator and names its own cases.
fn component_multisets() -> Vec<Vec<usize>> {
    let mut cases: Vec<Vec<usize>> = (0..CATALOG.len()).map(|a| vec![a]).collect();
    for a in 0..CATALOG.len() {
        for b in a..CATALOG.len() {
            cases.push(vec![a, b]);
        }
    }
    // One longer multiset per catalog entry, so a report with several findings
    // and several clean components is covered too.
    for a in 0..CATALOG.len() {
        cases.push(vec![a, (a + 1) % CATALOG.len(), (a + 2) % CATALOG.len(), a]);
    }
    cases
}

/// For EVERY one of those documents:
///
/// * `vulnerabilities[]` is ordered by affected component ref, then by
///   vulnerability id — a report's order is a function of its content;
/// * every entry's `affects[].ref` resolves to a component of the report;
/// * scanning the same document twice answers the identical report, because
///   `ScanSbom` is `@idempotent`.
#[tokio::test]
async fn the_report_is_ordered_and_a_rescan_is_identical() {
    for picks in component_multisets() {
        let (_engine, router) = emulator();
        let components: Vec<Value> = picks
            .iter()
            .enumerate()
            .map(|(position, &pick)| {
                let (name, version, purl) = CATALOG[pick];
                component(&format!("ref-{position}"), name, version, purl)
            })
            .collect();
        let request = json!({ "sbom": bom(components), "outputFormat": "CYCLONE_DX_1_5" });

        let (status, first) = post_scan(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {picks:?}: expected HTTP 200, got {status}: {first}"
        );

        let entries = first["sbom"]["vulnerabilities"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let keys: Vec<(String, String)> = entries
            .iter()
            .map(|v| {
                (
                    v["affects"][0]["ref"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    v["id"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(
            keys, sorted,
            "case {picks:?}: vulnerabilities must be ordered by (component ref, id), got {keys:?}"
        );

        let refs: BTreeSet<String> = first["sbom"]["components"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|c| c["bom-ref"].as_str().unwrap_or_default().to_string())
            .collect();
        for (component_ref, id) in &keys {
            assert!(
                refs.contains(component_ref),
                "case {picks:?}: finding {id} affects {component_ref:?}, \
                 which is not a component of the report; refs are {refs:?}"
            );
        }

        let (status, second) = post_scan(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {picks:?}: expected HTTP 200 on rescan, got {status}: {second}"
        );
        assert_eq!(
            first, second,
            "case {picks:?}: an @idempotent rescan must answer the identical report"
        );
    }
}
