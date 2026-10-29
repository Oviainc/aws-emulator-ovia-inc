Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-service-operations-not-found
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-service-operations-not-found servicediscovery-instance-registration-error-recovery servicediscovery-service-attributes-operations-not-found
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|GetService|4xx->4xx|status
Issue-Key: servicediscovery|RegisterInstance|4xx->4xx|$.Message
Issue-Key: servicediscovery|RegisterInstance|4xx->4xx|status
Issue-Key: servicediscovery|GetInstance|4xx->4xx|$.Message
Issue-Key: servicediscovery|GetInstance|4xx->4xx|status
Issue-Key: servicediscovery|DeleteService|4xx->4xx|$.Message
Issue-Key: servicediscovery|DeleteService|4xx->4xx|status
Issue-Key: servicediscovery|GetServiceAttributes|4xx->4xx|status
Issue-Key: servicediscovery|UpdateServiceAttributes|4xx->4xx|status
Issue-Key: servicediscovery|DeleteServiceAttributes|4xx->4xx|status
Issue-Key: servicediscovery|UpdateService|4xx->4xx|$.Message
Issue-Key: servicediscovery|UpdateService|4xx->4xx|status
Issue-Key: servicediscovery|ListInstances|4xx->4xx|$.Message
Issue-Key: servicediscovery|ListInstances|4xx->4xx|status
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# Cloud Map service, instance and service-attribute errors answer HTTP 404/409 instead of the 400 AWS serves

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetService

## Description
TEN independent, net-new wire divergences in the servicediscovery (Cloud Map) emulator on service-, instance- and service-attribute-scoped calls, all in one family: the HTTP status served for Cloud Map's client-error classes, and for seven of them the Message body as well. Each of the ten causes below must be closed before this issue is submitted. Model citations use services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json.

Shared AWS fact for causes 1-10: the model carries @httpError 404 on ServiceNotFound / InstanceNotFound and 409 on ResourceInUse, but every recorded AWS response in this wire corpus for those classes is HTTP 400, with the error code in the awsJson1_1 __type field. The recording is the wire truth: Cloud Map serves 400 and distinguishes the error only by __type. Each not-found Message is a per-operation rule built from the request (an id, an ARN, or deliberately empty), never free prose.

Cause 1 of 10: GetService for an unknown service id answers 404 instead of 400.
AWS: Smithy GetService{Id: Arn (required)} -> Service; errors InvalidInput (400), ServiceNotFound (modelled 404). Recorded: HTTP 400 with __type ServiceNotFound.
Emulator: HTTP 404 (code matched).
Why: clients keyed on the HTTP status misclassify a normal client error as a transport 404.
Fix: the error-to-status mapping in the shared awsJson1_1 error serializer.
Traces: servicediscovery-service-operations-not-found#task#2.

Cause 2 of 10: RegisterInstance against a missing service answers 404 with emulator prose instead of 400 whose Message is the service ARN.
AWS: Smithy RegisterInstance{ServiceId: Arn (required), InstanceId (required, pattern ^[0-9a-zA-Z_/:.@-]+$), CreatorRequestId, Attributes (required)} -> OperationId; errors DuplicateRequest, InvalidInput 400, ResourceInUse, ResourceLimitExceeded 400, ServiceNotFound. Recorded: HTTP 400, __type ServiceNotFound, Message = the full service ARN built from the request's ServiceId ('arn:aws:servicediscovery:<region>:<account>:service/<serviceId>') - no fixed text beyond the ARN form itself.
Emulator: HTTP 404 with Message 'No service exists with the specified ID <serviceId>.'
Why: a caller that parses the message to recover the ARN it asked for gets emulator prose, and the status is wrong.
Fix: RegisterInstance's ServiceNotFound construction (it must render the ARN) plus the shared status mapping.
Traces: servicediscovery-instance-registration-error-recovery#task#4; also servicediscovery-service-operations-not-found#task#8.

Cause 3 of 10: GetInstance for an unknown instance in an existing service answers 404 with prose instead of 400 whose Message is the instance id alone.
AWS: Smithy GetInstance{ServiceId: Arn (required), InstanceId: ResourceId (required, len 0..64)} -> {ResourceOwner, Instance}; errors InstanceNotFound, InvalidInput 400, ServiceNotFound. Recorded: HTTP 400, __type InstanceNotFound, Message = the InstanceId value from the request, with no fixed text around it.
Emulator: HTTP 404 with Message 'No instance exists with the specified ID <instanceId> in service <serviceId>.'
Why: the message is a different template, so id extraction breaks, and the status diverges.
Fix: GetInstance's InstanceNotFound construction plus the shared status mapping.
Traces: servicediscovery-instance-registration-error-recovery#task#8; also servicediscovery-instance-registration-error-recovery#task#13.

Cause 4 of 10: DeleteService with registered instances answers 409 with prose instead of 400 with AWS's fixed ResourceInUse text.
AWS: Smithy DeleteService{Id (required)} -> {}; errors InvalidInput 400, ResourceInUse, ServiceNotFound. Recorded: HTTP 400, __type ResourceInUse, Message exactly 'Service contains registered instances; delete the instances before deleting the service' - entirely fixed text, no id and no count.
Emulator: HTTP 409 with "The service <serviceId> can't be deleted because it still contains <n> registered instance(s)."
Why: both the status and the documented conflict text diverge. The open behavioral issue 20261023T124702Z-servicediscovery-cloud-map-servicediscovery-8-fidelity-divergences-missing-dnsconfig.md names this message in its Cause 8 but records the 409 as matching; the wire recording shows the status is wrong too, and this is a separate replay key.
Fix: DeleteService's ResourceInUse construction plus the shared status mapping.
Traces: servicediscovery-instance-registration-error-recovery#task#10.

Cause 5 of 10: GetServiceAttributes for an unknown service answers 404 instead of 400.
AWS: Smithy GetServiceAttributes{ServiceId: Arn (required)} -> ServiceAttributes; errors InvalidInput 400, ServiceNotFound. Recorded: HTTP 400 with __type ServiceNotFound (message matched).
Emulator: HTTP 404.
Why/Fix: as cause 1 - the shared error-status mapping.
Traces: servicediscovery-service-attributes-operations-not-found#task#2.

Cause 6 of 10: UpdateServiceAttributes for an unknown service answers 404 instead of 400.
AWS: Smithy UpdateServiceAttributes{ServiceId: Arn (required), Attributes: ServiceAttributesMap (required, length 1..30)} -> {}; errors InvalidInput 400, ServiceAttributesLimitExceededException 400, ServiceNotFound. Recorded: HTTP 400 with __type ServiceNotFound.
Emulator: HTTP 404.
Why/Fix: as cause 1.
Traces: servicediscovery-service-attributes-operations-not-found#task#3.

Cause 7 of 10: DeleteServiceAttributes for an unknown service answers 404 instead of 400.
AWS: Smithy DeleteServiceAttributes{ServiceId: Arn (required), Attributes: AttributeKeyList (required)} -> {}; errors InvalidInput 400, ServiceNotFound. Recorded: HTTP 400 with __type ServiceNotFound.
Emulator: HTTP 404.
Why/Fix: as cause 1.
Traces: servicediscovery-service-attributes-operations-not-found#task#4.

Cause 8 of 10: UpdateService on a missing service answers 404 with prose instead of 400 whose Message is the service ARN.
AWS: Smithy UpdateService{Id (required), Service: ServiceChange (required)} -> OperationId; errors DuplicateRequest, InvalidInput 400, ServiceNotFound. Recorded: HTTP 400, __type ServiceNotFound, Message = the service ARN derived from the request's Id.
Emulator: HTTP 404 with 'No service exists with the specified ID <serviceId>.'
Why/Fix: as cause 2, in UpdateService's handler.
Traces: servicediscovery-service-operations-not-found#task#3.

Cause 9 of 10: DeleteService on a missing service answers 404 with prose instead of 400 whose Message is the bare service id.
AWS: as cause 4. Recorded for the not-found case: HTTP 400, __type ServiceNotFound, Message = the Id value from the request (the bare 'srv-...' id, NOT the ARN - this operation's rule differs from UpdateService's).
Emulator: HTTP 404 with 'No service exists with the specified ID <serviceId>.'
Why: the per-operation asymmetry (bare id here, ARN elsewhere) is lost, and the status diverges.
Fix: DeleteService's ServiceNotFound construction plus the shared status mapping.
Traces: servicediscovery-service-operations-not-found#task#4.

Cause 10 of 10: ListInstances on a missing service answers 404 with prose instead of 400 whose Message is the service ARN.
AWS: Smithy ListInstances{ServiceId: Arn (required), NextToken, MaxResults 1..100} -> {ResourceOwner, Instances, NextToken}; errors InvalidInput 400, ServiceNotFound. Recorded: HTTP 400, __type ServiceNotFound, Message = the service ARN derived from the request's ServiceId.
Emulator: HTTP 404 with 'No service exists with the specified ID <serviceId>.'
Why/Fix: as cause 2, in ListInstances' handler.
Traces: servicediscovery-service-operations-not-found#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: 168 steps, 89 passed, 79 mismatched, score 53.0%. Reported mismatch clusters (expected = recorded AWS response):

Cause 1 - servicediscovery-service-operations-not-found#task#2, GetService: 'status: expected 400, got 404'.
Cause 2 - servicediscovery-instance-registration-error-recovery#task#4, RegisterInstance: 'status: expected 400, got 404'; '$.Message: value_mismatch (expected the service ARN arn:aws:servicediscovery:<region>:<account>:service/srv-doesnotexist000, got \'No service exists with the specified ID srv-doesnotexist000.\')'.
Cause 3 - servicediscovery-instance-registration-error-recovery#task#8, GetInstance: 'status: expected 400, got 404'; '$.Message: value_mismatch (expected the instance id, got \'No instance exists with the specified ID <instanceId> in service srv-xxwcg7jqecv6n0jy.\')'.
Cause 4 - servicediscovery-instance-registration-error-recovery#task#10, DeleteService: 'status: expected 400, got 409'; '$.Message: value_mismatch (expected \'Service contains registered instances; delete the instances before deleting the service\', got "The service srv-xxwcg7jqecv6n0jy can't be deleted because it still contains 1 registered instance(s).")'.
Causes 5/6/7 - servicediscovery-service-attributes-operations-not-found#task#2 (GetServiceAttributes), #task#3 (UpdateServiceAttributes), #task#4 (DeleteServiceAttributes): 'status: expected 400, got 404' in each.
Cause 8 - servicediscovery-service-operations-not-found#task#3, UpdateService: 'status: expected 400, got 404'; '$.Message: value_mismatch (expected the service ARN, got \'No service exists with the specified ID srv-doesnotexist000.\')'.
Cause 9 - servicediscovery-service-operations-not-found#task#4, DeleteService: 'status: expected 400, got 404'; '$.Message: value_mismatch (expected \'srv-doesnotexist000\' i.e. the bare requested id, got \'No service exists with the specified ID srv-doesnotexist000.\')'.
Cause 10 - servicediscovery-service-operations-not-found#task#5, ListInstances: 'status: expected 400, got 404'; '$.Message: value_mismatch (expected the service ARN, got \'No service exists with the specified ID srv-doesnotexist000.\')'.

## Steps to Reproduce
Boot the emulator from this checkout and replay the fixtures/steps in Issue-Trace/Issue-Step and Issue-Trace-Ids, or with botocore (sd = boto3.client('servicediscovery', endpoint_url=<emulator>)) read e.response['ResponseMetadata']['HTTPStatusCode'] and e.response['Error']:
1. Causes 1/8/9/10: against a service id that does not exist (srv-doesnotexist000) call sd.get_service(Id=...), sd.update_service(Id=..., Service={'DnsConfig':{'DnsRecords':[{'Type':'A','TTL':60}]}}), sd.delete_service(Id=...), sd.list_instances(ServiceId=...) -> emulator 404 with 'No service exists with the specified ID ...'; AWS 400 with, respectively, an empty message, the service ARN, the bare id, and the service ARN.
2. Cause 2: sd.register_instance(ServiceId='srv-doesnotexist000', InstanceId='i1', Attributes={'AWS_INSTANCE_IPV4':'192.0.2.10'}) -> emulator 404; AWS 400 ServiceNotFound whose Message is the service ARN.
3. Cause 3: create a namespace+service, register an instance, then sd.get_instance(ServiceId=svc, InstanceId='<never-registered>') -> emulator 404 with its prose; AWS 400 InstanceNotFound whose Message is just the instance id.
4. Cause 4: with one instance registered, sd.delete_service(Id=svc) -> emulator 409 with its prose; AWS 400 ResourceInUse 'Service contains registered instances; delete the instances before deleting the service'.
5. Causes 5/6/7: sd.get_service_attributes(ServiceId='srv-doesnotexist000'), sd.update_service_attributes(ServiceId='srv-doesnotexist000', Attributes={'k':'v'}), sd.delete_service_attributes(ServiceId='srv-doesnotexist000', Attributes=['k']) -> emulator 404; AWS 400 ServiceNotFound.
