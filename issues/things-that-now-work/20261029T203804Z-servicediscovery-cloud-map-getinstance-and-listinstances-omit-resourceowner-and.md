Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-http-namespace-service-instance-discovery-lifecycle
Issue-Step: task#13
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-http-namespace-service-instance-discovery-lifecycle servicediscovery-instance-registration-error-recovery servicediscovery-multi-service-instance-isolation-and-revision-tracking
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|GetInstance|2xx->2xx|$.Instance.CreatedByAccount
Issue-Key: servicediscovery|GetInstance|2xx->2xx|$.ResourceOwner
Issue-Key: servicediscovery|ListInstances|2xx->2xx|$.Instances[Id="sim2real-probe-servicediscovery-instance-d88ded80"].CreatedByAccount
Issue-Key: servicediscovery|ListInstances|2xx->2xx|$.ResourceOwner
Issue-Key: servicediscovery|ListInstances|2xx->2xx|$.Instances[Id="sim2real-probe-servicediscovery-err-instance-2baf6874"].CreatedByAccount
Issue-Key: servicediscovery|ListInstances|2xx->2xx|$.Instances[Id="sim2real-probe-servicediscovery-inst-a1-43beb973"].CreatedByAccount
Issue-Key: servicediscovery|ListInstances|2xx->2xx|$.Instances[Id="sim2real-probe-servicediscovery-inst-a2-43beb973"].CreatedByAccount
Issue-Key: servicediscovery|ListInstances|2xx->2xx|$.Instances[Id="sim2real-probe-servicediscovery-inst-b1-43beb973"].CreatedByAccount
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# Cloud Map GetInstance and ListInstances omit ResourceOwner and CreatedByAccount

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
SIX independent, net-new wire divergences in the servicediscovery (Cloud Map) emulator: the instance read paths return HTTP 200 but omit the modelled ownership members - the response-level ResourceOwner and the per-instance CreatedByAccount. They are separate replay keys over distinct operations and instance-set shapes (single read, single-entry page, EMPTY page, multi-entry page), so each must be closed before this issue is submitted. Model citations use services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json.

Shared AWS fact: AWSAccountId is a 12-character account id; GetInstanceResponse and ListInstancesResponse both declare a top-level ResourceOwner member, Instance declares CreatedByAccount and InstanceSummary declares CreatedByAccount ('the ID of the account that registered the instance'). On a single-account call all of them carry the CALLER'S own account id - the emulator's configured account, not a literal from a recording.

Cause 1 of 6: GetInstance omits the response-level ResourceOwner and Instance.CreatedByAccount.
AWS: Smithy GetInstance{ServiceId: Arn (required), InstanceId: ResourceId (required)} -> {ResourceOwner: AWSAccountId, Instance: Instance{Id (required), CreatorRequestId, Attributes, CreatedByAccount: AWSAccountId}}. Recorded: both present.
Emulator: HTTP 200 with Instance{Id, Attributes, ...} only - no top-level ResourceOwner, no Instance.CreatedByAccount.
Why: a client that reads either member (cross-account instance provenance) gets a KeyError where AWS always supplies it.
Fix: GetInstance's response serializer plus the instance record in the service state (it must store the registering account).
Traces: servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#13; same cluster also covers servicediscovery-instance-registration-error-recovery#task#7.

Cause 2 of 6: ListInstances omits the response-level ResourceOwner and each summary's CreatedByAccount on a single-entry page.
AWS: Smithy ListInstances{ServiceId (required), NextToken, MaxResults 1..100} -> {ResourceOwner: AWSAccountId, Instances: InstanceSummaryList of InstanceSummary{Id, Attributes, CreatedByAccount}, NextToken}. Recorded: ResourceOwner at the response level and CreatedByAccount on the single summary.
Emulator: HTTP 200, both members absent.
Why: as cause 1, for the list path.
Fix: ListInstances' response serializer and the InstanceSummary projection.
Traces: servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#14.

Cause 3 of 6: ListInstances omits ResourceOwner even when the page is EMPTY.
AWS: ResourceOwner is a response-level member and is present independently of the Instances list; the recording shows it on a response whose Instances list is empty.
Emulator: HTTP 200 with an empty Instances list and no ResourceOwner member, confirming the field is not emitted at all rather than derived per instance.
Why: a caller cannot read the owner of an empty-but-existing service listing; this is the path that proves the member is unconditional and must be fixed on the response, not on the summaries.
Fix: ListInstances' response serializer (populate ResourceOwner before/independently of the item projection).
Traces: servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#17; same cluster also covers servicediscovery-instance-registration-error-recovery#task#14.

Cause 4 of 6: ListInstances omits ResourceOwner and CreatedByAccount after a failed-then-recovered registration sequence.
AWS: as cause 2. Recorded: the surviving instance's summary carries CreatedByAccount and the response carries ResourceOwner.
Emulator: both absent.
Why/Fix: as cause 2; this is a distinct replay key reached through the error-recovery fixture, so closing cause 2 alone leaves it unverified.
Traces: servicediscovery-instance-registration-error-recovery#task#9.

Cause 5 of 6: ListInstances omits CreatedByAccount on EVERY summary of a multi-entry page (and the response-level ResourceOwner).
AWS: as cause 2, with CreatedByAccount on each of the two summaries.
Emulator: HTTP 200 with both instances listed, each missing CreatedByAccount, and no ResourceOwner.
Why: the gap is per item, so it scales with the page; owner-based filtering over a page is impossible.
Fix: the InstanceSummary projection in ListInstances (applied to every item).
Traces: servicediscovery-multi-service-instance-isolation-and-revision-tracking#task#12.

Cause 6 of 6: the same omissions on the second service's listing in the multi-service isolation fixture.
AWS: as cause 2.
Emulator: summary missing CreatedByAccount, response missing ResourceOwner.
Why/Fix: as cause 5; separate replay key over a different service's instance set.
Traces: servicediscovery-multi-service-instance-isolation-and-revision-tracking#task#13.

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: 168 steps, 89 passed, 79 mismatched, score 53.0%. All six clusters are status-matched 200 responses with missing members (expected = recorded AWS response; the recorded account id is the caller's own account):

Cause 1 - servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#13, GetInstance: '$.Instance.CreatedByAccount: missing (expected the caller account id, got None)'; '$.ResourceOwner: missing (expected the caller account id, got None)'. 2 steps affected.
Cause 2 - ...#task#14, ListInstances: '$.Instances[Id="<instance id>"].CreatedByAccount: missing'; '$.ResourceOwner: missing'.
Cause 3 - ...#task#17, ListInstances: '$.ResourceOwner: missing' on a response whose Instances list is empty. 2 steps affected.
Cause 4 - servicediscovery-instance-registration-error-recovery#task#9, ListInstances: '$.Instances[Id="<instance id>"].CreatedByAccount: missing'; '$.ResourceOwner: missing'.
Cause 5 - servicediscovery-multi-service-instance-isolation-and-revision-tracking#task#12, ListInstances: '.CreatedByAccount: missing' for both listed instances; '$.ResourceOwner: missing'.
Cause 6 - servicediscovery-multi-service-instance-isolation-and-revision-tracking#task#13, ListInstances: '.CreatedByAccount: missing' for the listed instance; '$.ResourceOwner: missing'.

## Steps to Reproduce
Boot the emulator from this checkout and replay the fixtures/steps in Issue-Trace/Issue-Step and Issue-Trace-Ids, or with botocore (sd = boto3.client('servicediscovery', endpoint_url=<emulator>)):
1. Create an HTTP namespace (poll its operation to SUCCESS), create a service in it, and register one instance: sd.register_instance(ServiceId=svc, InstanceId='i1', Attributes={'AWS_INSTANCE_IPV4':'192.0.2.10'}); poll the operation to SUCCESS.
2. Cause 1: sd.get_instance(ServiceId=svc, InstanceId='i1') -> response has no 'ResourceOwner' and Instance has no 'CreatedByAccount'; AWS returns both as the caller's account id.
3. Causes 2/4/5/6: sd.list_instances(ServiceId=svc) -> response has no 'ResourceOwner' and no summary carries 'CreatedByAccount'; register a second instance and a second service with its own instance and repeat to see the omission per item and per service.
4. Cause 3: deregister every instance (poll to SUCCESS) and call sd.list_instances(ServiceId=svc) again -> Instances == [] and still no 'ResourceOwner', where AWS returns the member on the empty page.
