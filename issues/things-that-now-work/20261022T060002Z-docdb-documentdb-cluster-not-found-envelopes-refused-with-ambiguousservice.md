Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-cluster-not-found-envelope-variants
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-cluster-not-found-envelope-variants
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|CreateDBClusterSnapshot|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|CreateDBClusterSnapshot|4xx->5xx|$headers.content-type
Issue-Key: docdb|CreateDBClusterSnapshot|4xx->5xx|status
Issue-Key: docdb|StopDBCluster|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|StopDBCluster|4xx->5xx|$headers.content-type
Issue-Key: docdb|StopDBCluster|4xx->5xx|status
Issue-Key: docdb|FailoverDBCluster|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|FailoverDBCluster|4xx->5xx|$headers.content-type
Issue-Key: docdb|FailoverDBCluster|4xx->5xx|status
Issue-Key: docdb|AddTagsToResource|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|AddTagsToResource|4xx->5xx|$headers.content-type
Issue-Key: docdb|AddTagsToResource|4xx->5xx|status
Issue-Key: docdb|DescribeDBClusterSnapshots|2xx->5xx|$.DescribeDBClusterSnapshotsResponse
Issue-Key: docdb|DescribeDBClusterSnapshots|2xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeDBClusterSnapshots|2xx->5xx|status
Issue-Key: docdb|DescribePendingMaintenanceActions|2xx->5xx|$.DescribePendingMaintenanceActionsResponse
Issue-Key: docdb|DescribePendingMaintenanceActions|2xx->5xx|$headers.content-type
Issue-Key: docdb|DescribePendingMaintenanceActions|2xx->5xx|status
Issue-Related: 20261022T060002Z-docdb-documentdb-catalog-and-api-version-query-reads-refused.md
Issue-Related: 20261022T060002Z-docdb-documentdb-cluster-lifecycle-query-calls-refused-with-ambiguousservice.md
Issue-Commit: b65dcc161724406fef2f9858d8de1b033f95eeb7

# DocumentDB cluster not-found envelopes refused with AmbiguousService 500 JSON

> _Note: this issue was identified against the code base at commit `b65dcc161724406fef2f9858d8de1b033f95eeb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusters

## Description
8 causes in this bundle; all 8 must be closed.

SHARED ROOT CAUSE (applies to every cause below)
AWS: DocumentDB is served on the RDS front door — its model declares awsService.endpointPrefix = 'rds' and sigv4Name = 'rds' (Smithy: com-amazonaws-docdb-amazonrdsv19, AmazonRDSv19 2014-10-31, protocol awsQuery, xmlNamespace http://rds.amazonaws.com/doc/2014-10-31/), as do RDS and Neptune. A missing cluster produces a 404 text/xml <ErrorResponse><Error><Type>Sender</Type><Code>DBClusterNotFoundFault</Code> (Smithy: DBClusterNotFoundFault, awsQueryError code DBClusterNotFoundFault, httpError 404); a list read over nothing produces 200 with an empty result element. 'AmbiguousService' is not an AWS error code.
Emulator: the router refuses before dispatch with HTTP 500, content-type application/x-amz-json-1.1, body {"__type":"AmbiguousService","message":"3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}; the collision is between service-com-amazonaws-rds-amazonrdsv19, service-com-amazonaws-docdb-amazonrdsv19 and service-com-amazonaws-neptune-amazonrdsv19. Two compounding faults: (a) nothing is served, (b) the refusal is JSON for an awsQuery client.
Why: a client cannot distinguish 'cluster does not exist' from a server fault — it gets 500 and, in botocore, a ResponseParserError rather than a ClientError with code DBClusterNotFoundFault, so retry/except paths keyed on the modeled exception never fire.
Fix: the request router's service-resolution table AND its error serializer — resolve 'rds' deterministically to one handler for the shared Query surface (disambiguate on the Query body's Action/Version) and serialize router-level refusals for awsQuery services as text/xml <ErrorResponse>.
Related: open issue 20261022T035032Z-rds-every-rds-rds-request-is-refused-with-ambiguousservice records the same collision from the behavioral corpus with no wire fixture key, so it covers none of the steps below.

Cause 1 of 8: DescribeDBClusters for a missing cluster (fixture docdb-cluster-not-found-envelope-variants, step task#1). AWS: 404 text/xml ErrorResponse, Error/Code=DBClusterNotFoundFault. Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 2 of 8: DeleteDBCluster for a missing cluster (same fixture, step task#2). AWS: 404 text/xml ErrorResponse, Error/Code=DBClusterNotFoundFault (recorded message variant 'The source cluster could not be found or cannot be accessed: <id>' is an example). Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 3 of 8: CreateDBClusterSnapshot of a missing cluster (same fixture, step task#3). AWS: 404 text/xml ErrorResponse, Error/Code=DBClusterNotFoundFault. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 4 of 8: StopDBCluster on a missing cluster (same fixture, step task#4). AWS: 404 text/xml ErrorResponse, Error/Code=DBClusterNotFoundFault. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 5 of 8: FailoverDBCluster on a missing cluster (same fixture, step task#5). AWS: 404 text/xml ErrorResponse, Error/Code=DBClusterNotFoundFault. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 6 of 8: AddTagsToResource on a missing cluster ARN (same fixture, step task#6). AWS: 404 text/xml ErrorResponse, Error/Code=DBClusterNotFoundFault (recorded message names the ARN). Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 7 of 8: DescribeDBClusterSnapshots filtered by a missing cluster (same fixture, step task#7). AWS: 200 text/xml <DescribeDBClusterSnapshotsResponse> with an EMPTY DBClusterSnapshots element — a filter that matches nothing is not an error (Smithy: DescribeDBClusterSnapshots -> DBClusterSnapshotMessage). Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 8 of 8: DescribePendingMaintenanceActions filtered by a missing resource (same fixture, step task#8). AWS: 200 text/xml <DescribePendingMaintenanceActionsResponse> with an EMPTY PendingMaintenanceActions element. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Traces: docdb-cluster-not-found-envelope-variants#task#1 .. #task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1. Representative step docdb-cluster-not-found-envelope-variants#task#1 (DescribeDBClusters, missing cluster):
  expected: status 404, content-type text/xml, {'ErrorResponse': {'Error': {'Type': 'Sender', 'Code': 'DBClusterNotFoundFault', 'Message': 'DBCluster <id> not found.'}}}
  actual:   status 500, content-type application/x-amz-json-1.1, $.ErrorResponse missing
Steps task#2..task#6 are identical in shape (expected 404 + Error/Code=DBClusterNotFoundFault for DeleteDBCluster, CreateDBClusterSnapshot, StopDBCluster, FailoverDBCluster, AddTagsToResource). Steps task#7 and task#8 expected status 200 text/xml with empty DBClusterSnapshots / PendingMaintenanceActions and also got 500 + application/x-amz-json-1.1.
The body served on these steps is the router refusal, captured verbatim elsewhere in the same corpus: {'__type': 'AmbiguousService', 'message': "3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. docdb = boto3.client('docdb', endpoint_url=<emulator>, region_name='us-west-2')
3. docdb.describe_db_clusters(DBClusterIdentifier='no-such-cluster') -> expected 404 text/xml ErrorResponse/Error/Code=DBClusterNotFoundFault; actual 500, content-type application/x-amz-json-1.1, {"__type":"AmbiguousService",...}.
4. Repeat with delete_db_cluster, create_db_cluster_snapshot, stop_db_cluster, failover_db_cluster, add_tags_to_resource on the same missing cluster — all expected 404 DBClusterNotFoundFault, all give the same 500 refusal.
5. docdb.describe_db_cluster_snapshots(DBClusterIdentifier='no-such-cluster') and describe_pending_maintenance_actions(...) -> expected 200 with an empty list; same 500 refusal.
6. Replay the fixture steps listed under Traces to confirm.
