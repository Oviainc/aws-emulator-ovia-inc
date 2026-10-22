Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 0761ec9a912f1ad457725ac2bfa14369ca92ad04

# Bootstrap the SimpleDBv2 emulator

_Filed: 2026-10-21T23:12:47Z_
_Operation: service-com-amazonaws-simpledbv2-simpledbv2 | Class: stub_

> _Note: this issue was identified against the code base at commit `0761ec9a912f1ad457725ac2bfa14369ca92ad04`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole SimpleDBv2 service (restJson1, sigv4 name `sdb`) is a fresh scaffold: all three modeled operations (`StartDomainExport`, `GetExport`, `ListExports`) answer HTTP 501 `NotImplemented` and hold no state. Implement the service's complete happy path — the domain-export lifecycle — statefully, so a realistic client script works end to end.

Happy path, in order (3 operations = the entire bound surface):

1. **StartDomainExport** (`POST /v2/StartDomainExport`, @idempotent) — input `domainName` (required), `s3Bucket` (required, pattern `^[a-z0-9A-Z]+[\.\-\w]*[a-z0-9A-Z]+$`), optional `clientToken` (@idempotencyToken — generate one when absent), `s3KeyPrefix`, `s3SseAlgorithm` (AES256|KMS), `s3SseKmsKeyId`, `s3BucketOwner` (`^[0-9]{12}$`). Must MINT and PERSIST a new export record keyed by a generated `exportArn` (20..2048 chars, `arn:aws:sdb:<region>:<account>:domain/<domainName>/export/<uuid>` shape), record `requestedAt` = now and `exportStatus` = `PENDING`, and return `{clientToken, exportArn, requestedAt}` read back from that record. Idempotency: a repeat call with the same `clientToken` must return the SAME `exportArn`/`requestedAt` rather than creating a second export; a same-token call with different parameters is `ConflictException`.

2. **GetExport** (`POST /v2/GetExport`, @readonly) — input `exportArn` (required). Must LOOK UP the persisted export and return every stored field: `exportArn`, `clientToken`, `exportStatus`, `domainName`, `requestedAt`, `s3Bucket` plus the stored optional `s3KeyPrefix`, `s3SseAlgorithm`, `s3SseKmsKeyId`, `s3BucketOwner`, and the terminal-state fields `exportManifest`, `itemsCount`, `exportDataCutoffTime` (set when the export reaches `SUCCEEDED`) or `failureCode`/`failureMessage` (when `FAILED`). An unknown ARN is `NoSuchExportException` (HTTP 400), a malformed/short ARN is `InvalidParameterValueException` (HTTP 400). It must NOT fabricate a status — the status returned is whatever the stored record holds.

3. **ListExports** (`POST /v2/ListExports`, @readonly, @paginated(inputToken=nextToken, outputToken=nextToken, items=exportSummaries)) — input optional `domainName`, `maxResults` (>=1), `nextToken` (1..2048). Must enumerate the persisted exports (filtered by `domainName` when given) as `exportSummaries` of `ExportSummary{exportArn, exportStatus, requestedAt, domainName}` built from stored records, honour `maxResults` as a page bound, and emit `nextToken` only when more records remain; passing that token back continues from the right offset. An unparseable/stale token is `InvalidNextTokenException` (HTTP 400). `exportSummaries` is required, so an empty store yields an empty list, not a missing member.

Every one of the three must compute its response from persisted state — no canned responses, no echoing the input back, no minting an `exportArn` that is never stored. `StartDomainExport` followed by `GetExport` must return exactly what was created, and both must appear in `ListExports`.

Status progression: an export may be created `PENDING` and advance to `SUCCEEDED` (deterministically on the next read, or immediately at creation) so clients that poll `GetExport` until a terminal state terminate; when it reaches `SUCCEEDED`, populate `exportManifest`, `itemsCount` and `exportDataCutoffTime`. Never return a status not derived from the stored record.

## State Model

In `ComAmazonawsSimpledbv2Simpledbv2State` (src/state.rs):

- `exports: HashMap<String /* exportArn */, Export>` — the authoritative store, insertion order preserved for stable pagination (e.g. an `IndexMap`, or a companion `export_order: Vec<String>`).
- `client_tokens: HashMap<String /* clientToken */, String /* exportArn */>` — idempotency index for `StartDomainExport`.
- `Export { export_arn: String, client_token: String, domain_name: String, export_status: ExportStatus /* PENDING|IN_PROGRESS|SUCCEEDED|FAILED */, requested_at: Timestamp, s3_bucket: String, s3_key_prefix: Option<String>, s3_sse_algorithm: Option<S3SseAlgorithm>, s3_sse_kms_key_id: Option<String>, s3_bucket_owner: Option<String>, export_manifest: Option<String>, items_count: Option<i64>, export_data_cutoff_time: Option<Timestamp>, failure_code: Option<String>, failure_message: Option<String> }`
- Optional `domains: HashSet<String>` (or treat any non-empty domain name as existing) to back `NoSuchDomainException` for `StartDomainExport`/`ListExports`.

All three handlers reach this through `crate::state::service_state(&*self.engine)` — the shared engine — so a write by `StartDomainExport` is visible to `GetExport`/`ListExports` on later requests.

Done = an integration script that starts an export, gets it by ARN, lists it (with and without `domainName`, and across a `maxResults`-bounded page) passes with correct bodies, not merely 2xx.

## Evidence

OPERATIONS.md for service-com-amazonaws-simpledbv2-simpledbv2 lists exactly 3 operations with these routes, required members and errors: StartDomainExport (`POST /v2/StartDomainExport`, @idempotent, required `domainName`/`s3Bucket`, output `clientToken`/`exportArn`/`requestedAt`, errors ConflictException/InvalidParameterCombinationException/InvalidParameterValueException/NoSuchDomainException/NumberExportsLimitExceeded), GetExport (`POST /v2/GetExport`, @readonly, required `exportArn`, output includes `exportStatus`/`domainName`/`requestedAt`/`s3Bucket`/`exportManifest`/`itemsCount`, errors InvalidParameterValueException/NoSuchExportException), ListExports (`POST /v2/ListExports`, @paginated inputToken=nextToken outputToken=nextToken items=exportSummaries, errors InvalidNextTokenException/InvalidParameterValueException/NoSuchDomainException). `smithy_query shape=ExportSummary` confirms the summary members {exportArn, exportStatus, requestedAt, domainName}, all @required. `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all three handler files (get_export.rs, list_exports.rs, start_domain_export.rs), i.e. the service holds zero state and answers 501 for every call. No tests assert any behavior beyond the generated state_roundtrip.rs example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK client at it with signing name `sdb` (restJson1).
2. Call StartDomainExport with domainName="my-domain", s3Bucket="my-export-bucket", s3KeyPrefix="dumps/" — expect 200 with a stored exportArn, the supplied-or-generated clientToken, and requestedAt; currently HTTP 501 NotImplemented.
3. Call StartDomainExport again with the SAME clientToken and identical parameters — expect the identical exportArn/requestedAt (no second export created).
4. Call GetExport with that exportArn — expect 200 echoing the PERSISTED record: domainName="my-domain", s3Bucket="my-export-bucket", s3KeyPrefix="dumps/", the same clientToken/requestedAt, and an exportStatus from the store.
5. Call GetExport with an unknown but well-formed ARN — expect NoSuchExportException (HTTP 400), not 501.
6. Call ListExports with no filter — expect exportSummaries containing the created export's {exportArn, exportStatus, requestedAt, domainName}.
7. Call ListExports with domainName="other-domain" — expect an empty exportSummaries list (required member present, empty).
8. Start a second and third export, then call ListExports with maxResults=1 and follow nextToken through all pages — expect each export exactly once and no nextToken on the final page.
9. Call ListExports with nextToken="garbage" — expect InvalidNextTokenException (HTTP 400).
