Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcampaigns.create-campaign-initial-state-initialized connectcampaigns.create-campaign-reserved-tag-key-rejected connectcampaigns.delete-campaign-removes-resource connectcampaigns.delete-connect-instance-config-active-campaign-rejected connectcampaigns.get-campaign-state-batch-partial-not-found connectcampaigns.pause-campaign-running-to-paused connectcampaigns.pause-campaign-wrong-state-rejected connectcampaigns.put-dial-request-batch-stopped-campaign-rejected connectcampaigns.resume-campaign-already-running-is-idempotent connectcampaigns.resume-campaign-paused-to-running connectcampaigns.start-campaign-transitions-to-running connectcampaigns.start-instance-onboarding-job-conflict-when-already-onboarded-rejected connectcampaigns.stop-campaign-terminal-state connectcampaigns.tag-untag-resource-lifecycle connectcampaigns.update-campaign-dialer-config-conflict-while-running-accepted connectcampaigns.update-campaign-name-persists connectcampaigns.update-campaign-outbound-call-config-persists connectcampaigns.put-dial-request-batch-duplicate-client-token-dedup
Issue-Shape-Id: com.amazonaws.connectcampaigns#AmazonConnectCampaignService
Issue-Key: connectcampaigns.create-campaign-initial-state-initialized|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.create-campaign-reserved-tag-key-rejected|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.delete-campaign-removes-resource|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.delete-connect-instance-config-active-campaign-rejected|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.get-campaign-state-batch-partial-not-found|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.pause-campaign-running-to-paused|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.pause-campaign-wrong-state-rejected|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.put-dial-request-batch-stopped-campaign-rejected|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.resume-campaign-already-running-is-idempotent|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.resume-campaign-paused-to-running|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.start-campaign-transitions-to-running|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.start-instance-onboarding-job-conflict-when-already-onboarded-rejected|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.stop-campaign-terminal-state|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.tag-untag-resource-lifecycle|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.update-campaign-dialer-config-conflict-while-running-accepted|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.update-campaign-name-persists|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.update-campaign-outbound-call-config-persists|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default contact flows")
Issue-Key: connectcampaigns.put-dial-request-batch-duplicate-client-token-dedup|_provision_campaign_fixture#1|AssertionError|raise AssertionError("newly created Connect instance has no default standard queue")
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Connect CreateInstance provisions no default resources: a new CONNECT_MANAGED instance has no default contact flows and no default STANDARD queue, blocking every ConnectCampaigns campaign fixture

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInstance

## Description
This issue bundles 2 root causes, both in the Amazon Connect emulator's CreateInstance / instance-default provisioning. They are grounded in connectcampaigns behavioral checks because those checks build their campaign fixture on a freshly created Connect instance and never reach a ConnectCampaigns call; the defect itself lives in the connect service crate. These 18 checks are NOT covered by the open issue '20261022T145747Z-connectcampaigns-is-entirely-unreachable-router-resolves-services-by.md' (AmbiguousService router collision) — they fail strictly earlier, inside Connect fixture provisioning, and that issue does not carry their replay keys. They are also not the separately filed unimplemented-ListHoursOfOperations scaffold item (that blocks only connectcampaigns.put-dial-request-batch-before-start-rejected).

Cause 1 of 2: a newly created Connect instance exposes NO default contact flows (17 failing checks: connectcampaigns.create-campaign-initial-state-initialized, create-campaign-reserved-tag-key-rejected, delete-campaign-removes-resource, delete-connect-instance-config-active-campaign-rejected, get-campaign-state-batch-partial-not-found, pause-campaign-running-to-paused, pause-campaign-wrong-state-rejected, put-dial-request-batch-stopped-campaign-rejected, resume-campaign-already-running-is-idempotent, resume-campaign-paused-to-running, start-campaign-transitions-to-running, start-instance-onboarding-job-conflict-when-already-onboarded-rejected, stop-campaign-terminal-state, tag-untag-resource-lifecycle, update-campaign-dialer-config-conflict-while-running-accepted, update-campaign-name-persists, update-campaign-outbound-call-config-persists).
AWS: CreateInstance (Smithy com.amazonaws.connect#CreateInstance, PUT /instance, 200; input IdentityManagementType=CONNECT_MANAGED, InboundCallsEnabled, OutboundCallsEnabled required) stands up a complete contact center: AWS documents that a new instance ships with a set of default/sample contact flows (sample inbound flow, default agent/customer whisper, default/agent hold flows, …). Consequently ListContactFlows (Smithy com.amazonaws.connect#ListContactFlows, GET /contact-flows-summary/{InstanceId}, output ContactFlowSummaryList) on an instance that has just become ACTIVE returns a NON-EMPTY ContactFlowSummaryList without the caller having created anything. The Smithy model documents the operations but not the default inventory, so for the specific inventory this is recorded/documented AWS behavior only: the requirement is that the list is non-empty for a fresh CONNECT_MANAGED instance, not any particular flow name or id.
Emulator: CreateInstance stores the instance with an empty contact-flow map, so ListContactFlows(InstanceId=<new instance>, ContactFlowTypes=["CONTACT_FLOW"]) returns HTTP 200 with ContactFlowSummaryList = [] forever until the client creates a flow itself. Every check above dies in fixture setup with AssertionError: 'newly created Connect instance has no default contact flows'.
Why: any client that discovers a flow to route contacts to — which is the normal Connect bootstrap path and what ConnectCampaigns needs for outboundCallConfig.connectContactFlowId — finds nothing to select and cannot proceed. The entire ConnectCampaigns state-machine, tagging, batch-read and update surface becomes unreachable even once the router/AmbiguousService defect is fixed, because no campaign can be created at all.
Fix: the Connect CreateInstance handler / instance state seeding (the instance record's contact-flow collection), so that a CONNECT_MANAGED instance is born with the default contact-flow set and ListContactFlows/DescribeContactFlow answer from it.

Cause 2 of 2: a newly created Connect instance exposes NO default STANDARD queue (1 failing check: connectcampaigns.put-dial-request-batch-duplicate-client-token-dedup).
AWS: a new instance is also provisioned with a default standard queue (documented as 'BasicQueue', attached to the default hours of operation), so ListQueues (Smithy com.amazonaws.connect#ListQueues, GET /queues-summary/{InstanceId}, input QueueTypes with values STANDARD|AGENT, output QueueSummaryList) with QueueTypes=["STANDARD"] returns a non-empty QueueSummaryList on a fresh instance. Recorded/documented behavior only — the model states the shapes, not the default inventory; the requirement is a non-empty STANDARD list, not a particular queue name or id.
Emulator: the instance record is created with an empty queue map, so ListQueues(QueueTypes=["STANDARD"]) returns HTTP 200 with QueueSummaryList = [] and the check fails in fixture setup with AssertionError: 'newly created Connect instance has no default standard queue'.
Why: a client that routes to the instance's default queue (and ConnectCampaigns progressive/predictive dialer configs that need a real connectQueueId) has nothing to reference; a caller cannot even work around it by creating a queue, because CreateQueue requires a HoursOfOperationId and the default hours-of-operation inventory is likewise absent (ListHoursOfOperations is an unimplemented scaffold, filed separately).
Fix: the same Connect CreateInstance handler / instance state seeding, extended to the default queue (and its hours-of-operation linkage) so ListQueues/DescribeQueue answer for a fresh instance.

Traces: see Evidence and trace_ids; all 18 ids are the failing connectcampaigns checks listed above.

## Evidence
Expected (AWS):
- CreateInstance(IdentityManagementType=CONNECT_MANAGED, InboundCallsEnabled=true, OutboundCallsEnabled=true) -> HTTP 200 with Id/Arn; once DescribeInstance reports InstanceStatus=ACTIVE, the instance already owns default resources.
- ListContactFlows(InstanceId=<that id>, ContactFlowTypes=["CONTACT_FLOW"], MaxResults=25) -> HTTP 200 with a NON-EMPTY ContactFlowSummaryList (the default/sample flows), each entry carrying an Id usable as ConnectCampaigns outboundCallConfig.connectContactFlowId.
- ListQueues(InstanceId=<that id>, QueueTypes=["STANDARD"]) -> HTTP 200 with a NON-EMPTY QueueSummaryList (the default standard queue).

Actual (emulator):
- ListContactFlows returns HTTP 200 with ContactFlowSummaryList = [] -> 17 checks abort in fixture provisioning with `AssertionError: newly created Connect instance has no default contact flows`, e.g.
  connectcampaigns.create-campaign-initial-state-initialized [mismatch]: File "check_connectcampaigns_create_campaign_initial_state_initialized.py", line 87, in _provision_campaign_fixture -> AssertionError: newly created Connect instance has no default contact flows
  connectcampaigns.start-campaign-transitions-to-running [mismatch]: same assertion
  connectcampaigns.update-campaign-outbound-call-config-persists [mismatch]: same assertion
- ListQueues(QueueTypes=["STANDARD"]) returns HTTP 200 with QueueSummaryList = [] ->
  connectcampaigns.put-dial-request-batch-duplicate-client-token-dedup [mismatch]: File "check_connectcampaigns_put_dial_request_batch_duplicate_client_token_dedup.py", line ~, in _provision_campaign_fixture -> AssertionError: newly created Connect instance has no default standard queue

Code evidence (handler seeds no defaults): services/service-com-amazonaws-connect-amazonconnectservice/src/ops/create_instance.rs lines 220-241 build the InstanceRecord with `queues: BTreeMap::new()` and `contact_flows: BTreeMap::new()` and insert it as-is; nothing else in the create path populates either collection.

Note: no HTTP status or error code diverges here — both operations answer 200 — the divergence is the empty default inventory in an otherwise successful response.

## Steps to Reproduce
1. Boot the emulator from mainline (default build) and point a boto3 `connect` client at it.
2. instance_id = connect.create_instance(IdentityManagementType="CONNECT_MANAGED", InstanceAlias="ccfixture1", InboundCallsEnabled=True, OutboundCallsEnabled=True)["Id"]
3. Poll connect.describe_instance(InstanceId=instance_id)["Instance"]["InstanceStatus"] until it is "ACTIVE".
4. Cause 1: connect.list_contact_flows(InstanceId=instance_id, ContactFlowTypes=["CONTACT_FLOW"], MaxResults=25) -> observe ContactFlowSummaryList == [] (AWS returns the instance's default/sample flows).
5. Cause 2: connect.list_queues(InstanceId=instance_id, QueueTypes=["STANDARD"]) -> observe QueueSummaryList == [] (AWS returns the default standard queue).
6. Re-replay the 18 check ids in trace_ids to confirm a fix; the 17 contact-flow checks and the 1 queue check must both get past `_provision_campaign_fixture`.
