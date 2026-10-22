Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: b135c4508c1fb26e9a8e4477f81bd51ec13345b2

# Bootstrap the com-amazonaws-lambda-awsgirapiservice (Lambda) emulator

_Filed: 2026-10-21T23:58:09Z_
_Operation: service-com-amazonaws-lambda-awsgirapiservice | Class: missing_

> _Note: this issue was identified against the code base at commit `b135c4508c1fb26e9a8e4477f81bd51ec13345b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution

The five-operation first slice below is implemented and state-backed: `CreateFunction`,
`GetFunction`, `ListFunctions`, `Invoke` and `DeleteFunction` all compute their answers
from `crate::state`, covered by `tests/all/function_lifecycle.rs`. Two things differ from
the plan above, deliberately:

- **`State` is `Pending` at creation, not `Active`.** The model's own `CreateFunction`,
  `StateReasonCode` and `LastUpdateStatus` documentation describes the provisioning window
  ("During this time, you can't invoke or modify the function"), so creation reports
  `Pending`/`Creating` and the clock makes the move to `Active` due shortly after.
- **`Invoke` does not run the deployment package.** `DryRun` is served in full (no function
  code runs for it on AWS either), but `RequestResponse` and `Event` answer HTTP 501: this
  emulator has no in-process runtime for a Lambda zip archive or container image, and a
  payload, `FunctionError` or log tail invented for a run that never happened would be
  indistinguishable from a real one. Everything around the run — name/version resolution,
  the lifecycle-state refusal, the documented payload limits and the JSON check — is real.

Still open as their own gap issues: the other 83 operations (aliases, layers, versions
beyond `Publish` at creation, event source mappings, URL configs, concurrency, permissions,
code signing, tagging, `InvokeAsync`, `InvokeWithResponseStream`, ...), and with them the
`MasterRegion` filter, which can never match while nothing creates a Lambda@Edge replica.

## Description

Every one of the 88 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and `src/state.rs` holds no function data. No client workflow works at all.

First slice — implement the Lambda function lifecycle end-to-end, in this order (wire protocol is `restJson1`, sigv4 signing name `lambda`):

1. **CreateFunction** — `POST /2015-03-31/functions` -> **201**. Input: `FunctionName` (required), `Role` (required), `Code` (FunctionCode, required: `ZipFile` blob or `S3Bucket`/`S3Key`/`S3ObjectVersion` or `ImageUri`), `Runtime`, `Handler`, `Description`, `Timeout`, `MemorySize`, `Environment`, `Tags`, `Architectures`, `PackageType`, `Publish` (default false). Must persist a new function and return the stored FunctionConfiguration: `FunctionName`, `FunctionArn` (`arn:aws:lambda:<region>:<account>:function:<name>`), `Runtime`, `Role`, `Handler`, `CodeSize` (byte length of the stored zip, not a constant), `CodeSha256` (base64 SHA-256 of the stored code bytes), `Version` = `$LATEST`, `LastModified` (ISO-8601 of creation), `RevisionId` (fresh UUID), `State` = `Active`, `PackageType`, `MemorySize` (default 128), `Timeout` (default 3), `Architectures` (default `[x86_64]`), `EphemeralStorage` (default 512), `TracingConfig` (`Mode: PassThrough`), `Environment`. Creating a name that already exists -> `ResourceConflictException` (409).
2. **GetFunction** — `GET /2015-03-31/functions/{FunctionName}` -> 200. Must read the persisted record and return `Configuration` (the exact values stored by CreateFunction / later updates), `Code` (`FunctionCodeLocation` with `RepositoryType` = `S3` and a `Location` URL for zip packages, or `ImageUri`/`ResolvedImageUri` for Image), `Tags` (as supplied at create), and `Concurrency` when reserved concurrency was set. Unknown name or unknown `Qualifier` -> `ResourceNotFoundException` (404). `FunctionName` must be accepted as bare name, `name:qualifier`, partial ARN and full ARN, all resolving to the same record.
3. **ListFunctions** — `GET /2015-03-31/functions` -> 200. Must return `Functions` = the FunctionConfiguration of every persisted function (NOT an empty list, NOT a canned one), honouring `MaxItems` (1..10000) and `Marker`, and returning `NextMarker` only when a page was truncated; a subsequent call with that marker must continue from where the previous page ended without repeats.
4. **Invoke** — `POST /2015-03-31/functions/{FunctionName}/invocations` -> 200. Input: `FunctionName` (label), `Payload` (httpPayload blob), `InvocationType` header (`RequestResponse` default / `Event` / `DryRun`), `LogType` header (`None`/`Tail`), `Qualifier` query. Must look the function up in state: unknown function -> `ResourceNotFoundException` (404). For `RequestResponse` return `StatusCode` 200 with a `Payload` body and `X-Amz-Executed-Version` set to the resolved version (`$LATEST` or the numeric version); for `Event` return `StatusCode` 202 with an empty payload; for `DryRun` return `StatusCode` 204. When `LogType=Tail` set `X-Amz-Log-Result` to a base64 log tail. Malformed JSON payload -> `InvalidRequestContentException` (400). The invocation must be recorded in state (count/last payload) so a later Invoke/GetFunction is consistent — never a hardcoded 200 for a name that was never created.
5. **DeleteFunction** — `DELETE /2015-03-31/functions/{FunctionName}` -> `StatusCode` 204. Must remove the record (or just the one version when `Qualifier` is a numeric version) from state; unknown name -> `ResourceNotFoundException` (404). After deletion GetFunction must 404 and ListFunctions must no longer include it.

Every one of these five must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back without storing it, no random ARN/RevisionId generated per-call without being saved. A CreateFunction followed by GetFunction must return exactly what was created.

Shared behaviour this slice needs: `FunctionName` resolution (bare name / `name:qualifier` / partial ARN / full ARN), the modeled `ResourceNotFoundException` (404), `ResourceConflictException` (409) and `InvalidParameterValueException` (400) envelopes with `__type`/`message` as restJson1 requires, and the length/pattern constraints on `FunctionName` and `Role` enforced as 400 rather than silently accepted.

## State Model

In `ComAmazonawsLambdaAwsgirapiserviceState`:

- `functions: BTreeMap<String /* function name */, FunctionRecord>` — ordered so ListFunctions paginates deterministically.
- `FunctionRecord { name: String, arn: String, role: String, runtime: Option<String>, handler: Option<String>, description: Option<String>, timeout: i32, memory_size: i32, package_type: String, architectures: Vec<String>, environment: BTreeMap<String,String>, ephemeral_storage_mb: i32, code: CodeRecord, code_size: i64, code_sha256: String, last_modified: String /* ISO-8601 */, revision_id: String, state: String /* "Active" */, tags: BTreeMap<String,String>, reserved_concurrent_executions: Option<i32>, latest_version: u64 /* highest published version */, versions: BTreeMap<String /* "1","2",... */, FunctionVersionRecord>, invoke_count: u64, last_invoke_payload: Option<Vec<u8>> }`
- `CodeRecord { zip_bytes: Option<Vec<u8>>, s3_bucket: Option<String>, s3_key: Option<String>, s3_object_version: Option<String>, image_uri: Option<String>, location: Option<String> }` — the bytes are what `code_size`/`code_sha256` are derived from.
- `FunctionVersionRecord { version: String, snapshot_of_config: FunctionRecord-like fields, code_sha256: String, created: String }` — only needed once Publish/PublishVersion joins the slice; CreateFunction with `Publish=true` must create version `1`.
- `next_marker_cursor`: pagination is implemented by slicing the ordered `functions` keys after the marker key, so `Marker` is the last-returned function name.

All other operations (aliases, layers, event source mappings, capacity providers, durable executions, URL configs, concurrency/provisioned concurrency, permissions, code signing, tagging, InvokeAsync, InvokeWithResponseStream, ...) stay stubbed at 501 for now and are tracked as separate gap issues.

## Evidence

OPERATIONS.md (crate root) lines 1-14 declare 88 operations, protocol `restJson1`, SDK id `Lambda`, signing name `lambda`. Operation contracts cited above come from OPERATIONS.md: CreateFunction (lines 316-322: `POST /2015-03-31/functions -> 201`, required `FunctionName`/`Role`/`Code`/`Publish`, output includes `FunctionArn`, `CodeSize` required, `CodeSha256`, `Version`, `RevisionId`, `State`; errors include `ResourceConflictException (409)`), ListFunctions (lines 324-330: `@paginated(items=Functions)`, inputs `Marker`/`MaxItems` 1..10000, outputs `NextMarker`/`Functions`), GetFunction (lines 397-403: outputs `Configuration`/`Code`/`Tags`/`Concurrency`, error `ResourceNotFoundException (404)`), Invoke (lines 461-466: `Payload` @httpPayload, `InvocationType`/`LogType` headers, outputs `StatusCode` @httpResponseCode, `FunctionError`, `LogResult`, `ExecutedVersion`; errors `InvalidRequestContentException (400)`, `ResourceNotFoundException (404)`), DeleteFunction (lines 26-32: `DELETE /2015-03-31/functions/{FunctionName}`, output `StatusCode`, errors 404/409). All-stub status confirmed: `src/ops/create_function.rs` line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 364 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateFunction"))`; README.md lines 36-53 document that this marker means the operation answers 501. Default values (MemorySize 128, Timeout 3, EphemeralStorage 512, Architectures [x86_64], TracingConfig PassThrough, Version `$LATEST`) and Invoke's 200/202/204 per InvocationType are well-established AWS Lambda behaviour.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `lambda` client at it.
2. aws lambda create-function --function-name demo --runtime python3.12 --role arn:aws:iam::123456789012:role/svc --handler index.handler --zip-file fileb://demo.zip  => expect HTTP 201 with FunctionArn, CodeSha256, CodeSize matching the uploaded zip, Version=$LATEST, State=Active (today: HTTP 501 NotImplemented).
3. aws lambda get-function --function-name demo  => expect 200 with Configuration identical to what create returned and Code.Location present.
4. aws lambda list-functions  => expect Functions to contain `demo`; repeat with --max-items 1 over several functions and follow NextMarker to page through without repeats.
5. aws lambda invoke --function-name demo --payload '{"k":1}' out.json  => expect StatusCode 200, X-Amz-Executed-Version: $LATEST, a payload body in out.json; with --invocation-type Event expect 202.
6. aws lambda invoke --function-name does-not-exist --payload '{}' out.json  => expect ResourceNotFoundException with HTTP 404.
7. aws lambda delete-function --function-name demo  => expect 204, then get-function => ResourceNotFoundException 404 and list-functions no longer includes `demo`.
