Issue-Source: trace-sweep-divergence
Issue-Trace: backupsearch-validation-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backupsearch-validation-error-paths backupsearch-error-not-found-paths backupsearch-tagging-on-search-job
Issue-Shape-Id: com.amazonaws.backupsearch#CryoBackupSearchService
Issue-Key: backupsearch|GetSearchJob|4xx->4xx|$.message
Issue-Key: backupsearch|GetSearchJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backupsearch|GetSearchJob|4xx->4xx|status
Issue-Key: backupsearch|ListSearchJobResults|4xx->4xx|$.message
Issue-Key: backupsearch|StopSearchJob|4xx->4xx|$.message
Issue-Key: backupsearch|StartSearchJob|4xx->4xx|$.message
Issue-Key: backupsearch|StartSearchResultExportJob|4xx->4xx|$.message
Issue-Key: backupsearch|TagResource|2xx->2xx|status
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# BackupSearch refusal fidelity on the wire: not-found classification, TagResource success status and ValidationException message rendering diverge (5 causes)

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSearchJob

## Description
Five distinct root causes in how the BackupSearch (sdkId BackupSearch, restJson1) emulator classifies and renders refusals and empty-body successes, found by replaying the recorded wire corpus (simparity-wire-aws-v1). Each cause must be closed before this issue is done; each names its own fixture#step below. The open issues 20261022T200842Z-backupsearch-search-job-lifecycle-iam-authorization-tag-api.md and 20261022T200842Z-backupsearch-export-job-and-backup-listing-operations-answer.md cover different refusal paths (tag-API not-found CODE, IAM, export-job gating) and carry no wire replay key for anything below.

=== Cause 1 of 5: a syntactically invalid search-job identifier is answered 404 ResourceNotFoundException instead of 400 ValidationException ===
AWS: Smithy GetSearchJob, GET /search-jobs/{SearchJobIdentifier} (SearchJobIdentifier is an @httpLabel @required GenericId), errors include ValidationException (httpError 400) and ResourceNotFoundException (httpError 404). Recorded behavior: an identifier that is not a well-formed search-job id ('not-a-valid-uuid') never reaches the store lookup — it is rejected 400 with x-amzn-errortype ValidationException (recorded message example: "Invalid Job Id"). The requirement is the classification: malformed identifier => 400 ValidationException; well-formed identifier naming nothing => 404 ResourceNotFoundException.
Emulator: answers HTTP 404 with x-amzn-errortype ResourceNotFoundException and message "no search job with identifier 'not-a-valid-uuid' exists" — every unparseable identifier is treated as a missing resource.
Why: the error class is what a caller branches on. 400 ValidationException is terminal and says "the request itself is wrong"; 404 ResourceNotFoundException says "the id is fine but the job is gone", which callers legitimately swallow (treat as deleted/expired) or retry around. A client that validates ids by probing, or that logs 404s as benign, silently accepts a malformed identifier instead of failing fast on it.
Fix: the GetSearchJob handler's input validation — check the identifier's syntax and raise the modeled ValidationException before the search-job store lookup runs.
Traces: backupsearch-validation-error-paths#task#4 (representative).

=== Cause 2 of 5: not-found refusals carry an emulator-authored message instead of the service's fixed not-found text ===
AWS: recorded behavior only — Smithy ResourceNotFoundException (httpError 404) documents its @required message member only as "Request references a resource which does not exist", so the model states no text. The recordings show three different operations (GetSearchJob, ListSearchJobResults, StopSearchJob) given the SAME nonexistent, well-formed search-job identifier all answering HTTP 404 with the byte-identical, request-independent message "Resource is not found". Being identical across three operations and carrying none of the request's data is what makes this a fixed service template rather than one call's example text.
Emulator: same status (404) and same error type, but the message is composed from the request: "no search job with identifier '48434c42-40a0-4057-b04d-3b484c610e2b' exists".
Why: the message is the only human-facing content of a 404 here (ResourceNotFoundException has no structured detail beyond resourceId/resourceType), so it reaches operator logs, user-facing surfaces and message assertions; a caller that matches the service's fixed text — or that relies on the service NOT echoing the requested identifier back — sees a different string on every call. Status and error code are correct, so this cause is the low-severity one of the bundle.
Fix: the shared not-found error construction for this service (the error channel that builds ResourceNotFoundException for the search-job operations) — emit the service's fixed not-found text rather than a per-request sentence.
Traces: backupsearch-error-not-found-paths#task#1 (GetSearchJob), backupsearch-error-not-found-paths#task#4 (ListSearchJobResults), backupsearch-error-not-found-paths#task#5 (StopSearchJob).

=== Cause 3 of 5: an enum violation inside a list is reported at the indexed member path instead of the list member path, with the value unbracketed and the constraint un-nested ===
AWS: Smithy SearchScope.BackupResourceTypes -> ResourceTypeList (@length min 1 max 1) of ResourceType, an enum with values [S3, EBS]; StartSearchJob declares ValidationException (httpError 400). This message is built FROM THE REQUEST, so its rule is the requirement: AWS reports one validation error per failing TOP-LEVEL member, naming the list's path, quoting the whole rejected list value in brackets, and nesting the member constraint — "1 validation error detected: Value '[NOT_A_REAL_TYPE]' at 'searchScope.backupResourceTypes' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [S3, EBS]]" (HTTP 400, ValidationException).
Emulator: reports the same count and the same enum value set, but at the indexed element path with the bare element value and no nesting: "1 validation error detected: Value 'NOT_A_REAL_TYPE' at 'searchScope.backupResourceTypes.1.member' failed to satisfy constraint: Member must satisfy enum value set: [S3, EBS]".
Why: ValidationException carries no structured field list, so the message IS the machine-readable description of what failed; SDK wrappers, CLI users and test suites parse the "at '<path>'" segment to point at the offending input. The emulator's path names an element position that AWS never emits (and uses an index of its own), so any such mapping resolves to nothing.
Fix: the request-validation layer's message renderer for constrained list members — aggregate the failure at the list member, bracket the rejected list value, and nest the element constraint.
Traces: backupsearch-validation-error-paths#task#1 (cluster also covers backupsearch-validation-error-paths#task#2).

=== Cause 4 of 5: a pattern-constraint validation message echoes the modeled regex including the ^ and $ anchors AWS strips ===
AWS: Smithy StartSearchResultExportJob.RoleArn -> IamRoleArn, @length 20..2048, @pattern "^arn:(?:aws|aws-cn|aws-us-gov):iam::[a-z0-9-]+:role/(.+)$"; ValidationException (httpError 400). The message is request-derived, and the recorded rule is that the service quotes the pattern WITHOUT its anchors: "1 validation error detected: Value 'not-a-valid-role-arn' at 'roleArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:(?:aws|aws-cn|aws-us-gov):iam::[a-z0-9-]+:role/(.+)" (HTTP 400).
Emulator: emits the modeled pattern verbatim, anchors included: "... regular expression pattern: ^arn:(?:aws|aws-cn|aws-us-gov):iam::[a-z0-9-]+:role/(.+)$".
Why: same consumer as Cause 3 — the quoted pattern is what a caller shows a user or compares against; a client that pins the service's pattern text never matches, and the anchors make the emulator's message describe a regex that is not the one AWS reports. This is the same defect class already filed for another service (20261022T212810Z-featurestore-runtime-sagemaker-...-regex-anchors), which does not cover backupsearch.
Fix: the shared @pattern-constraint message renderer used by this service's request validation — strip a leading ^ and trailing $ when quoting the pattern. (Note StartSearchResultExportJob's handler is also an unimplemented scaffold stub, filed separately as 20261022T200842Z-backupsearch-startsearchresultexportjob-stub.md; this cause is in the validation layer that runs AHEAD of that handler, which is why the step reaches a 400 at all.)
Traces: backupsearch-validation-error-paths#task#3.

=== Cause 5 of 5: TagResource answers HTTP 200 where AWS answers HTTP 204 ===
AWS: Smithy TagResource, POST /tags/{ResourceArn}, @idempotent, empty output structure TagResourceResponse; the model carries NO explicit success code (http code 200 is reported as codeDefaulted), so the recording is the authority: a successful TagResource on an existing search-job ARN answers HTTP 204 with no content.
Emulator: answers HTTP 200 for the same successful call.
Why: the success status is part of the contract a client observes. Callers and intermediaries distinguish 204 (no content, nothing to parse) from 200 (a body is expected); response handling, assertions on the HTTP status, and proxies/caches that treat the two differently all see the wrong outcome for a call that otherwise succeeded.
Fix: the TagResource handler's success serialization for this service — return 204 with an empty body for the empty output shape.
Traces: backupsearch-tagging-on-search-job#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false, state_reset_failures=0; 32 steps, 6 passed).

Cause 1 — backupsearch-validation-error-paths#task#4, GetSearchJob: "status: expected 400, got 404; $.message: value_mismatch (expected 'Invalid Job Id', got \"no search job with identifier 'not-a-valid-uuid' exists\"); $headers.x-amzn-errortype: value_mismatch (expected 'ValidationException', got 'ResourceNotFoundException')".

Cause 2 — backupsearch-error-not-found-paths#task#1 (GetSearchJob), #task#4 (ListSearchJobResults), #task#5 (StopSearchJob), all 404 expected / 404 actual: "$.message: value_mismatch (expected 'Resource is not found', got \"no search job with identifier '48434c42-40a0-4057-b04d-3b484c610e2b' exists\")" — identical expected text in all three.

Cause 3 — backupsearch-validation-error-paths#task#1 (and task#2), StartSearchJob, 400/400: "$.message: value_mismatch (expected \"1 validation error detected: Value '[NOT_A_REAL_TYPE]' at 'searchScope.backupResourceTypes' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [S3, EBS]]\", got \"1 validation error detected: Value 'NOT_A_REAL_TYPE' at 'searchScope.backupResourceTypes.1.member' failed to satisfy constraint: Member must satisfy enum value set: [S3, EBS]\")".

Cause 4 — backupsearch-validation-error-paths#task#3, StartSearchResultExportJob, 400/400: expected "... Member must satisfy regular expression pattern: arn:(?:aws|aws-cn|aws-us-gov):iam::[a-z0-9-]+:role/(.+)", got "... pattern: ^arn:(?:aws|aws-cn|aws-us-gov):iam::[a-z0-9-]+:role/(.+)$".

Cause 5 — backupsearch-tagging-on-search-job#task#3, TagResource: "status: expected 204, got 200" (no other key diverged).

## Steps to Reproduce
Boot the emulator and point a botocore backupsearch client (restJson1) at it.
1. (Cause 1, fixture backupsearch-validation-error-paths step task#4) get_search_job(SearchJobIdentifier="not-a-valid-uuid") — emulator answers 404 ResourceNotFoundException; AWS answers 400 ValidationException (message example "Invalid Job Id").
2. (Cause 2, fixture backupsearch-error-not-found-paths steps task#1/task#4/task#5) with a well-formed but nonexistent identifier (e.g. 48434c42-40a0-4057-b04d-3b484c610e2b) call get_search_job, list_search_job_results and stop_search_job — all three answer 404 ResourceNotFoundException in both, but the emulator's message echoes the identifier while AWS returns its fixed not-found text.
3. (Cause 3, fixture backupsearch-validation-error-paths step task#1) start_search_job(Name=..., SearchScope={"BackupResourceTypes":["NOT_A_REAL_TYPE"]}) — compare the ValidationException message: AWS points at 'searchScope.backupResourceTypes' with the bracketed list value and a nested member constraint, the emulator points at 'searchScope.backupResourceTypes.1.member'.
4. (Cause 4, fixture backupsearch-validation-error-paths step task#3) start_search_result_export_job(SearchJobIdentifier=<id>, ExportSpecification={"s3ExportSpecification":{"DestinationBucket":<bucket>}}, RoleArn="not-a-valid-role-arn") — compare the quoted pattern in the 400 message: AWS omits ^ and $, the emulator includes them.
5. (Cause 5, fixture backupsearch-tagging-on-search-job step task#3) start_search_job(...), then tag_resource(ResourceArn=<search job ARN>, Tags={"k":"v"}) and inspect ResponseMetadata.HTTPStatusCode — emulator 200, AWS 204.
