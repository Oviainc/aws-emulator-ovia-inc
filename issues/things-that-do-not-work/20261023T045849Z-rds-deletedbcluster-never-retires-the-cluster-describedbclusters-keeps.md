Issue-Source: trace-sweep-divergence
Issue-Trace: rdsdata-execute-statement-field-union-and-error-envelopes
Issue-Step: teardown#33
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rdsdata-execute-statement-field-union-and-error-envelopes
Issue-Shape-Id: com.amazonaws.rdsdata#RdsDataService
Issue-Key: rds|DescribeDBClusters|4xx->2xx|$
Issue-Key: rds|DescribeDBClusters|4xx->2xx|status
Issue-Commit: 0f903c65ef17614b948ed7843da2bc7f944d5d1a

# rds DeleteDBCluster never retires the cluster: DescribeDBClusters keeps answering 200 available

> _Note: this issue was identified against the code base at commit `0f903c65ef17614b948ed7843da2bc7f944d5d1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusters

## Description
Cause 1 of 1: after the fixture's teardown deletes the Aurora cluster, DescribeDBClusters on that identifier keeps returning the cluster, so the deletion never completes from the client's point of view.
AWS: Smithy: DescribeDBClusters models DBClusterNotFoundFault (errorKind client, httpError 404, awsQueryError code DBClusterNotFoundFault), and DeleteDBCluster returns the cluster in 'deleting' state and then removes it. Once the cluster is gone, DescribeDBClusters with that DBClusterIdentifier answers HTTP 404 with Error.Code DBClusterNotFoundFault - which is exactly what the recorded teardown polls for.
Emulator: answers HTTP 200 for the full 60s poll window, still describing the cluster with Status 'available' (and HttpEndpointEnabled 'true'), i.e. the delete left the cluster record untouched in state.
Why: a client that deletes a cluster and waits for the standard 'cluster gone' signal - DescribeDBClusters returning DBClusterNotFoundFault, which is how the AWS waiters are written - waits forever; teardown/cleanup code never terminates, and a re-create of the same identifier would collide with a cluster that should no longer exist.
Fix: the DeleteDBCluster handler and the rds cluster state store - the delete must transition the cluster out of 'available' and remove it, and DescribeDBClusters must then take its DBClusterNotFoundFault path. The open issue 'RDS omits state and existence preconditions and misclassifies the resulting errors' covers snapshot/subnet-group/maintenance preconditions and does NOT cover cluster deletion, and 'Implement rds CreateDBCluster: unimplemented scaffold stub' is about creation, not the delete/describe lifecycle.
Traces: rdsdata-execute-statement-field-union-and-error-envelopes#teardown#33.

## Evidence
rdsdata-execute-statement-field-union-and-error-envelopes#teardown#33, Action=DescribeDBClusters: status expected 404, got 200; the poll was capped at 60s with the emulator returning DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster for the deleted identifier with Status 'available', Engine 'aurora-postgresql', HttpEndpointEnabled 'true' - expected instead a 404 ErrorResponse with Error.Code DBClusterNotFoundFault and no DBCluster.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay rdsdata-execute-statement-field-union-and-error-envelopes through its teardown: Action=DeleteDBCluster for the fixture's cluster, then poll Action=DescribeDBClusters with that DBClusterIdentifier.
3. The poll never sees 404/DBClusterNotFoundFault: every attempt returns HTTP 200 with the cluster still present and Status 'available', and the step is capped at 60s.
