Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the CodeGuru Security emulator

_Filed: 2026-10-21T21:38:09Z_
_Operation: service-com-amazonaws-codegurusecurity-awscodegurusecurity | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-codegurusecurity-awscodegurusecurity/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), holding zero state. Implement a FIRST stateful slice covering the service's core scan lifecycle (restJson1, sigv4 name `codeguru-security`).

### Happy path to implement (in order)

1. **CreateUploadUrl** — `POST /uploadUrl`. Input: `scanName` (required). Output (all required): `s3Url` (a non-empty URL the emulator can serve or at least mint deterministically), `requestHeaders` (map<string,string>), `codeArtifactId` (UUID matching `^[a-f0-9]{8}-...{12}$`). The minted `codeArtifactId` MUST be recorded in state so step 2 can validate it.
2. **CreateScan** — `POST /scans`. Input: `resourceId` (union `ResourceId` with `codeArtifactId`, required), `scanName` (required, len 1..140, pattern `^[a-zA-Z0-9-_$:.]*$`), optional `clientToken` (@idempotencyToken), `scanType` (Express|Standard, default Standard), `analysisType` (All|Security, default Security), `tags`. Output (required): `scanName`, `runId` (fresh UUID), `resourceId`, `scanState` (`InProgress` or `Successful`), plus `scanNameArn` = `arn:aws:codeguru-security:<region>:<account>:scans/<scanName>`. Must persist a Scan record; an unknown `codeArtifactId` returns `ResourceNotFoundException` (404); re-creating the same `scanName` with a different resource (or a conflicting in-progress run) returns `ConflictException` (409); repeating the same `clientToken` must be idempotent (same `runId`, no new revision).
3. **GetScan** — `GET /scans/{scanName}` with optional `?runId=`. Output must be read back from the persisted record: `scanName`, `runId`, `scanState`, `createdAt`, `analysisType` (required), plus `updatedAt`, `numberOfRevisions` (incremented when the same `scanName` is re-scanned), `scanNameArn`, `errorMessage`. Unknown `scanName` (or unknown `runId`) ⇒ `ResourceNotFoundException` 404.
4. **ListScans** — `GET /scans`. Output `summaries` (list<ScanSummary>: `scanName`, `runId`, `scanState`, `createdAt` required; `updatedAt`, `scanNameArn`) built from the scan store, with real `maxResults`/`nextToken` pagination (@paginated inputToken=nextToken, outputToken=nextToken, items=summaries): a truncated page returns a `nextToken` that resumes exactly where the previous page stopped, and `nextToken` is absent on the last page.
5. **GetFindings** — `GET /findings/{scanName}` with `?status=All|Open|Closed`, `?maxResults`, `?nextToken`. Output `findings` (list<Finding>) for the stored scan plus `nextToken`. A completed scan must deterministically produce zero or more stored `Finding` records (`id`, `title`, `type`, `status`, `severity`, `createdAt`, `updatedAt`, `detectorId`, `detectorName`, `ruleId`, `resource`, `vulnerability`, `remediation`); `status` filtering and pagination must be applied to the stored list, not faked. Unknown `scanName` ⇒ 404; findings for a still-`InProgress` scan behave per `ConflictException`/empty-list semantics.
6. **BatchGetFindings** — `POST /batchGetFindings`. Input `findingIdentifiers` (list<FindingIdentifier{scanName, findingId}>, len 1..25, required). Output (both required): `findings` resolved from the SAME finding store populated in step 5, and `failedFindings` (list<BatchGetFindingsError>) carrying one entry per identifier whose `scanName`/`findingId` is not found — not an empty array and not a 404 for the whole call.

No canned, echoed, or hardcoded responses: every field above must be computed from persisted state. A `CreateScan` followed by `GetScan`/`ListScans` must return exactly what was created (same `runId`, `scanNameArn`, `analysisType`, timestamps), and finding IDs returned by `GetFindings` must resolve through `BatchGetFindings`.

## State Model

Service state (behind `crate::state::service_state(&*self.engine)`), per account/region:

- `upload_artifacts: HashMap<String /*codeArtifactId (UUID)*/, UploadArtifact { code_artifact_id: String, scan_name: String, s3_url: String, request_headers: BTreeMap<String,String>, created_at: SystemTime }>` — populated by CreateUploadUrl, consumed/validated by CreateScan.
- `scans: BTreeMap<String /*scanName*/, Scan { scan_name: String, scan_name_arn: String, resource_id_code_artifact_id: String, scan_type: ScanType, analysis_type: AnalysisType, created_at: SystemTime, updated_at: Option<SystemTime>, number_of_revisions: i64, tags: BTreeMap<String,String>, runs: Vec<ScanRun> }>` — insertion-ordered for stable ListScans paging.
- `ScanRun { run_id: Uuid, scan_state: ScanState /*InProgress|Successful|Failed*/, created_at: SystemTime, updated_at: Option<SystemTime>, error_message: Option<String>, finding_ids: Vec<String> }` — latest run answers GetScan when no `runId` is given.
- `findings: HashMap<(String /*scanName*/, String /*findingId*/), Finding { id, title, description, type, status: Status, severity: Severity, created_at, updated_at, generator_id, detector_id, detector_name, rule_id, detector_tags, resource, vulnerability, remediation }>` — the single source for both GetFindings and BatchGetFindings.
- `idempotency: HashMap<String /*clientToken*/, (String /*scanName*/, Uuid /*runId*/)>` — makes CreateScan retries return the original run.
- `tags_by_arn: HashMap<String /*scanNameArn*/, BTreeMap<String,String>>` (shared with the scan record) so a later Tag/ListTags slice reuses the same store.

Out of scope for this first slice (stay 501, filed separately later): GetAccountConfiguration, UpdateAccountConfiguration, GetMetricsSummary, ListFindingsMetrics, TagResource, UntagResource, ListTagsForResource.

Done = an integration script that calls CreateUploadUrl → CreateScan → GetScan → ListScans → GetFindings → BatchGetFindings succeeds end-to-end with correct, stateful bodies (not merely 2xx), and unknown scan names/artifact IDs return the modeled `ResourceNotFoundException` 404 rather than 501.

## Evidence

`services/service-com-amazonaws-codegurusecurity-awscodegurusecurity/OPERATIONS.md` lists all 13 bound operations with routes, required input/output members and modeled errors (CreateUploadUrl `POST /uploadUrl`; CreateScan `POST /scans`; GetScan `GET /scans/{scanName}`; ListScans `GET /scans` @paginated items=summaries; GetFindings `GET /findings/{scanName}` @paginated items=findings; BatchGetFindings `POST /batchGetFindings`, `findingIdentifiers` len 1..25, output `findings` + `failedFindings` both required). `smithy_query` for `ResourceId`, `ScanSummary`, `Finding`, `FindingIdentifier` confirms ResourceId is a union whose only member is `codeArtifactId` (documented as "Returned by CreateUploadUrl", i.e. the upload→scan chain), ScanSummary's required members, and FindingIdentifier{scanName, findingId}. `src/ops/create_scan.rs` (and every sibling in `src/ops/`) still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateScan"))` — no state is touched. No tests exist for any of these operations.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK client for `codeguru-security` at it.
2. Call CreateUploadUrl(scanName="demo-scan") — expect `s3Url`, `requestHeaders`, and a UUID `codeArtifactId`; actual: HTTP 501 NotImplemented.
3. Call CreateScan(resourceId={codeArtifactId: <id from step 2>}, scanName="demo-scan", scanType="Standard", analysisType="Security") — expect `runId`, `scanState`, `scanNameArn`; actual 501.
4. Call GetScan(scanName="demo-scan") — expect the same `runId`/`analysisType`/`createdAt` that CreateScan produced, `numberOfRevisions`=1.
5. Call ListScans(maxResults=1) — expect one ScanSummary for "demo-scan" plus a `nextToken` only if more scans exist; re-paging with that token must not repeat the first entry.
6. Call GetFindings(scanName="demo-scan", status="All") — expect the stored findings list (possibly empty) for the completed scan, with pagination honored.
7. Call BatchGetFindings(findingIdentifiers=[{scanName:"demo-scan", findingId:<id from step 6>}, {scanName:"demo-scan", findingId:"does-not-exist"}]) — expect the first in `findings` and the second in `failedFindings`.
8. Call GetScan(scanName="no-such-scan") — expect ResourceNotFoundException (HTTP 404), not 501.
