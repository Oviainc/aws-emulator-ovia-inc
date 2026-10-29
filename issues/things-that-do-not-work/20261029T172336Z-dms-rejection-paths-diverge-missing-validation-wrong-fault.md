Issue-Source: trace-sweep-divergence
Issue-Trace: b2-provisioning-rejections
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: b2-provisioning-rejections b1-endpoint-lifecycle
Issue-Shape-Id: com.amazonaws.databasemigrationservice#AmazonDMSv20160101
Issue-Key: dms|CreateReplicationInstance|4xx->2xx|$.__type
Issue-Key: dms|CreateReplicationInstance|4xx->2xx|$.message
Issue-Key: dms|CreateReplicationInstance|4xx->2xx|status
Issue-Key: dms|CreateReplicationInstance|4xx->4xx|$.__type
Issue-Key: dms|CreateReplicationInstance|4xx->4xx|$.message
Issue-Key: dms|CreateReplicationTask|4xx->4xx|$.message
Issue-Key: dms|DescribeReplicationInstances|4xx->2xx|$.__type
Issue-Key: dms|DescribeReplicationInstances|4xx->2xx|$.message
Issue-Key: dms|DescribeReplicationInstances|4xx->2xx|status
Issue-Key: dms|CreateEndpoint|4xx->4xx|$.message
Issue-Key: dms|DescribeEndpoints|4xx->2xx|$.__type
Issue-Key: dms|DescribeEndpoints|4xx->2xx|$.message
Issue-Key: dms|DescribeEndpoints|4xx->2xx|status
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# DMS rejection paths diverge: missing validation, wrong fault code and non-AWS error message text

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReplicationInstance

## Description
The emulator's DMS error paths diverge from AWS in three ways that recur across operations: a request AWS refuses is accepted with HTTP 200, a refusal is raised with the wrong fault code, and a refusal carries message text the emulator invented rather than AWS's wording. Six causes, all in service dms.

=== Cause 1 of 6: CreateReplicationInstance does not validate ReplicationInstanceClass ===
Fixture b2-provisioning-rejections step task#4.

AWS: CreateReplicationInstance requires ReplicationInstanceClass (Smithy: required member, target ReplicationInstanceClass, a string with @length min 0 max 30) to name a real DMS instance class, and refuses a value that is not one. The recorded AWS response is HTTP 400 with error code InvalidParameterValueException and message 'Invalid ReplicationInstance class' -- that whole string is fixed text, with no variable part. Recorded behavior only for the code: InvalidParameterValueException is NOT among CreateReplicationInstance's modeled errors (the model lists AccessDeniedFault, InsufficientResourceCapacityFault, InvalidResourceStateFault, InvalidSubnet, KMSKeyNotAccessibleFault, ReplicationSubnetGroupDoesNotCoverEnoughAZs, ResourceAlreadyExistsFault, ResourceNotFoundFault, ResourceQuotaExceededFault, StorageQuotaExceededFault), so the recording is the authority on both the code and the message. Because the create is refused, nothing is persisted.

Emulator: HTTP 200 -- the request succeeds and a replication instance is created. $.__type and $.message are absent.

Why: a client that typo-ed or used a retired instance class gets a green create against the emulator and ships a configuration real DMS rejects; its InvalidParameterValueException handling is never exercised. Worse, the instance the emulator should not have created stays in state and poisons later steps -- Cause 2 below is exactly that contamination.

Fix: CreateReplicationInstance's input validation, before anything is persisted -- services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/create_replication_instance.rs. Validate ReplicationInstanceClass against the known DMS instance classes and return HTTP 400 InvalidParameterValueException with message 'Invalid ReplicationInstance class', persisting nothing.

Traces: b2-provisioning-rejections#task#4

=== Cause 2 of 6: CreateReplicationInstance does not validate that ReplicationSubnetGroupIdentifier exists, and reports a name collision instead ===
Fixture b2-provisioning-rejections step task#5.

AWS: CreateReplicationInstance resolves the optional ReplicationSubnetGroupIdentifier against the account's replication subnet groups and refuses the request when no such group exists. Recorded AWS response: HTTP 400, error code InvalidParameterValueException, message "Replication Subnet Group '<ReplicationSubnetGroupIdentifier>' does not exist." -- fixed text "Replication Subnet Group '" ... "' does not exist." with the single-quoted subnet group identifier taken verbatim from the request. Recorded behavior only for the code, as in Cause 1. AWS performs this parameter validation before the identifier-uniqueness check, so an unknown subnet group is reported even when the instance identifier is otherwise contentious.

Emulator: HTTP 400 but with $.__type 'ResourceAlreadyExistsFault' and message 'replication instance <identifier> already exists as <arn>' -- it never looks at ReplicationSubnetGroupIdentifier at all, and instead collides with the instance the un-validated Cause 1 step left in state. Both the fault code and the message are wrong.

Why: the client is told its name is taken when the actual fault is an unknown subnet group, so it retries with a fresh name and fails forever instead of fixing the subnet group. SDK retry/branching keyed on ResourceAlreadyExistsFault vs InvalidParameterValueException takes the wrong path. Note this cause has two parts that must both be fixed: the missing subnet-group existence check, and its precedence ahead of the uniqueness check.

Fix: the same handler's validation -- src/ops/create_replication_instance.rs: look ReplicationSubnetGroupIdentifier up in the subnet-group state and return HTTP 400 InvalidParameterValueException with AWS's wording above, ordered before the instance-identifier uniqueness check.

Traces: b2-provisioning-rejections#task#5

=== Cause 3 of 6: CreateReplicationTask's unknown-endpoint-ARN message is not AWS's wording ===
Fixture b2-provisioning-rejections step task#6.

AWS: CreateReplicationTask resolves the required SourceEndpointArn / TargetEndpointArn (Smithy: both required String members; ResourceNotFoundFault is modeled, HTTP 400, "The resource could not be found.") and refuses when the ARN names no endpoint. Recorded AWS response: HTTP 400 with message 'Endpoint ARN:<arn> does not exist' -- fixed text 'Endpoint ARN:' immediately followed by the offending endpoint ARN from the request, then ' does not exist'. AWS does NOT name which input member carried the ARN.

Emulator: HTTP 400 with the right status and fault code but message 'SourceEndpointArn <arn> does not name an endpoint in this account' -- emulator-invented prose that names the input member and uses different wording.

Why: the message is the only place the offending ARN is reported, and operators and log/alert matchers key on its text. A client or test asserting AWS's wording fails against the emulator even though the condition was detected correctly.

Fix: CreateReplicationTask's endpoint-resolution error construction -- services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/create_replication_task.rs: emit 'Endpoint ARN:' + the unresolved ARN + ' does not exist'.

Traces: b2-provisioning-rejections#task#6

=== Cause 4 of 6: DescribeReplicationInstances returns an empty 200 where AWS raises ResourceNotFoundFault ===
Fixture b2-provisioning-rejections step task#8.

AWS: DescribeReplicationInstances models exactly one error, ResourceNotFoundFault (Smithy: HTTP 400, "The resource could not be found."), and raises it -- rather than returning an empty list -- when the supplied Filters match no replication instance. Recorded AWS response: HTTP 400, __type ResourceNotFoundFault, message 'No instances found matching provided filters' (entirely fixed text, no variable part).

Emulator: HTTP 200 with no error members at all ($.__type and $.message missing) -- it returns an empty ReplicationInstances list.

Why: "does this instance exist?" is normally written as a filtered describe and a caught ResourceNotFoundFault. Against the emulator the exception never arrives, so absence reads as success and the client treats a missing instance as present-but-empty; waiters and polling loops that expect the fault spin or exit on the wrong branch.

Fix: DescribeReplicationInstances' handler -- services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/describe_replication_instances.rs: when Filters were supplied and the filtered result set is empty, return HTTP 400 ResourceNotFoundFault with message 'No instances found matching provided filters' instead of an empty 200.

Traces: b2-provisioning-rejections#task#8

=== Cause 5 of 6: CreateEndpoint's duplicate-identifier message is not AWS's wording ===
Fixture b1-endpoint-lifecycle step task#6.

AWS: CreateEndpoint refuses a second endpoint with an EndpointIdentifier already in use (Smithy: ResourceAlreadyExistsFault, HTTP 400, "The resource you are attempting to create already exists."). Recorded AWS response: HTTP 400 with message 'ReplicationEndpoint "<EndpointIdentifier>" already in use.' -- fixed text 'ReplicationEndpoint "' ... '" already in use.' with the double-quoted endpoint identifier taken from the request. AWS reports the identifier the caller sent; it does not disclose the existing resource's ARN in the message.

Emulator: HTTP 400 with the right fault code but message 'endpoint <identifier> already exists as <arn>' -- different wording, lowercased resource noun, no quoting, and it leaks the pre-existing endpoint ARN in place of AWS's fixed phrase.

Why: idempotency and "create-if-absent" helpers routinely match on this message in addition to the fault code, and operators read it directly. The divergent text breaks those matchers, and the ARN it substitutes is a per-run generated value that is not part of any AWS message template.

Fix: CreateEndpoint's uniqueness-check error construction -- services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/create_endpoint.rs: build the ResourceAlreadyExistsFault message as 'ReplicationEndpoint "' + EndpointIdentifier + '" already in use.'

Traces: b1-endpoint-lifecycle#task#6

=== Cause 6 of 6: DescribeEndpoints returns an empty 200 where AWS raises ResourceNotFoundFault ===
Fixture b1-endpoint-lifecycle step task#9.

AWS: DescribeEndpoints likewise models only ResourceNotFoundFault (Smithy: HTTP 400) and raises it when the supplied Filters match no endpoint -- it does not answer with an empty Endpoints list. Recorded AWS response: HTTP 400, __type ResourceNotFoundFault, message 'No Endpoints found matching provided filters' (all fixed text; note the capitalised 'Endpoints', unlike the instances wording in Cause 4).

Emulator: HTTP 200 with $.__type and $.message absent -- an empty list again.

Why: same as Cause 4, on the endpoint lifecycle: after a delete, a client confirms removal by a filtered DescribeEndpoints and expects ResourceNotFoundFault. The emulator's empty 200 makes the deleted-vs-existing distinction invisible to exception-based code.

Fix: DescribeEndpoints' handler -- services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/describe_endpoints.rs: empty filtered result with Filters supplied must become HTTP 400 ResourceNotFoundFault with message 'No Endpoints found matching provided filters'. Causes 4 and 6 share this pattern but are two handlers with two distinct message strings; both must be changed and both steps re-replayed.

Traces: b1-endpoint-lifecycle#task#9

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. Six divergence clusters, all service dms, none blocked by a stub and none owned by an open issue.

Cause 1 -- b2-provisioning-rejections#task#4, CreateReplicationInstance, expected HTTP 400, actual HTTP 200:
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidParameterValueException', got None)
  $.message: missing (expected 'Invalid ReplicationInstance class', got None)

Cause 2 -- b2-provisioning-rejections#task#5, CreateReplicationInstance, expected HTTP 400, actual HTTP 400:
  $.__type: value_mismatch (expected 'InvalidParameterValueException', got 'ResourceAlreadyExistsFault')
  $.message: value_mismatch (expected "Replication Subnet Group 'sim2real-probe-dms-sng-0001' does not exist.", got 'replication instance sim2real-probe-dms-ri-0001 already exists as arn:aws:dms:us-west-2:000000000000:rep:SC8LQUDD0UIGGAD224JXZOBB47')
  (The quoted subnet-group and instance identifiers and the ARN are this run's values; the requirement is the template "Replication Subnet Group '<request ReplicationSubnetGroupIdentifier>' does not exist.")

Cause 3 -- b2-provisioning-rejections#task#6, CreateReplicationTask, expected HTTP 400, actual HTTP 400:
  $.message: value_mismatch (expected 'Endpoint ARN:arn:aws:dms:us-west-2:000000000000:endpoint:AAAAAAAAAAAAAAAAAAAAAAAAAA does not exist', got 'SourceEndpointArn arn:aws:dms:us-west-2:000000000000:endpoint:AAAAAAAAAAAAAAAAAAAAAAAAAA does not name an endpoint in this account')
  (Fixed text 'Endpoint ARN:' and ' does not exist'; the ARN is the request's unresolved endpoint ARN.)

Cause 4 -- b2-provisioning-rejections#task#8, DescribeReplicationInstances, expected HTTP 400, actual HTTP 200:
  status: expected 400, got 200
  $.__type: missing (expected 'ResourceNotFoundFault', got None)
  $.message: missing (expected 'No instances found matching provided filters', got None)

Cause 5 -- b1-endpoint-lifecycle#task#6, CreateEndpoint, expected HTTP 400, actual HTTP 400:
  $.message: value_mismatch (expected 'ReplicationEndpoint "sim2real-probe-dms-ep-36ad8d1ec3" already in use.', got 'endpoint sim2real-probe-dms-ep-36ad8d1ec3 already exists as arn:aws:dms:us-west-2:000000000000:endpoint:AJSY3RQLLHDGIRIL4JRGG0VV95')
  (Fixed text 'ReplicationEndpoint "' ... '" already in use.'; the quoted value is the request's EndpointIdentifier. The ARN in the emulator's text is a per-run generated value with no place in AWS's template.)

Cause 6 -- b1-endpoint-lifecycle#task#9, DescribeEndpoints, expected HTTP 400, actual HTTP 200:
  status: expected 400, got 200
  $.__type: missing (expected 'ResourceNotFoundFault', got None)
  $.message: missing (expected 'No Endpoints found matching provided filters', got None)

Smithy evidence (services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/smithy-model/com-amazonaws-databasemigrationservice-amazondmsv20160101.json, awsJson1_1, sigv4Name dms, api version 2016-01-01): DescribeEndpoints and DescribeReplicationInstances each model exactly one error, ResourceNotFoundFault (HTTP 400); CreateEndpoint and CreateReplicationTask model ResourceAlreadyExistsFault and ResourceNotFoundFault (both HTTP 400); CreateReplicationInstance's ReplicationInstanceClass is a required String with @length max 30 and the operation does NOT model InvalidParameterValueException, so that code and its messages rest on the recordings.

Not covered elsewhere: the one open DMS issue (20261023T134102Z-...-createendpoint-skips-serviceaccessrolearn-validation-and-createreplicationsubnetgroup-is.md) is behavioral and concerns CreateEndpoint ServiceAccessRoleArn validation and the CreateReplicationSubnetGroup 501 stub; it names none of these six causes.

## Steps to Reproduce
Fixture b2-provisioning-rejections:
1. (Cause 1, step task#4) CreateReplicationInstance with a valid new ReplicationInstanceIdentifier and a ReplicationInstanceClass that is not a real DMS class. Expect HTTP 400 InvalidParameterValueException 'Invalid ReplicationInstance class' and nothing persisted; the emulator returns HTTP 200 and creates the instance.
2. (Cause 2, step task#5) CreateReplicationInstance naming a ReplicationSubnetGroupIdentifier that does not exist. Expect HTTP 400 InvalidParameterValueException "Replication Subnet Group '<identifier>' does not exist."; the emulator returns HTTP 400 ResourceAlreadyExistsFault about the instance name left behind by step 1.
3. (Cause 3, step task#6) CreateReplicationTask with a well-formed SourceEndpointArn that names no endpoint. Expect HTTP 400 with message 'Endpoint ARN:<arn> does not exist'; the emulator returns 'SourceEndpointArn <arn> does not name an endpoint in this account'.
4. (Cause 4, step task#8) DescribeReplicationInstances with Filters that match no instance. Expect HTTP 400 ResourceNotFoundFault 'No instances found matching provided filters'; the emulator returns HTTP 200 with an empty list.

Fixture b1-endpoint-lifecycle:
5. (Cause 5, step task#6) CreateEndpoint re-using an EndpointIdentifier already created earlier in the fixture. Expect HTTP 400 ResourceAlreadyExistsFault with message 'ReplicationEndpoint "<EndpointIdentifier>" already in use.'; the emulator returns 'endpoint <id> already exists as <arn>'.
6. (Cause 6, step task#9) After deleting the endpoint, DescribeEndpoints with Filters matching nothing. Expect HTTP 400 ResourceNotFoundFault 'No Endpoints found matching provided filters'; the emulator returns HTTP 200 with an empty list.

All six steps must pass on re-replay; Cause 2's expected error only becomes observable once Cause 1 stops persisting the instance it should have refused.
