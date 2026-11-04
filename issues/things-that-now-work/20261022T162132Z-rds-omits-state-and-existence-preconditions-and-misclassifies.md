Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-instance-snapshot-and-restore
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-instance-snapshot-and-restore rds-maintenance-and-account-attributes rds-subnet-group-az-coverage-and-naming
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBSnapshot|4xx->2xx|$.ErrorResponse
Issue-Key: rds|CreateDBSnapshot|4xx->2xx|status
Issue-Key: rds|DescribeDBSnapshots|4xx->2xx|$.ErrorResponse
Issue-Key: rds|DescribeDBSnapshots|4xx->2xx|status
Issue-Key: rds|ApplyPendingMaintenanceAction|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|ApplyPendingMaintenanceAction|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|ApplyPendingMaintenanceAction|4xx->4xx|status
Issue-Key: rds|CreateDBSubnetGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|CreateDBSubnetGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 41eca1b5094053f7eb75d5bb9c5bde3b8db45df6

# RDS omits state and existence preconditions and misclassifies the resulting errors

> _Note: this issue was identified against the code base at commit `41eca1b5094053f7eb75d5bb9c5bde3b8db45df6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBSnapshot

## Description
Four operations skip a precondition AWS enforces, or enforce it and then report it with the wrong HTTP status and error code. Two of the four succeed (200) where AWS fails the request, and two fail with a code the client cannot be expecting. The already-open issue 'RDS awsQuery error envelopes omit RequestId and misclassify parameter validation' covers envelope/RequestId defects on other steps and does NOT cover these four: causes 1 and 2 are missing checks that produce no error at all, and causes 3 and 4 change the HTTP status and/or the error code, not just the message.

Cause 1 of 4: CreateDBSnapshot does not reject an instance that is not in the available state.
AWS: Smithy: CreateDBSnapshot errors include InvalidDBInstanceStateFault (errorKind client, httpError 400, awsQueryError code InvalidDBInstanceState). AWS refuses to snapshot an instance that is still creating and returns HTTP 400 with Error.Type Sender and Error.Code InvalidDBInstanceState (recorded message, as an example: 'Cannot create a snapshot because the database instance <id> is not currently in the available state.').
Emulator: returns HTTP 200 and no ErrorResponse - the snapshot is accepted against a non-available instance.
Why: a client that relies on the guard to serialise its workflow proceeds as if a snapshot of a consistent instance exists; it never sees InvalidDBInstanceState and so never waits for the instance to become available.
Fix: the CreateDBSnapshot handler's instance-state precondition check.
Traces: rds-db-instance-snapshot-and-restore#task#2.

Cause 2 of 4: DescribeDBSnapshots returns 200 for a snapshot identifier that does not exist.
AWS: Smithy: DescribeDBSnapshots' only modeled error is DBSnapshotNotFoundFault (client, httpError 404, awsQueryError code DBSnapshotNotFound). When DBSnapshotIdentifier names a snapshot that does not exist AWS returns HTTP 404 with Error.Type Sender and Error.Code DBSnapshotNotFound.
Emulator: returns HTTP 200 with no ErrorResponse.
Why: a client that uses DescribeDBSnapshots as an existence probe concludes the snapshot exists; a restore or delete built on that conclusion then fails somewhere else, and error handling keyed on DBSnapshotNotFound never runs.
Fix: the DescribeDBSnapshots handler's identifier lookup and its not-found path.
Traces: rds-db-instance-snapshot-and-restore#task#3.

Cause 3 of 4: ApplyPendingMaintenanceAction answers an unknown resource with the wrong status and code.
AWS: Smithy: ApplyPendingMaintenanceAction models ResourceNotFoundFault (client, httpError 404, awsQueryError code ResourceNotFoundFault) and InvalidDBInstanceStateFault (400). Recorded behavior for this input: AWS returns HTTP 400 with Error.Code InvalidParameterValue, treating a ResourceIdentifier ARN that names no resource as a parameter-value fault rather than a 404 (recorded message, as an example: 'Unknown resource: arn:aws:rds:us-east-1:...:db:nonexistent-db-probe').
Emulator: returns HTTP 404 with Error.Code ResourceNotFoundFault.
Why: both the status class and the code differ, so a client that retries or branches on 4xx-vs-404, or that matches InvalidParameterValue to report a bad argument back to its caller, takes the wrong branch.
Fix: the ApplyPendingMaintenanceAction handler's resource-identifier validation and the error it raises.
Traces: rds-maintenance-and-account-attributes#task#3.

Cause 4 of 4: CreateDBSubnetGroup validates AZ coverage before subnet existence, so the wrong error code is returned.
AWS: Smithy: CreateDBSubnetGroup models both InvalidSubnet (client, httpError 400, awsQueryError code InvalidSubnet) and DBSubnetGroupDoesNotCoverEnoughAZs (client, httpError 400, code DBSubnetGroupDoesNotCoverEnoughAZs). Recorded behavior for a request naming one subnet id that does not exist: AWS returns HTTP 400 with Error.Code InvalidParameterValue (recorded message, as an example: 'Some input subnets in :[subnet-0123456789abcdef0] are invalid.'), i.e. subnet validity is decided before AZ coverage.
Emulator: returns HTTP 400 with Error.Code DBSubnetGroupDoesNotCoverEnoughAZs.
Why: the status matches but the code does not, so a client that distinguishes 'you named a subnet I cannot see' from 'your subnets are all in one AZ' is told the wrong thing and would 'fix' the request by adding more subnets in more AZs rather than correcting the subnet id.
Fix: the ordering of the subnet-existence and AZ-coverage checks in the CreateDBSubnetGroup handler's validation.
Traces: rds-subnet-group-az-coverage-and-naming#task#13 (the subnet id in this step is a literal in the request, not a value captured from an earlier step, so this cause does not depend on any earlier step of the fixture succeeding).

## Evidence
Cause 1: status expected 400, got 200; $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'InvalidDBInstanceState', 'Message': 'Cannot create a snapshot because the database instance ... is not currently in the available state.'}, 'RequestId': ...}, got None). Cause 2: status expected 404, got 200; $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'DBSnapshotNotFound', ...}}, got None). Cause 3: status expected 400, got 404; $.ErrorResponse.Error.Code: value_mismatch (expected 'InvalidParameterValue', got 'ResourceNotFoundFault'). Cause 4: $.ErrorResponse.Error.Code: value_mismatch (expected 'InvalidParameterValue', got 'DBSubnetGroupDoesNotCoverEnoughAZs') at status 400.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture rds-db-instance-snapshot-and-restore step task#2: Action=CreateDBSnapshot against an instance still in 'creating' returns 200 instead of 400 InvalidDBInstanceState.
3. Replay task#3 of the same fixture: Action=DescribeDBSnapshots with a DBSnapshotIdentifier that does not exist returns 200 instead of 404 DBSnapshotNotFound.
4. Replay rds-maintenance-and-account-attributes#task#3: Action=ApplyPendingMaintenanceAction with a ResourceIdentifier ARN naming no resource returns 404/ResourceNotFoundFault instead of 400/InvalidParameterValue.
5. Replay rds-subnet-group-az-coverage-and-naming#task#13: Action=CreateDBSubnetGroup naming one non-existent subnet returns 400/DBSubnetGroupDoesNotCoverEnoughAZs instead of 400/InvalidParameterValue.

## Resolution

Causes 1, 2 and 3 are served:

* Cause 1 — `CreateDBSnapshot` refuses a source instance whose stored status is not
  `available` with `InvalidDBInstanceState` (400) and the sentence the service words for
  that class.
* Cause 2 — `DescribeDBSnapshots` answers `DBSnapshotNotFound` (404) for an identifier the
  request's region holds no record of.
* Cause 3 — `ApplyPendingMaintenanceAction` resolves its `ResourceIdentifier` against the
  DB clusters and DB instances of the request's region and refuses a value that resolves to
  neither as that parameter's value: 400 `InvalidParameterValue`, message
  `Unknown resource: <the identifier the request sent>`.

Cause 4 is DECLINED, and the two `rds|CreateDBSubnetGroup|4xx->4xx` keys in this report's
front matter (`$.ErrorResponse.Error.Code` and `.Message`) remain unserved: deciding them
needs the EC2 subnet inventory of the recording account. One recorded step of that fixture
expects the "input subnets are invalid" parameter refusal for a well-formed subnet id that
exists in no account, while another expects the Availability-Zone coverage fault for
well-formed subnet ids that DO exist there and that EC2 placed in one zone. Which
identifiers exist, and which zone each sits in, are values AWS generated inside that one
account; no rule derives them from the request, so the two requests are indistinguishable
to this emulator, which holds no subnets and derives a zone from the identifier. Serving
both as recorded would mean writing those identifiers into the service.
