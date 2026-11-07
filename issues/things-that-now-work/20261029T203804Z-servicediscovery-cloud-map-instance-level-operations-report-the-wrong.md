Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-service-operations-not-found
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-service-operations-not-found
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|GetInstance|4xx->4xx|$.__type
Issue-Key: servicediscovery|GetInstancesHealthStatus|4xx->4xx|$.Message
Issue-Key: servicediscovery|GetInstancesHealthStatus|4xx->4xx|$.__type
Issue-Key: servicediscovery|GetInstancesHealthStatus|4xx->4xx|status
Issue-Key: servicediscovery|DeregisterInstance|4xx->4xx|$.Message
Issue-Key: servicediscovery|DeregisterInstance|4xx->4xx|status
Issue-Key: servicediscovery|UpdateInstanceCustomHealthStatus|4xx->4xx|$.Message
Issue-Key: servicediscovery|UpdateInstanceCustomHealthStatus|4xx->4xx|status
Issue-Related: 20261029T203804Z-servicediscovery-cloud-map-service-instance-and-service-attribute-errors.md
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# Cloud Map instance-level operations report the wrong error identity and message for a missing service

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInstance

## Description
FOUR independent, net-new wire divergences in the servicediscovery (Cloud Map) emulator, all on instance-level operations invoked with a ServiceId that does not exist. Two of them return the WRONG error code (ServiceNotFound where AWS reports InstanceNotFound) and all four return the wrong status and message. Each of the four causes below must be closed before this issue is submitted. Model citations use services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json.

Shared AWS fact: the model carries @httpError 404 on ServiceNotFound/InstanceNotFound, but every recorded AWS response in this corpus for these classes is HTTP 400 with the code in the awsJson1_1 __type field; the recording is the wire truth. The message for each of these errors is a per-operation template whose variable part is derived from the request (the instance id, or the service ARN built from the requested ServiceId).

Cause 1 of 4: GetInstance with a ServiceId that does not exist reports ServiceNotFound where AWS reports InstanceNotFound.
AWS: Smithy GetInstance{ServiceId: Arn (required), InstanceId: ResourceId (required)} -> {ResourceOwner, Instance}; errors InstanceNotFound, InvalidInput (400), ServiceNotFound. Recorded: HTTP 400, __type InstanceNotFound, Message = the InstanceId value from the request. AWS resolves this read as an instance miss regardless of whether the enclosing service exists - the service's absence is not surfaced as a distinct error on this operation.
Emulator: HTTP 404, __type ServiceNotFound, Message 'No service exists with the specified ID <serviceId>.'
Why: error-handling code that catches InstanceNotFound (the modelled, documented error for a missing instance read) never fires against the emulator; it sees a different exception class, a different message template and a different status, so retry/absent-handling logic takes the wrong branch.
Fix: GetInstance's lookup ordering and error construction in its handler - resolve the instance and raise InstanceNotFound with the requested instance id as the message, instead of failing fast on the service lookup.
Traces: servicediscovery-service-operations-not-found#task#6.

Cause 2 of 4: GetInstancesHealthStatus with a ServiceId that does not exist reports ServiceNotFound where AWS reports InstanceNotFound with its own documented template.
AWS: Smithy GetInstancesHealthStatus{ServiceId: Arn (required), Instances: InstanceIdList, MaxResults, NextToken} -> {Status, NextToken}; errors InstanceNotFound, InvalidInput (400), ServiceNotFound. Recorded: HTTP 400, __type InstanceNotFound, Message "Instance(s) not found for service '<service ARN>' when trying to retreive: <what was requested>" - fixed text 'Instance(s) not found for service ', the quoted ARN, ' when trying to retreive: ' (AWS's own spelling of 'retreive' is part of the literal template), then the requested instance scope, which is the literal 'All instances' when the Instances member is omitted and otherwise the requested ids.
Emulator: HTTP 404, __type ServiceNotFound, Message 'No service exists with the specified ID <serviceId>.'
Why: as cause 1, plus the message loses the ARN and the scope indicator AWS reports.
Fix: GetInstancesHealthStatus' handler - raise InstanceNotFound with the template above.
Traces: servicediscovery-service-operations-not-found#task#7.

Cause 3 of 4: DeregisterInstance on a missing service answers 404 with prose instead of 400 whose Message is the service ARN.
AWS: Smithy DeregisterInstance{ServiceId: Arn (required), InstanceId: ResourceId (required)} -> OperationId; errors DuplicateRequest, InstanceNotFound, InvalidInput (400), ResourceInUse, ServiceNotFound. Recorded: HTTP 400, __type ServiceNotFound, Message = the service ARN derived from the requested ServiceId.
Emulator: HTTP 404 with 'No service exists with the specified ID <serviceId>.'
Why: the status is wrong and the message carries emulator prose instead of the ARN AWS echoes.
Fix: DeregisterInstance's ServiceNotFound construction plus the shared error-status mapping.
Traces: servicediscovery-service-operations-not-found#task#9.

Cause 4 of 4: UpdateInstanceCustomHealthStatus on a missing service answers 404 with prose instead of 400 whose Message is "no service '<service ARN>'".
AWS: Smithy UpdateInstanceCustomHealthStatus{ServiceId: Arn (required), InstanceId: ResourceId (required), Status: CustomHealthStatus (required)} -> {}; errors CustomHealthNotFound, InstanceNotFound, InvalidInput (400), ServiceNotFound. Recorded: HTTP 400, __type ServiceNotFound, Message "no service '<service ARN>'" - fixed text 'no service ' plus the single-quoted service ARN built from the request.
Emulator: HTTP 404 with 'No service exists with the specified ID <serviceId>.'
Why: the documented lower-case template and the ARN are both lost and the status diverges. The open behavioral issue 20261023T124702Z-servicediscovery-cloud-map-servicediscovery-8-fidelity-divergences-missing-dnsconfig.md names this template under its Cause 8 for the no-custom-health-config path; this wire step reaches it through a missing service and is a separate replay key.
Fix: UpdateInstanceCustomHealthStatus' ServiceNotFound construction plus the shared status mapping.
Traces: servicediscovery-service-operations-not-found#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: 168 steps, 89 passed, 79 mismatched, score 53.0%. Reported mismatch clusters (expected = recorded AWS response):

Cause 1 - servicediscovery-service-operations-not-found#task#6, GetInstance: 'status: expected 400, got 404'; '$.__type: value_mismatch (expected \'InstanceNotFound\', got \'ServiceNotFound\')'; '$.Message: value_mismatch (expected the requested instance id, got \'No service exists with the specified ID srv-doesnotexist000.\')'.
Cause 2 - servicediscovery-service-operations-not-found#task#7, GetInstancesHealthStatus: 'status: expected 400, got 404'; '$.__type: value_mismatch (expected \'InstanceNotFound\', got \'ServiceNotFound\')'; '$.Message: value_mismatch (expected "Instance(s) not found for service \'arn:aws:servicediscovery:<region>:<account>:service/srv-doesnotexist000\' when trying to retreive: All instances", got \'No service exists with the specified ID srv-doesnotexist000.\')'.
Cause 3 - servicediscovery-service-operations-not-found#task#9, DeregisterInstance: 'status: expected 400, got 404'; '$.Message: value_mismatch (expected the service ARN, got \'No service exists with the specified ID srv-doesnotexist000.\')'.
Cause 4 - servicediscovery-service-operations-not-found#task#10, UpdateInstanceCustomHealthStatus: 'status: expected 400, got 404'; '$.Message: value_mismatch (expected "no service \'arn:aws:servicediscovery:<region>:<account>:service/srv-doesnotexist000\'", got \'No service exists with the specified ID srv-doesnotexist000.\')'.

## Steps to Reproduce
Boot the emulator from this checkout and replay servicediscovery-service-operations-not-found steps task#6, task#7, task#9 and task#10, or with botocore (sd = boto3.client('servicediscovery', endpoint_url=<emulator>)) against a service id that does not exist:
1. Cause 1: sd.get_instance(ServiceId='srv-doesnotexist000', InstanceId='i1') -> emulator 404 ServiceNotFound; AWS 400 InstanceNotFound whose Message is 'i1'.
2. Cause 2: sd.get_instances_health_status(ServiceId='srv-doesnotexist000') -> emulator 404 ServiceNotFound; AWS 400 InstanceNotFound "Instance(s) not found for service '<arn>' when trying to retreive: All instances". Repeat with Instances=['i1'] and compare the scope part of the message.
3. Cause 3: sd.deregister_instance(ServiceId='srv-doesnotexist000', InstanceId='i1') -> emulator 404 with prose; AWS 400 ServiceNotFound whose Message is the service ARN.
4. Cause 4: sd.update_instance_custom_health_status(ServiceId='srv-doesnotexist000', InstanceId='i1', Status='HEALTHY') -> emulator 404 with prose; AWS 400 ServiceNotFound "no service '<arn>'".
