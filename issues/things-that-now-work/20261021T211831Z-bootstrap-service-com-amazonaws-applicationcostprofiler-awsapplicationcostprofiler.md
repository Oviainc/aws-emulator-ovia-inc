Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the ApplicationCostProfiler emulator

_Filed: 2026-10-21T21:18:31Z_
_Operation: service-com-amazonaws-applicationcostprofiler-awsapplicationcostprofiler | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-applicationcostprofiler-awsapplicationcostprofiler/src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding no state, so no client workflow works at all. Implement a first, small stateful slice covering the report-definition lifecycle (restJson1, sigv4 signing name `application-cost-profiler`).

### Happy path to implement (in order)

1. **PutReportDefinition** — `POST /reportDefinition`. Input (all required): `reportId` (1..255, `^[0-9A-Za-z\.\-_]+$`), `reportDescription` (1..1024, must contain non-whitespace), `reportFrequency` (`ALL|DAILY|MONTHLY`), `format` (`CSV|PARQUET`), `destinationS3Location` (`S3Location{bucket (3..63, S3 bucket-name pattern), prefix (1..512)}`). Must INSERT a new definition into the store with `createdAt`/`lastUpdated` set to now, and return `reportId`. Re-putting an existing `reportId` must overwrite/refresh `lastUpdated` rather than silently duplicate.
2. **GetReportDefinition** — `GET /reportDefinition/{reportId}`. Must read the stored definition and return ALL required output members from state: `reportId`, `reportDescription`, `reportFrequency`, `format`, `destinationS3Location`, `createdAt`, `lastUpdated`. Values must be exactly what PutReportDefinition stored — not echoed from the request, not defaulted. An unknown `reportId` must fail with a modeled error (`ValidationException`, HTTP 400 — the model declares no ResourceNotFound for this service), never a 200 with an empty body.
3. **ListReportDefinitions** — `GET /reportDefinition`. Must return every stored definition in `reportDefinitions` (list<ReportDefinition>, same fields as GetReportDefinition). Honour the `@paginated` contract: `maxResults` (query, 1..100) caps the page size and `nextToken` (query) resumes; emit `nextToken` only when more definitions remain, and omit it on the last page. The token must be base64-shaped to satisfy the modeled pattern `^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$`.
4. **UpdateReportDefinition** — `PUT /reportDefinition/{reportId}`. Must mutate the stored definition in place (`reportDescription`, `reportFrequency`, `format`, `destinationS3Location`), bump `lastUpdated` while PRESERVING `createdAt`, and return `reportId`. A subsequent GetReportDefinition must show the new values. Updating an unknown `reportId` must return `ValidationException` (400).
5. **DeleteReportDefinition** — `DELETE /reportDefinition/{reportId}`. Must REMOVE the definition from the store and return `reportId`. A following GetReportDefinition must fail and ListReportDefinitions must no longer include it.
6. **ImportApplicationUsage** — `POST /importApplicationUsage`. Input `sourceS3Location` (`SourceS3Location{bucket, key, region?}`, required). Must mint a fresh unique `importId` matching `^[0-9A-Za-z\.\-_]*$` (1..255) AND record the import (id + source location + timestamp) in state so ids are unique across calls. A random id with nothing stored is NOT acceptable.

Every one of these must compute its response from persisted state reached through `crate::state::service_state(&*self.engine)` — no canned responses, no echoing the input back, no random id without storage. Also delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each implemented handler so the stub census reflects reality.

### Input validation required by the model

Reject with `ValidationException` (HTTP 400, via `crate::scaffolded::Scaffolded::validation` or the modeled error variant): missing required members, `reportId` violating length/pattern, `reportDescription` blank or >1024, `reportFrequency`/`format` outside the modeled enum values, `destinationS3Location.bucket` violating the S3 bucket-name pattern/length, `maxResults` outside 1..100, and an unparsable `nextToken`.

## State Model

In `src/state.rs`, on the service state struct:

- `report_definitions: BTreeMap<String /* reportId */, ReportDefinitionRecord>` — BTreeMap so ListReportDefinitions has a stable, resumable order for paging.
  - `ReportDefinitionRecord { report_id: String, report_description: String, report_frequency: ReportFrequency, format: Format, destination_bucket: String, destination_prefix: String, created_at: SystemTime, last_updated: SystemTime }`
- `imports: Vec<UsageImportRecord>` (or `BTreeMap<String /* importId */, UsageImportRecord>`)
  - `UsageImportRecord { import_id: String, source_bucket: String, source_key: String, source_region: Option<String>, submitted_at: SystemTime }`
- `next_import_seq: u64` — monotonic counter used to mint collision-free `importId` values.

ListReportDefinitions pages by using the last returned `reportId` as the cursor (base64-encoded into `nextToken`) and resuming the BTreeMap range after it.

Out of scope for this slice: nothing — these are all 6 modeled operations; remaining fidelity concerns (throttling, AccessDenied, ServiceQuotaExceeded on the report-definition quota) are follow-up gap issues.

## Evidence

`services/service-com-amazonaws-applicationcostprofiler-awsapplicationcostprofiler/OPERATIONS.md` lists all 6 bound operations with their routes, required input/output members, constraints and errors (restJson1; PutReportDefinition `POST /reportDefinition`, GetReportDefinition `GET /reportDefinition/{reportId}`, ListReportDefinitions `GET /reportDefinition` with `@paginated(inputToken=nextToken, outputToken=nextToken, items=reportDefinitions)`, UpdateReportDefinition `PUT /reportDefinition/{reportId}`, DeleteReportDefinition `DELETE /reportDefinition/{reportId}`, ImportApplicationUsage `POST /importApplicationUsage`). `smithy_query` on `S3Location`/`SourceS3Location` gives the required members (`bucket`+`prefix`; `bucket`+`key`+optional `region`) and their length/pattern constraints. All six files in `src/ops/` are generated scaffolds that return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501, as documented in the crate README ("Every generated stub ... answers HTTP 501 NotImplemented until you write it"). No tests assert any behavior beyond `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Call PutReportDefinition with reportId="demo-report", reportDescription="demo", reportFrequency="DAILY", format="CSV", destinationS3Location={bucket:"my-acp-bucket", prefix:"reports/"} -> expect 200 with reportId="demo-report" (currently HTTP 501).
2. Call GetReportDefinition with reportId="demo-report" -> expect 200 echoing the stored description, frequency, format and destination plus non-null createdAt and lastUpdated.
3. Call ListReportDefinitions -> expect the stored definition in reportDefinitions; add a second definition and call with maxResults=1 -> expect one item plus a nextToken, then pass that nextToken -> expect the second item and no nextToken.
4. Call UpdateReportDefinition on "demo-report" with reportFrequency="MONTHLY", format="PARQUET" -> expect 200, then GetReportDefinition -> expect the new frequency/format, unchanged createdAt and an advanced lastUpdated.
5. Call ImportApplicationUsage with sourceS3Location={bucket:"my-usage-bucket", key:"usage/2024-01.csv"} -> expect 200 with a non-empty importId; call again -> expect a DIFFERENT importId.
6. Call DeleteReportDefinition on "demo-report" -> expect 200 with reportId, then GetReportDefinition on it -> expect ValidationException (HTTP 400) and ListReportDefinitions no longer containing it.
7. Call PutReportDefinition with reportFrequency="HOURLY" or an empty reportDescription -> expect ValidationException (HTTP 400), not a 200.
