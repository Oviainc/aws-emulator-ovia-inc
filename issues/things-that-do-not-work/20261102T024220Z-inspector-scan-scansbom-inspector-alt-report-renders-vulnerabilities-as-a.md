Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: inspector-scan.scan-sbom-output-format-inspector-alt-shape
Issue-Shape-Id: com.amazonaws.inspectorscan#InspectorScan
Issue-Key: inspector-scan.scan-sbom-output-format-inspector-alt-shape|run#11|AttributeError|missing_on_alt = [v.get("id") for v in alt_vulns if "vendorAdvisories" not in v]
Issue-Commit: 96a8fc837c53388088f2db30720a835b9b6b1473

# ScanSbom INSPECTOR_ALT report renders `vulnerabilities` as a CVE-keyed map instead of a list of vulnerability nodes, and no node carries `vendorAdvisories`

> _Note: this issue was identified against the code base at commit `96a8fc837c53388088f2db30720a835b9b6b1473`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ScanSbom

## Description
One cause, on inspector-scan ScanSbom (the service's only operation), grounded in the single failing behavioral check listed under it.

Cause 1 of 1: with outputFormat=INSPECTOR_ALT the report's top-level `vulnerabilities` member is a JSON OBJECT keyed by CVE id whose values are arrays of finding objects, where AWS returns a JSON ARRAY of vulnerability nodes; and none of the emulator's INSPECTOR_ALT nodes carries the `vendorAdvisories` member that distinguishes the INSPECTOR_ALT form from the plain INSPECTOR form.

AWS: Smithy (services/service-com-amazonaws-inspectorscan-inspectorscan/smithy-model/com-amazonaws-inspectorscan-inspectorscan.json) models ScanSbom as POST /scan/sbom, input {sbom: Sbom (document, required), outputFormat: OutputFormat}, output {sbom: Sbom (document)}; OutputFormat enum values CYCLONE_DX_1_5, INSPECTOR, INSPECTOR_ALT. The `sbom` output is an opaque Smithy `document`, so the model says nothing about the report interior — the report shape is recorded behavior only. Recorded behavior: outputFormat=INSPECTOR_ALT returns an alternate report that is distinguishable from both CYCLONE_DX_1_5 and INSPECTOR. Its top-level `vulnerabilities` is a LIST, one node per matched advisory, each node an object carrying at least the advisory `id`, its `severity`, and a `vendorAdvisories` member that no INSPECTOR node carries. Scanning an SBOM whose only component is log4j-core 2.12.1 (purl pkg:maven/org.apache.logging.log4j/log4j-core@2.12.1) therefore yields an INSPECTOR_ALT node for CVE-2021-44228 with severity critical and a `vendorAdvisories` member on it.

Emulator: `vulnerabilities` for INSPECTOR_ALT is a map keyed by the advisory id — {"CVE-2021-44228": [ {<finding object>} ], ...} — so a client iterating the member walks the CVE-id STRINGS, not vulnerability nodes. The replay failed with `AttributeError: 'str' object has no attribute 'get'` on `[v.get("id") for v in alt_vulns if "vendorAdvisories" not in v]`: the member is present and non-empty (so it is not the previously reported "no vulnerabilities list at all" state) but its element type is wrong. No node in either the INSPECTOR_ALT or the INSPECTOR form carries `vendorAdvisories`, so the two forms are also still indistinguishable by the field INSPECTOR_ALT exists to carry.

Why: a client that selects INSPECTOR_ALT gets a document it cannot parse with the code it uses against AWS. Indexing `vulnerabilities[0]` yields a string (or raises, as the check did) instead of a finding object; a severity/CVE scan of the list finds nothing; and the vendor-advisory data the caller chose INSPECTOR_ALT in order to read is absent entirely, so the format selection is a no-op versus INSPECTOR. A dependency gate reading INSPECTOR_ALT either crashes or reports a Log4Shell-bearing SBOM as carrying no parseable findings.

Fix: the INSPECTOR_ALT branch of the Inspector-native report serializer in the ScanSbom handler — the `keyed_by_id` path of `inspector_report`, which builds `Value::Object(by_id)` for the `vulnerabilities` member, plus the per-node renderer `inspector_finding` it feeds, which must emit `vendorAdvisories` for the INSPECTOR_ALT form only. INSPECTOR_ALT must render `vulnerabilities` as a list of vulnerability nodes (each with `id` and `severity`) and add `vendorAdvisories` per node, while the INSPECTOR form keeps its node rendering with no `vendorAdvisories`.

Relation to already-open issues: 20261029T202150Z-inspector-scan-scansbom-authorization-never-enforced-wrong-sbom-validation-messages.md (Cause 7) carries this check id, but against the earlier emulator state where INSPECTOR_ALT had NO `vulnerabilities` member at all; the member now exists and the check fails at a later assertion on its element type, which that issue's replay key no longer matches, so the surviving divergence is recorded here. 20261029T171842Z-inspector-scan-scansbom-report-body-omits-inspector-scan-results-and.md (fixed) concerned the report body from the wire corpus and does not cover the INSPECTOR_ALT list-vs-map shape or `vendorAdvisories`.

Traces: inspector-scan.scan-sbom-output-format-inspector-alt-shape.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: inspector-scan 12/21 passed; this check is the only failure not already owned by an open issue.

[error] inspector-scan.scan-sbom-output-format-inspector-alt-shape
  File "check_inspector_scan_scan_sbom_output_format_inspector_alt_shape.py", line 39, in run
    missing_on_alt = [v.get("id") for v in alt_vulns if "vendorAdvisories" not in v]
  AttributeError: 'str' object has no attribute 'get'

The check scans an SBOM with one component (log4j-core 2.12.1, purl pkg:maven/org.apache.logging.log4j/log4j-core@2.12.1) three times — outputFormat=CYCLONE_DX_1_5, INSPECTOR and INSPECTOR_ALT — asserts INSPECTOR_ALT is distinguishable from the raw CycloneDX document, then asserts that every INSPECTOR_ALT vulnerability node carries `vendorAdvisories` and no INSPECTOR node does.

Expected (recorded AWS): `alt["vulnerabilities"]` is a list of objects; `[v for v in alt["vulnerabilities"]]` yields dicts each having `id`, `severity` and `vendorAdvisories`; a node mentioning CVE-2021-44228 has severity critical.
Actual (emulator): `alt["vulnerabilities"]` is an object keyed by advisory id, so iterating it yields the str key 'CVE-2021-44228' and the dict-member test raises AttributeError. Confirmed in the handler: `inspector_report(..., keyed_by_id: bool)` sets the `vulnerabilities` member to `Value::Object(by_id)` when `keyed_by_id` is true (the INSPECTOR_ALT branch) and `Value::Array(rendered)` otherwise, and `inspector_finding` emits no `vendorAdvisories` member in either form (services/service-com-amazonaws-inspectorscan-inspectorscan/src/ops/scan_sbom.rs, `inspector_finding` and `inspector_report`). The handler's own doc comment states the list-vs-map difference is "this emulator's choice" and "a recorded response should correct them" — the recording does.

Per-run values in the above (scanId, scanTimestamp, serialNumber) fill the report template and are not requirements; the requirement is that `vulnerabilities` is a list of vulnerability nodes and that each INSPECTOR_ALT node carries `vendorAdvisories`.

## Steps to Reproduce
1. Replay the behavioral suite (simparity-behavioral-aws-v1) and re-run inspector-scan.scan-sbom-output-format-inspector-alt-shape.
2. Or directly: build a CycloneDX 1.5 SBOM {bomFormat: 'CycloneDX', specVersion: '1.5', serialNumber: 'urn:uuid:<any>', version: 1, components: [{bom-ref: 'comp-log4j', type: 'library', name: 'log4j-core', version: '2.12.1', purl: 'pkg:maven/org.apache.logging.log4j/log4j-core@2.12.1'}]}.
3. Call ScanSbom with outputFormat='INSPECTOR_ALT' and inspect response['sbom']['vulnerabilities']: expect a JSON array of objects; observe a JSON object keyed by 'CVE-2021-44228'.
4. For each element of that member, check for a 'vendorAdvisories' key: expect it present on every INSPECTOR_ALT node; observe the elements are not objects at all, and that no node anywhere in the document carries 'vendorAdvisories'.
5. Call ScanSbom with outputFormat='INSPECTOR' on the same SBOM and confirm no node carries 'vendorAdvisories' (that half must stay true after the fix), and that the two forms are then distinguishable by that member.
