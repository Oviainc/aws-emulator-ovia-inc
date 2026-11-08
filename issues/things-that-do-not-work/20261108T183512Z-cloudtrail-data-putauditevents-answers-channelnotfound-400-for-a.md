Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail-data.e2e-audit-event-pipeline-blocked-for-new-account
Issue-Shape-Id: com.amazonaws.cloudtraildata#CloudTrailDataService
Issue-Key: cloudtrail-data|PutAuditEvents|ChannelNotFound
Issue-Commit: b668b970d0d9c7dee5ca992802cd99dd932aec82

# cloudtrail-data PutAuditEvents answers ChannelNotFound/400 for a channel CloudTrail CreateChannel just created: the control plane never registers the channel in the data plane's registry

> _Note: this issue was identified against the code base at commit `b668b970d0d9c7dee5ca992802cd99dd932aec82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAuditEvents

## Description
One net-new root cause.

Cause 1 of 1: a channel created by the CloudTrail control plane is invisible to the cloudtrail-data data plane, so the documented custom-events-into-CloudTrail-Lake pipeline can never deliver an event.

AWS: PutAuditEvents ingests a batch onto an EXISTING channel and answers HTTP 200 with both required output members — `successful`, one AuditEventResultEntry per accepted event echoing that event's `id` (the caller's AuditEvent$id) plus the `eventID` CloudTrail assigned, and `failed` empty (Smithy com.amazonaws.cloudtraildata#PutAuditEvents: input `channelArn` required, @pattern ^arn:.*$; `auditEvents` required @length 1..100; output `successful` and `failed` both required; http POST /PutAuditEvents, success 200). ChannelNotFound (errorKind client, HTTP 400, message per recorded behavior `The channel <channelArn> could not be found.` — fixed text with the request's channelArn interpolated) is modelled ONLY for a channel that does not exist; a channel the account just created through cloudtrail:CreateChannel with Source=Custom and an EVENT_DATA_STORE destination exists, so PutAuditEvents against it must ingest, not refuse.

Emulator: the walk CreateEventDataStore -> CreateChannel -> PutAuditEvents now gets past its first two steps (the control-plane 501s that issues/things-that-do-not-work/20261022T215433Z-cloudtrail-data-channel-provisioning-prerequisites-answer-501-notimplemented.md describes are gone), and CreateChannel returns a ChannelArn of the form arn:aws:cloudtrail:<region>:<account>:channel/<uuid>. PutAuditEvents against that exact ARN is then refused with error code `ChannelNotFound`, HTTP 400, message `The channel <that arn> could not be found.` — i.e. the data plane reports the live channel as absent. Code grounding (for the fixer, not part of the requirement): services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/create_channel.rs inserts the new ChannelRecord only into the cloudtrail crate's own registry (`crate::state::channels`, line ~310), while services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/src/ops/put_audit_events.rs resolves `channelArn` against the cloudtraildata crate's registry, whose own module doc (src/state.rs, lines 5-10) says the CloudTrail crate is expected to call `register_channel` on it once CreateChannel is implemented. Nothing ever does, so that registry is permanently empty and every PutAuditEvents call on a real channel takes the not-found branch.

Why: a client that follows the documented ingestion pipeline (create event data store, create a Custom channel, put audit events) can never ingest a single event, and the refusal it gets — ChannelNotFound — tells it the channel it holds the CreateChannel response for does not exist. Error handling branches on an impossible condition: retrying, re-creating the channel or re-resolving the ARN never helps, and a caller cannot distinguish a genuinely absent channel from a live one. It also makes every negative PutAuditEvents result unfalsifiable, since ChannelNotFound is the only answer the operation can ever give.

Fix: the channel registration handoff between the two crates — CreateChannel (and correspondingly DeleteChannel/UpdateChannel) in the cloudtrail crate's ops module must register/deregister the channel in the shared cloudtrail-data channel registry (its `register_channel` state API), or PutAuditEvents' channel resolution in the cloudtrail-data crate must resolve against the control plane's channel registry. The owner account and source carried on the registration are what the data plane's ChannelInsufficientPermission / external-id checks need, so the registration must carry them, not just the ARN.

Traces: cloudtrail-data.e2e-audit-event-pipeline-blocked-for-new-account (the check walks CreateEventDataStore -> CreateChannel -> PutAuditEvents and expects `failed == []` with `successful[0].id` echoing the submitted event id).

## Evidence
Check cloudtrail-data.e2e-audit-event-pipeline-blocked-for-new-account, replayed against the emulator:

1. cloudtrail.create_event_data_store(...) SUCCEEDED and returned an EventDataStoreArn (so the check took its "Lake-capable account" branch).
2. cloudtrail.create_channel(Name=<unique>, Source='Custom', Destinations=[{'Type':'EVENT_DATA_STORE','Location':<that EventDataStoreArn>}], Tags=[...]) SUCCEEDED and returned ChannelArn = arn:aws:cloudtrail:us-west-2:<account>:channel/<uuid>.
3. cloudtrail_data.put_audit_events(channelArn=<that ChannelArn>, auditEvents=[{'id': 'sim2real-<hex>', 'eventData': '<json audit event>'}])
   Expected: HTTP 200, failed == [], successful[0]['id'] == the submitted event id.
   Observed: botocore.errorfactory.ChannelNotFound — "An error occurred (ChannelNotFound) when calling the PutAuditEvents operation: The channel arn:aws:cloudtrail:us-west-2:<account>:channel/<uuid> could not be found." (HTTP 400).

The ARN in the refusal is byte-identical to the ARN CreateChannel returned one call earlier, so the data plane is refusing a channel the control plane holds. (The channel id in the observed run happened to be the first id this process minted; the id value itself is a per-run detail and is not part of the requirement.)

This is distinct from the already-open issue 20261022T215433Z-cloudtrail-data-channel-provisioning-prerequisites-answer-501-notimplemented.md, which is about CreateEventDataStore/CreateChannel answering 501 NotImplemented: both now answer successfully, and the failure has moved to the terminal PutAuditEvents step.

## Steps to Reproduce
Boot the emulator and point botocore `cloudtrail` and `cloudtrail-data` clients at it in us-west-2.
1. eds = cloudtrail.create_event_data_store(Name='repro-eds', AdvancedEventSelectors=[{'Name':'select-activity-audit-log','FieldSelectors':[{'Field':'eventCategory','Equals':['ActivityAuditLog']}]}], MultiRegionEnabled=False, RetentionPeriod=7, TerminationProtectionEnabled=False) -> note eds['EventDataStoreArn'].
2. ch = cloudtrail.create_channel(Name='repro-ch', Source='Custom', Destinations=[{'Type':'EVENT_DATA_STORE','Location': eds['EventDataStoreArn']}]) -> note ch['ChannelArn'].
3. cloudtrail.get_channel(Channel=ch['ChannelArn']) / cloudtrail.list_channels() both report the channel, confirming it exists.
4. cloudtrail_data.put_audit_events(channelArn=ch['ChannelArn'], auditEvents=[{'id': 'repro-1', 'eventData': json.dumps({'version':'1.0','userIdentity':{'type':'CustomUserIdentity','principalId':'p'},'eventSource':'repro.example.com','eventName':'ReproAction','eventTime':'2026-01-01T00:00:00','UID':'repro-uid','recipientAccountId':'<account>'})}])
   Expect HTTP 200 with failed == [] and successful[0]['id'] == 'repro-1'; observe ChannelNotFound / HTTP 400 with 'The channel <ch["ChannelArn"]> could not be found.'
