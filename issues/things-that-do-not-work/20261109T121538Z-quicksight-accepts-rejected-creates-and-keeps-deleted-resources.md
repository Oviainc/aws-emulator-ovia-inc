Issue-Source: trace-sweep-divergence
Issue-Trace: quicksight-data-source-creation-rejected-unroutable-host-and-mismatched-parameters
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: quicksight-data-source-creation-rejected-unroutable-host-and-mismatched-parameters quicksight-folder-lifecycle-and-duplicate-rejected quicksight-group-lifecycle-and-duplicate-rejected
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight|CreateDataSource|4xx->2xx|$.Message
Issue-Key: quicksight|CreateDataSource|4xx->2xx|$.RequestId
Issue-Key: quicksight|CreateDataSource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: quicksight|CreateDataSource|4xx->2xx|status
Issue-Key: quicksight|DescribeDataSource|4xx->2xx|$.Message
Issue-Key: quicksight|DescribeDataSource|4xx->2xx|$.RequestId
Issue-Key: quicksight|DescribeDataSource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: quicksight|DescribeDataSource|4xx->2xx|status
Issue-Key: quicksight|DescribeFolder|4xx->2xx|$.Message
Issue-Key: quicksight|DescribeFolder|4xx->2xx|$.RequestId
Issue-Key: quicksight|DescribeFolder|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: quicksight|DescribeFolder|4xx->2xx|status
Issue-Key: quicksight|DescribeGroup|4xx->2xx|$.Message
Issue-Key: quicksight|DescribeGroup|4xx->2xx|$.ResourceType
Issue-Key: quicksight|DescribeGroup|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: quicksight|DescribeGroup|4xx->2xx|status
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# QuickSight accepts rejected creates and keeps deleted resources: 200 where AWS answers the modeled 400/404

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDataSource

## Description
Four net-new root causes in the QuickSight emulator, all of the same shape: a request AWS answers with a modeled client error (a create it refuses, or a read of a resource that does not exist) is answered 200 with a success body, so the error never reaches the client and the emulator's resource inventory disagrees with AWS's. This bundle has 4 causes; all four must be closed.

Cause 1 of 4 — CreateDataSource accepts a data source whose connection target is unroutable (fixture quicksight-data-source-creation-rejected-unroutable-host-and-mismatched-parameters, step task#1).
AWS: CreateDataSource (Smithy: POST /accounts/{AwsAccountId}/data-sources; modeled 400 errors are InvalidParameterValueException, documented "One or more parameters has a value that isn't valid.", and CustomerManagedKeyUnavailableException). QuickSight validates the supplied DataSourceParameters by attempting to reach the target before it will register the data source, so a host it cannot route to is rejected synchronously: HTTP 400, with a message whose fixed text is "Unable to route to the host address " followed by one variable part — the host/address taken from the request's DataSourceParameters. The recorded response in this fixture is a 400 whose Message is "Unable to route to the host address " + the request's host; the error type header was recorded but not asserted by the fixture, so the specific code is recorded behavior only (Hypothesis: InvalidParameterValueException, the only parameter-validation 400 the model binds to this operation). The literal address in the recording is only the value that filled the template.
Emulator: answers HTTP 200 with a CreateDataSource success body (and a RequestId), registering the data source. Replay: "status: expected 400, got 200; $.Message: missing (expected 'Unable to route to the host address 10.255.255.1', got None); $headers.x-amzn-errortype: missing".
Why: a client that relies on create-time validation believes an unreachable source was accepted; it never surfaces the routing failure, and every later call that assumes the resource is absent behaves differently than against AWS.
Fix: CreateDataSource's validation/handler in the quicksight crate's ops layer (services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/create_data_source.rs) — reachability of the declared connection target must be evaluated and turned into the 400 error above before any state is written. Related but distinct open issue: 20261029T191733Z-quicksight-createdatasource-never-runs-the-connection-test-an-athena.md covers the connection test not running for an ATHENA workgroup and reporting CREATION_SUCCESSFUL; this cause is the host-routing rejection path and its HTTP status/message, which that issue does not carry a wire replay key for.

Cause 2 of 4 — DescribeDataSource returns 200 for a data source AWS never created (same fixture, step task#2).
AWS: DescribeDataSource (Smithy: GET /accounts/{AwsAccountId}/data-sources/{DataSourceId}; errors include ResourceNotFoundException, httpError 404, "One or more resources can't be found."). Because the create in cause 1 was refused, the id does not exist, so AWS answers HTTP 404 ResourceNotFoundException with the data-source not-found template: fixed text "DataSource " and " is not found", variable part the ARN assembled as arn:aws:quicksight:<region>:<AwsAccountId>:datasource/<DataSourceId> from the serving region and the request's path labels. Recorded response: 404 with that message and no ResourceType member.
Emulator: HTTP 200 with a DataSource body, because the refused create was persisted. Replay: "status: expected 404, got 200; $.Message: missing (expected 'DataSource arn:aws:quicksight:us-west-2:000000000000:datasource/<id> is not found', got None)".
Why: the resource is visible in the emulator but absent in AWS, so a client's existence check inverts.
Fix: follows from cause 1 (no state should have been written); the not-found path in DescribeDataSource's handler must then produce the 404 above.

Cause 3 of 4 — DescribeFolder returns 200 for a deleted folder (fixture quicksight-folder-lifecycle-and-duplicate-rejected, step task#7).
AWS: DescribeFolder (Smithy: GET /accounts/{AwsAccountId}/folders/{FolderId}; errors include ResourceNotFoundException, httpError 404). After DeleteFolder succeeds, the folder is gone and AWS answers HTTP 404 ResourceNotFoundException. Message template (recorded behavior only; the model does not fix the text): fixed text "Folder: " and " not found" around one variable part, the folder ARN arn:aws:quicksight:<region>:<AwsAccountId>:folder/<FolderId> built from the serving region and the request's path labels. Note the folder template uses "Folder: <arn> not found", not the data source's "DataSource <arn> is not found".
Emulator: HTTP 200 with the folder still described. Replay: "status: expected 404, got 200; $.Message: missing (expected 'Folder: arn:aws:quicksight:us-west-2:000000000000:folder/<id> not found', got None)".
Why: DeleteFolder appears to succeed but does not remove the folder from the emulator's state, so delete-then-verify loops never terminate as they do on AWS and re-creating the same id can collide.
Fix: DeleteFolder's state mutation plus DescribeFolder's not-found path (ops/delete_folder.rs, ops/describe_folder.rs and the folder store) — the delete must remove the record and the describe must raise ResourceNotFoundException with the message above.

Cause 4 of 4 — DescribeGroup returns 200 for a deleted group (fixture quicksight-group-lifecycle-and-duplicate-rejected, step task#7).
AWS: DescribeGroup (Smithy: GET /accounts/{AwsAccountId}/namespaces/{Namespace}/groups/{GroupName}; errors include ResourceNotFoundException, httpError 404, whose shape carries Message, ResourceType (enum ExceptionResourceType) and RequestId). After DeleteGroup, AWS answers HTTP 404 ResourceNotFoundException with ResourceType = "GROUP" and the identity-namespace not-found template: fixed text "Group ", " does not exist in the namespace ", " of account ", ". AWS account id: ", ", groupName: ", ". " (note the trailing space), with variable parts the request's GroupName, Namespace and AwsAccountId.
Emulator: HTTP 200 with the group still described, no error and no ResourceType. Replay: "status: expected 404, got 200; $.Message: missing (expected 'Group <name> does not exist in the namespace default of account <account>. AWS account id: <account>, groupName: <name>. ', got None); $.ResourceType: missing (expected 'GROUP', got None)".
Why: a deleted group stays readable, so membership/cleanup logic that waits for the 404 never completes, and the ResourceType discriminator clients switch on is absent.
Fix: DeleteGroup's state mutation and DescribeGroup's not-found path (ops/delete_group.rs, ops/describe_group.rs and the group store), including populating ResourceType on the error.

Traces: quicksight-data-source-creation-rejected-unroutable-host-and-mismatched-parameters#task#1 and #task#2 (each cluster also covers the repeated step task#3 / task#4 in the same fixture), quicksight-folder-lifecycle-and-duplicate-rejected#task#7, quicksight-group-lifecycle-and-duplicate-rejected#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):

1) CreateDataSource with an unroutable connection host — fixture quicksight-data-source-creation-rejected-unroutable-host-and-mismatched-parameters, step task#1 (cluster also covers task#3)
   Expected (AWS): HTTP 400, body Message = "Unable to route to the host address <host from DataSourceParameters>", x-amzn-errortype present.
   Actual (emulator): HTTP 200, no Message, no x-amzn-errortype, RequestId emitted.

2) DescribeDataSource for that id — step task#2 (cluster also covers task#4)
   Expected: HTTP 404, Message = "DataSource arn:aws:quicksight:<region>:<account>:datasource/<DataSourceId> is not found", no ResourceType.
   Actual: HTTP 200 with a DataSource body.

3) DescribeFolder after DeleteFolder — fixture quicksight-folder-lifecycle-and-duplicate-rejected, step task#7
   Expected: HTTP 404, Message = "Folder: arn:aws:quicksight:<region>:<account>:folder/<FolderId> not found".
   Actual: HTTP 200 with the folder.

4) DescribeGroup after DeleteGroup — fixture quicksight-group-lifecycle-and-duplicate-rejected, step task#7
   Expected: HTTP 404, ResourceType = "GROUP", Message = "Group <GroupName> does not exist in the namespace <Namespace> of account <AwsAccountId>. AWS account id: <AwsAccountId>, groupName: <GroupName>. ".
   Actual: HTTP 200 with the group.

Smithy citations: CreateDataSource errors -> InvalidParameterValueException (client, httpError 400); DescribeDataSource / DescribeFolder / DescribeGroup errors -> ResourceNotFoundException (client, httpError 404); ResourceNotFoundException members = Message, ResourceType (ExceptionResourceType), RequestId. The model fixes the status and shape; the message templates are recorded behavior only.

## Steps to Reproduce
1. Replay fixture quicksight-data-source-creation-rejected-unroutable-host-and-mismatched-parameters: CreateDataSource(AwsAccountId=<12-digit>, DataSourceId=<id>, Name=<id>, Type=<db type>, DataSourceParameters pointing at an unroutable host) — the emulator answers 200 instead of 400 "Unable to route to the host address <host>" (step task#1).
2. In the same fixture, DescribeDataSource(AwsAccountId, DataSourceId=<same id>) — the emulator answers 200 with a DataSource instead of 404 "DataSource <arn> is not found" (step task#2).
3. Replay fixture quicksight-folder-lifecycle-and-duplicate-rejected: CreateFolder -> UpdateFolder -> DeleteFolder, then DescribeFolder(FolderId=<deleted id>) — the emulator answers 200 instead of 404 "Folder: <arn> not found" (step task#7).
4. Replay fixture quicksight-group-lifecycle-and-duplicate-rejected: CreateGroup -> UpdateGroup -> DeleteGroup, then DescribeGroup(GroupName=<deleted name>, Namespace=default) — the emulator answers 200 instead of 404 with ResourceType GROUP (step task#7).
