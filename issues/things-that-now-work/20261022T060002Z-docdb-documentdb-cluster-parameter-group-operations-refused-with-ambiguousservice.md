Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-cluster-parameter-group-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-cluster-parameter-group-lifecycle
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|CreateDBClusterParameterGroup|2xx->5xx|$.CreateDBClusterParameterGroupResponse
Issue-Key: docdb|CreateDBClusterParameterGroup|2xx->5xx|$headers.content-type
Issue-Key: docdb|CreateDBClusterParameterGroup|2xx->5xx|status
Issue-Key: docdb|DescribeDBClusterParameterGroups|2xx->5xx|$.DescribeDBClusterParameterGroupsResponse
Issue-Key: docdb|DescribeDBClusterParameterGroups|2xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeDBClusterParameterGroups|2xx->5xx|status
Issue-Key: docdb|CreateDBClusterParameterGroup|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|CreateDBClusterParameterGroup|4xx->5xx|$headers.content-type
Issue-Key: docdb|CreateDBClusterParameterGroup|4xx->5xx|status
Issue-Key: docdb|DescribeDBClusterParameters|2xx->5xx|$.DescribeDBClusterParametersResponse
Issue-Key: docdb|DescribeDBClusterParameters|2xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeDBClusterParameters|2xx->5xx|status
Issue-Key: docdb|ModifyDBClusterParameterGroup|2xx->5xx|$.ModifyDBClusterParameterGroupResponse
Issue-Key: docdb|ModifyDBClusterParameterGroup|2xx->5xx|$headers.content-type
Issue-Key: docdb|ModifyDBClusterParameterGroup|2xx->5xx|status
Issue-Key: docdb|ModifyDBClusterParameterGroup|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|ModifyDBClusterParameterGroup|4xx->5xx|$headers.content-type
Issue-Key: docdb|ModifyDBClusterParameterGroup|4xx->5xx|status
Issue-Key: docdb|ResetDBClusterParameterGroup|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|ResetDBClusterParameterGroup|4xx->5xx|$headers.content-type
Issue-Key: docdb|ResetDBClusterParameterGroup|4xx->5xx|status
Issue-Key: docdb|DescribeDBClusterParameters|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DescribeDBClusterParameters|4xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeDBClusterParameters|4xx->5xx|status
Issue-Key: docdb|DeleteDBClusterParameterGroup|2xx->5xx|status
Issue-Related: 20261022T060002Z-docdb-documentdb-cluster-create-validation-and-subnet-group-paths.md
Issue-Commit: b65dcc161724406fef2f9858d8de1b033f95eeb7

# DocumentDB cluster-parameter-group operations refused with AmbiguousService 500 JSON

> _Note: this issue was identified against the code base at commit `b65dcc161724406fef2f9858d8de1b033f95eeb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBClusterParameterGroup

## Description
10 causes in this bundle; all 10 must be closed.

SHARED ROOT CAUSE (applies to every cause below)
AWS: DocumentDB is served on the RDS front door — its model declares awsService.endpointPrefix = 'rds' and sigv4Name = 'rds' (Smithy: com-amazonaws-docdb-amazonrdsv19, AmazonRDSv19 2014-10-31, protocol awsQuery, xmlNamespace http://rds.amazonaws.com/doc/2014-10-31/), as do RDS and Neptune. Responses are 200 text/xml <...Response> envelopes or modeled awsQuery errors as text/xml <ErrorResponse><Error><Type>Sender</Type><Code>..</Code>. 'AmbiguousService' is not an AWS error code.
Emulator: the router refuses before dispatch with HTTP 500, content-type application/x-amz-json-1.1, body {"__type":"AmbiguousService","message":"3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}; the collision is between service-com-amazonaws-rds-amazonrdsv19, service-com-amazonaws-docdb-amazonrdsv19 and service-com-amazonaws-neptune-amazonrdsv19. Two compounding faults: (a) nothing is served, (b) the refusal is JSON for an awsQuery client.
Why: cluster parameter groups cannot be created, read, modified, reset or deleted, and the JSON refusal makes botocore raise ResponseParserError instead of a typed ClientError, so no caller can branch on DBParameterGroupAlreadyExists / DBParameterGroupNotFound / InvalidDBParameterGroupState.
Fix: the request router's service-resolution table AND its error serializer — resolve 'rds' deterministically to one handler for the shared Query surface (disambiguate on the Query body's Action/Version) and serialize router-level refusals for awsQuery services as text/xml <ErrorResponse>.
Related: open issue 20261022T035032Z-rds-every-rds-rds-request-is-refused-with-ambiguousservice records the same collision from the behavioral corpus with no wire fixture key, so it covers none of the steps below.

Cause 1 of 10: CreateDBClusterParameterGroup happy path (fixture docdb-cluster-parameter-group-lifecycle, step task#1). AWS: 200 text/xml <CreateDBClusterParameterGroupResponse> with DBClusterParameterGroup (name, family, description, ARN). Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 2 of 10: DescribeDBClusterParameterGroups for the created group (same fixture, step task#2). AWS: 200 text/xml <DescribeDBClusterParameterGroupsResponse> with DBClusterParameterGroups/DBClusterParameterGroup (Smithy: -> DBClusterParameterGroupsMessage). Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 3 of 10: CreateDBClusterParameterGroup for a name that already exists (same fixture, step task#3; cluster also covers task#10). AWS: 400 text/xml ErrorResponse, Error/Type=Sender, Error/Code=DBParameterGroupAlreadyExists; message built from the requested group name (recorded text is an example). Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 4 of 10: DescribeDBClusterParameters on the group (same fixture, step task#4; cluster also covers task#6). AWS: 200 text/xml <DescribeDBClusterParametersResponse> with a Parameters element (empty for a freshly created group). Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 5 of 10: ModifyDBClusterParameterGroup with a valid parameter value (same fixture, step task#5). AWS: 200 text/xml <ModifyDBClusterParameterGroupResponse> echoing DBClusterParameterGroupName. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 6 of 10: ModifyDBClusterParameterGroup with an out-of-range parameter value (same fixture, step task#7; cluster also covers task#8). AWS: 400 text/xml ErrorResponse, Error/Type=Sender, Error/Code=InvalidParameterValue; message built from the request naming the parameter and its allowed values (recorded example concerns 'tls'). Emulator: 500 AmbiguousService. Why/Fix: shared root cause above; the guard itself belongs in the operation's validation once routing reaches it.

Cause 7 of 10: ResetDBClusterParameterGroup while a parameter has pending changes (same fixture, step task#9). AWS: 400 text/xml ErrorResponse, Error/Type=Sender, Error/Code=InvalidDBParameterGroupState (recorded message names the pending parameter). Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 8 of 10: DescribeDBClusterParameterGroups for a missing group (same fixture, step task#11). AWS: 404 text/xml ErrorResponse, Error/Code=DBParameterGroupNotFound (Smithy: DescribeDBClusterParameterGroups error DBParameterGroupNotFoundFault, awsQueryError code DBParameterGroupNotFound, httpError 404). Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 9 of 10: DescribeDBClusterParameters for a missing group (same fixture, step task#12). AWS: 404 text/xml ErrorResponse, Error/Code=DBParameterGroupNotFound. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 10 of 10: DeleteDBClusterParameterGroup teardown (same fixture, step teardown#13). AWS: 200 text/xml <DeleteDBClusterParameterGroupResponse>. Emulator: 500. Why/Fix: shared root cause above.

(The same fixture's teardown#14 poll is the 8th cause of the companion issue 'DocumentDB cluster-create validation and subnet-group paths refused with AmbiguousService 500' and is tracked there.)

Traces: docdb-cluster-parameter-group-lifecycle#task#1, #task#2, #task#3, #task#4, #task#5, #task#7, #task#9, #task#11, #task#12, #teardown#13.

## Evidence
Replay of trace set simparity-wire-aws-v1. Representative step docdb-cluster-parameter-group-lifecycle#task#1 (CreateDBClusterParameterGroup):
  expected: status 200, content-type text/xml, $.CreateDBClusterParameterGroupResponse.CreateDBClusterParameterGroupResult.DBClusterParameterGroup present (family docdb5.0)
  actual:   status 500, content-type application/x-amz-json-1.1, $.CreateDBClusterParameterGroupResponse missing
Error-path steps diverge the same way: expected 400 text/xml ErrorResponse with Error/Code DBParameterGroupAlreadyExists (task#3), InvalidParameterValue (task#7), InvalidDBParameterGroupState (task#9); expected 404 with Error/Code DBParameterGroupNotFound (task#11, task#12); actual 500 + application/x-amz-json-1.1 in every case.
The body served is the router refusal, captured verbatim on this fixture's teardown#14: {'__type': 'AmbiguousService', 'message': "3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. docdb = boto3.client('docdb', endpoint_url=<emulator>, region_name='us-west-2')
3. docdb.create_db_cluster_parameter_group(DBClusterParameterGroupName='probe-cpg', DBParameterGroupFamily='docdb5.0', Description='probe') -> expected 200 text/xml CreateDBClusterParameterGroupResponse; actual 500, content-type application/x-amz-json-1.1, {"__type":"AmbiguousService",...}.
4. Repeat the call -> expected 400 ErrorResponse/Error/Code=DBParameterGroupAlreadyExists; same 500 refusal.
5. describe_db_cluster_parameter_groups / describe_db_cluster_parameters / modify_db_cluster_parameter_group / reset_db_cluster_parameter_group / delete_db_cluster_parameter_group -> all give the same 500 refusal instead of their 200 or modeled 400/404.
6. Replay the fixture steps listed under Traces to confirm.
