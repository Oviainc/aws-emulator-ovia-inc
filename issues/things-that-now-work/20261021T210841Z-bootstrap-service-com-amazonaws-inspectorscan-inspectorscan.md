Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Inspector Scan emulator

_Filed: 2026-10-21T21:08:41Z_
_Operation: service-com-amazonaws-inspectorscan-inspectorscan | Class: stub_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate `service-com-amazonaws-inspectorscan-inspectorscan` is a fresh scaffold: the single operation file `src/ops/scan_sbom.rs` still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("ScanSbom"))` (HTTP 501). Nothing in the service holds state.

This service binds exactly ONE operation, so the bootstrap happy path is that operation end-to-end, driven off a seeded vulnerability database rather than canned output:

1. **ScanSbom** (`POST /scan/sbom`, restJson1, `@idempotent`, success 200)
   - Input: `sbom` (document, **required**) — a CycloneDX 1.5 JSON document; `outputFormat` (enum `OutputFormat`: `CYCLONE_DX_1_5` | `INSPECTOR` | `INSPECTOR_ALT`, optional — when absent the service answers in CycloneDX 1.5 form, matching the input format).
   - Output: `sbom` (document) — the *vulnerability report* for the scanned SBOM, NOT the request echoed back.
   - Errors to wire: `ValidationException` (400) for a malformed/non-CycloneDX-1.5 document, a missing/unsupported `bomFormat`/`specVersion`, a `components` array of more than 2000 entries (documented limit), or a component with no resolvable name/version/purl; `AccessDeniedException` (403), `ThrottlingException` (429), `InternalServerException` (500) must be constructible and serialize with the modeled shape (`message`, and `reason`/`fields` where the model declares them).

Required behaviour (this is what makes it not a stub):
- Parse the submitted CycloneDX document: read `bomFormat`, `specVersion`, `metadata`, and each entry of `components[]` (`name`, `version`, `purl`, `type`, `bom-ref`).
- For each component, look up matches in a **persisted vulnerability database held in this service's state** (`crate::state::service_state`, reached through `self.engine`). Findings MUST come from that store — not from a hardcoded literal in the handler and not derived from the request alone.
- Build the response document from the matched findings:
  - `CYCLONE_DX_1_5`: return the input BOM augmented with a top-level `vulnerabilities[]` array, each entry carrying `id` (CVE), `source`, `ratings[]` (both an NVD/CVSS v3.1 rating and the Amazon Inspector rating, with `score`, `severity`, `method`, `vector`), `cwes`, `description`, `advisories`, `affects[].ref` pointing at the matched component's `bom-ref`, and `properties` (e.g. fixed-in version); preserve `bomFormat`/`specVersion` and bump/stamp `metadata.timestamp` and `serialNumber`.
  - `INSPECTOR` / `INSPECTOR_ALT`: return the Inspector-native report document (a findings list keyed by vulnerability id with severity, score, package coordinates, and remediation), derived from the SAME matched findings so the two formats never disagree.
- A clean SBOM (no component matches a stored vulnerability) must return 200 with an empty `vulnerabilities[]` / empty findings list — not an error and not a fabricated finding.
- Because the operation is `@idempotent`, submitting the identical SBOM twice must yield the same findings set (deterministic ordering of `vulnerabilities[]`, e.g. by component ref then CVE id).
- Record each scan in state so repeat/aggregate behaviour is observable and testable.

## State Model

`ComAmazonawsInspectorscanInspectorscanState` (in `src/state.rs`), reached via `crate::state::service_state(&*self.engine)` and mutated under its lock:

- `vulnerability_db: Vec<VulnRecord>` — the seeded advisory data the scan matches against. `VulnRecord { cve_id: String, source: String, source_url: Option<String>, package_name: String, package_type: String, affected_version_range: String, fixed_in_version: Option<String>, cvss_v3_score: f64, cvss_v3_vector: String, nvd_severity: String, inspector_score: f64, inspector_severity: String, cwes: Vec<String>, description: String, advisories: Vec<String>, published: String, modified: String }`. Seeded at state construction with a small fixture set (a handful of well-known ecosystems: npm, pypi, maven, os packages) so a realistic SBOM produces findings.
- `index: HashMap<(String /*package_type*/, String /*package_name*/), Vec<usize /*index into vulnerability_db*/>>` — lookup path used per component.
- `scans: Vec<ScanRecord>` — audit of completed scans. `ScanRecord { scan_id: String, received_at: SystemTime, output_format: OutputFormat, component_count: usize, sbom_serial_number: Option<String>, finding_ids: Vec<String>, sbom_digest: String /*hash of the submitted document, for idempotent repeat detection*/ }`.
- `next_serial: u64` — monotonic counter backing generated `serialNumber`/`scan_id` values so two scans are distinguishable in state.

Every field of the response document must be computed from `vulnerability_db` + the parsed input; no hollow 200, no echo of the request `sbom` as the response `sbom`.

Done when a client can POST a CycloneDX 1.5 SBOM containing a known-vulnerable component and receive a 200 whose `sbom` document lists the matching CVE with NVD and Inspector ratings, POST a clean SBOM and receive an empty vulnerability list, and receive a 400 `ValidationException` for a malformed document or an over-2000-component SBOM.

## Evidence

OPERATIONS.md (`services/service-com-amazonaws-inspectorscan-inspectorscan/OPERATIONS.md`) lists the service's only operation: `ScanSbom` — `POST /scan/sbom -> 200`, `@idempotent`, input `sbom` (document, required) and `outputFormat` (OutputFormat: CYCLONE_DX_1_5 | INSPECTOR | INSPECTOR_ALT), output `sbom` (document), errors AccessDeniedException (403), InternalServerException (500), ThrottlingException (429), ValidationException (400); wire protocol `restJson1`, sigv4 name `inspector-scan`. smithy_query(query="operation", operation="ScanSbom") confirms the same signature (input shape `ScanSbomRequest`, output shape `ScanSbomResponse`, http POST /scan/sbom 200, idempotent). The modeled documentation reproduced in `src/ops/scan_sbom.rs` lines 52-79 states the operation "Scans a provided CycloneDX 1.5 SBOM and reports on any vulnerabilities discovered in that SBOM", that the output "reports NVD and CVSS scores", and that the format "limits you to passing 2000 components before throwing a ValidException error" — grounding both the report-shaped output and the component-count validation. The handler itself (`src/ops/scan_sbom.rs` lines 150-168) binds `input`, `ctx` and `self.engine` to `_` and returns `Scaffolded::not_implemented("ScanSbom")`, i.e. HTTP 501 with zero state. No test asserts any ScanSbom behaviour (only the generated `tests/state_roundtrip.rs` state-API example exists).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with signing name `inspector-scan`.
2. POST /scan/sbom with a valid CycloneDX 1.5 document whose `components[]` includes a package present in the seeded vulnerability DB, and `outputFormat` omitted. Expect 200 and a response `sbom` document containing a `vulnerabilities[]` array with the matching CVE id, an NVD/CVSS v3.1 rating and an Amazon Inspector rating, and `affects[].ref` pointing at that component's `bom-ref`. Today: HTTP 501 NotImplemented.
3. Repeat the identical request with `outputFormat=INSPECTOR`. Expect 200 and the Inspector-native findings document reporting the SAME vulnerability set as the CycloneDX response.
4. POST the same SBOM a second time (operation is @idempotent). Expect an identical findings set and ordering.
5. POST a well-formed CycloneDX 1.5 SBOM whose components match nothing in the DB. Expect 200 with an empty `vulnerabilities[]` / empty findings list — not an error, not a fabricated finding.
6. POST a document that is not CycloneDX 1.5 (wrong `bomFormat`/`specVersion`, or a non-object document). Expect HTTP 400 `ValidationException` with a message naming the problem.
7. POST a CycloneDX 1.5 SBOM with more than 2000 entries in `components[]`. Expect HTTP 400 `ValidationException` per the documented limit.
