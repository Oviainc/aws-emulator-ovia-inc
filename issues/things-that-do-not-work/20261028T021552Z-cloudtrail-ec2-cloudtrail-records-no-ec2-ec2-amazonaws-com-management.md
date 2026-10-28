Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__ec2.obs-ec2-3-run-and-terminate-instances-audit-records-join-the-sdk-request-id
Issue-Key: cloudtrail__ec2.obs-ec2-3-run-and-terminate-instances-audit-records-join-the-sdk-request-id|_await_records#33|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: 198aaba66ae988b1c226897aad5a72b32428db18

# CloudTrail records no EC2 (ec2.amazonaws.com) management event, so RunInstances/TerminateInstances can never be joined to the SDK request id through LookupEvents

> _Note: this issue was identified against the code base at commit `198aaba66ae988b1c226897aad5a72b32428db18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1 — a served EC2 management call is never written into the account's management-event log, so CloudTrail event history has no RunInstances or TerminateInstances record to return and a caller polling LookupEvents for its own call never converges.

AWS: every management call of every service is recorded in the Region's event history and is readable through LookupEvents (Smithy, cloudtrail model: LookupEvents, input LookupAttributes/StartTime/EndTime/EventCategory/MaxResults(range 1..50)/NextToken, output Events[].CloudTrailEvent, paginated on NextToken; the operation's own doc enumerates the supported lookup attributes, Event name among them). For an EC2 write the recorded behavior the check measured against real AWS is: one record per call, indexed within a few minutes, with eventSource 'ec2.amazonaws.com', eventName 'RunInstances' / 'TerminateInstances', readOnly false, no errorCode on a success, requestID equal to the value the SDK returned in ResponseMetadata.RequestId, awsRegion equal to the caller's Region, the envelope's EventId equal to the record's eventID, requestParameters echoing what was sent (instancesSet.items[].imageId, instanceType, tagSpecificationSet.items[].tags) and responseElements echoing what EC2 minted (instancesSet.items[].instanceId/subnetId/instanceState.name = 'pending', reservationId), plus an envelope Resources entry ('AWS::EC2::Instance', <instance id>). For TerminateInstances, responseElements.instancesSet.items[].currentState.name is the same 'shutting-down' the SDK saw in the same response. These are per-record field semantics the model does not state — recorded behavior only.

Emulator: event history stays empty for EC2. LookupEvents is implemented and pages out of the shared service_common::management_events log (services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/lookup_events.rs), but that log is only ever written from CloudTrail's own mutating handlers — services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/state.rs documents record_management_event as 'reached from each mutating handler' of this service. EC2's handlers write nothing into it: services/service-com-amazonaws-ec2-amazonec2/src/ops/terminate_instances.rs mutates only crate::state (begin_instance_termination) and never touches the management-event seam, and the same holds for run_instances.rs. The check therefore creates a VPC/subnet, runs and terminates one instance, then polls LookupEvents (EventName=RunInstances and EventName=TerminateInstances, joining on requestID) under a 900 s budget and sees no matching record at all, so it is still polling when the harness's 60 s cap kills it: verdict 'mismatch', detail 'Check timed out after 60.0s'. Hypothesis: the 60 s cap is consumed by that LookupEvents poll rather than by the earlier bounded wait for the instance to reach 'terminated' (terminate_instances.rs does schedule the 'terminated' transition and materialize_due_transitions applies it, so that wait is expected to converge); the absence of any ec2.amazonaws.com record is certain from the code above either way.

Why: a client cannot answer 'who launched or terminated this instance'. The instance-lifecycle audit join a client relies on — correlate the SDK's own ResponseMetadata.RequestId with the audit record, read back the parameters actually sent and the ids EC2 minted, and attribute the terminate to its caller — returns an empty Events list, which is indistinguishable from 'nothing happened'. Every incident reconstruction, compliance-evidence export and automated reaction built on EC2 API-call records is dead, and a poll for one's own record never terminates instead of failing fast.

Fix: the shared management-event recording seam (service_common::management_events / ServedApiCall, the one CloudTrail's state.rs::record_management_event already uses) must be driven from EC2's served operations — RunInstances and TerminateInstances at minimum — populating eventSource/eventName/readOnly/awsRegion/requestID/eventID, requestParameters from the deserialized input and responseElements from the emitted output, and the audited resource list naming AWS::EC2::Instance for each instance id, so CloudTrail's LookupEvents can serve them.

Related but NOT covering this: 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md describes the same shared seam for the dynamodb/s3/iam/lambda/elbv2/cloudformation/codepipeline/cognito-idp legs, and 20261024T030130Z-cloudtrail-rds-... for the RDS leg; neither issue's replay set names cloudtrail__ec2.obs-ec2-3-run-and-terminate-instances-audit-records-join-the-sdk-request-id, so the EC2 leg is not covered by them and is filed here.

Traces: cloudtrail__ec2.obs-ec2-3-run-and-terminate-instances-audit-records-join-the-sdk-request-id.

## Evidence
Failing check cloudtrail__ec2.obs-ec2-3-run-and-terminate-instances-audit-records-join-the-sdk-request-id — verdict 'mismatch', detail 'Check timed out after 60.0s'.

Expected (real AWS, as the check asserts): after a successful RunInstances and TerminateInstances, LookupEvents over a window covering them returns one record for each; run_rec['eventSource'] == 'ec2.amazonaws.com', run_rec['eventName'] == 'RunInstances', run_rec['requestID'] == run_resp['ResponseMetadata']['RequestId'], run_rec['readOnly'] is False, 'errorCode' not in run_rec, run_env['EventId'] == run_rec['eventID'], requestParameters.instancesSet.items[0].imageId == the ImageId sent, requestParameters.instanceType == 't3.micro', requestParameters.tagSpecificationSet carrying the tags sent, responseElements.instancesSet.items[0].instanceId == the created instance id with instanceState.name == 'pending' and subnetId == the subnet sent, responseElements.reservationId == run_resp['ReservationId'], and the envelope Resources containing ('AWS::EC2::Instance', <instance id>); the TerminateInstances record likewise with its own distinct EventId and responseElements...currentState.name == 'shutting-down'. The check's own failure text for this case: "DATAPLANE_UNIMPLEMENTED: CloudTrail event history never produced a record for <names> after <N>s (requestIDs ..., instance i-...). Real AWS indexes an EC2 management event within a few minutes."

Actual (emulator): no record for either event name ever appears, so the check's _await_records loop (budget _INDEX_BUDGET_S = 900.0, _POLL_INTERVAL_S = 20.0, paging LookupEvents to exhaustion) never completes and the harness's 60 s cap ends the check before any field assertion runs.

Corroborating code in the checkout: services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/lookup_events.rs serves pages from service_common::management_events; services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/state.rs lines 12-17 state the shared log is written by record_management_event 'reached from each mutating handler' of CloudTrail itself; services/service-com-amazonaws-ec2-amazonec2/src/ops/terminate_instances.rs handle() (lines 175-253) writes only EC2's own store and never records a served API call.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. ec2 CreateVpc (10.42.0.0/16), CreateSubnet (10.42.1.0/24) in an available AZ, resolve an Amazon-owned al2023 x86_64 AMI with DescribeImages.
3. ec2 RunInstances(ImageId=<ami>, InstanceType=t3.micro, MinCount=1, MaxCount=1, SubnetId=<subnet>, TagSpecifications=[instance+volume tags]); keep run_resp['ResponseMetadata']['RequestId'], run_resp['ReservationId'] and the instance id.
4. ec2 TerminateInstances(InstanceIds=[<instance id>]); keep that response's ResponseMetadata.RequestId (it must differ from step 3's).
5. cloudtrail LookupEvents(StartTime=<just before step 3>, EndTime=<now+5min>, MaxResults=50, LookupAttributes=[{AttributeKey: 'EventName', AttributeValue: 'RunInstances'}]), paging NextToken to exhaustion; repeat with AttributeValue 'TerminateInstances'.
6. Expect one record per call, selected by requestID equal to the request id from steps 3/4, with eventSource 'ec2.amazonaws.com', readOnly false, no errorCode, the requestParameters/responseElements content listed in Evidence, and an envelope Resources entry ('AWS::EC2::Instance', <instance id>).
7. Observe Events: [] for both lookups, no matter how long the poll runs.
