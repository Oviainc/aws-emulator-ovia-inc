Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-create-partner-input-on-regular-input-fails
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-create-partner-input-on-regular-input-fails medialive-input-security-group-lifecycle medialive-rtmp-push-input-lifecycle
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|CreateInputSecurityGroup|2xx->2xx|status
Issue-Key: medialive|DeleteInput|2xx->2xx|$
Issue-Key: medialive|DescribeInputSecurityGroup|2xx->2xx|$.state
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# medialive input and input-security-group lifecycle responses diverge on status, body and state

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInputSecurityGroup

## Description
Three wire defects in the medialive Input / InputSecurityGroup lifecycle: the create returns the wrong success status, the delete returns an empty body instead of the deleted resource, and a deleted security group never reaches its terminal state. This bundle has THREE causes; all must be closed.

Cause 1 of 3: CreateInputSecurityGroup returns HTTP 200 instead of 201 (fixture medialive-create-partner-input-on-regular-input-fails, step setup#1).
AWS: recorded behavior — the recording shows POST /prod/inputSecurityGroups answering HTTP 201 on success. (Smithy declares `@http code 200` for this operation, so the model and the recording disagree; the recorded wire response is the authority here, and the emulator currently follows the model.)
Emulator: answers HTTP 200.
Why: a client or test that asserts on the created-resource status (201) treats the create as a non-create; anything keying retry/idempotency decisions off the exact 2xx code takes the wrong branch.
Fix: the CreateInputSecurityGroup handler's success status.
Traces: medialive-create-partner-input-on-regular-input-fails#setup#1 (also medialive-input-security-group-lifecycle#task#1, medialive-rtmp-push-input-lifecycle#setup#1).

Cause 2 of 3: DeleteInput returns an empty body (fixture medialive-create-partner-input-on-regular-input-fails, step teardown#4).
AWS: recorded behavior — DELETE /prod/inputs/{InputId} answers HTTP 200 with the deleted input's full representation, including `state: 'DELETING'`, plus arn/id/name/type/securityGroups/destinations/inputClass/inputNetworkLocation and the empty collections (sources, mediaConnectFlows, inputDevices, attachedChannels, inputPartnerIds, tags). (Smithy models DeleteInputResponse with no members; the recording shows the populated body, and the recording governs.)
Emulator: answers 200 with `{}`.
Why: a client that reads `state` from the delete response to learn the input entered DELETING gets nothing, so a delete-then-wait flow has no starting state and a caller that dereferences the returned input fails on a null field.
Fix: the DeleteInput handler's response projection — return the deleted input with its state transitioned to DELETING.
Traces: medialive-create-partner-input-on-regular-input-fails#teardown#4 (also medialive-rtmp-push-input-lifecycle#teardown#8).

Cause 3 of 3: a deleted input security group stays IDLE instead of becoming DELETED (fixture medialive-input-security-group-lifecycle, step task#7).
AWS: recorded behavior — after DeleteInputSecurityGroup, DescribeInputSecurityGroup answers HTTP 200 with `state: 'DELETED'`. The InputSecurityGroup state member is an enum whose terminal value for a deleted group is DELETED.
Emulator: answers 200 with `state: 'IDLE'`, i.e. the delete leaves the group looking live and reusable.
Why: a client polling for the terminal DELETED state never sees it and waits forever; one that treats IDLE as 'available' will attach a deleted security group to a new input.
Fix: the DeleteInputSecurityGroup handler's state mutation — set the stored group's state to DELETED so Describe reports it.
Traces: medialive-input-security-group-lifecycle#task#7 (also #task#9).

## Evidence
Replay of simparity-wire-aws-v1 (3 clusters, verdict=mismatch):
- CreateInputSecurityGroup status=201->200: 'status: expected 201, got 200' (3 affected steps).
- DeleteInput 200->200: $ empty_body — expected {'arn': 'arn:aws:medialive:...:input:1000000', 'id': '1000000', 'name': ..., 'type': 'RTMP_PUSH', 'state': 'DELETING', 'securityGroups': ['3000000'], 'destinations': [...], 'inputClass': 'SINGLE_PIPELINE', 'inputNetworkLocation': 'AWS', 'tags': {}, ...}, got {} (2 affected steps).
- DescribeInputSecurityGroup 200->200: $.state value_mismatch (expected 'DELETED', got 'IDLE') (2 affected steps).
Smithy: CreateInputSecurityGroup POST /prod/inputSecurityGroups declares code 200 while the recording shows 201; DeleteInput DELETE /prod/inputs/{InputId} code 200 with an empty modeled output while the recording returns the input. Generated ids/arns/names above are examples.

## Steps to Reproduce
1. POST /prod/inputSecurityGroups with a whitelist rule — observe HTTP 200 where the recording shows 201.
2. POST /prod/inputs to create an RTMP_PUSH input, then DELETE /prod/inputs/<id> — the 200 response body is {} instead of the deleted input with state DELETING.
3. DELETE /prod/inputSecurityGroups/<sgId>, then GET /prod/inputSecurityGroups/<sgId> — state is 'IDLE' instead of 'DELETED'.
4. Equivalent replay: fixture medialive-create-partner-input-on-regular-input-fails step setup#1, plus the steps in trace_ids.
