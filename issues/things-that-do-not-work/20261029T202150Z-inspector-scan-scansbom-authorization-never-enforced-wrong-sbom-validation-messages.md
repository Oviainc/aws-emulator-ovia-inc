Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: inspector-scan.scan-sbom-access-denied-without-permission-rejected inspector-scan.scan-sbom-malformed-document-rejected inspector-scan.scan-sbom-missing-required-fields-rejected inspector-scan.scan-sbom-wrong-bom-format-rejected inspector-scan.scan-sbom-component-limit-exceeded-rejected inspector-scan.scan-sbom-invalid-output-format-rejected inspector-scan.scan-sbom-default-output-format-matches-explicit inspector-scan.scan-sbom-output-format-inspector-json-shape inspector-scan.scan-sbom-output-format-inspector-alt-shape inspector-scan.scan-sbom-epss-and-cvss-ratings-present inspector-scan.scan-sbom-log4j-critical-vulnerability-detected inspector-scan.scan-sbom-multiple-vulnerable-components-all-detected
Issue-Shape-Id: com.amazonaws.inspectorscan#InspectorScan
Issue-Key: inspector-scan.scan-sbom-access-denied-without-permission-rejected|run#16|AssertionError|assert code == "AccessDeniedException", (code, response)
Issue-Key: inspector-scan.scan-sbom-malformed-document-rejected|run#5|AssertionError|assert m == "Input SBOM failed validation: Invalid json", f"expected the invalid-json message, got {m!r}"
Issue-Key: inspector-scan.scan-sbom-missing-required-fields-rejected|run#5|AssertionError|assert m == 'Input SBOM failed validation: Invalid value for "bomFormat"', m
Issue-Key: inspector-scan.scan-sbom-wrong-bom-format-rejected|run#6|AssertionError|assert m == 'Input SBOM failed validation: Invalid value for "bomFormat"', (
Issue-Key: inspector-scan.scan-sbom-component-limit-exceeded-rejected|run#7|AssertionError|assert m == "Input SBOM failed validation: Max number of components exceeded (5000)", m
Issue-Key: inspector-scan.scan-sbom-invalid-output-format-rejected|run#6|AssertionError|assert m == (
Issue-Key: inspector-scan.scan-sbom-default-output-format-matches-explicit|run#5|AssertionError|assert set(default_report.keys()) == set(inspector_report.keys()), (default_report, inspector_report)
Issue-Key: inspector-scan.scan-sbom-output-format-inspector-json-shape|run#8|AssertionError|assert vuln is not None, f"CVE-2021-44228 not found among {[v.get('id') for v in vulns]}"
Issue-Key: inspector-scan.scan-sbom-output-format-inspector-alt-shape|run#9|AssertionError|assert alt_vulns, f"expected INSPECTOR_ALT to report vulnerabilities, got {alt}"
Issue-Key: inspector-scan.scan-sbom-epss-and-cvss-ratings-present|run#10|AssertionError|assert "EPSS" in sources, f"expected an EPSS rating source, got {sources}"
Issue-Key: inspector-scan.scan-sbom-log4j-critical-vulnerability-detected|run#23|AssertionError|assert critical_count >= 1, "aggregate critical count did not reflect the known critical finding"
Issue-Key: inspector-scan.scan-sbom-multiple-vulnerable-components-all-detected|run#17|AssertionError|assert commons_ref in commons_refs, (
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# ScanSbom: authorization never enforced, wrong SBOM-validation messages and limits, wrong default output format, and INSPECTOR/CycloneDX reports missing scan enrichment

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ScanSbom

## Description
Ten causes, all on inspector-scan ScanSbom (the service's only operation), grounded in the 12 failing behavioral checks listed under each cause. Model facts cited from services/service-com-amazonaws-inspectorscan-inspectorscan/smithy-model/com-amazonaws-inspectorscan-inspectorscan.json: ScanSbom POST /scan/sbom, input {sbom: Sbom (document, required), outputFormat: OutputFormat}, output {sbom: Sbom (document)}, errors AccessDeniedException(403), InternalServerException(500), ThrottlingException(429), ValidationException(400); OutputFormat enum values CYCLONE_DX_1_5, INSPECTOR, INSPECTOR_ALT; ValidationException members message(required), reason(required, ValidationExceptionReason), fields. The `sbom` document interior is NOT modeled (it is an opaque document), so everything about the report body below is recorded behavior only.

Cause 1 of 10: ScanSbom authorizes every signed caller — a principal with no identity-based policy is served a full vulnerability report instead of AccessDeniedException.
AWS: Smithy models AccessDeniedException @httpError 403 on ScanSbom ('You do not have sufficient access to perform this action'). A caller whose IAM identity has no attached policy at all (implicit deny) is refused with error code AccessDeniedException and the standard implicit-deny message, whose fixed text is 'User: <principal-arn> is not authorized to perform: inspector-scan:ScanSbom on resource: <resource-arn> because no identity-based policy allows the inspector-scan:ScanSbom action' — the variable parts are the caller's principal ARN (from the signing credentials) and the resource ARN arn:aws:inspector-scan:<region>:<account>:/scan/sbom built from the request's region and account. No report member is returned on the refusal.
Emulator: the call succeeds. HTTP 200 with a complete `sbom` report document; no error code at all (the check observed code None and a 200 body carrying the CycloneDX report). The handler's own module doc states 'Authorization is NOT this handler's job ... AccessDeniedException ... is never raised here'.
Why: a client (or a test of least-privilege posture) that relies on ScanSbom refusing an unauthorized principal sees the call succeed and the scan result leak to a principal that is not allowed to scan; any IAM-regression test built on this operation silently passes.
Fix: the ScanSbom handler's authorization gate (or the central request-authorization layer the handler delegates to) must evaluate the presented principal's permissions for inspector-scan:ScanSbom and return the modeled AccessDeniedException with the implicit-deny message, emitting no `sbom` member.
Traces: inspector-scan.scan-sbom-access-denied-without-permission-rejected.

Cause 2 of 10: every SBOM-document refusal carries the emulator's own member-path prose instead of AWS's 'Input SBOM failed validation: <condition>' sentence.
AWS: ScanSbom refuses a bad SBOM with 400 ValidationException whose message is the fixed text 'Input SBOM failed validation: ' followed by the condition that failed; the trailing clause is decided by the CLASS of input defect, not copied from the request. Recorded classes: a `sbom` value that is not a parseable CycloneDX object -> 'Input SBOM failed validation: Invalid json'; a document missing the required CycloneDX fields -> 'Input SBOM failed validation: Invalid value for "bomFormat"'; a document whose bomFormat is a non-CycloneDX value (e.g. SPDX) -> 'Input SBOM failed validation: Invalid value for "bomFormat"'. No report member is present on the refusal.
Emulator: status and error code are right (400 ValidationException) but the message is emulator-internal prose naming its own member path: 'sbom must be a CycloneDX 1.5 JSON object' (bare-string document), 'sbom.bomFormat is required and must be "CycloneDX"; only CycloneDX 1.5 documents can be scanned' (empty document), 'sbom.bomFormat must be "CycloneDX", got "SPDX"; only CycloneDX 1.5 documents can be scanned' (SPDX document).
Why: clients and gating scripts that match on the refusal text, and users who are shown it, get a sentence AWS never sends; the emulator's wording also implies a member-level field error where AWS reports a whole-document validation failure.
Fix: the shared ValidationException constructor used by the ScanSbom input validation path — it must emit the 'Input SBOM failed validation: <condition>' wording per defect class. Related open issue 20261029T171842Z-inspector-scan-scansbom-validationexception-wire-shape-and-message-diverge-from.md reports the same wording divergence (plus the extra `reason`/`fields` members) from the wire corpus; it does not carry these behavioral check ids, so they are recorded here.
Traces: inspector-scan.scan-sbom-malformed-document-rejected, inspector-scan.scan-sbom-missing-required-fields-rejected, inspector-scan.scan-sbom-wrong-bom-format-rejected.

Cause 3 of 10: the component ceiling is enforced at 2000 instead of the 5000 the live service enforces, and its refusal message is wrong.
AWS: the live service accepts exactly 5000 components and refuses 5001 with 400 ValidationException, message 'Input SBOM failed validation: Max number of components exceeded (5000)' — fixed text with the limit as its only variable part. (The Smithy doc on ScanSbomRequest.sbom still says 'This format limits you to passing 2000 components', which is stale relative to the live service; recorded behavior is 5000.)
Emulator: a 2001-component document is already refused, and the message is 'sbom.components carries more than the 2000 components a CycloneDX 1.5 scan accepts'. The limit is the constant MAX_COMPONENTS = 2000 enforced during the component walk.
Why: legitimate SBOMs of 2001-5000 components — common for real application images — are rejected outright against a service that accepts them, and a client matching the documented limit message sees different text and a different number.
Fix: the component-count limit constant and its refusal in the ScanSbom input validation (raise the ceiling to 5000 and emit the AWS message).
Traces: inspector-scan.scan-sbom-component-limit-exceeded-rejected.

Cause 4 of 10: the outputFormat enum-constraint message lists the enum values in a different order than AWS does.
AWS: an outputFormat outside the modeled enum is refused with 400 ValidationException whose message is the standard constraint sentence '1 validation error detected: Value '<rejected value>' at 'outputFormat' failed to satisfy constraint: Member must satisfy enum value set: [INSPECTOR_ALT, CYCLONE_DX_1_5, INSPECTOR]' — fixed text plus the rejected value taken from the request; the bracketed value set is fixed text and AWS emits it in the order INSPECTOR_ALT, CYCLONE_DX_1_5, INSPECTOR (not the model's declaration order).
Emulator: same status, code and sentence, but the value set is rendered in model-declaration order: '... Member must satisfy enum value set: [CYCLONE_DX_1_5, INSPECTOR, INSPECTOR_ALT]'.
Why: a client asserting on the full constraint message (the only machine-readable statement of which values are allowed, since ValidationException carries no enumeration member here) sees text that differs from AWS.
Fix: the enum-constraint message rendering for OutputFormat in the request-validation layer for this operation.
Traces: inspector-scan.scan-sbom-invalid-output-format-rejected.

Cause 5 of 10: omitting outputFormat answers in CycloneDX form instead of the Inspector proprietary form.
AWS: when outputFormat is absent, ScanSbom returns the Inspector finding-report shape — structurally the same document an explicit outputFormat=INSPECTOR call returns for the same input (recorded behavior only; the default-format recording carries the Inspector-shape members vulnerability_count and messages, not a CycloneDX BOM).
Emulator: an absent outputFormat is mapped to CYCLONE_DX_1_5 and the response is the augmented input BOM ('bomFormat': 'CycloneDX', 'specVersion', 'components', 'metadata', 'serialNumber', 'vulnerabilities'), whose top-level key set differs from the explicit-INSPECTOR response, and which has no vulnerability_count.
Why: every client that calls ScanSbom without specifying a format — the default SDK call — parses a different document shape than AWS returns, so field lookups for the Inspector report members fail.
Fix: output-format defaulting in the ScanSbom handler (the None branch that selects CycloneDx15) must select the Inspector report form.
Traces: inspector-scan.scan-sbom-default-output-format-matches-explicit.

Cause 6 of 10: the INSPECTOR report exposes findings as a map keyed by CVE instead of a top-level `vulnerabilities` list of finding objects.
AWS: recorded behavior only — the Inspector-format report carries a top-level `vulnerabilities` array with one entry per matched advisory, each entry carrying the advisory `id` (the CVE identifier) and its `severity`.
Emulator: the Inspector report has no `vulnerabilities` member at all; findings live under a `findings` object keyed by CVE id ({'findings': {'CVE-2021-44228': [ ... ]}}) alongside scanTimestamp/scanId/componentCount/findingCount/sbomFormat/sbomSpecVersion, so a lookup of `vulnerabilities` returns empty and a known-vulnerable SBOM reads as clean.
Why: a vulnerability gate reading the documented report member is told a Log4Shell-bearing SBOM has no findings and passes the build.
Fix: the INSPECTOR report serializer in the ScanSbom handler (inspector_report) must render the matched advisory set as the top-level `vulnerabilities` list with id and severity. Related open issue 20261029T171842Z... (report-body issue 20261029T171842Z-inspector-scan-scansbom-report-body-omits-inspector-scan-results-and.md) reports the missing vulnerabilities/vulnerability_count members from the wire corpus; it does not carry these behavioral check ids.
Traces: inspector-scan.scan-sbom-output-format-inspector-json-shape.

Cause 7 of 10: INSPECTOR_ALT is byte-equivalent in shape to INSPECTOR — it is not a distinct form and carries no vendorAdvisories.
AWS: recorded behavior only — outputFormat=INSPECTOR_ALT returns an alternate report that is distinguishable from both CYCLONE_DX_1_5 and INSPECTOR: like INSPECTOR it carries a top-level `vulnerabilities` list, and every vulnerability node in it additionally carries a `vendorAdvisories` member, which no INSPECTOR node carries.
Emulator: INSPECTOR_ALT is produced by the same renderer as INSPECTOR with a single boolean flag, so it too has no `vulnerabilities` list (findings under the `findings` map) and no `vendorAdvisories` member anywhere.
Why: a client that selects INSPECTOR_ALT specifically to read vendor advisory data gets a document indistinguishable from INSPECTOR and missing the field it asked for.
Fix: the INSPECTOR_ALT branch of the report serializer in the ScanSbom handler — it needs its own node rendering that emits vendorAdvisories per vulnerability, on top of the `vulnerabilities` list from Cause 6.
Traces: inspector-scan.scan-sbom-output-format-inspector-alt-shape.

Cause 8 of 10: CycloneDX vulnerability ratings carry no EPSS rating, only CVSS ones.
AWS: recorded behavior only — a CycloneDX 1.5 finding is enriched with ratings from distinct scoring vendors, including an EPSS rating whose `source.name` is 'EPSS' and whose `score` is an exploit probability in [0, 1], alongside the CVSS ratings (NVD and Amazon Inspector) on the 0-10 scale.
Emulator: ratings contain only the two CVSS entries; observed rating sources were {'NVD', 'Amazon Inspector'} with no 'EPSS' entry. The advisory record type (state.rs VulnRecord) has no EPSS field at all, so the data is absent, not just unrendered.
Why: a client that prioritises remediation by exploit probability finds no EPSS score and cannot distinguish 'critical but not exploited' from 'actively exploited'.
Fix: the advisory record and its seed data in the service state (add the EPSS probability per advisory) plus the CycloneDX ratings builder in the ScanSbom handler that emits the per-vendor rating entries.
Traces: inspector-scan.scan-sbom-epss-and-cvss-ratings-present.

Cause 9 of 10: the CycloneDX report's metadata carries no Inspector scan-summary properties, so the aggregate severity counters are absent.
AWS: recorded behavior only — the CycloneDX report's `metadata.properties` carries the namespaced scan summary 'amazon:inspector:sbom_scanner:critical_vulnerabilities', ':high_vulnerabilities', ':medium_vulnerabilities', ':low_vulnerabilities', ':other_vulnerabilities' and ':malicious_packages', each value the decimal count for that scan (so a scan matching one critical advisory reports critical_vulnerabilities >= 1).
Emulator: the handler writes only `metadata.timestamp` and a `metadata.tools.components` scanner entry; `metadata.properties` is never inserted, so every counter reads as absent (the check's parsed critical count was 0 while the same report listed CVE-2021-44228 as critical).
Why: a client that gates on the aggregate critical counter rather than walking the vulnerability list sees zero criticals for an SBOM the same response reports a critical finding on.
Fix: the CYCLONE_DX_1_5 report builder's metadata assembly in the ScanSbom handler must emit the namespaced summary property list computed from the matched findings. Related open issue 20261029T171842Z-inspector-scan-scansbom-report-body-omits-inspector-scan-results-and.md reports the same missing metadata.properties from the wire corpus; it does not carry this behavioral check id.
Traces: inspector-scan.scan-sbom-log4j-critical-vulnerability-detected.

Cause 10 of 10: findings do not fan out to every vulnerable component — the advisory data matches only some known-vulnerable coordinates.
AWS: recorded behavior only — scanning an SBOM with two independently vulnerable components matches each component against the advisory set independently; each component's returned bom-ref appears in the `affects` list of at least one finding whose CVE applies to that exact component. Apache Commons Collections 3.2.1 (purl pkg:maven/commons-collections/commons-collections@3.2.1) is a known-vulnerable coordinate (the unsafe-deserialization RCE) and produces its own finding.
Emulator: a two-component SBOM (log4j-core 2.12.1 plus commons-collections 3.2.1) returns only CVE-2021-44228; no finding is attributed to the commons-collections bom-ref (observed vulnerability ids ['CVE-2021-44228']), so that component is reported clean.
Why: the API's whole purpose — a component with a published advisory is reported as carrying nothing, so a dependency gate passes a build it must fail. It also makes 'no findings' unverifiable: a caller cannot tell an unmatched coordinate from a safe one.
Fix: the seeded advisory set in the service state (coverage for the ecosystems/coordinates the service matches) and the per-component matching path in the ScanSbom handler that resolves a component coordinate to advisory records.
Traces: inspector-scan.scan-sbom-multiple-vulnerable-components-all-detected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: inspector-scan 9/21 passed, 12 mismatches, all on ScanSbom.

Cause 1 — scan-sbom-access-denied-without-permission-rejected: expected error code 'AccessDeniedException'; got code None and HTTPStatusCode 200 with a full report body {'sbom': {'serialNumber': 'urn:uuid:...', 'bomFormat': 'CycloneDX', 'components': [...], 'vulnerabilities': [], ...}} for an IAM user created with no policies and a fresh access key.

Cause 2 — malformed-document-rejected: expected message 'Input SBOM failed validation: Invalid json', got 'sbom must be a CycloneDX 1.5 JSON object'. missing-required-fields-rejected (sbom={}): expected 'Input SBOM failed validation: Invalid value for "bomFormat"', got 'sbom.bomFormat is required and must be "CycloneDX"; only CycloneDX 1.5 documents can be scanned'. wrong-bom-format-rejected (bomFormat='SPDX'): expected 'Input SBOM failed validation: Invalid value for "bomFormat"', got 'sbom.bomFormat must be "CycloneDX", got "SPDX"; only CycloneDX 1.5 documents can be scanned'. Error code (ValidationException) and status matched in all three.

Cause 3 — component-limit-exceeded-rejected: 5001 components; expected message 'Input SBOM failed validation: Max number of components exceeded (5000)', got 'sbom.components carries more than the 2000 components a CycloneDX 1.5 scan accepts' (so the 5000-component positive control in the same check would also be refused).

Cause 4 — invalid-output-format-rejected: expected "1 validation error detected: Value 'NOT_A_REAL_OUTPUT_FORMAT' at 'outputFormat' failed to satisfy constraint: Member must satisfy enum value set: [INSPECTOR_ALT, CYCLONE_DX_1_5, INSPECTOR]", got the same sentence with the set rendered '[CYCLONE_DX_1_5, INSPECTOR, INSPECTOR_ALT]'.

Cause 5 — default-output-format-matches-explicit: default-call report keys were {serialNumber, specVersion, bomFormat, components, metadata, vulnerabilities} (a CycloneDX document) and did not equal the explicit outputFormat=INSPECTOR key set; 'vulnerability_count' absent.

Cause 6 — output-format-inspector-json-shape: outputFormat=INSPECTOR report had no 'vulnerabilities' member, so 'CVE-2021-44228 not found among []' for an SBOM containing log4j-core 2.12.1.

Cause 7 — output-format-inspector-alt-shape: outputFormat=INSPECTOR_ALT returned {'findings': {'CVE-2021-44228': [...]}, 'scanTimestamp': ..., 'sbomSpecVersion': '1.5', 'componentCount': 1, 'findingCount': 1, 'scanId': ..., 'sbomFormat': ...} — no 'vulnerabilities' list and no 'vendorAdvisories' on any node.

Cause 8 — epss-and-cvss-ratings-present: rating source names were {'NVD', 'Amazon Inspector'}; expected an 'EPSS' source with a score in [0, 1] distinct from the CVSS score.

Cause 9 — log4j-critical-vulnerability-detected: the CycloneDX report listed CVE-2021-44228 with an NVD critical rating, but metadata.properties was absent, so amazon:inspector:sbom_scanner:critical_vulnerabilities parsed as 0 ('aggregate critical count did not reflect the known critical finding').

Cause 10 — multiple-vulnerable-components-all-detected: SBOM with log4j-core 2.12.1 and commons-collections 3.2.1; returned vulnerability ids ['CVE-2021-44228'] and no finding whose affects[] named the commons-collections component's returned bom-ref.

(Per-run values in the above — serialNumbers, scan ids, timestamps, the generated IAM user name/ARN — fill the templates described in the issue; they are not requirements.)

## Steps to Reproduce
Replay the behavioral suite (simparity-behavioral-aws-v1) against the emulator and re-run the 12 inspector-scan checks in trace_ids. Individually:
1. Create an IAM user with no policies, mint an access key, and call ScanSbom with those credentials on a minimal valid CycloneDX 1.5 SBOM — expect 403 AccessDeniedException with the implicit-deny message and no `sbom` member; observe HTTP 200 and a full report.
2. Call ScanSbom with sbom set to (a) the bare string 'this-is-not-a-cyclonedx-sbom-just-a-string', (b) {}, (c) a document with bomFormat='SPDX' — expect 400 ValidationException with 'Input SBOM failed validation: Invalid json' / '... Invalid value for "bomFormat"' respectively; observe the emulator's member-path sentences.
3. Call ScanSbom with 5001 components (expect 400 'Input SBOM failed validation: Max number of components exceeded (5000)') and with exactly 5000 components (expect 200); observe the 2000-component refusal text.
4. Call ScanSbom with outputFormat='NOT_A_REAL_OUTPUT_FORMAT' and compare the enum value-set ordering in the ValidationException message.
5. Call ScanSbom twice on the same SBOM, once omitting outputFormat and once with outputFormat='INSPECTOR', and compare the top-level key sets of the returned `sbom` documents.
6/7. Call ScanSbom on an SBOM containing log4j-core 2.12.1 (purl pkg:maven/org.apache.logging.log4j/log4j-core@2.12.1) with outputFormat='INSPECTOR' and with 'INSPECTOR_ALT'; look for a top-level `vulnerabilities` list containing CVE-2021-44228 with severity critical, and for `vendorAdvisories` on each INSPECTOR_ALT node and on no INSPECTOR node.
8/9. Same SBOM with outputFormat='CYCLONE_DX_1_5': inspect vulnerabilities[].ratings for an 'EPSS'-sourced rating, and metadata.properties for the amazon:inspector:sbom_scanner:*_vulnerabilities counters.
10. Scan an SBOM containing both log4j-core 2.12.1 and commons-collections 3.2.1 (pkg:maven/commons-collections/commons-collections@3.2.1) with outputFormat='CYCLONE_DX_1_5' and check that each component's returned bom-ref appears in the affects[] of a finding for its own CVE.
