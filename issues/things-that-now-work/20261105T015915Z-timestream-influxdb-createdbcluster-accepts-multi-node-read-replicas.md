Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: timestream-influxdb.create-db-cluster-reaches-available
Issue-Shape-Id: com.amazonaws.timestreaminfluxdb#AmazonTimestreamInfluxDB
Issue-Key: timestream-influxdb.create-db-cluster-reaches-available|run#8|AssertionError|raise AssertionError(
Issue-Commit: b7ef27c459f4b9552530df460b80da7c4b39527f

# timestream-influxdb: CreateDbCluster accepts MULTI_NODE_READ_REPLICAS without a Read Replicas Marketplace subscription instead of rejecting with ValidationException

> _Note: this issue was identified against the code base at commit `b7ef27c459f4b9552530df460b80da7c4b39527f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDbCluster

## Description
Cause 1 of 1: CreateDbCluster is overly permissive — it creates a DB cluster for the Read Replicas deployment type on an account that holds no AWS Marketplace subscription for Read Replicas, where AWS refuses the call.

AWS: Smithy model services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/smithy-model/com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb.json declares CreateDbCluster with input member `deploymentType` targeting enum `ClusterDeploymentType`, whose ONLY value is `MULTI_NODE_READ_REPLICAS`, and declares `ValidationException` (errorKind client, httpError 400, required members `message` and `reason`) among its errors. The entitlement precondition itself is recorded behavior only: the recording shows that when the calling account carries no active AWS Marketplace subscription for Read Replicas, AWS refuses CreateDbCluster with HTTP 400, error code `ValidationException`, and the message

  "Timestream for InfluxDB didn't find an active AWS Marketplace subscription for Read Replicas associated with the account. You must first create a subscription in Marketplace to access Read replicas availability settings. You might need to wait up to 5 minutes for that subscription to be active."

That entire message is FIXED text for this error class — it carries no variable parts interpolated from the request or from state (no cluster name, id, member name, limit or timestamp appears in it), so it must be emitted verbatim. Because the call is refused, no cluster is created, and a subsequent ListDbClusters must NOT contain an item whose `name` equals the rejected request's `name`.

Emulator: the emulator accepts the request and returns a success response carrying `dbClusterId` (the replay shows a created cluster id, e.g. one 10-character id the check then queued for cleanup) plus `dbClusterStatus`. No ValidationException is raised, no entitlement precondition is evaluated at all, and the cluster is persisted into state so ListDbClusters reports it.

Why: a client that relies on AWS's behavior gets two things wrong. First, error-path code never runs: a caller that catches ValidationException to tell the operator "subscribe to Read Replicas in AWS Marketplace" (the only remediation AWS offers, since MULTI_NODE_READ_REPLICAS is the sole deployment type) is never reached, so the entitlement requirement is invisible in emulation and surfaces for the first time against real AWS. Second, the emulator leaks state AWS never creates: the caller gets a cluster id that does not correspond to any AWS resource, and cleanup/teardown logic built around it (DeleteDbCluster, ListDbClusters reconciliation) diverges, since on AWS ListDbClusters stays empty after the refusal.

Fix: belongs in the CreateDbCluster handler's request validation for this service crate — the entitlement/deployment-type precondition must be checked and the request refused with ValidationException (HTTP 400, the fixed message above, and the modeled required `reason`) BEFORE any cluster record is written to state, so that the state mutation and the ListDbClusters view stay consistent with the refusal.

Traces: behavioral check timestream-influxdb.create-db-cluster-reaches-available (suite group "overly-permissive", tags axis:rejection, concern:negative-classes). The check's positive control CreateDbParameterGroup passes; the assertion fires on the else-branch of the CreateDbCluster try/except, i.e. precisely because the call succeeded instead of being refused. No open issue covers CreateDbCluster: the existing open timestream-influxdb issues cover TagResource, CreateDbParameterGroup, CreateDbBackup, RebootDbInstance and the ValidationException constraint-message wording, none of which touch this operation or this entitlement precondition.

## Evidence
Failing check: timestream-influxdb.create-db-cluster-reaches-available [mismatch]

Replay output:
  File "check_timestream_influxdb_create_db_cluster_reaches_available.py", line 66, in run
    raise AssertionError(
  AssertionError: CreateDbCluster succeeded — this sandbox now carries (or AWS no longer requires) an AWS Marketplace subscription for Read Replicas; the cluster '30f3ab6464' this check created is queued for cleanup, and this check needs to be rewritten back to a happy-path lifecycle assertion

Request shape exercised (deploymentType is the sole modeled enum value):
  CreateDbCluster(name=<generated>, username="sim2realadmin", password=<random hex>,
                  organization="sim2realorg", bucket="sim2real-bucket",
                  dbInstanceType="db.influx.medium", vpcSubnetIds=[<2 subnets>],
                  vpcSecurityGroupIds=[<sg>], allocatedStorage=20,
                  dbStorageType="InfluxIOIncludedT1",
                  deploymentType="MULTI_NODE_READ_REPLICAS", networkType="IPV4",
                  tags={...})

Expected (AWS, recorded): HTTP 400, Error.Code = "ValidationException", Error.Message =
  "Timestream for InfluxDB didn't find an active AWS Marketplace subscription for Read Replicas associated with the account. You must first create a subscription in Marketplace to access Read replicas availability settings. You might need to wait up to 5 minutes for that subscription to be active."
  (entire message is fixed text; no request- or state-derived values)
  and ListDbClusters()["items"] contains NO entry whose name equals the requested name.

Actual (emulator): HTTP 200 with CreateDbClusterOutput{dbClusterId=<id>, dbClusterStatus=<status>}; the cluster is persisted and therefore visible to ListDbClusters. The modeled ValidationException is never emitted.

Model citations:
  CreateDbCluster.deploymentType -> ClusterDeploymentType, enum values = ["MULTI_NODE_READ_REPLICAS"] (only value)
  CreateDbCluster errors include ValidationException (client, httpError 400)
  ValidationException members: message (required), reason (required, ValidationExceptionReason)

## Steps to Reproduce
1. Clone the repo and boot the emulator (the behavioral suite does this).
2. Using a principal/account with NO active AWS Marketplace subscription for Read Replicas, call CreateDbParameterGroup(name=<unique>, description="control") — this succeeds as the positive control.
3. Call CreateDbCluster with the only modeled deployment type:
   aws timestream-influxdb create-db-cluster \
     --name <3-40 chars matching ^[a-zA-z][a-zA-Z0-9]*(-[a-zA-Z0-9]+)*$> \
     --username sim2realadmin --password <8-64 alphanumerics> \
     --organization sim2realorg --bucket sim2real-bucket \
     --db-instance-type db.influx.medium \
     --vpc-subnet-ids <subnet-a> <subnet-b> --vpc-security-group-ids <sg-id> \
     --allocated-storage 20 --db-storage-type InfluxIOIncludedT1 \
     --deployment-type MULTI_NODE_READ_REPLICAS --network-type IPV4
4. Observe the emulator returns 200 with a dbClusterId instead of HTTP 400 ValidationException with the fixed Marketplace-subscription message.
5. Call ListDbClusters and observe the emulator lists a cluster with the requested name; on AWS the list contains no such cluster because the create was refused.

Replay just this check with: timestream-influxdb.create-db-cluster-reaches-available
