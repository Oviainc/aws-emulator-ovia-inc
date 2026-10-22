Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-instance-core-lifecycle-and-features
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-instance-core-lifecycle-and-features rds-option-group-lifecycle-batch-08 rds-option-group-mutation-envelopes rds-create-db-instance-catalog-triple rds-db-instance-target-error-paths rds-engine-versions-pagination
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBInstance|4xx->2xx|$.ErrorResponse
Issue-Key: rds|CreateDBInstance|4xx->2xx|status
Issue-Key: rds|DescribeDBInstances|4xx->2xx|$.ErrorResponse
Issue-Key: rds|DescribeDBInstances|4xx->2xx|status
Issue-Key: rds|ModifyDBInstance|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|ModifyDBInstance|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|ModifyDBInstance|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: rds|ModifyDBInstance|4xx->4xx|status
Issue-Key: rds|DeleteDBInstance|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|DeleteDBInstance|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DeleteDBInstance|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: rds|DeleteDBInstance|4xx->4xx|status
Issue-Key: rds|DescribeOptionGroups|4xx->2xx|$.ErrorResponse
Issue-Key: rds|DescribeOptionGroups|4xx->2xx|status
Issue-Key: rds|CreateOptionGroup|4xx->2xx|$.ErrorResponse
Issue-Key: rds|CreateOptionGroup|4xx->2xx|status
Issue-Key: rds|DescribeDBEngineVersions|4xx->2xx|$.ErrorResponse
Issue-Key: rds|DescribeDBEngineVersions|4xx->2xx|status
Issue-Key: rds|CreateDBInstance|2xx->4xx|$.CreateDBInstanceResponse
Issue-Key: rds|CreateDBInstance|2xx->4xx|status
Issue-Related: 20261022T092913Z-rds-awsquery-error-envelopes-omit-requestid-and-misclassify.md
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# RDS writes and reads skip existence and parameter guards, answering 200

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
Ten causes with one shared fault: the RDS handlers do not validate referenced resources or request parameters before mutating or reading state, so requests AWS refuses are accepted with 200 (and the resulting phantom state then corrupts later steps).

Cause 1 of 10 — CreateDBInstance naming a DBSubnetGroupName that does not exist. AWS: DBSubnetGroupNotFoundFault, awsQueryError DBSubnetGroupNotFoundFault, httpResponseCode 404 (Smithy: CreateDBInstance errors); recorded body Code 'DBSubnetGroupNotFoundFault', message naming the group. Emulator: HTTP 200 and the instance is created. Why: a client that relies on the create failing proceeds against an instance that AWS would never have provisioned. Fix: CreateDBInstance handler — resolve DBSubnetGroupName in state and raise DBSubnetGroupNotFoundFault before inserting the instance. Traces: rds-db-instance-core-lifecycle-and-features#task#1.

Cause 2 of 10 — DescribeDBInstances for an identifier that was never created. AWS: DBInstanceNotFoundFault / DBInstanceNotFound 404 (Smithy: DescribeDBInstances errors). Emulator: HTTP 200 with an instance body. Note this is downstream of cause 1 in the same fixture: the create that should have 404'd left the identifier in state. Fix: CreateDBInstance guard (cause 1) plus DescribeDBInstances not-found lookup. Traces: rds-db-instance-core-lifecycle-and-features#task#2.

Cause 3 of 10 — ModifyDBInstance for an identifier that does not exist. AWS: DBInstanceNotFoundFault 404 (Smithy: ModifyDBInstance errors). Emulator: HTTP 400 with Code 'InvalidDBInstanceState' — it finds a phantom instance in 'creating' and reports a state conflict instead of a not-found. Why: a client distinguishing 'absent' (404 DBInstanceNotFound) from 'busy' (400 InvalidDBInstanceState) takes the retry branch forever. Fix: ModifyDBInstance handler — not-found must be checked and raised ahead of the state-transition guard. Traces: rds-db-instance-core-lifecycle-and-features#task#3.

Cause 4 of 10 — DeleteDBInstance for an identifier that does not exist. AWS: DBInstanceNotFoundFault 404 (Smithy: DeleteDBInstance errors). Emulator: 400 'InvalidDBInstanceState'. Why/Fix: as cause 3, in the DeleteDBInstance handler. Traces: rds-db-instance-core-lifecycle-and-features#task#5.

Cause 5 of 10 — DescribeOptionGroups for a deleted/absent OptionGroupName. AWS: OptionGroupNotFoundFault, awsQueryError OptionGroupNotFoundFault, httpResponseCode 404 (Smithy: DescribeOptionGroups errors). Emulator: HTTP 200 with a result body. Why: delete-then-verify flows can never observe deletion. Fix: DescribeOptionGroups handler — raise OptionGroupNotFoundFault when a name filter matches nothing. Traces: rds-option-group-lifecycle-batch-08#teardown#10.

Cause 6 of 10 — CreateDBInstance with Engine=mysql and a nonexistent EngineVersion (8.0.99). AWS: 400 with Code 'InvalidParameterCombination' (recorded response; example message 'Cannot find version 8.0.99 for mysql'). Emulator: HTTP 200, instance created with EngineVersion echoed verbatim. Fix: CreateDBInstance validation must resolve Engine/EngineVersion against the engine-version catalog. Traces: rds-create-db-instance-catalog-triple#task#8.

Cause 7 of 10 — CreateOptionGroup with EngineName=mysql and a nonexistent MajorEngineVersion (9.9). AWS: 400 Code 'InvalidParameterCombination' (recorded; example message 'Cannot find major version 9.9 for mysql'). Emulator: HTTP 200 and the option group is created. Fix: CreateOptionGroup validation against the engine catalog. Traces: rds-option-group-mutation-envelopes#task#3.

Cause 8 of 10 — CreateDBInstance with DBInstanceClass 'db.t4g.nonexistent'. AWS: 400 Code 'InvalidParameterValue' (recorded; example message 'Invalid DB Instance class: db.t4g.nonexistent'). Emulator: 400 but Code 'DBInstanceAlreadyExists' — it runs the duplicate-identifier check before validating the instance class, so the request is refused for the wrong reason. Why: the client is told to pick a new identifier when the real defect is the instance class. Fix: CreateDBInstance validation ordering — validate DBInstanceClass against the orderable-options catalog before the existence/conflict check. Traces: rds-create-db-instance-catalog-triple#task#9.

Cause 9 of 10 — DescribeDBEngineVersions with a Marker that was not issued by a prior page. AWS: 400 Code 'InvalidParameterValue' (recorded; example message 'Invalid marker. Please use the marker returned from a prior request or omit the marker parameter.'). Emulator: HTTP 200 with a result page. Why: paginators silently resynchronise on a bogus cursor instead of surfacing the error. Fix: the RDS Query pagination layer — validate/decode Marker and reject unknown tokens. Traces: rds-engine-versions-pagination#task#4.

Cause 10 of 10 — CreateDBInstance that AWS accepts (200, DBInstance 'creating') is refused 400 by the emulator, because the unguarded create of cause 6 persisted an instance under the same identifier earlier in the fixture. AWS: 200 with the CreateDBInstanceResult/DBInstance envelope (Smithy: CreateDBInstance output DBInstance). Emulator: 400, whole $.CreateDBInstanceResponse absent. Fix: closing cause 6 removes the phantom; the create path must not leave state behind for a request it should have rejected. Traces: rds-create-db-instance-catalog-triple#task#12.

## Evidence
Cause 1: rds-db-instance-core-lifecycle-and-features#task#1 — status expected 404, got 200; expected $.ErrorResponse.Error.Code 'DBSubnetGroupNotFoundFault'.
Cause 2: same fixture task#2 — expected 404 (Code 'DBInstanceNotFound'), got 200.
Cause 3: task#3 — expected 404/'DBInstanceNotFound', got 400/'InvalidDBInstanceState'.
Cause 4: task#5 — expected 404/'DBInstanceNotFound', got 400/'InvalidDBInstanceState'.
Cause 5: rds-option-group-lifecycle-batch-08#teardown#10 (also rds-option-group-mutation-envelopes#task#8) — expected 404/'OptionGroupNotFoundFault', got 200.
Cause 6: rds-create-db-instance-catalog-triple#task#8 (also rds-db-instance-target-error-paths#task#1) — expected 400/'InvalidParameterCombination', got 200.
Cause 7: rds-option-group-mutation-envelopes#task#3 — expected 400/'InvalidParameterCombination', got 200.
Cause 8: rds-create-db-instance-catalog-triple#task#9 (3 steps) — Code expected 'InvalidParameterValue', got 'DBInstanceAlreadyExists'.
Cause 9: rds-engine-versions-pagination#task#4 — expected 400/'InvalidParameterValue', got 200.
Cause 10: rds-create-db-instance-catalog-triple#task#12 — expected 200 with CreateDBInstanceResult, got 400.

## Steps to Reproduce
1) Replay the wire corpus (replay_wire_corpus).
2) Fixture rds-db-instance-core-lifecycle-and-features: task#1 CreateDBInstance with DBSubnetGroupName that was never created -> expect 404 DBSubnetGroupNotFoundFault, observe 200; then task#2/#3/#5 Describe/Modify/DeleteDBInstance on the same identifier.
3) Fixture rds-option-group-lifecycle-batch-08 teardown#10: DescribeOptionGroups on a deleted group -> expect 404 OptionGroupNotFoundFault.
4) Fixture rds-create-db-instance-catalog-triple: task#8 (EngineVersion 8.0.99), task#9 (DBInstanceClass db.t4g.nonexistent), task#12 (create that should succeed).
5) Fixture rds-option-group-mutation-envelopes task#3 (MajorEngineVersion 9.9) and rds-engine-versions-pagination task#4 (bogus Marker).
