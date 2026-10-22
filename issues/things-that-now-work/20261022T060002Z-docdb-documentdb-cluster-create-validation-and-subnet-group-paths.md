Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-cluster-create-validation-errors
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-cluster-create-validation-errors docdb-cluster-lifecycle docdb-db-subnet-group-lifecycle docdb-cluster-parameter-group-lifecycle
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|CreateDBCluster|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|CreateDBCluster|4xx->5xx|$headers.content-type
Issue-Key: docdb|CreateDBCluster|4xx->5xx|status
Issue-Key: docdb|DescribeDBSubnetGroups|2xx->5xx|$.DescribeDBSubnetGroupsResponse
Issue-Key: docdb|DescribeDBSubnetGroups|2xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeDBSubnetGroups|2xx->5xx|status
Issue-Key: docdb|DeleteDBSubnetGroup|2xx->5xx|status
Issue-Key: docdb|DescribeDBSubnetGroups|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DescribeDBSubnetGroups|4xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeDBSubnetGroups|4xx->5xx|status
Issue-Key: docdb|DeleteDBSubnetGroup|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DeleteDBSubnetGroup|4xx->5xx|$headers.content-type
Issue-Key: docdb|DeleteDBSubnetGroup|4xx->5xx|status
Issue-Key: docdb|DescribeDBSubnetGroups|4xx->5xx|$
Issue-Key: docdb|DescribeDBClusterParameterGroups|4xx->5xx|$
Issue-Key: docdb|DescribeDBClusterParameterGroups|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DescribeDBClusterParameterGroups|4xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeDBClusterParameterGroups|4xx->5xx|status
Issue-Commit: b65dcc161724406fef2f9858d8de1b033f95eeb7

# DocumentDB cluster-create validation and subnet-group paths refused with AmbiguousService 500

> _Note: this issue was identified against the code base at commit `b65dcc161724406fef2f9858d8de1b033f95eeb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBSubnetGroups

## Description
8 causes in this bundle; all 8 must be closed.

SHARED ROOT CAUSE (applies to every cause below)
AWS: DocumentDB is served on the RDS front door — its model declares awsService.endpointPrefix = 'rds' and sigv4Name = 'rds' (Smithy: com-amazonaws-docdb-amazonrdsv19, AmazonRDSv19 2014-10-31, protocol awsQuery, xmlNamespace http://rds.amazonaws.com/doc/2014-10-31/), as do RDS and Neptune. Responses are 200 text/xml <...Response> envelopes or modeled awsQuery errors as text/xml <ErrorResponse><Error><Type>Sender</Type><Code>..</Code>. 'AmbiguousService' is not an AWS error code.
Emulator: the router refuses before dispatch with HTTP 500, content-type application/x-amz-json-1.1, body {"__type":"AmbiguousService","message":"3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}; the collision is between service-com-amazonaws-rds-amazonrdsv19, service-com-amazonaws-docdb-amazonrdsv19 and service-com-amazonaws-neptune-amazonrdsv19. Two compounding faults: (a) nothing is served, (b) the refusal is JSON for an awsQuery client.
Why: no DocumentDB subnet-group or cluster-create validation behaviour can be observed; the caller gets a botocore ResponseParserError instead of a typed error, so every guard the fixtures assert is invisible.
Fix: the request router's service-resolution table AND its error serializer — resolve 'rds' deterministically to one handler for the shared Query surface (disambiguate on the Query body's Action/Version), and serialize router-level refusals for awsQuery services as text/xml <ErrorResponse>.
Related: open issue 20261022T035032Z-rds-every-rds-rds-request-is-refused-with-ambiguousservice records the same collision from the behavioral corpus with no wire fixture key, so it covers none of the steps below.
Note: fixture docdb-cluster-create-validation-errors also has a blocked ec2 CreateVpc setup step whose issue is already filed against the ec2 stub; it is not part of this bundle.

Cause 1 of 8: CreateDBCluster naming a non-existent DB subnet group (fixture docdb-cluster-create-validation-errors, step task#6). AWS: 404 text/xml <ErrorResponse> Error/Type=Sender, Error/Code=DBSubnetGroupNotFoundFault (Smithy: CreateDBCluster error DBSubnetGroupNotFoundFault, awsQueryError code DBSubnetGroupNotFoundFault, httpError 404); recorded message naming the missing group is an example. Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 2 of 8: CreateDBCluster with an unknown EngineVersion (fixture docdb-cluster-create-validation-errors, step task#7; cluster also covers that fixture's task#8, task#9, task#10 and docdb-cluster-lifecycle#task#11). AWS: 400 text/xml <ErrorResponse> Error/Type=Sender, Error/Code=InvalidParameterCombination, message built from the request's version and engine (recorded example: 'Cannot find version 99.9.9 for docdb'). Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 3 of 8: DescribeDBSubnetGroups for an existing group (fixture docdb-cluster-create-validation-errors, step task#11; cluster also covers docdb-db-subnet-group-lifecycle#task#6). AWS: 200 text/xml <DescribeDBSubnetGroupsResponse> with DBSubnetGroups/DBSubnetGroup (Smithy: DescribeDBSubnetGroups -> DBSubnetGroupMessage). Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 4 of 8: DeleteDBSubnetGroup teardown (fixture docdb-cluster-create-validation-errors, step teardown#12; cluster also covers docdb-cluster-lifecycle#teardown#16 and docdb-db-subnet-group-lifecycle#teardown#14). AWS: 200 text/xml <DeleteDBSubnetGroupResponse>. Emulator: 500. Why/Fix: shared root cause above.

Cause 5 of 8: DescribeDBSubnetGroups for a missing group (fixture docdb-db-subnet-group-lifecycle, step task#12). AWS: 404 text/xml <ErrorResponse> Error/Type=Sender, Error/Code=DBSubnetGroupNotFoundFault (Smithy: awsQueryError code DBSubnetGroupNotFoundFault, httpError 404). Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 6 of 8: DeleteDBSubnetGroup for a missing group (fixture docdb-db-subnet-group-lifecycle, step task#13). AWS: 404 text/xml <ErrorResponse> Error/Code=DBSubnetGroupNotFoundFault. Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 7 of 8: polling DescribeDBSubnetGroups until the deleted group reports DBSubnetGroupNotFoundFault (fixture docdb-db-subnet-group-lifecycle, step teardown#15). AWS: 404 text/xml <ErrorResponse> Error/Code=DBSubnetGroupNotFoundFault. Emulator: poll ends after 1 attempt with 500 AmbiguousService and can never satisfy until_error, so waiter semantics break too. Why/Fix: shared root cause above.

Cause 8 of 8: polling DescribeDBClusterParameterGroups until the deleted group reports DBParameterGroupNotFound (fixture docdb-cluster-parameter-group-lifecycle, step teardown#14). AWS: 404 text/xml <ErrorResponse> Error/Type=Sender, Error/Code=DBParameterGroupNotFound (Smithy: DescribeDBClusterParameterGroups error DBParameterGroupNotFoundFault, awsQueryError code DBParameterGroupNotFound, httpError 404). Emulator: poll ends after 1 attempt with 500 AmbiguousService. Why/Fix: shared root cause above.

Traces: docdb-cluster-create-validation-errors#task#6, #task#7, #task#11, #teardown#12; docdb-db-subnet-group-lifecycle#task#12, #task#13, #teardown#15; docdb-cluster-parameter-group-lifecycle#teardown#14.

## Evidence
Replay of trace set simparity-wire-aws-v1. Representative step docdb-cluster-create-validation-errors#task#6 (CreateDBCluster with a missing subnet group):
  expected: status 404, content-type text/xml, {'ErrorResponse': {'Error': {'Type': 'Sender', 'Code': 'DBSubnetGroupNotFoundFault', 'Message': "DB subnet group '<name>' does not exist."}}}
  actual:   status 500, content-type application/x-amz-json-1.1, $.ErrorResponse missing
The body is the router refusal, captured verbatim on docdb-db-subnet-group-lifecycle#teardown#15:
  {'__type': 'AmbiguousService', 'message': "3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}
  plus 'poll ended: 500 AmbiguousService cannot satisfy until_error [DBSubnetGroupNotFoundFault] after 1 attempt(s)' (same shape on docdb-cluster-parameter-group-lifecycle#teardown#14 for DBParameterGroupNotFound).
Other causes: expected 400 ErrorResponse/Error/Code=InvalidParameterCombination (task#7) and expected 200 text/xml DescribeDBSubnetGroupsResponse / DeleteDBSubnetGroupResponse; actual 500 + application/x-amz-json-1.1 in every case.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. docdb = boto3.client('docdb', endpoint_url=<emulator>, region_name='us-west-2')
3. docdb.describe_db_subnet_groups(DBSubnetGroupName='no-such-group') -> expected 404 text/xml ErrorResponse/Error/Code=DBSubnetGroupNotFoundFault; actual 500, content-type application/x-amz-json-1.1, {"__type":"AmbiguousService",...}.
4. docdb.create_db_cluster(..., DBSubnetGroupName='no-such-group') -> expected 404 DBSubnetGroupNotFoundFault; same 500 refusal. With EngineVersion='99.9.9' -> expected 400 InvalidParameterCombination; same 500 refusal.
5. docdb.delete_db_subnet_group(DBSubnetGroupName=<existing>) -> expected 200; same 500 refusal.
6. Replay the fixture steps listed under Traces to confirm.
