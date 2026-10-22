Issue-Source: trace-sweep-divergence
Issue-Trace: redshiftdata-absent-cluster-endpoint-rejection
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshiftdata-absent-cluster-endpoint-rejection redshiftdata-absent-workgroup-endpoint-rejection redshift-data-error-validation-paths redshiftdata-target-selection-validation redshiftdata-absent-statement-id-across-readers redshiftdata-list-statements-filters-and-token
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data|ExecuteStatement|4xx->2xx|$.Message
Issue-Key: redshift-data|ExecuteStatement|4xx->2xx|$.__type
Issue-Key: redshift-data|ExecuteStatement|4xx->2xx|status
Issue-Key: redshift-data|ExecuteStatement|4xx->4xx|$.Message
Issue-Key: redshift-data|DescribeStatement|4xx->4xx|$.Message
Issue-Key: redshift-data|DescribeStatement|4xx->4xx|status
Issue-Key: redshift-data|GetStatementResult|4xx->4xx|$.Message
Issue-Key: redshift-data|GetStatementResult|4xx->4xx|status
Issue-Key: redshift-data|CancelStatement|4xx->4xx|$.Message
Issue-Key: redshift-data|CancelStatement|4xx->4xx|status
Issue-Key: redshift-data|DescribeStatement|4xx->4xx|$.message
Issue-Key: redshift-data|ListStatements|4xx->4xx|$.Message
Issue-Commit: 95a76c8c1eddf6006dd75f7d22fb2d46e0f7cba1

# redshift-data error responses diverge: wrong HTTP status, missing endpoint validation, wrong messages

> _Note: this issue was identified against the code base at commit `95a76c8c1eddf6006dd75f7d22fb2d46e0f7cba1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Replay of the simparity-wire-aws-v1 corpus against the redshift-data emulator produced 7 divergence clusters (34 steps replayed, 5 passed, score 14.7%; bootstrap_signal=false, state_reset_failures=0, so none of these are reset artifacts). They share one fault: the emulator's redshift-data error surface is not the one AWS serves on its validation and statement-lookup paths - one path returns HTTP 200 where AWS rejects, three return the wrong HTTP status for an error code that otherwise matches, and three return a different fixed message for a rule whose message AWS emits deterministically. This bundle carries SEVEN causes; all seven must be closed before submitting.

=== Cause 1 of 7: ExecuteStatement target-selection validation message ===
AWS: ExecuteStatement picks its compute target from ClusterIdentifier (provisioned cluster), WorkgroupName (serverless workgroup), a Glue resource, or SessionId. Smithy: ExecuteStatementInput.ClusterIdentifier / .WorkgroupName / .SessionId are all optional, so the mutual-exclusion rule is service-enforced, not model-enforced; the service-common error ValidationException carries @httpError 400. Recorded behavior: AWS answers HTTP 400 with one canonical, request-independent message for every bad target selection - "There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId" - and three recorded steps across two fixtures all expect exactly that string.
Emulator: HTTP 400 with the correct error code, but Message "one of ClusterIdentifier (a provisioned cluster) or WorkgroupName (a serverless workgroup) is required".
Why: the emulator asserts a different rule than AWS does. AWS's message says at most one target may be given and enumerates four accepted targets; the emulator's says at least one of two is required. A client (or a negative test) reading the message to learn why its request was refused is told the wrong rule, and the two Glue/SessionId targets are not acknowledged at all.
Fix: ExecuteStatement's input validation - the target-selection check should cover all four targets and emit AWS's single canonical message for both the none-supplied and the more-than-one-supplied case.
Traces: redshift-data-error-validation-paths#task#1 (representative), plus redshiftdata-target-selection-validation#task#1 and #task#2.

=== Cause 2 of 7: DescribeStatement on an unknown statement id answers 404 instead of 400 ===
AWS: Smithy - ValidationException is @httpError 400 and ResourceNotFoundException is @httpError 404, but the recording shows AWS does NOT use the not-found shape for an unknown statement id: DescribeStatement on an id that was never issued answers HTTP 400 with Message exactly "Query does not exist." and no id interpolated into the text.
Emulator: HTTP 404. The error code itself matches (the replay flagged only status and $.Message), and the message is "Query does not exist: aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee" - the requested id is interpolated into a message AWS keeps fixed.
Why: the HTTP status is a requirement, and an error code whose modeled @httpError is 400 served over 404 is internally inconsistent. SDKs and gateways classify on status, so a caller that expects a 400 client-validation error for a stale statement id instead sees a 404, which many SDK error mappers and retry policies treat as a different category of failure.
Fix: DescribeStatement's handler/statement lookup and the error-to-status mapping for the unknown-statement path - return the 400-mapped error and stop interpolating the id.
Traces: redshift-data-error-validation-paths#task#2 (representative), plus redshiftdata-absent-statement-id-across-readers#task#1 and #task#5.

=== Cause 3 of 7: GetStatementResult on an unknown statement id answers 404 instead of 400 ===
AWS: same unknown-statement-id rule as Cause 2 - HTTP 400 with Message exactly "Query does not exist." (Smithy: ValidationException @httpError 400; recorded behavior for the message).
Emulator: HTTP 404 with "Query does not exist: aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".
Why: as Cause 2 - a result reader polling a statement id that has expired or was never issued is handed a 404 where AWS hands it a 400, so error classification on the client diverges.
Fix: GetStatementResult's handler/statement lookup; ideally one shared unknown-statement error constructor used by all readers so the three operations cannot drift.
Traces: redshift-data-error-validation-paths#task#3 (representative), plus redshiftdata-absent-statement-id-across-readers#task#2.

=== Cause 4 of 7: CancelStatement on an unknown statement id answers 404 instead of 400 ===
AWS: same unknown-statement-id rule as Cause 2 - HTTP 400 with Message exactly "Query does not exist."
Emulator: HTTP 404 with "Query does not exist: aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".
Why: as Cause 2, on the cancel path.
Fix: CancelStatement's handler/statement lookup, via the same shared unknown-statement error constructor.
Traces: redshift-data-error-validation-paths#task#4 (representative), plus redshiftdata-absent-statement-id-across-readers#task#4.

=== Cause 5 of 7: ExecuteStatement accepts a target that does not exist in the region (200 instead of 400) ===
AWS: ExecuteStatement resolves ClusterIdentifier / WorkgroupName to a real endpoint in the caller's region before accepting the statement. When no such provisioned cluster or serverless workgroup exists, AWS rejects the call: HTTP 400 with $.__type "ValidationException" (Smithy: ValidationException @httpError 400) and Message "Redshift endpoint doesn't exist in this region." The recording shows the same rejection for an absent cluster and for an absent workgroup.
Emulator: HTTP 200 with an ExecuteStatement success body - no $.__type and no $.Message at all. It accepts the statement and hands back an id for a compute target that does not exist.
Why: this is the most severe cause in the bundle. A client is told its query was submitted against a cluster/workgroup that is not there, so a typo in ClusterIdentifier or a workgroup that was never created surfaces as success instead of a validation failure; any negative test asserting the rejection passes vacuously, and the follow-on DescribeStatement/GetStatementResult sequence runs against a statement AWS would never have created.
Fix: ExecuteStatement's handler must resolve the selected target against the emulator's cluster/workgroup state before admitting the statement, and raise the 400 validation error when it does not resolve. The same gate belongs on BatchExecuteStatement, which shares the target-selection input.
Traces: redshiftdata-absent-cluster-endpoint-rejection#task#5 (representative, carried in Issue-Trace/Issue-Step), plus redshiftdata-absent-workgroup-endpoint-rejection#task#1.

=== Cause 6 of 7: constraint-violation message quotes the @pattern with its anchors ===
AWS: DescribeStatementRequest.Id targets UUID, whose Smithy @pattern is ^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\d{0,2})?$ . The message here is one built from the request, so its rule is known: AWS emits the standard constraint-violation text "1 validation error detected: Value '<value>' at 'id' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" under the lowercase $.message key, and the pattern it quotes is the member's @pattern with the leading ^ and trailing $ anchors stripped.
Emulator: HTTP 400 and the right overall text, but it quotes the anchored form of the pattern (leading ^ and trailing $ retained).
Why: lowest severity of the bundle, but the quoted pattern is the part of the message derived from the model, so a client or conformance harness that extracts it and compares it against the service's advertised constraint sees a pattern AWS never reports.
Fix: the shared constraint-violation message formatter (not this operation) - strip the ^ / $ anchors when rendering a @pattern into a validation message, so every redshift-data member gets the AWS rendering.
Traces: redshiftdata-absent-statement-id-across-readers#task#6.

=== Cause 7 of 7: ListStatements rejects a bad NextToken with a non-AWS message ===
AWS: ListStatementsRequest.NextToken is a plain String with no modeled length or pattern constraint, so the model says nothing about this path - recorded behavior only: a NextToken that is not a token the service issued is refused with HTTP 400 and the fixed Message "Invalid token value".
Emulator: HTTP 400 with the correct error code but Message "NextToken is not a continuation token this operation issued".
Why: lowest severity - the status and code are right, only the fixed message AWS returns for an invalid pagination token differs, which breaks callers and tests that match on it.
Fix: ListStatements' pagination-token validation - emit AWS's fixed message for an invalid continuation token.
Traces: redshiftdata-list-statements-filters-and-token#task#4.

No open issue covers any of these - list_existing_issues came back empty for 'redshift'.

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, steps_total=34, steps_passed=5, steps_mismatched=29, steps_errored=0, steps_blocked=0, state_reset_failures=0, score_pct=14.7, divergence_count=7 (all service=redshift-data, all verdict=mismatch, none with stub_path or owned_by).

Cause 1 - ExecuteStatement, redshift-data-error-validation-paths#task#1 (3 steps affected)
  expected: 400, $.Message = 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId'
  actual:   400, $.Message = 'one of ClusterIdentifier (a provisioned cluster) or WorkgroupName (a serverless workgroup) is required'

Cause 2 - DescribeStatement, redshift-data-error-validation-paths#task#2 (3 steps affected)
  expected: 400, $.Message = 'Query does not exist.'
  actual:   404, $.Message = 'Query does not exist: aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'

Cause 3 - GetStatementResult, redshift-data-error-validation-paths#task#3 (2 steps affected)
  expected: 400, $.Message = 'Query does not exist.'
  actual:   404, $.Message = 'Query does not exist: aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'

Cause 4 - CancelStatement, redshift-data-error-validation-paths#task#4 (2 steps affected)
  expected: 400, $.Message = 'Query does not exist.'
  actual:   404, $.Message = 'Query does not exist: aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'

Cause 5 - ExecuteStatement, redshiftdata-absent-cluster-endpoint-rejection#task#5 (2 steps affected)
  expected: 400, $.__type = 'ValidationException', $.Message = "Redshift endpoint doesn't exist in this region."
  actual:   200, $.__type missing (None), $.Message missing (None)

Cause 6 - DescribeStatement, redshiftdata-absent-statement-id-across-readers#task#6 (1 step)
  expected: 400, $.message = "1 validation error detected: Value 'not-a-uuid' at 'id' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\\d{0,2})?"
  actual:   400, $.message = "1 validation error detected: Value 'not-a-uuid' at 'id' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\\d{0,2})?$"

Cause 7 - ListStatements, redshiftdata-list-statements-filters-and-token#task#4 (1 step)
  expected: 400, $.Message = 'Invalid token value'
  actual:   400, $.Message = 'NextToken is not a continuation token this operation issued'

Model corroboration (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json, awsJson1_1, sdkId 'Redshift Data'):
  ValidationException: errorKind=client, httpError=400 (service-common error)
  ResourceNotFoundException: errorKind=client, httpError=404 (service-common error; NOT the shape AWS uses for an unknown statement id per the recording)
  DescribeStatementRequest.Id -> UUID, required, @pattern ^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\\d{0,2})?$
  ExecuteStatementInput.ClusterIdentifier / .WorkgroupName / .SessionId all optional (mutual exclusion is service-enforced)
  ListStatementsRequest.NextToken -> String, no modeled constraint

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus); all seven clusters below reproduce. Each can also be driven by hand against the emulator's data listener with awsJson1_1 requests to the RedshiftData target prefix:
3. Cause 1 - fixture redshift-data-error-validation-paths, step task#1: ExecuteStatement with a bad compute-target selection (no target, or more than one of ClusterIdentifier / WorkgroupName / Glue resource / SessionId). Expect 400 with Message 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId'; observe the emulator's 'one of ... is required' text. Also redshiftdata-target-selection-validation#task#1 and #task#2.
4. Cause 2 - fixture redshift-data-error-validation-paths, step task#2: DescribeStatement with an Id that is a well-formed UUID but was never issued. Expect HTTP 400 and Message 'Query does not exist.'; observe HTTP 404 and the id interpolated into the message. Also redshiftdata-absent-statement-id-across-readers#task#1 and #task#5.
5. Cause 3 - fixture redshift-data-error-validation-paths, step task#3: GetStatementResult with that same never-issued Id. Expect 400 'Query does not exist.'; observe 404. Also redshiftdata-absent-statement-id-across-readers#task#2.
6. Cause 4 - fixture redshift-data-error-validation-paths, step task#4: CancelStatement with that same never-issued Id. Expect 400 'Query does not exist.'; observe 404. Also redshiftdata-absent-statement-id-across-readers#task#4.
7. Cause 5 - fixture redshiftdata-absent-cluster-endpoint-rejection, step task#5: ExecuteStatement naming a ClusterIdentifier that does not exist in the region. Expect HTTP 400 with __type ValidationException and Message "Redshift endpoint doesn't exist in this region."; observe HTTP 200 and a success body. Repeat with a nonexistent WorkgroupName (redshiftdata-absent-workgroup-endpoint-rejection#task#1) for the serverless side.
8. Cause 6 - fixture redshiftdata-absent-statement-id-across-readers, step task#6: DescribeStatement with Id 'not-a-uuid'. Expect 400 whose $.message quotes the UUID pattern without ^ / $ anchors; observe the anchored pattern.
9. Cause 7 - fixture redshiftdata-list-statements-filters-and-token, step task#4: ListStatements with a NextToken the emulator never issued. Expect 400 with Message 'Invalid token value'; observe 'NextToken is not a continuation token this operation issued'.
10. Re-replay all seven fixture#step bindings (Issue-Trace/Issue-Step plus every entry in trace_ids) to confirm a fix.
