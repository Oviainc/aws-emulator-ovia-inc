Issue-Source: trace-sweep-divergence
Issue-Trace: inspector-scan-scan-basic-sbom-default-format
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: inspector-scan-scan-basic-sbom-default-format inspector-scan-scan-sbom-all-output-formats inspector-scan-scan-sbom-multi-ecosystem-packages inspector-scan-scan-sbom-empty-components inspector-scan-scan-sbom-inspector-format-log4shell-critical
Issue-Shape-Id: com.amazonaws.inspectorscan#InspectorScan
Issue-Key: inspector-scan|ScanSbom|2xx->2xx|$.sbom.messages
Issue-Key: inspector-scan|ScanSbom|2xx->2xx|$.sbom.vulnerability_count
Issue-Key: inspector-scan|ScanSbom|2xx->2xx|$.sbom.components[*].bom-ref
Issue-Key: inspector-scan|ScanSbom|2xx->2xx|$.sbom.components[*].properties
Issue-Key: inspector-scan|ScanSbom|2xx->2xx|$.sbom.metadata.properties
Issue-Key: inspector-scan|ScanSbom|2xx->2xx|$.sbom.metadata.tools.services
Issue-Key: inspector-scan|ScanSbom|2xx->2xx|$.sbom.vulnerabilities
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# ScanSbom report body omits Inspector scan results and rewrites document-local refs

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ScanSbom

## Description
Four causes. Every one is a 200 response whose `sbom` report document is missing the scan results Amazon Inspector adds to it, so a client gets a syntactically valid but informationally empty report.

Cause 1 of 4: the INSPECTOR-format report omits `sbom.vulnerability_count` and `sbom.messages`.
AWS: ScanSbom returns output member `sbom` (Smithy: ScanSbomResponse.sbom -> Sbom, a document; input outputFormat -> OutputFormat {CYCLONE_DX_1_5, INSPECTOR, INSPECTOR_ALT}); the model does not describe the document's interior, so recorded behavior only: the Inspector-format report carries a `vulnerability_count` object keyed by severity (critical/high/medium/low/other), each value the number of findings of that severity in the scan, and a `messages` array with one entry per component the scanner could not rule on, each entry an object of `purl` (the component's purl, taken from the submitted document) and `info_message` (the fixed sentence 'Component skipped: no supported rules found.').
Emulator: both members are absent (`$.sbom.vulnerability_count: missing (got None)`, `$.sbom.messages: missing (got None)`).
Why: a client reading the per-severity counts to gate a build, or reading `messages` to see which components were skipped, reads nothing at all and concludes the SBOM is clean and fully covered.
Fix: the INSPECTOR/INSPECTOR_ALT report serializer in the ScanSbom handler — it must emit the severity-count object (zeros when there are no findings) and the skipped-component message list.
Traces: inspector-scan-scan-basic-sbom-default-format#task#1 (representative), and the same cluster's inspector-scan-scan-sbom-all-output-formats#task#2, #task#3 and inspector-scan-scan-sbom-multi-ecosystem-packages#task#1.

Cause 2 of 4: the CYCLONE_DX_1_5 report omits the Inspector scanner annotations and replaces the document's own component refs.
AWS: recorded behavior only — the CycloneDX report carries (a) `metadata.tools.services` with the scanning service entry whose `name` is the fixed text 'Amazon Inspector Scan SBOM API' and whose `version` is the service's build version string, (b) `metadata.properties` with the namespaced scan summary 'amazon:inspector:sbom_scanner:critical_vulnerabilities', '…:high_vulnerabilities', '…:medium_vulnerabilities', '…:low_vulnerabilities', '…:other_vulnerabilities' and '…:malicious_packages', each value the decimal count for the scan, (c) per-component `properties` carrying 'amazon:inspector:sbom_scanner:info' with the fixed value 'Component skipped: no supported rules found.' for a component no rule applied to, and (d) a document-local `bom-ref` on each component of the form comp-<n> (a sequential document-local identifier) rather than the component's purl.
Emulator: `metadata.tools.services` and `metadata.properties` are absent, component `properties` is absent, and `components[0].bom-ref` is rewritten to the component's purl (expected 'comp-1', got 'pkg:npm/sim2real-fixture-gamma@0.0.1'). The handler instead adds a `metadata.tools.components` scanner entry and synthesizes purl-shaped refs.
Why: CycloneDX consumers that key findings by `bom-ref`, or that read the Inspector property namespace for the scan summary, get a document whose refs do not match the ones AWS issues and which carries none of the scan's summary annotations.
Fix: the CYCLONE_DX_1_5 report builder / bom-ref synthesis in the ScanSbom handler.
Traces: inspector-scan-scan-sbom-all-output-formats#task#1.

Cause 3 of 4: `vulnerability_count` is omitted even for a document with no components, where the recording still reports an all-zero count object.
AWS: recorded behavior only — a scan of an SBOM with an empty `components` array still answers 200 with `vulnerability_count` present and every severity bucket 0.
Emulator: `$.sbom.vulnerability_count: missing (got None)`.
Why: a client cannot distinguish "scanned, nothing found" from "no counts reported"; the zero-finding case is exactly the one a gate treats as a pass, so the member must exist.
Fix: same report serializer as Cause 1 — the count object is unconditional, not emitted only when there are findings.
Traces: inspector-scan-scan-sbom-empty-components#task#1.

Cause 4 of 4: a vulnerable component produces no `vulnerabilities` entry at all in the INSPECTOR-format report.
AWS: recorded behavior only — scanning an SBOM containing a component that a known advisory applies to answers 200 with `sbom.vulnerabilities` holding one entry per matched advisory, carrying the advisory `id` (the CVE identifier of the matched advisory) and its `severity`; the recorded step shows the Log4Shell advisory matched at critical severity.
Emulator: `$.sbom.vulnerabilities: missing (got None)` — the report contains no vulnerabilities member, so a known-vulnerable SBOM is reported as carrying nothing.
Why: this is the entire point of the API: a client scanning a vulnerable SBOM is told it is clean, so a vulnerability gate passes a build it must fail.
Fix: the ScanSbom handler's advisory matching and the INSPECTOR-format renderer — the matched advisory set must be rendered into the report document for that output format.
Traces: inspector-scan-scan-sbom-inspector-format-log4shell-critical#task#1.

Shared location: services/service-com-amazonaws-inspectorscan-inspectorscan/src/ops/scan_sbom.rs (report assembly ~L1189-1270, the metadata.tools branch ~L1246-1269, per-finding rendering ~L1150-1187). No open issue names this service: list_existing_issues reports none matching 'inspector-scan'.

## Evidence
Four replay clusters, all service=inspector-scan, operation=ScanSbom, expected_status 200, actual_status 200:
1) keys=$.sbom.messages,$.sbom.vulnerability_count (4 affected) — $.sbom.messages: missing (expected [{'purl': 'pkg:npm/sim2real-fixture-alpha@0.0.1', 'info_message': 'Component skipped: no supported rules found.'}, {'purl': 'pkg:pypi/sim2real-fixture-beta@0.0.1', 'info_message': 'Component skipped: no supported rules found.'}], got None); $.sbom.vulnerability_count: missing (expected {'critical': 0, 'high': 0, 'medium': 0, 'low': 0, 'other': 0}, got None).
2) keys=$.sbom.components[0].bom-ref,$.sbom.components[0].properties,$.sbom.metadata.properties,$.sbom.metadata.tools.services — $.sbom.metadata.tools.services: missing (expected [{'name': 'Amazon Inspector Scan SBOM API', 'version': '1a2b3c4d+73effe98+658310b0'}], got None); $.sbom.metadata.properties: missing (expected the six 'amazon:inspector:sbom_scanner:*_vulnerabilities'/'malicious_packages' entries, got None); $.sbom.components[0].bom-ref: value_mismatch (expected 'comp-1', got 'pkg:npm/sim2real-fixture-gamma@0.0.1'); $.sbom.components[0].properties: missing (expected [{'name': 'amazon:inspector:sbom_scanner:info', 'value': 'Component skipped: no supported rules found.'}], got None).
3) keys=$.sbom.vulnerability_count — missing (expected {'critical': 0, 'high': 0, 'medium': 0, 'low': 0, 'other': 0}, got None).
4) keys=$.sbom.vulnerabilities — missing (expected [{'id': 'CVE-2021-44228', 'severity': 'critical'}], got None).
(The recorded `version` string and the fixture purls are per-recording values filling the templates above, not requirements.)

## Steps to Reproduce
Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator, then compare these ScanSbom steps:
- inspector-scan-scan-basic-sbom-default-format#task#1: POST /scan/sbom with a small multi-ecosystem SBOM and the default output format; the 200 body's sbom document has no vulnerability_count and no messages.
- inspector-scan-scan-sbom-all-output-formats#task#1: same document with outputFormat=CYCLONE_DX_1_5; metadata.tools.services, metadata.properties and components[].properties are absent and components[0].bom-ref is the purl instead of comp-1.
- inspector-scan-scan-sbom-empty-components#task#1: SBOM with an empty components array; vulnerability_count is absent instead of an all-zero object.
- inspector-scan-scan-sbom-inspector-format-log4shell-critical#task#1: SBOM containing the vulnerable log4j component with the Inspector output format; sbom.vulnerabilities is absent instead of reporting the matched critical advisory.
