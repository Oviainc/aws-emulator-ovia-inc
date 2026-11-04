Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: timestream-influxdb.create-db-instance-full-lifecycle
Issue-Shape-Id: com.amazonaws.timestreaminfluxdb#AmazonTimestreamInfluxDB
Issue-Key: timestream-influxdb.create-db-instance-full-lifecycle|run#15|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Commit: 04dc54a1e9195c8cf637d3927b453ed3a21231a5

# timestream-influxdb: RebootDbInstance on a deleted DB instance id answers NotImplemented instead of ResourceNotFoundException "The DB instance with db id does not exist"

> _Note: this issue was identified against the code base at commit `04dc54a1e9195c8cf637d3927b453ed3a21231a5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RebootDbInstance

## Description
One net-new root cause.

Cause 1 of 1: a control-plane mutation aimed at an id whose DB instance has already been deleted is not refused as not-found; it is refused as an unimplemented operation, so the emulator never reaches the not-found decision at all. Failing check id: timestream-influxdb.create-db-instance-full-lifecycle.

Related-but-not-covering: the open issue 20261104T042504Z-timestream-influxdb-rebootdbinstance-stub.md ("Implement timestream-influxdb RebootDbInstance: unimplemented scaffold stub") asks for the handler to exist, but its Issue-Trace-Ids carries only timestream-influxdb.reboot-db-instance-conflict-while-creating-then-succeeds. The check filed here is NOT in that issue's replay key, so a fixer closing that issue will never re-replay this lifecycle check and will not learn whether the deleted-id refusal is right. This issue carries that replay key and the not-found requirement.

AWS: Smithy (com.amazonaws.timestreaminfluxdb#AmazonTimestreamInfluxDB) models RebootDbInstance with input member `identifier` (target DbInstanceIdentifier, required, @length min 3 max 64, @pattern ^[a-zA-Z0-9]+$) and the modeled error set AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); the operation is @idempotent. When the identifier names no live DB instance — including an id whose instance was deleted via DeleteDbInstance — AWS refuses with HTTP 404 and error code `ResourceNotFoundException`. The recorded message for this class is "The DB instance with db id does not exist": that whole string is the error class's fixed text as AWS words it for a DB-instance lookup miss — it carries no id, name, timestamp or request-scoped value, so the requirement is the literal sentence, not a template to be filled from the request. A dead id must stay unusable for mutations, not merely absent from a status read.

Emulator: the full happy path works — CreateDbInstance returns status CREATING, GetDbInstance polls to AVAILABLE with a non-empty `endpoint` and `influxAuthParametersSecretArn`, DeleteDbInstance drives the instance to DELETED/absent. Then RebootDbInstance on that same, now-deleted id is refused with error code `NotImplemented` and message "operation RebootDbInstance is scaffolded but not implemented in this emulator" (the scaffold's HTTP 501), instead of 404/ResourceNotFoundException. The replay assertion reads: AssertionError: expected a mutation on the deleted instance to fail with ResourceNotFoundException, got 'NotImplemented'.

Why: a client cannot distinguish "this resource is gone" from "this emulator cannot do reboots". Botocore surfaces NotImplemented, which is not in the operation's modeled error set, so error-handling code that branches on ResourceNotFoundException (the standard way to confirm a delete completed, or to make a reboot retry loop idempotent) takes the unexpected-error path and aborts. Teardown and reconciliation logic that treats ResourceNotFoundException as success will instead raise. The non-2xx status is also wrong (501 server-side-unsupported rather than 404 client-error), so retry/backoff classification is wrong too.

Fix: the RebootDbInstance operation handler, services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/src/ops/reboot_db_instance.rs — it must resolve `identifier` against the service's DB-instance state and, when no live instance is found for it, return the modeled ResourceNotFoundException (HTTP 404) with the AWS fixed message, rather than the scaffold refusal. The lookup-miss branch belongs with the handler's state read, not in the serializer.

Traces: timestream-influxdb.create-db-instance-full-lifecycle (fails at the final step, after Create/Get/Delete all behaved correctly).

## Evidence
Failing check: timestream-influxdb.create-db-instance-full-lifecycle [mismatch]

Expected (real AWS): RebootDbInstance(identifier=<deleted instance id>) -> HTTP 404, Error.Code = "ResourceNotFoundException", Error.Message = "The DB instance with db id does not exist" (entirely fixed text for this error class; no request- or state-derived parts).

Actual (emulator): HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation RebootDbInstance is scaffolded but not implemented in this emulator".

Replay output:
  File "check_timestream_influxdb_create_db_instance_full_lifecycle.py", line 65, in run
    assert code == "ResourceNotFoundException", (
  AssertionError: expected a mutation on the deleted instance to fail with ResourceNotFoundException, got 'NotImplemented'

Preceding steps in the same check all passed against the emulator, which shows the divergence is isolated to RebootDbInstance and not to the instance lifecycle:
  - CreateDbInstance returned status CREATING
  - GetDbInstance polled to AVAILABLE with non-empty endpoint and influxAuthParametersSecretArn
  - DeleteDbInstance drove the instance to DELETED / not-found

Model citation: Smithy RebootDbInstance errors = [AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400]; input identifier: required, @length 3..64, @pattern ^[a-zA-Z0-9]+$. "NotImplemented" is not a member of that set.

## Steps to Reproduce
1. Boot the timestream-influxdb emulator.
2. CreateDbInstance with name=<unique>, password=<random>, dbInstanceType=db.influx.medium, two vpcSubnetIds, one vpcSecurityGroupIds, allocatedStorage=20, dbStorageType=InfluxIOIncludedT1, deploymentType=SINGLE_AZ. Note the returned `id`.
3. Poll GetDbInstance(identifier=<id>) until status == AVAILABLE.
4. Call DeleteDbInstance(identifier=<id>) and poll GetDbInstance until it returns status DELETED or refuses with ResourceNotFoundException.
5. Call RebootDbInstance(identifier=<id>) on that same dead id.
6. Observe HTTP 501 with Error.Code "NotImplemented" and message "operation RebootDbInstance is scaffolded but not implemented in this emulator". Real AWS answers HTTP 404 with Error.Code "ResourceNotFoundException" and Error.Message "The DB instance with db id does not exist".
7. Equivalent: replay timestream-influxdb.create-db-instance-full-lifecycle.
