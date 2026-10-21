Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the Cost and Usage Report Service emulator

_Filed: 2026-10-21T21:38:05Z_
_Operation: service-com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 7 operations in `src/ops/` is an unmodified scaffold that carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns HTTP 501 `NotImplemented` while holding zero state (verified in `src/ops/put_report_definition.rs`). Implement a FIRST stateful slice covering the report-definition lifecycle, which is this service's entire primary resource.

Wire protocol (per README/OPERATIONS.md): `awsJson1_1`, `POST /` with `X-Amz-Target: AWSOrigamiServiceGatewayService.<OperationName>`, sigv4 name `cur`.

### Happy path to implement (in order)

1. **PutReportDefinition** — input `ReportDefinition` (required; members `ReportName` required, `TimeUnit`, `Format`, `Compression`, `AdditionalSchemaElements`, `S3Bucket`, `S3Prefix`, `S3Region` all required, plus optional `AdditionalArtifacts`, `RefreshClosedReports`, `ReportVersioning`, `BillingViewArn`, `ReportStatus`) and optional `Tags` (list<Tag{Key,Value}>, 0..200). Output has no members. Must INSERT the full definition into state keyed by `ReportDefinition.ReportName`, storing every member verbatim plus the tags. Must return `DuplicateReportNameException` (HTTP 400) when a report with that name already exists, and `ReportLimitReachedException` (HTTP 400) once the account already holds five report definitions (AWS limit, per the modeled error doc: "This account already has five reports defined").
2. **DescribeReportDefinitions** — input `MaxResults` (integer, range 5..5), `NextToken` (0..256, `^[A-Za-z0-9_\.\-=]*$`). Output `ReportDefinitions` (list<ReportDefinition>) + `NextToken`. Must enumerate the definitions actually persisted by PutReportDefinition/ModifyReportDefinition — including all required members as stored — with a deterministic order, honoring `MaxResults` as the page size and round-tripping an opaque `NextToken` that resumes where the previous page ended; omit `NextToken` on the last page.
3. **ModifyReportDefinition** — input `ReportName` (required) + `ReportDefinition` (required). Output has no members. Must REPLACE the stored definition for `ReportName` (including a rename when the new definition carries a different `ReportName`), so the next DescribeReportDefinitions reflects the change. Reject an unknown `ReportName` with `ValidationException` (HTTP 400) rather than silently succeeding.
4. **TagResource / ListTagsForResource** — TagResource input `ReportName` + `Tags` (required) must merge tags onto the stored report (same key overwrites); ListTagsForResource input `ReportName` must return exactly the tags persisted for that report, and `ResourceNotFoundException` (HTTP 400) when the report does not exist.
5. **DeleteReportDefinition** — input `ReportName` (required); output `ResponseMessage` (string). Must REMOVE the definition from state so a following DescribeReportDefinitions no longer lists it and ListTagsForResource on it returns `ResourceNotFoundException`.

No canned, echoed, or hardcoded responses: each response must be computed from the persisted store. A Put followed by a Describe must return exactly what was created; a Delete must be observable by every later read. `UntagResource` may be included if cheap (remove `TagKeys` from the stored tag map), otherwise it stays stubbed.

## State Model

Service state lives behind `crate::state::service_state(&*self.engine)` (the lock guard shown in the stub docs) and holds:

- `reports: IndexMap<String, StoredReport>` — keyed by `ReportName`, insertion-ordered so Describe paging is deterministic.
- `StoredReport { definition: ReportDefinition, tags: BTreeMap<String /*Key*/, String /*Value*/>, created_at: SystemTime }` where `definition` keeps every modeled member as supplied: `report_name`, `time_unit`, `format`, `compression`, `additional_schema_elements: Vec<SchemaElement>`, `s3_bucket`, `s3_prefix`, `s3_region`, `additional_artifacts: Vec<AdditionalArtifact>`, `refresh_closed_reports: Option<bool>`, `report_versioning: Option<ReportVersioning>`, `billing_view_arn: Option<String>`, `report_status: Option<ReportStatus>`.
- Pagination: `NextToken` encodes the index/name of the next report (e.g. base64 of the last returned `ReportName`), validated against the modeled token pattern and resolved back against `reports`.
- Report count limit constant `MAX_REPORTS = 5` enforced by PutReportDefinition.

Operations outside this slice (e.g. `UntagResource` if skipped) stay at 501 and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md for this crate lists all 7 operations with their input/output members and errors (PutReportDefinition: `ReportDefinition` required + `Tags`; DescribeReportDefinitions: `MaxResults` range 5..5, `NextToken` 0..256 `^[A-Za-z0-9_\.\-=]*$`, output `ReportDefinitions` + `NextToken`, @paginated; ModifyReportDefinition: `ReportName` + `ReportDefinition`; DeleteReportDefinition: `ReportName` -> `ResponseMessage`; TagResource/ListTagsForResource/UntagResource with `ResourceNotFoundException`). `smithy_query(shape="ReportDefinition,Tag")` gives the required members of `ReportDefinition` and `Tag{Key,Value}`. `src/ops/put_report_definition.rs` lines 3-12 and 162 show the `SCAFFOLDED, NOT IMPLEMENTED` marker and `Err(crate::scaffolded::Scaffolded::not_implemented("PutReportDefinition"))` — i.e. HTTP 501 with no state; the same stub shape exists for all eight files under `src/ops/`. Modeled error docs ground the limit/duplicate semantics: `ReportLimitReachedException` = "This account already has five reports defined", `DuplicateReportNameException` = "A report with the specified name already exists in the account".

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `cur` client at it.
2. PutReportDefinition with ReportDefinition{ReportName:"demo-report", TimeUnit:"DAILY", Format:"textORcsv", Compression:"GZIP", AdditionalSchemaElements:["RESOURCES"], S3Bucket:"my-bucket", S3Prefix:"cur/", S3Region:"us-east-1"} and Tags=[{Key:"team",Value:"fin"}] — expect HTTP 200 (currently 501).
3. PutReportDefinition again with the same ReportName — expect HTTP 400 DuplicateReportNameException.
4. DescribeReportDefinitions — expect ReportDefinitions to contain "demo-report" with every field exactly as supplied.
5. ListTagsForResource(ReportName="demo-report") — expect Tags=[{team,fin}].
6. TagResource(ReportName="demo-report", Tags=[{Key:"env",Value:"dev"}]) then ListTagsForResource — expect both tags.
7. ModifyReportDefinition(ReportName="demo-report", ReportDefinition with TimeUnit:"HOURLY") then DescribeReportDefinitions — expect TimeUnit HOURLY.
8. Create 5 reports total, then PutReportDefinition a 6th — expect HTTP 400 ReportLimitReachedException.
9. DescribeReportDefinitions with MaxResults=5 and follow NextToken — expect each report exactly once across pages and no NextToken on the final page.
10. DeleteReportDefinition(ReportName="demo-report") — expect 200 with ResponseMessage; then DescribeReportDefinitions no longer lists it and ListTagsForResource on it returns ResourceNotFoundException.
