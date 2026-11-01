Issue-Source: trace-sweep-divergence
Issue-Trace: inspector-scan-scan-sbom-multi-ecosystem-packages
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: inspector-scan-scan-sbom-multi-ecosystem-packages inspector-scan-scan-sbom-all-output-formats
Issue-Shape-Id: com.amazonaws.inspectorscan#InspectorScan
Issue-Key: inspector-scan|ScanSbom|2xx->2xx|$.sbom.messages[*].info_message
Issue-Key: inspector-scan|ScanSbom|2xx->2xx|$.sbom.metadata.tools.services[*].version
Issue-Commit: 7a2231a5ae71d5b25f333b80023a2e9de4b75219

# ScanSbom report emits emulator-local constants for the scanner service version and the component skip reason

> _Note: this issue was identified against the code base at commit `7a2231a5ae71d5b25f333b80023a2e9de4b75219`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ScanSbom

## Description
Two causes, both on inspector-scan ScanSbom (the service's only operation), both in the report-body rendering: a field whose value AWS derives per component or per service build is served from a single hard-coded constant in the handler. Both steps answered the correct status (200) and the correct overall report shape — only these two values diverge.

Model context (services/service-com-amazonaws-inspectorscan-inspectorscan/smithy-model/com-amazonaws-inspectorscan-inspectorscan.json): ScanSbom POST /scan/sbom, input {sbom: Sbom (document, required), outputFormat: OutputFormat}, output {sbom: Sbom (document)}, errors AccessDeniedException(403)/InternalServerException(500)/ThrottlingException(429)/ValidationException(400). The `sbom` member is an opaque Smithy `document`, so the model says NOTHING about the report interior: everything below about the report body is recorded behavior only.

Cause 1 of 2: every skipped component is reported with the same generic skip sentence, so the reason a component was not scanned is lost.
AWS: recorded behavior only — the Inspector-form report's top-level `messages[]` carries one entry per component the scan drew no conclusion about, and the entry's `info_message` states WHICH skip condition applied for that component. Two distinct sentences are recorded in one response for one multi-ecosystem SBOM: a component whose purl names a namespace/ecosystem the scanner does not support reports the fixed text 'Component skipped: unsupported purl namespace.', while a component in a supported ecosystem that the advisory data holds no rule for reports the fixed text 'Component skipped: no supported rules found.' Both sentences are fixed text (no variable parts); which one a given entry carries is decided by the component's resolved purl, and the entry's accompanying `purl` member is the variable part taken from the submitted document.
Emulator: one constant is emitted for every skipped component regardless of why it was skipped. services/service-com-amazonaws-inspectorscan-inspectorscan/src/ops/scan_sbom.rs:245 defines SKIPPED_COMPONENT_MESSAGE = "Component skipped: no supported rules found." and it is the only value ever written, both into `messages[].info_message` (same file, ~line 1785) and into the CycloneDX `amazon:inspector:sbom_scanner:info` component property (~line 1372). The skip set itself is a single undifferentiated HashSet filled from one predicate (`has_supported_rules`, ~line 1709), so the unsupported-ecosystem case is not distinguished at all. On the multi-ecosystem fixture, `$.sbom.messages[2].info_message` came back 'Component skipped: no supported rules found.' where AWS sends 'Component skipped: unsupported purl namespace.' (entries [0] and [1] matched, so the emulator's generic text is right for the no-rules class and wrong for the unsupported-namespace class).
Why: `messages[]` is the only way a caller can tell an uncovered component from a clean one, and the reason tells it which action to take: an unsupported purl namespace is a permanent gap the caller must handle by generating a different SBOM or scanning that ecosystem elsewhere, while 'no supported rules found' means the ecosystem is in scope and the coordinate simply matched nothing. Collapsing the two makes a scan-coverage report read as though every unscanned component were merely rule-less, so coverage gating and SBOM-generator triage draw the wrong conclusion.
Fix: the skip classification in the ScanSbom handler — the per-component skip decision must record WHICH condition fired (unsupported purl namespace vs. no matching rules) and the `messages[]`/`:info` renderers must emit that condition's sentence instead of one shared constant.
Traces: inspector-scan-scan-sbom-multi-ecosystem-packages#task#1.

Cause 2 of 2: the CycloneDX report advertises the emulator crate's own package version as the scanning service's build version.
AWS: recorded behavior only — the CycloneDX 1.5 report's `metadata.tools.services[0]` names the scanning service and its `version` is the BUILD VERSION of the Inspector Scan service that answered the request: a composite scanner build identifier, recorded as three '+'-joined hex build ids (recorded value '1a2b3c4d+73effe98+658310b0'). The literal recorded string is NOT the requirement — it changes whenever AWS redeploys the scanner — the requirement is that the member reports the scanning service's own build identity in that form, and never a value belonging to something other than the scanner.
Emulator: `$.sbom.metadata.tools.services[0].version` is '0.1.0'. scan_sbom.rs:239 sets SCANNER_SERVICE_VERSION = env!("CARGO_PKG_VERSION") and scanner_service() (~line 1318) writes it verbatim beside SCANNER_SERVICE_NAME, so the value the report attributes to the Inspector scanner is actually the emulator service crate's Cargo manifest version — it moves when the emulator crate is bumped and has no relation to any scanner build.
Why: the client reads this member to record WHICH scanner version produced a report (audit trails, re-scan decisions, 'has this SBOM been scanned by a newer scanner than last time'). A value sourced from the serving crate's manifest is both in the wrong form and semantically unrelated, so provenance recorded from it is wrong, and any comparison across scans is meaningless; it also leaks the emulator's own packaging into a response field that is supposed to describe AWS.
Fix: the scanner-service metadata entry in the ScanSbom CycloneDX report serializer — source `metadata.tools.services[].version` from a scanner build identifier held in the service's own state/configuration (in the recorded '+'-joined build-id form), not from env!("CARGO_PKG_VERSION").
Traces: inspector-scan-scan-sbom-all-output-formats#task#1.

Related open issues, neither of which covers these two values: 20261029T171842Z-inspector-scan-scansbom-validationexception-wire-shape-and-message-diverge-from.md (ValidationException wording/shape — it owns the three sbom-refusal fixtures in this same corpus) and 20261029T202150Z-inspector-scan-scansbom-authorization-never-enforced-wrong-sbom-validation-messages.md (authorization, validation messages/limits, default output format, report enrichment). The now-fixed 20261029T171842Z-inspector-scan-scansbom-report-body-omits-inspector-scan-results-and.md added the `messages[]` and `metadata.tools.services` members these two causes are about, but set their values from the constants above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator: 11 steps, 6 passed, 5 mismatched, score 54.5%. Three divergence clusters on inspector-scan ScanSbom; one (the ValidationException cluster, 3 affected steps) is owned by an open issue and is not re-filed. The two below are net-new.

Cause 1 — fixture inspector-scan-scan-sbom-multi-ecosystem-packages, step task#1, expected status 200, actual status 200:
  $.sbom.messages[2].info_message: value_mismatch (expected 'Component skipped: unsupported purl namespace.', got 'Component skipped: no supported rules found.')
No other path in that response diverged — messages[0] and messages[1] matched, i.e. the emulator's single constant happens to be correct for those entries and wrong for the unsupported-namespace one. Divergence key: service=inspector-scan|ScanSbom|status=200->200|keys=$.sbom.messages[2].info_message.

Cause 2 — fixture inspector-scan-scan-sbom-all-output-formats, step task#1, expected status 200, actual status 200:
  $.sbom.metadata.tools.services[0].version: value_mismatch (expected '1a2b3c4d+73effe98+658310b0', got '0.1.0')
Divergence key: service=inspector-scan|ScanSbom|status=200->200|keys=$.sbom.metadata.tools.services[0].version. The recorded build-id string is an example of the form, not a required literal; '0.1.0' is the emulator crate's Cargo package version (scan_sbom.rs:239, env!("CARGO_PKG_VERSION")), which is not a scanner build identity at all.

Source confirmation (services/service-com-amazonaws-inspectorscan-inspectorscan/src/ops/scan_sbom.rs): line 239 SCANNER_SERVICE_VERSION = env!("CARGO_PKG_VERSION"); line 245 SKIPPED_COMPONENT_MESSAGE single constant; ~1318 scanner_service(); ~1372 the :info property; ~1709 the one-predicate skip set; ~1785 messages[].info_message.

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator and look at the two inspector-scan ScanSbom clusters whose verdict is mismatch with expected_status == actual_status == 200.
2. Cause 1: POST /scan/sbom (ScanSbom) with an SBOM whose components span several purl ecosystems, including at least one component whose purl namespace/ecosystem the scanner does not support and at least one component in a supported ecosystem with no matching advisory — fixture inspector-scan-scan-sbom-multi-ecosystem-packages, step task#1. Read the response's `sbom.messages[]` and compare each entry's `info_message` against the component its `purl` names: the unsupported-namespace entry must read 'Component skipped: unsupported purl namespace.' and the rule-less one 'Component skipped: no supported rules found.'; observe the same generic sentence on both.
3. Cause 2: POST /scan/sbom with outputFormat=CYCLONE_DX_1_5 (fixture inspector-scan-scan-sbom-all-output-formats, step task#1) and read $.sbom.metadata.tools.services[0].version — expect the scanning service's own build identifier in the recorded '+'-joined build-id form; observe '0.1.0', the emulator service crate's manifest version. Bumping the crate version changes the reported scanner version, which confirms the source.
