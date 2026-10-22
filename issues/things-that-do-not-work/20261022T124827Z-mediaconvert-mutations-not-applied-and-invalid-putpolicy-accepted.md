Issue-Source: trace-sweep-divergence
Issue-Trace: mediaconvert-policy-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediaconvert-policy-lifecycle mediaconvert-preset-lifecycle mediaconvert-queue-lifecycle
Issue-Shape-Id: com.amazonaws.mediaconvert#MediaConvert
Issue-Key: mediaconvert|PutPolicy|4xx->2xx|$.accountId
Issue-Key: mediaconvert|PutPolicy|4xx->2xx|$.errorType
Issue-Key: mediaconvert|PutPolicy|4xx->2xx|$.httpStatus
Issue-Key: mediaconvert|PutPolicy|4xx->2xx|$.message
Issue-Key: mediaconvert|PutPolicy|4xx->2xx|$.operation
Issue-Key: mediaconvert|PutPolicy|4xx->2xx|$.requestId
Issue-Key: mediaconvert|PutPolicy|4xx->2xx|$.settingsValidationErrorsJsonBlob
Issue-Key: mediaconvert|PutPolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mediaconvert|PutPolicy|4xx->2xx|status
Issue-Key: mediaconvert|GetPreset|4xx->2xx|$
Issue-Key: mediaconvert|GetPreset|4xx->2xx|$.accountId
Issue-Key: mediaconvert|GetPreset|4xx->2xx|$.errorType
Issue-Key: mediaconvert|GetPreset|4xx->2xx|$.httpStatus
Issue-Key: mediaconvert|GetPreset|4xx->2xx|$.message
Issue-Key: mediaconvert|GetPreset|4xx->2xx|$.operation
Issue-Key: mediaconvert|GetPreset|4xx->2xx|$.requestId
Issue-Key: mediaconvert|GetPreset|4xx->2xx|$.settingsValidationErrorsJsonBlob
Issue-Key: mediaconvert|GetPreset|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mediaconvert|GetPreset|4xx->2xx|status
Issue-Key: mediaconvert|GetQueue|2xx->2xx|$.queue.description
Issue-Key: mediaconvert|GetQueue|2xx->2xx|$.queue.status
Issue-Related: 20261022T124827Z-mediaconvert-resource-projections-omit-default-members-aws-always.md
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# MediaConvert mutations not applied and invalid PutPolicy accepted

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutPolicy

## Description
This issue bundles 3 distinct net-new WIRE root causes with one shared fault: a request that CHANGES account or resource state is answered as if it succeeded while the state the next read returns is wrong - an invalid policy is stored instead of rejected, a deleted preset is still readable, and an updated queue reads back unchanged. All 3 must be closed before submitting. The replay reports owned_by=[] and no stub_path for all three clusters.

=== Cause 1 of 3: PutPolicy accepts a policy that DISALLOWS every input location; AWS rejects it with BadRequestException (400) ===
AWS: Smithy: PutPolicy is PUT /2017-08-29/policy with success code 200, input Policy @required (jsonName 'policy') and output Policy; BadRequestException is modeled at httpError 400. The Policy shape has exactly three members, each targeting the InputPolicy enum (ALLOWED|DISALLOWED): S3Inputs ('s3Inputs'), HttpInputs ('httpInputs'), HttpsInputs ('httpsInputs'). Recorded behavior: a PutPolicy that sets ALL THREE to DISALLOWED is REJECTED - HTTP 400, errorType 'BadRequestException', header x-amzn-errortype 'BadRequestException' - and no policy is stored. The message rule is known (AWS states the constraint: a policy requires at least one input location to be ALLOWED) and the recorded text is an example of it: 'You set your input policy to DISALLOWED for all input locations: S3Inputs, HttpInputs, and HttpsInputs. Policies require at least one input location to be set to ALLOWED. Set at least one of S3Inputs, HttpInputs, or HttpsInputs to ALLOWED and resubmit your policy.' The requirement is the 400 / BadRequestException pair plus a message naming the all-DISALLOWED constraint; the exact prose is not.
Emulator: HTTP 200 and the all-DISALLOWED policy is STORED and echoed back. services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/put_policy.rs:171 builds `crate::state::PolicyRecord::from_requested(&input.policy)` and assigns it unconditionally - the handler performs no cross-member check, relying only on the per-member enum validation the boundary already did.
Why: this is an over-permissive acceptance of a request AWS refuses. A client validating its policy wiring gets a green 200 for a configuration AWS will not accept, so a misconfiguration (an account that would allow no inputs at all, blocking every future job) is never surfaced; negative-path tests asserting BadRequestException pass against the emulator and fail in production. It also leaves account state AWS would never have stored, which GetPolicy then reads back.
Fix: PutPolicy's validation in its handler (put_policy.rs) - before replacing the stored PolicyRecord, reject a request in which S3Inputs, HttpInputs and HttpsInputs are all DISALLOWED with BadRequestException (400) and a message naming that constraint, leaving the previously stored policy untouched.
Traces: mediaconvert-policy-lifecycle#task#3.

=== Cause 2 of 3: a deleted preset is still readable - GetPreset keeps answering 200 where AWS answers 404 ===
AWS: Smithy: DeletePreset is DELETE /2017-08-29/presets/{Name} (Name @httpLabel @required) with success code 202 and empty output; GetPreset is GET /2017-08-29/presets/{Name} -> 200 with NotFoundException modeled at httpError 404. Recorded behavior: once the preset is deleted, GetPreset on that name returns HTTP 404 with errorType 'NotFoundException' (message example: 'The resource you specified in name was not found.') - the delete is immediately effective, so the read-after-delete is a 404 and never becomes a 200 again.
Emulator: GetPreset keeps returning HTTP 200 with the full preset (its updated description, createdAt/lastUpdated, settings) - the replay polled 15 times and never saw a 404. The underlying reason is that DeletePreset was never written: services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/delete_preset.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and its handle() returns Scaffolded::not_implemented("DeletePreset"), so nothing is removed from the preset store. (A stub issue covers DeletePreset merely answering 501; this cause is the resulting STATE divergence on the GetPreset read path, which that issue's replay key does not cover - closing it requires the delete to actually remove the record so GetPreset 404s.)
Why: a client cannot delete a preset at all, and worse, it cannot even detect that: it reads the preset back as live. Cleanup/teardown that asserts the resource is gone never converges, quota/naming reuse after a delete behaves differently from AWS, and any 'does this preset exist' check returns the wrong answer forever.
Fix: implement the DeletePreset handler (delete_preset.rs) against the preset store in the service state - remove the named preset and return the empty 202, deleting the SCAFFOLDED marker line - so that GetPreset on that name then raises NotFoundException (404) consistently with the other read paths.
Traces: mediaconvert-preset-lifecycle#teardown#8.

=== Cause 3 of 3: a queue update is not reflected by GetQueue - status and description read back as the pre-update values ===
AWS: Smithy: UpdateQueue is PUT /2017-08-29/queues/{Name} -> 200 with Status (QueueStatus ACTIVE|PAUSED) and Description among its input members; GetQueue is GET /2017-08-29/queues/{Name} -> 200 with output Queue. Recorded behavior: after a queue is updated to Status PAUSED with Description 'updated description', GetQueue on that name returns queue.status 'PAUSED' and queue.description 'updated description' - the mutation is read-your-write visible on the next GetQueue.
Emulator: GetQueue returns 200 with queue.status 'ACTIVE' and queue.description 'probe queue for lifecycle test', i.e. the pre-update values; the update left no trace in the queue store. (The open issue 20261022T110844Z-mediaconvert-job-queue-plane-5-fidelity-divergences-updatequeue.md records UpdateQueue being an unimplemented scaffold from the behavioral suite, and a stub issue covers the 501 itself; neither carries this wire replay key, and this cause is the observable state divergence on the GetQueue read - it is closed only when a queue mutation is persisted and projected by GetQueue.)
Why: the queue's two client-visible controls are inert: a client cannot pause a queue, cannot re-describe one, and - because the read reports the old values with a 200 - cannot tell that its change was lost. Code that pauses a queue and then waits for status PAUSED waits forever, and anything that derives behavior from queue.status (job gating, operator dashboards) acts on stale state.
Fix: persist queue mutations in the queue store and project them from the shared Queue view GetQueue serializes, so an applied UpdateQueue is visible to the next GetQueue (status and description at minimum, alongside lastUpdated).
Traces: mediaconvert-queue-lifecycle#task#6.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator: 62 steps, 27 passed, score 43.5%. bootstrap_signal=false, emulator_up=true, state_reset_failures=0, so no divergence below is a state-bleed or boot artifact.

Cause 1 - mediaconvert-policy-lifecycle#task#3, PutPolicy [mismatch]
  Expected: status 400; body errorType 'BadRequestException', httpStatus 400, accountId '<caller account>', requestId <present>, operation 'PutPolicy', settingsValidationErrorsJsonBlob '', message naming the all-DISALLOWED constraint (recorded example: 'You set your input policy to DISALLOWED for all input locations: S3Inputs, HttpInputs, and HttpsInputs. Policies require at least one input location to be set to ALLOWED. Set at least one of S3Inputs, HttpInputs, or HttpsInputs to ALLOWED and resubmit your policy.'); header x-amzn-errortype 'BadRequestException'
  Actual:   status 200 - the policy was accepted and stored; none of those members present. Source: ops/put_policy.rs:171-177 (PolicyRecord::from_requested then state.lock().policy = Some(record), with no cross-member validation).

Cause 2 - mediaconvert-preset-lifecycle#teardown#8, GetPreset [mismatch]
  Expected: status 404 with errorType 'NotFoundException', httpStatus 404, operation 'GetPreset', settingsValidationErrorsJsonBlob '', message example 'The resource you specified in name was not found.'
  Actual:   status 200 with the live preset body (name, arn, category, description 'updated preset description', createdAt/lastUpdated, settings) - and the replay's poll gave up after 15 attempts still seeing 200. Source: ops/delete_preset.rs line 4 ('//! SCAFFOLDED, NOT IMPLEMENTED') and line 163 (Err(Scaffolded::not_implemented("DeletePreset"))).

Cause 3 - mediaconvert-queue-lifecycle#task#6, GetQueue, status 200 == 200 [mismatch]
  Expected: $.queue.status == 'PAUSED', $.queue.description == 'updated description'
  Actual:   $.queue.status == 'ACTIVE', $.queue.description == 'probe queue for lifecycle test' (the pre-update values). The concurrentJobs/maximumConcurrentFeeds omissions reported in the same cluster are the projection defect filed separately as the resource-projection bundle, not this cause.

Model citations used: Smithy PutPolicy (PUT /2017-08-29/policy, 200, Policy @required, BadRequestException 400), Policy (S3Inputs/HttpInputs/HttpsInputs -> InputPolicy enum ALLOWED|DISALLOWED), BadRequestException (errorKind client, httpError 400, member Message); DeletePreset (DELETE /2017-08-29/presets/{Name}, 202, empty output, NotFoundException 404), GetPreset (GET /2017-08-29/presets/{Name}, 200, NotFoundException 404); UpdateQueue (PUT /2017-08-29/queues/{Name}, 200, Status -> QueueStatus, Description), GetQueue (GET /2017-08-29/queues/{Name}, 200, output Queue with Status/Description/LastUpdated). The all-DISALLOWED rejection, the immediate effectiveness of the preset delete and the read-your-write visibility of the queue update are recorded behavior.

## Steps to Reproduce
Boot the emulator and point a botocore mediaconvert client at it (region us-west-2).

Cause 1: call put_policy(Policy={'S3Inputs':'DISALLOWED','HttpInputs':'DISALLOWED','HttpsInputs':'DISALLOWED'}). Expect ClientError with code 'BadRequestException' (HTTP 400, header x-amzn-errortype BadRequestException) and a message naming the all-DISALLOWED constraint, with get_policy() still returning the previous policy; observe HTTP 200 and the all-DISALLOWED policy echoed back and stored. Positive control: a policy with at least one ALLOWED must still succeed with 200.

Cause 2: create_preset(Name=P, Description='probe preset', Category='probe-category', Settings={'ContainerSettings':{'Container':'MP4'},'VideoDescription':{...H_264 QVBR...}}); update_preset(Name=P, Description='updated preset description'); delete_preset(Name=P); then poll get_preset(Name=P). Expect NotFoundException (404) once the delete is applied; observe DeletePreset answering 501 NotImplemented and get_preset continuing to return 200 with the live preset indefinitely.

Cause 3: create_queue(Name=Q, Description='probe queue for lifecycle test'); update_queue(Name=Q, Status='PAUSED', Description='updated description'); call get_queue(Name=Q). Expect Queue.Status == 'PAUSED' and Queue.Description == 'updated description'; observe Status 'ACTIVE' and the original description, i.e. the mutation is not persisted.
