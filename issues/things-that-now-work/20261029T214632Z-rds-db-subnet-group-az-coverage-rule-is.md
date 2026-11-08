Issue-Source: trace-sweep-divergence
Issue-Trace: rds-subnet-group-az-coverage-and-naming
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-subnet-group-az-coverage-and-naming
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBSubnetGroup|4xx->2xx|$.ErrorResponse
Issue-Key: rds|CreateDBSubnetGroup|4xx->2xx|status
Issue-Key: rds|ModifyDBSubnetGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|ModifyDBSubnetGroup|4xx->2xx|$.ErrorResponse
Issue-Key: rds|ModifyDBSubnetGroup|4xx->2xx|status
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS DB subnet group AZ-coverage rule is unenforced on create and mis-worded on modify

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBSubnetGroup

## Description
The DB subnet group AZ-coverage precondition is implemented inconsistently: on CreateDBSubnetGroup it is not enforced at all, and where it does fire the emulator words it with its own sentence and picks a different error code from AWS's. 3 causes.

Cause 1 of 3: CreateDBSubnetGroup naming subnets that cover only one AZ.
AWS: Smithy CreateDBSubnetGroup/ModifyDBSubnetGroup both model DBSubnetGroupDoesNotCoverEnoughAZs (awsQueryError DBSubnetGroupDoesNotCoverEnoughAZs, httpError 400). Recorded: 400, Code DBSubnetGroupDoesNotCoverEnoughAZs, Type Sender, Message "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: <az-list>. Add subnets to cover at least 2 AZs." Fixed text as quoted; the variable part is the comma-separated list of AZs the named subnets actually cover.
Emulator: 200 — the group is created with one-AZ coverage and no $.ErrorResponse at all.
Why: a client can create a subnet group RDS would reject, so a later CreateDBInstance/CreateDBCluster against it behaves differently from AWS.
Fix: the CreateDBSubnetGroup handler's AZ-coverage precondition.
Traces: rds-subnet-group-az-coverage-and-naming#task#8.

Cause 2 of 3: ModifyDBSubnetGroup reducing coverage to one AZ.
AWS: 400 DBSubnetGroupDoesNotCoverEnoughAZs with the sentence above, naming the AZs actually covered.
Emulator: 400 but Message 'the request names 1 distinct subnet(s): a DB subnet group must cover at least two Availability Zones, so it must name at least two subnets' — the emulator's own prose, counting subnets rather than naming AZ coverage.
Why: the message AWS guarantees is not reproduced and the client is told a subnet-count rule instead of the AZ-coverage rule.
Fix: the ModifyDBSubnetGroup handler's AZ-coverage message.
Traces: rds-subnet-group-az-coverage-and-naming#task#11.

Cause 3 of 3: ModifyDBSubnetGroup with an otherwise-invalid SubnetIds list.
AWS: 400 with Code InvalidParameterValue (the recording asserts the code; the message is shape-asserted).
Emulator: 200 — the modify is accepted with no error.
Why: an invalid subnet list is applied, leaving the group pointing at subnets RDS rejects.
Fix: the ModifyDBSubnetGroup handler's SubnetIds validation.
Traces: rds-subnet-group-az-coverage-and-naming#task#12.

## Evidence
Replay cluster details:
1. rds-subnet-group-az-coverage-and-naming#task#8 CreateDBSubnetGroup: status expected 400 got 200; $.ErrorResponse missing (recording: Type Sender, Code DBSubnetGroupDoesNotCoverEnoughAZs, Message "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: us-west-2a. Add subnets to cover at least 2 AZs.").
2. rds-subnet-group-az-coverage-and-naming#task#11 ModifyDBSubnetGroup 400->400: Message expected "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: us-west-2a. Add subnets to cover at least 2 AZs.", got 'the request names 1 distinct subnet(s): a DB subnet group must cover at least two Availability Zones, so it must name at least two subnets'.
3. rds-subnet-group-az-coverage-and-naming#task#12 ModifyDBSubnetGroup: status expected 400 got 200; $.ErrorResponse missing (recording: Code InvalidParameterValue, Message shape-asserted).
(The AZ name 'us-west-2a' is this run's coverage; the requirement is the fixed sentence with the request's own AZ coverage substituted.)
Note: a fourth cluster in this fixture (CreateDBSubnetGroup task#13, expected Code InvalidParameterValue 'Some input subnets in :[...] are invalid.' got DBSubnetGroupDoesNotCoverEnoughAZs) is already owned by open issue 20261022T162132Z-rds-omits-state-and-existence-preconditions-and-misclassifies.md and is NOT bundled here.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run rds-subnet-group-az-coverage-and-naming#task#8 and the steps in trace_ids; each creates or modifies a DB subnet group with one-AZ or invalid subnet coverage and the replay compares status and $.ErrorResponse.
