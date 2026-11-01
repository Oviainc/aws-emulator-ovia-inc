Issue-Source: trace-sweep-divergence
Issue-Trace: securityagent-agent-space-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityagent-agent-space-lifecycle securityagent-agent-space-target-domain-association
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|BatchGetAgentSpaces|2xx->2xx|$.agentInstances[*]
Issue-Key: securityagent|BatchGetAgentSpaces|2xx->2xx|$.agentSpaces[*].targetDomainIds
Issue-Key: securityagent|BatchGetAgentSpaces|2xx->2xx|$.agentSpaces[*].targetDomains
Issue-Key: securityagent|BatchGetAgentSpaces|2xx->2xx|$.ids[*]
Issue-Key: securityagent|BatchGetAgentSpaces|2xx->2xx|$.agentSpaces[*].codeReviewSettings.generalPurposeScanning
Issue-Key: securityagent|BatchGetAgentSpaces|2xx->2xx|$.agentSpaces[*].description
Issue-Key: securityagent|BatchGetAgentSpaces|2xx->2xx|$.agentSpaces[*]
Issue-Key: securityagent|BatchGetAgentSpaces|2xx->2xx|$.notFound[*]
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# BatchGetAgentSpaces returns stale, incomplete agent space state

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetAgentSpaces

## Description
Four distinct causes, all reached through BatchGetAgentSpaces on an agent space the same fixture just created, updated or deleted. The operation's read path builds its response from an incomplete projection of agent-space state and ignores subsequent mutations, so every one of them must be closed before this issue is done.

Cause 1 of 4 - BatchGetAgentSpaces drops the agentInstances and ids response members and nulls empty list members.
AWS: recorded behavior only. The Smithy model's BatchGetAgentSpacesOutput declares only agentSpaces + notFound (smithy_query operation=BatchGetAgentSpaces), but the recording shows the live service additionally returning a top-level `agentInstances` list - one entry per requested space, carrying id, agentSpaceId, ownerId, arn (arn:aws:securityagent:<region>:<account>:agent-instance/<agentSpaceId>), name, description, awsResources, targetDomains, targetDomainIds, kmsKeyId, createdAt, updatedAt, codeReviewSettings - and a top-level `ids` list of the resolved agent space ids. The recording also shows the per-space list members `targetDomains` and `targetDomainIds` present as empty arrays [] when the space has no associated domain, never absent/null; `targetDomains` is itself unmodeled (AgentSpace in the model has targetDomainIds but no targetDomains) and is present in the recorded payload.
Emulator: returns neither member - $.agentInstances[0] and $.ids[0] are absent (got None) - and emits null instead of [] for $.agentSpaces[0].targetDomains and $.agentSpaces[0].targetDomainIds. Status is 200 in both, so the divergence is purely in the body.
Why: a client reading the agent-instance view (arn, ownerId, instance id) off a batch-get gets nothing to read and either NPEs or treats the space as having no instance; a client iterating targetDomainIds to enumerate associated domains dereferences null rather than looping zero times, which is an error instead of the empty case AWS gives it.
Fix: the BatchGetAgentSpaces handler's response serializer - project the full agent-instance view alongside agentSpaces, emit the resolved id list, and serialize absent list members as empty lists rather than null.
Traces: securityagent-agent-space-lifecycle#task#3.

Cause 2 of 4 - the same omission occurs for a space that DOES have an associated target domain, so it is not merely an empty-collection bug.
AWS: recorded behavior only. The recording for a space with one associated domain returns agentInstances[0] fully populated, including targetDomains[0] with targetDomainId, domainName, createdAt, verifiedAt, status, verificationStatus, verificationStatusReason and verificationDetails (method DNS_TXT plus dnsTxt token/dnsRecordName/dnsRecordType), and targetDomainIds listing that domain's id; `ids` again lists the resolved agent space id.
Emulator: $.agentInstances[0] and $.ids[0] are absent (got None) exactly as in cause 1, so the domain association is invisible on this read path even though the association itself succeeded earlier in the fixture.
Why: a client that associates a target domain and then batch-gets the space to read back the verification material (the DNS TXT record name and token it must publish) has no member to read it from and cannot complete domain verification.
Fix: same component as cause 1 - the BatchGetAgentSpaces response serializer must populate agentInstances (including its nested targetDomains projection) and ids from state, not only for empty spaces.
Traces: securityagent-agent-space-target-domain-association#task#3.

Cause 3 of 4 - UpdateAgentSpace mutations are not visible to a later BatchGetAgentSpaces.
AWS: Smithy: UpdateAgentSpace accepts name, description, awsResources, targetDomainIds and codeReviewSettings for an agentSpaceId and returns the updated view; AgentSpace.description and AgentSpace.codeReviewSettings are the stored fields a later read returns. A read-after-write therefore reflects the values the update supplied, and updatedAt advances.
Emulator: after an UpdateAgentSpace that set description and turned codeReviewSettings.generalPurposeScanning on, BatchGetAgentSpaces still returns the creation-time values - $.agentSpaces[0].description is the original creation description instead of the updated one, and $.agentSpaces[0].codeReviewSettings.generalPurposeScanning is false where the update set it true (plus the cause 1/2 omissions on the same response).
Why: the update is acknowledged but not durable, so a client that configures a space and then verifies the configuration sees its change silently lost, and any workflow gated on reading back the new setting never proceeds.
Fix: UpdateAgentSpace's state mutation - persist the supplied members (and bump updatedAt) onto the stored agent space that the read path serves.
Traces: securityagent-agent-space-lifecycle#task#5.

Cause 4 of 4 - a deleted agent space is still returned by BatchGetAgentSpaces instead of being reported in notFound.
AWS: Smithy: BatchGetAgentSpacesOutput.notFound is an AgentSpaceIdList. Per the recording, an id that does not resolve (here, one deleted earlier in the fixture) is omitted from agentSpaces and listed in notFound; the batch still answers 200 rather than erroring.
Emulator: after the space is deleted, the response still carries $.agentSpaces[0] with the full (pre-delete) space body and leaves $.notFound empty - $.notFound[0] is absent where the deleted id was expected.
Why: delete is not reflected in the read path, so a client that polls batch-get to confirm teardown never observes the deletion, and a client that treats presence in agentSpaces as existence operates on a resource that is gone.
Fix: the agent space state store and the BatchGetAgentSpaces handler's lookup - DeleteAgentSpace must remove (or tombstone) the space, and unresolved ids must be routed into notFound rather than served from stale state.
Traces: securityagent-agent-space-lifecycle#teardown#7.

## Evidence
Cause 1 (securityagent-agent-space-lifecycle#task#3, BatchGetAgentSpaces, expected 200 / actual 200): $.agentInstances[0] missing (expected the full agent-instance object: id, agentSpaceId, ownerId, arn arn:aws:securityagent:<region>:<account>:agent-instance/<agentSpaceId>, name, description, awsResources, targetDomains, targetDomainIds, kmsKeyId, createdAt, updatedAt, codeReviewSettings; got None); $.ids[0] missing (expected the resolved agent space id, got None); $.agentSpaces[0].targetDomains missing (expected [], got None); $.agentSpaces[0].targetDomainIds missing (expected [], got None).

Cause 2 (securityagent-agent-space-target-domain-association#task#3, BatchGetAgentSpaces, 200 / 200): $.agentInstances[0] missing - expected object includes targetDomains[0] = {targetDomainId, domainName, createdAt, verifiedAt: null, status: PENDING, verificationStatus: PENDING, verificationStatusReason: "Target domain is pending ownership verification. Add the DNS TXT record to the DNS resolver associated with your domain before attempting verification", verificationDetails: {method: DNS_TXT, dnsTxt: {token, dnsRecordName _aws_securityagent-challenge.<domainName>, dnsRecordType TXT}, httpRoute: null}} and targetDomainIds listing that domain id; got None. $.ids[0] missing (got None).

Cause 3 (securityagent-agent-space-lifecycle#task#5, BatchGetAgentSpaces, 200 / 200): $.agentSpaces[0].description expected the value set by the preceding UpdateAgentSpace, got the creation-time description; $.agentSpaces[0].codeReviewSettings.generalPurposeScanning expected true (set by the update), got false. Same response also missing $.agentInstances[0], $.ids[0] and emitting null for targetDomains/targetDomainIds.

Cause 4 (securityagent-agent-space-lifecycle#teardown#7, BatchGetAgentSpaces, 200 / 200): $.agentSpaces[0] extra - expected None, got the deleted space's full body (agentSpaceId, name, description, codeReviewSettings, createdAt, updatedAt, id, ownerId, arn); $.notFound[0] missing - expected the requested (deleted) agent space id, got None. This cluster also covers securityagent-agent-space-target-domain-association#teardown#7.

All four replayed against an emulator reporting healthy (/__admin/health 200, data listener up); no other open securityagent issue covers them (no existing issues match 'securityagent').

## Steps to Reproduce
1. git clone the repo and boot the emulator from that checkout.
2. Replay fixture securityagent-agent-space-lifecycle. At setup#1 CreateAgentSpace succeeds; at task#3 call BatchGetAgentSpaces with that agentSpaceId and compare the body against the recording - agentInstances, ids are absent and targetDomains/targetDomainIds come back null instead of [] (cause 1).
3. In the same fixture, task#4 UpdateAgentSpace sets a new description and codeReviewSettings.generalPurposeScanning=true; replay task#5 BatchGetAgentSpaces and observe the pre-update description and generalPurposeScanning=false (cause 3).
4. Continue to teardown#7: after the space is deleted, BatchGetAgentSpaces for its id still returns it in agentSpaces and leaves notFound empty (cause 4).
5. Replay fixture securityagent-agent-space-target-domain-association to task#3: with a target domain associated, BatchGetAgentSpaces still omits agentInstances (and its targetDomains verification material) and ids (cause 2).
