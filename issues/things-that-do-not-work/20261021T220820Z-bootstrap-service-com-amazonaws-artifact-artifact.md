Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the AWS Artifact (artifact) emulator

_Filed: 2026-10-21T22:08:20Z_
_Operation: service-com-amazonaws-artifact-artifact | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 17 operations in `services/service-com-amazonaws-artifact-artifact/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no AWS Artifact client works at all. Implement a FIRST SMALL SLICE: the canonical AWS Artifact report-retrieval workflow, end-to-end and stateful, on the `restJson1` wire protocol (sigv4 signing name `artifact`).

### Happy path to implement (in order)

1. **ListReports** — `GET /v1/report/list` -> 200. Input: `maxResults` (1..300, @httpQuery), `nextToken` (1..2048, @httpQuery). Output: `reports` (list<ReportSummary>), `nextToken`. Must page over the persisted report catalog: honour `maxResults` as a real page size, return an opaque `nextToken` only when more items remain, and resume exactly where the previous page ended. Each `ReportSummary` must be the stored record (`id` matching `^report-[a-zA-Z0-9]{16}$`, `name`, `arn`, `version`, `state`, `uploadState`, `description`, `series`, `category`, `companyName`, `productName`, `periodStart`, `periodEnd`, `acceptanceType`) — not a fabricated per-call value: two successive calls must return identical ids/arns.
2. **GetReportMetadata** — `GET /v1/report/getMetadata` -> 200. Input: `reportId` (required, `^report-[a-zA-Z0-9]{16}$`), optional `reportVersion` (long, min 1). Output: `reportDetails` (ReportDetail). Must look the report up by id (and by version when given, defaulting to the latest version) and return the SAME field values that `ListReports` reported for that id. Unknown `reportId` -> `ResourceNotFoundException` (404); malformed id or `reportVersion` < 1 -> `ValidationException` (400).
3. **ListReportVersions** — `GET /v1/report/listVersions` -> 200. Input: `reportId` (required), `maxResults`, `nextToken`. Output: `reports` (list<ReportSummary>, required), `nextToken`. Must return every stored version of that one report (one `ReportSummary` per version, `version` distinct), paginated with the same token rules as ListReports. Unknown id -> `ResourceNotFoundException` (404).
4. **GetTermForReport** — `GET /v1/report/getTermForReport` -> 200. Input: `reportId` (required), optional `reportVersion`. Output: `documentPresignedUrl`, `termToken`. Must MINT and PERSIST a term token bound to (reportId, reportVersion) — the token is the acceptance receipt that step 5 consumes. Tokens must be unique per call and stored, not a constant string.
5. **GetReport** — `GET /v1/report/get` -> 200. Input: `reportId` (required), `termToken` (required, 1..256, `^[a-zA-Z0-9_\-\s]*$`), optional `reportVersion`. Output: `documentPresignedUrl`. Must VALIDATE the `termToken` against the token store: only a token previously issued by `GetTermForReport` for that same report/version yields a URL. A token that was never issued, or one issued for a different report, must fail (`ValidationException` 400 / `ConflictException` 409 per the modeled errors) — it must NOT return a URL. Unknown `reportId` -> `ResourceNotFoundException` (404).

All five must compute their responses from persisted state. No canned bodies, no echoed inputs, no random id generated without being stored: a `ListReports` id must be fetchable by `GetReportMetadata`, and a `termToken` minted by `GetTermForReport` must be the one `GetReport` accepts.

Because AWS Artifact's report catalog is service-owned (there is no CreateReport operation), the bootstrap must SEED a small deterministic catalog at state init (e.g. 3 reports, one with 2 versions, ids matching the `^report-[a-zA-Z0-9]{16}$` pattern) and serve every read from that seed, so the workflow is observable without a create call.

## State Model

In `src/state.rs` (`ComAmazonawsArtifactArtifactState`, reached via `crate::state::service_state(&*self.engine)`):

- `reports: BTreeMap<String /* reportId */, Report>` where
  `Report { id: String, name: String, arn: String, series: String, category: String, company_name: String, product_name: String, acceptance_type: AcceptanceType, versions: BTreeMap<i64 /* version */, ReportVersion> }`
- `ReportVersion { version: i64, state: PublishedState, upload_state: UploadState, status_message: Option<String>, description: Option<String>, period_start: Timestamp, period_end: Timestamp, term_text: String, document_url: String }` — the source for both `ReportSummary` (ListReports/ListReportVersions) and `ReportDetail` (GetReportMetadata).
- `term_tokens: HashMap<String /* termToken */, TermGrant>` where `TermGrant { token: String, report_id: String, report_version: i64, issued_at: Timestamp }` — written by GetTermForReport, read (and matched on report_id + version) by GetReport.
- Seed helper that populates `reports` deterministically on first access so reads work on a cold store.

The remaining 12 operations (compliance-inquiry family, account settings, customer agreements, tagging) stay 501 for now and are tracked as individual gap issues.

## Evidence

`services/service-com-amazonaws-artifact-artifact/OPERATIONS.md` lists all 17 operations with routes, members and errors: ListReports (`GET /v1/report/list`, @paginated inputToken=nextToken/outputToken=nextToken/items=reports), GetReportMetadata (`GET /v1/report/getMetadata`, required `reportId` pattern `^report-[a-zA-Z0-9]{16}$`), ListReportVersions (`GET /v1/report/listVersions`, required output `reports`), GetTermForReport (`GET /v1/report/getTermForReport`, outputs `documentPresignedUrl` + `termToken`), GetReport (`GET /v1/report/get`, required `reportId` AND required `termToken`, output `documentPresignedUrl`) — the required `termToken` on GetReport is what grounds the term-then-download dependency. Shape members cited above come from `smithy_query` on `ReportSummary` and `ReportDetail` in `smithy-model/com-amazonaws-artifact-artifact.json`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-artifact-artifact/src/ops/` matches all 17 handler files (create_compliance_inquiry.rs, export_compliance_inquiry.rs, get_account_settings.rs, get_compliance_inquiry_metadata.rs, get_report.rs, get_report_metadata.rs, get_term_for_report.rs, list_compliance_inquiries.rs, list_compliance_inquiry_queries.rs, list_customer_agreements.rs, list_report_versions.rs, list_reports.rs, list_tags_for_resource.rs, put_account_settings.rs, put_compliance_inquiry_feedback.rs, tag_resource.rs, untag_resource.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. README.md documents the 501-until-written contract and the shared-engine state API. No integration test exercises any report operation.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with signing name `artifact`.
2. `aws artifact list-reports --max-results 2` — expect 200 with up to 2 `reports` entries and a `nextToken`; actual: HTTP 501 NotImplemented.
3. Re-run with `--next-token <token>` — expect the following page with no repeated report ids.
4. `aws artifact get-report-metadata --report-id <id from page 1>` — expect `reportDetails` whose name/arn/version match what ListReports returned for that id.
5. `aws artifact list-report-versions --report-id <same id>` — expect one ReportSummary per stored version with distinct `version` values.
6. `aws artifact get-term-for-report --report-id <same id>` — expect a `termToken` and `documentPresignedUrl`.
7. `aws artifact get-report --report-id <same id> --term-token <token from previous step>` — expect 200 with a `documentPresignedUrl`.
8. `aws artifact get-report --report-id <same id> --term-token bogus-token` — expect a modeled failure (ValidationException 400 / ConflictException 409), NOT a presigned URL.
9. `aws artifact get-report-metadata --report-id report-0000000000000000` (unseeded id) — expect ResourceNotFoundException (404).
