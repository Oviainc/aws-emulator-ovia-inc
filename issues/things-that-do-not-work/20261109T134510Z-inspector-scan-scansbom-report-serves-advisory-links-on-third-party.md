Issue-Source: trace-sweep-divergence
Issue-Trace: inspector-scan-scan-sbom-inspector-format-log4shell-critical
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: inspector-scan-scan-sbom-inspector-format-log4shell-critical inspector-scan-scan-sbom-source-reference-nvd-url-present
Issue-Shape-Id: com.amazonaws.inspectorscan#InspectorScan
Issue-Key: inspector-scan|ScanSbom|2xx->2xx|$.sbom.vulnerabilities[*].referenceUrls[*]
Issue-Key: inspector-scan|ScanSbom|2xx->2xx|$.sbom.vulnerabilities[*].sourceUrl
Issue-Key: inspector-scan|ScanSbom|2xx->2xx|$.sbom.vulnerabilities[*].advisories[*].url
Issue-Key: inspector-scan|ScanSbom|2xx->2xx|$.sbom.vulnerabilities[*].ratings[*].source.url
Issue-Commit: 01e7e7e4d8371571a5c4b5afb30bf8b87d00507b

# ScanSbom report serves advisory links on third-party hosts the service does not own

> _Note: this issue was identified against the code base at commit `01e7e7e4d8371571a5c4b5afb30bf8b87d00507b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ScanSbom

## Description
Two causes, both on inspector-scan ScanSbom (the service's only operation), both the same fault in the report-body rendering: every URL-bearing member of a vulnerability node is filled from an absolute link to a PUBLIC THIRD-PARTY host (nvd.nist.gov, logging.apache.org, www.cisa.gov, github.com, ...) baked into the service's seeded advisory data, where the recorded response carries NO value at that path. The replay comparator reports both clusters with the verdict host_not_own ("expected None, got '<absolute third-party URL>'"): the emulated service hands the client a host it does not serve. Both steps answered the correct status (200) and the correct overall report shape — only these URL members diverge. The two causes are the SAME fault reached through the two report forms' two different renderers, so both renderers (and the seed behind them) must be fixed before either closes.

Model context (services/service-com-amazonaws-inspectorscan-inspectorscan/smithy-model/com-amazonaws-inspectorscan-inspectorscan.json): Smithy models ScanSbom as POST /scan/sbom, idempotent, input {sbom: Sbom (document, required), outputFormat: OutputFormat}, output {sbom: Sbom (document)}, errors AccessDeniedException(403)/InternalServerException(500)/ThrottlingException(429)/ValidationException(400). The `sbom` output is an opaque Smithy `document`, so the model says NOTHING about the report interior: everything below about the report body is recorded behavior only.

Cause 1 of 2: the INSPECTOR-form vulnerability node advertises third-party advisory links in `sourceUrl` and `referenceUrls[]`.
AWS: recorded behavior only — in the recorded Inspector-form report for a Log4Shell-bearing SBOM, the vulnerability node's `sourceUrl` and `referenceUrls[]` carry NO value at those paths (the replay's expected value at each is None), i.e. the recorded report does not hand the caller absolute links on hosts outside the responding service. A URL's HOST is an environment property of whatever endpoint answered, never a fixed requirement, so the requirement here is negative and about the host: a report member must not be populated with a constant link to a host the serving endpoint does not own. Hypothesis: the recording's URL members are absent because the responder emits only hosts it serves, and nothing in this corpus shows a recorded alternative value for these members, so omitting them is the only behavior the recording supports.
Emulator: the Inspector-native renderer fills them from the seeded advisory record verbatim. services/service-com-amazonaws-inspectorscan-inspectorscan/src/ops/scan_sbom.rs:1399 sets "sourceUrl": advisory.source_url and :1401 sets "referenceUrls": advisory.advisories; those fields are literal public URLs in the seed (services/service-com-amazonaws-inspectorscan-inspectorscan/src/state.rs, seed_vulnerability_db(), ~lines 496-724: source_url "https://nvd.nist.gov/vuln/detail/CVE-..." on every record, and advisories lists such as "https://logging.apache.org/log4j/2.x/security.html" and "https://www.cisa.gov/uscert/apache-log4j-vulnerability-guidance" for CVE-2021-44228, ~lines 588-614). On the Inspector-format fixture the response therefore came back with $.sbom.vulnerabilities[0].sourceUrl = 'https://nvd.nist.gov/vuln/detail/CVE-2021-44228', $.sbom.vulnerabilities[0].referenceUrls[0] = 'https://logging.apache.org/log4j/2.x/security.html' and [1] = 'https://www.cisa.gov/uscert/apache-log4j-vulnerability-guidance', all flagged host_not_own against an expected None. No status or error-code divergence: expected 200, actual 200.
Why: a client that follows a link out of a scan report leaves the emulated environment entirely. In a sealed test environment the fetch is an unplanned egress to the public internet (and fails, or worse, succeeds and is recorded as the emulator's own content); in any environment the fetched page describes the REAL advisory, whose affected ranges, scores and fixed-in versions need not match the emulator's seeded record, so a caller triaging from the link reasons about data the emulator never served. It also makes the report assert a provenance (NVD / the vendor's security page) that the emulated advisory data does not have.
Fix: the advisory-link rendering of the Inspector-native report serializer in the ScanSbom handler (`inspector_finding`, scan_sbom.rs around lines 1399-1401) together with the advisory seed it reads — `sourceUrl`/`referenceUrls` must not be emitted as absolute URLs on hosts this service does not serve, and `VulnRecord.source_url` / `VulnRecord.advisories` in crate::state must stop holding literal third-party hosts.
Traces: inspector-scan-scan-sbom-inspector-format-log4shell-critical#task#1.

Cause 2 of 2: the CycloneDX-form vulnerability node advertises the same third-party links in `ratings[].source.url` and `advisories[].url`.
AWS: recorded behavior only — in the recorded CycloneDX 1.5 report the vulnerability node's rating source URL and `advisories[].url` likewise carry no value at those paths (expected None at each), so the same negative, host-level requirement applies: the responding service does not put a host it does not own into the report. The source NAME member is unaffected and matched (only the url-bearing members diverged).
Emulator: the CycloneDX renderer builds one `source` object from the seeded record — scan_sbom.rs:1122-1125 json!({"name": advisory.source, "url": advisory.source_url}) — and attaches it to each rating, and scan_sbom.rs:1172-1176 renders "advisories" as advisory.advisories.iter().map(|url| json!({"url": url})). On the source-reference fixture the response came back with $.sbom.vulnerabilities[0].ratings[0].source.url = 'https://nvd.nist.gov/vuln/detail/CVE-2021-44228', $.sbom.vulnerabilities[0].advisories[0].url = 'https://logging.apache.org/log4j/2.x/security.html' and advisories[1].url = 'https://www.cisa.gov/uscert/apache-log4j-vulnerability-guidance', each flagged host_not_own against an expected None. Expected 200, actual 200.
Why: same client consequence as Cause 1, reached through the format most SBOM tooling consumes: CycloneDX consumers routinely dereference `vulnerability.source.url` and `vulnerability.advisories[].url` automatically (link enrichment, advisory de-duplication), so a report carrying public hosts turns an offline scan into outbound traffic and mixes real-world advisory content into findings the emulator synthesised from its own seed.
Fix: the same change on the CycloneDX branch — the vulnerability renderer `cyclonedx_vulnerability` in the ScanSbom handler (scan_sbom.rs, the `source` object at ~1122-1125 that each rating carries, and the `advisories[].url` mapping at ~1172-1176) plus the same seeded `VulnRecord` fields in crate::state.
Traces: inspector-scan-scan-sbom-source-reference-nvd-url-present#task#1.

Related open issues, none of which covers these URL members: 20261101T230730Z-inspector-scan-scansbom-report-emits-emulator-local-constants-for-the.md (scanner service version and the component skip sentence — it owns the other two clusters in this replay), 20261029T202150Z-inspector-scan-scansbom-authorization-never-enforced-wrong-sbom-validation-messages.md (authorization, validation messages/limits, default output format, report enrichment) and 20261102T024220Z-inspector-scan-scansbom-inspector-alt-report-renders-vulnerabilities-as-a.md (INSPECTOR_ALT list-vs-map shape and vendorAdvisories).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator: 9 fixtures, 11 steps, 7 passed, 4 mismatched, score 63.6%, emulator_up true (health 200). Four divergence clusters, all inspector-scan ScanSbom; two are owned by open issue 20261101T230730Z-... and are not re-filed. The two below are net-new and are the two causes of this issue.

Cause 1 — fixture inspector-scan-scan-sbom-inspector-format-log4shell-critical, step task#1, expected status 200, actual status 200, verdict mismatch:
  $.sbom.vulnerabilities[0].referenceUrls[0]: host_not_own (expected None, got 'https://logging.apache.org/log4j/2.x/security.html')
  $.sbom.vulnerabilities[0].referenceUrls[1]: host_not_own (expected None, got 'https://www.cisa.gov/uscert/apache-log4j-vulnerability-guidance')
  $.sbom.vulnerabilities[0].sourceUrl: host_not_own (expected None, got 'https://nvd.nist.gov/vuln/detail/CVE-2021-44228')
  Divergence key: service=inspector-scan|ScanSbom|status=200->200|keys=$.sbom.vulnerabilities[0].referenceUrls[0],$.sbom.vulnerabilities[0].referenceUrls[1],$.sbom.vulnerabilities[0].sourceUrl

Cause 2 — fixture inspector-scan-scan-sbom-source-reference-nvd-url-present, step task#1, expected status 200, actual status 200, verdict mismatch:
  $.sbom.vulnerabilities[0].ratings[0].source.url: host_not_own (expected None, got 'https://nvd.nist.gov/vuln/detail/CVE-2021-44228')
  $.sbom.vulnerabilities[0].advisories[0].url: host_not_own (expected None, got 'https://logging.apache.org/log4j/2.x/security.html')
  $.sbom.vulnerabilities[0].advisories[1].url: host_not_own (expected None, got 'https://www.cisa.gov/uscert/apache-log4j-vulnerability-guidance')
  Divergence key: service=inspector-scan|ScanSbom|status=200->200|keys=$.sbom.vulnerabilities[0].advisories[0].url,$.sbom.vulnerabilities[0].advisories[1].url,$.sbom.vulnerabilities[0].ratings[0].source.url

The specific CVE, package and host strings above are the seed's values for one advisory, not requirements: the requirement is that no report member hands back a host the service does not serve. Every other path in both responses matched (status, report envelope, counts, severities, scores, CWEs, descriptions, affects refs), so the divergence is confined to the URL-bearing members.

Source confirmation:
- services/service-com-amazonaws-inspectorscan-inspectorscan/src/ops/scan_sbom.rs:1122-1125 — CycloneDX `source` = {name: advisory.source, url: advisory.source_url}, carried on each rating.
- scan_sbom.rs:1172-1176 — CycloneDX "advisories" = advisory.advisories.map(|url| {"url": url}).
- scan_sbom.rs:1399 — Inspector form "sourceUrl": advisory.source_url; scan_sbom.rs:1401 — "referenceUrls": advisory.advisories.
- services/service-com-amazonaws-inspectorscan-inspectorscan/src/state.rs:37-74 — VulnRecord.source_url / .advisories are documented as the CycloneDX source/advisory URLs; state.rs:496-724 seed_vulnerability_db() fills them with literal public URLs (nvd.nist.gov on all ten records; logging.apache.org + www.cisa.gov for CVE-2021-44228 at ~588-614; github.com, pkg.go.dev, openssl.org, sourceware.org on the others).
So every URL in a report is the seeded third-party literal projected through one of the two renderers, which is why the same fault surfaces once per report form.

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator and look at the two inspector-scan ScanSbom clusters whose verdict is mismatch with expected_status == actual_status == 200 and whose per-path verdict is host_not_own.
2. Cause 1: POST /scan/sbom (ScanSbom) with outputFormat=INSPECTOR and a CycloneDX 1.5 SBOM containing a component the seeded advisory data matches (e.g. purl pkg:maven/org.apache.logging.log4j/log4j-core@2.12.1) — fixture inspector-scan-scan-sbom-inspector-format-log4shell-critical, step task#1. Read $.sbom.vulnerabilities[0].sourceUrl and $.sbom.vulnerabilities[0].referenceUrls[]: expect no value at those paths (the recorded report carries none, and in no case a host the service does not serve); observe absolute links to nvd.nist.gov, logging.apache.org and www.cisa.gov.
3. Cause 2: POST /scan/sbom with outputFormat=CYCLONE_DX_1_5 on the same SBOM — fixture inspector-scan-scan-sbom-source-reference-nvd-url-present, step task#1. Read $.sbom.vulnerabilities[0].ratings[0].source.url and $.sbom.vulnerabilities[0].advisories[].url: expect no value at those paths; observe the same third-party hosts.
4. Confirm the source: the returned URLs are exactly the literals in state.rs seed_vulnerability_db() for the matched advisory (VulnRecord.source_url and VulnRecord.advisories), so editing the seed changes the reported links — they are not derived from the request, the endpoint or any state the caller supplied.
5. After a fix, re-replay both fixtures: both must pass, i.e. neither report form may emit a URL on a host the emulator does not serve.
