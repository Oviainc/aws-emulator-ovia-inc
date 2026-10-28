Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-cluster-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-cluster-lifecycle docdb-db-subnet-group-lifecycle
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|CreateDBCluster|2xx->5xx|$.CreateDBClusterResponse
Issue-Key: docdb|CreateDBCluster|2xx->5xx|$headers.content-type
Issue-Key: docdb|CreateDBCluster|2xx->5xx|status
Issue-Key: docdb|DescribeDBClusters|2xx->5xx|$
Issue-Key: docdb|DescribeDBClusters|2xx->5xx|$.DescribeDBClustersResponse
Issue-Key: docdb|DescribeDBClusters|2xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeDBClusters|2xx->5xx|status
Issue-Key: docdb|AddTagsToResource|2xx->5xx|$.AddTagsToResourceResponse
Issue-Key: docdb|AddTagsToResource|2xx->5xx|$headers.content-type
Issue-Key: docdb|AddTagsToResource|2xx->5xx|status
Issue-Key: docdb|ListTagsForResource|2xx->5xx|$.ListTagsForResourceResponse
Issue-Key: docdb|ListTagsForResource|2xx->5xx|$headers.content-type
Issue-Key: docdb|ListTagsForResource|2xx->5xx|status
Issue-Key: docdb|DeleteDBCluster|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DeleteDBCluster|4xx->5xx|$headers.content-type
Issue-Key: docdb|DeleteDBCluster|4xx->5xx|status
Issue-Key: docdb|ModifyDBCluster|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|ModifyDBCluster|4xx->5xx|$headers.content-type
Issue-Key: docdb|ModifyDBCluster|4xx->5xx|status
Issue-Key: docdb|DeleteDBCluster|2xx->5xx|status
Issue-Key: docdb|DescribeDBClusters|4xx->5xx|$
Issue-Related: 20261022T060002Z-docdb-documentdb-catalog-and-api-version-query-reads-refused.md
Issue-Commit: b65dcc161724406fef2f9858d8de1b033f95eeb7

# DocumentDB cluster lifecycle Query calls refused with AmbiguousService 500 JSON

> _Note: this issue was identified against the code base at commit `b65dcc161724406fef2f9858d8de1b033f95eeb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
8 causes in this bundle; all 8 must be closed.

SHARED ROOT CAUSE (applies to every cause below)
AWS: DocumentDB is served on the RDS front door — its model declares awsService.endpointPrefix = 'rds' and sigv4Name = 'rds' (Smithy: com-amazonaws-docdb-amazonrdsv19, AmazonRDSv19 2014-10-31, protocol awsQuery, xmlNamespace http://rds.amazonaws.com/doc/2014-10-31/), as do RDS and Neptune. AWS answers 200 with a text/xml <...Response> envelope or a modeled awsQuery error as text/xml <ErrorResponse><Error><Type>Sender</Type><Code>..</Code>. 'AmbiguousService' is not an AWS error code and exists in no AWS model.
Emulator: the router rejects the request before dispatch with HTTP 500, content-type application/x-amz-json-1.1 and body {"__type":"AmbiguousService","message":"3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"} — the colliding registrations are service-com-amazonaws-rds-amazonrdsv19, service-com-amazonaws-docdb-amazonrdsv19 and service-com-amazonaws-neptune-amazonrdsv19. Two compounding faults: (a) nothing is served, (b) the refusal is JSON for an awsQuery client.
Why: the whole DocumentDB cluster lifecycle is unavailable, and because the refusal is JSON a botocore client raises ResponseParserError instead of a ClientError, so no error code reaches the caller. Waiters that poll for a terminal state never terminate (see Cause 8).
Fix: the request router's service-resolution table AND its error serializer — resolve 'rds' deterministically to one handler for the RDS/DocDB/Neptune Query surface (disambiguate on the Query body's Action/Version) and serialize router-level refusals for awsQuery services as text/xml <ErrorResponse>.
Related: open issue 20261022T035032Z-rds-every-rds-rds-request-is-refused-with-ambiguousservice records the same collision from the behavioral corpus and carries no wire fixture key, so it covers none of the steps below.

Cause 1 of 8: CreateDBCluster happy path (fixture docdb-cluster-lifecycle, step task#7). AWS: 200 text/xml <CreateDBClusterResponse> with CreateDBClusterResult/DBCluster (Smithy: CreateDBCluster -> CreateDBClusterResult; required input DBClusterIdentifier, Engine). Emulator: 500 application/x-amz-json-1.1, no CreateDBClusterResponse element. Why/Fix: shared root cause above.

Cause 2 of 8: DescribeDBClusters on the created cluster (fixture docdb-cluster-lifecycle, step task#8). AWS: 200 text/xml <DescribeDBClustersResponse> with DBClusters/DBCluster (Smithy: DescribeDBClusters -> DBClusterMessage). Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 3 of 8: AddTagsToResource on the cluster ARN (fixture docdb-cluster-lifecycle, step task#9). AWS: 200 text/xml <AddTagsToResourceResponse> with only ResponseMetadata. Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 4 of 8: ListTagsForResource (fixture docdb-cluster-lifecycle, step task#10; cluster also covers docdb-db-subnet-group-lifecycle#task#11). AWS: 200 text/xml <ListTagsForResourceResponse> with TagList/Tag entries. Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 5 of 8: DeleteDBCluster without FinalDBSnapshotIdentifier or SkipFinalSnapshot (fixture docdb-cluster-lifecycle, step task#12). AWS: 400 text/xml <ErrorResponse> Error/Type=Sender, Error/Code=InvalidParameterCombination; the rule is 'exactly one of FinalDBSnapshotIdentifier / SkipFinalSnapshot must be given' (recorded message 'Please specify FinalDBSnapshotIdentifier or SkipFinalSnapshot' is an example of that rule). Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 6 of 8: ModifyDBCluster with a too-short MasterUserPassword (fixture docdb-cluster-lifecycle, step task#13). AWS: 400 text/xml <ErrorResponse> Error/Type=Sender, Error/Code=InvalidParameterValue, message built from the request naming the member and the violated constraint (recorded example: 'The parameter MasterUserPassword is not a valid password because it is shorter than 8 characters.'). Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above; belongs in the operation's validation once routing reaches it.

Cause 7 of 8: DeleteDBCluster teardown (fixture docdb-cluster-lifecycle, step teardown#14). AWS: 200 text/xml <DeleteDBClusterResponse>. Emulator: 500. Why/Fix: shared root cause above.

Cause 8 of 8: polling DescribeDBClusters until the deleted cluster reports DBClusterNotFoundFault (fixture docdb-cluster-lifecycle, step teardown#15). AWS: 404 text/xml <ErrorResponse> Error/Type=Sender, Error/Code=DBClusterNotFoundFault (Smithy: DescribeDBClusters error DBClusterNotFoundFault, awsQueryError code DBClusterNotFoundFault, httpError 404). Emulator: the poll ends after 1 attempt with 500 AmbiguousService and can never satisfy until_error, i.e. the refusal also breaks waiter semantics. Why/Fix: shared root cause above.

Traces: docdb-cluster-lifecycle#task#7, #task#8, #task#9, #task#10, #task#12, #task#13, #teardown#14, #teardown#15.

## Evidence
Replay of trace set simparity-wire-aws-v1. Representative step docdb-cluster-lifecycle#task#7 (CreateDBCluster):
  expected: status 200, content-type text/xml, $.CreateDBClusterResponse.CreateDBClusterResult.DBCluster present
  actual:   status 500, content-type application/x-amz-json-1.1, $.CreateDBClusterResponse missing
Step docdb-cluster-lifecycle#teardown#15 captured the body verbatim:
  expected {'ErrorResponse': {'Error': {'Type': 'Sender', 'Code': 'DBClusterNotFoundFault', ...}}}
  actual   {'__type': 'AmbiguousService', 'message': "3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}
  and: 'poll ended: 500 AmbiguousService cannot satisfy until_error [DBClusterNotFoundFault] after 1 attempt(s)'
Other causes: expected 200 text/xml for DescribeDBClusters/AddTagsToResource/ListTagsForResource/DeleteDBCluster and expected 400 text/xml ErrorResponse with Error/Code InvalidParameterCombination (task#12) and InvalidParameterValue (task#13); actual 500 + application/x-amz-json-1.1 in every case.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. docdb = boto3.client('docdb', endpoint_url=<emulator>, region_name='us-west-2')
3. docdb.create_db_cluster(DBClusterIdentifier='probe', Engine='docdb', MasterUsername='probeadmin', MasterUserPassword='<8+ chars>', DBSubnetGroupName=<group>) -> expected 200 text/xml CreateDBClusterResponse; actual 500 with {"__type":"AmbiguousService",...} (botocore: ResponseParserError).
4. describe_db_clusters / add_tags_to_resource / list_tags_for_resource / delete_db_cluster behave identically.
5. delete_db_cluster(DBClusterIdentifier='probe') with neither SkipFinalSnapshot nor FinalDBSnapshotIdentifier -> expected 400 ErrorResponse/Error/Code=InvalidParameterCombination; actual the same 500 JSON refusal.
6. Replay the fixture steps listed under Traces to confirm.
