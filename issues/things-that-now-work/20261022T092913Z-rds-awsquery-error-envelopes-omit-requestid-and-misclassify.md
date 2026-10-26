Issue-Source: trace-sweep-divergence
Issue-Trace: rds-already-exists-conflict-class
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-already-exists-conflict-class rds-db-snapshot-and-export-task-error-paths rds-identifier-validation-and-lowercasing rds-engine-versions-pagination
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateOptionGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateOptionGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: rds|CreateDBSnapshot|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateDBSnapshot|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: rds|DescribeDBSnapshots|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DescribeDBSnapshots|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: rds|CreateDBInstance|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|CreateDBInstance|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateDBInstance|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: rds|CreateOptionGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|DescribeDBEngineVersions|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|DescribeDBEngineVersions|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# RDS awsQuery error envelopes omit RequestId and misclassify parameter validation

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOptionGroup

## Description
Six causes, all in the RDS error-serialization path. RDS is an awsQuery service (Smithy: AmazonRDSv19, protocols ['awsQuery'], version 2014-10-31, xmlNamespace http://rds.amazonaws.com/doc/2014-10-31/), so every failure must be serialized as <ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId/></ErrorResponse>, and the Code must be the operation's modeled awsQueryError code. The emulator drops RequestId from the envelope entirely and answers its own generic 'ValidationException' code for request-parameter violations.

Cause 1 of 6 — CreateOptionGroup, duplicate option group. AWS: the modeled error is OptionGroupAlreadyExistsFault with awsQueryError code OptionGroupAlreadyExistsFault / httpResponseCode 400, and the envelope carries ErrorResponse.RequestId (Smithy: CreateOptionGroup errors). Emulator: status 400 and the code are right, but $.ErrorResponse.RequestId is absent (expected a shaped id, got None). Why: a client or SDK that logs/correlates on the response RequestId gets nothing, and strict XML envelope parsers see a non-conforming ErrorResponse. Fix: the RDS awsQuery error serializer — it must always emit ErrorResponse/RequestId from the per-request id. (The recorded message 'An option group named <name> already exists.' differs from the emulator's 'Option group already exists: <name>'; the message rule is not documented, so that half is an example, not the requirement.) Traces: rds-already-exists-conflict-class#task#2.

Cause 2 of 6 — CreateDBSnapshot against an absent DB instance. AWS: DBInstanceNotFoundFault, awsQueryError DBInstanceNotFound, httpResponseCode 404, with ErrorResponse.RequestId (Smithy: CreateDBSnapshot errors). Emulator: 404 and code correct, $.ErrorResponse.RequestId missing. Why/Fix: as cause 1. Traces: rds-db-snapshot-and-export-task-error-paths#task#3.

Cause 3 of 6 — DescribeDBSnapshots against an absent snapshot. AWS: DBSnapshotNotFound 404 with ErrorResponse.RequestId. Emulator: 404 and code correct, RequestId missing. Why/Fix: as cause 1. Traces: rds-db-snapshot-and-export-task-error-paths#task#6.

Cause 4 of 6 — CreateDBInstance with a DBInstanceIdentifier whose first character is a digit. AWS: rejects with query error code InvalidParameterValue at HTTP 400 (recorded response; the code is the requirement, the text 'Invalid database identifier:  1rdserr-db-59a9a1f4' is an example of a message built from the request). Emulator: HTTP 400 but Code 'ValidationException' and its own text "the first character of DBInstanceIdentifier must be a letter; got '1rdserr-db-59a9a1f4'", and again no ErrorResponse.RequestId. Why: 'ValidationException' is not an RDS query error code — SDK error-code matching and retry/catch logic keyed on InvalidParameterValue never fires. Fix: CreateDBInstance input validation must raise the InvalidParameterValue query error rather than the generic framework ValidationException. Traces: rds-identifier-validation-and-lowercasing#task#1.

Cause 5 of 6 — CreateOptionGroup with an OptionGroupName whose first character is a digit. Same divergence: expected Code InvalidParameterValue at 400, got 'ValidationException' plus a missing RequestId. Fix: CreateOptionGroup validation. Traces: rds-identifier-validation-and-lowercasing#task#10.

Cause 6 of 6 — DescribeDBEngineVersions with MaxRecords=19. AWS: Code InvalidParameterValue at 400 (recorded; the range 20..100 is the rule, example text 'Invalid value 19 for MaxRecords. Must be between 20 and 100'). Emulator: 400 with Code 'ValidationException' and text 'invalid value 19 for MaxRecords: must be a value from 20 to 100'. Fix: the shared MaxRecords validator in the RDS Query request layer must map range violations to InvalidParameterValue. Traces: rds-engine-versions-pagination#task#3.

## Evidence
Cause 1: rds-already-exists-conflict-class#task#2 — expected 400, got 400; $.ErrorResponse.RequestId missing (expected '<shaped-not-asserted>', got None).
Causes 2-3: rds-db-snapshot-and-export-task-error-paths#task#3 and #task#6 — 404/404 with $.ErrorResponse.RequestId missing.
Cause 4: rds-identifier-validation-and-lowercasing#task#1 (4 steps in cluster) — $.ErrorResponse.Error.Code expected 'InvalidParameterValue', got 'ValidationException'; RequestId missing.
Cause 5: rds-identifier-validation-and-lowercasing#task#10 — same Code divergence for OptionGroupName.
Cause 6: rds-engine-versions-pagination#task#3 — Code expected 'InvalidParameterValue', got 'ValidationException'.

## Steps to Reproduce
1) Replay the wire corpus against the emulator (replay_wire_corpus).
2) Inspect fixture rds-already-exists-conflict-class step task#2: CreateOptionGroup twice with the same OptionGroupName and read the 400 XML body — ErrorResponse has no RequestId child.
3) Inspect rds-identifier-validation-and-lowercasing task#1/task#10: CreateDBInstance / CreateOptionGroup with an identifier starting with a digit and read ErrorResponse/Error/Code — 'ValidationException' instead of 'InvalidParameterValue'.
4) Inspect rds-engine-versions-pagination task#3: DescribeDBEngineVersions with MaxRecords=19.

## Resolution

Every refusal this service answers now carries ErrorResponse/RequestId (causes 1-3), and a
handler refusal of a parameter VALUE carries the Query API common error code
`InvalidParameterValue` rather than the framework's synthetic `ValidationException` (causes
4-6), at every shared value validator in the crate: the identifier constraints, the
option-group `EngineName` value list, the documented `Port` and `BackupRetentionPeriod`
ranges, the per-operation filter-name sets, `MaxRecords` and the pagination `Marker`. The
refusals whose wording the service owns are worded as it words them: an identifier member
it names by its subject, the unissued `Marker`, `CreateDBSnapshot`'s absent source instance
and `DescribeDBSnapshots`' absent snapshot. The rule is pinned end to end by
`services/service-com-amazonaws-rds-amazonrdsv19/tests/all/parameter_refusal_codes.rs`.

### Still open, carried over from cause 4

One step of the `CreateDBInstance` 4xx message cluster in
`rds-identifier-validation-and-lowercasing` still diverges on
`$.ErrorResponse.Error.Message`. Its Code is correct (`InvalidParameterValue`), and the
leading-digit wording this report quotes is now produced exactly — the same template closed
the sibling `CreateOptionGroup` message cluster of that fixture — so the remaining step is a
CreateDBInstance refusal of a different kind whose wording this report does not state. It
was left unguessed rather than filled with an invented string: a message is a requirement,
so it needs the recorded wording for that step, and a follow-up sweep of that fixture should
file it with the expected text attached.
