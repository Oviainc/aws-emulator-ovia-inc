Issue-Source: trace-sweep-divergence
Issue-Trace: iot-vpce-traj-attach-target-mistakes
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-vpce-traj-attach-target-mistakes iot-vpce-traj-effective-policy-lookup-mistakes iot-vpce-traj-rollback-order-refusals
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|AttachPolicy|4xx->4xx|$.message
Issue-Key: iot|ListAttachedPolicies|4xx->4xx|$.message
Issue-Key: iot|DetachPolicy|4xx->4xx|$.message
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# iot policy-target plane: 4 causes — Attach/Detach/ListAttachedPolicies refusal messages are emulator-authored prose instead of AWS's fixed text

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachPolicy

## Description
Four net-new root causes on the IoT policy-attachment surface. All four refusals already answer the right HTTP status and the right error class; only the MESSAGE diverges, and in every case the emulator substitutes its own explanatory prose for AWS's fixed sentence. Causes 1-3 are the same wrong template emitted by the shared policy-target resolver for three different operations; cause 4 is a different message on the policy-not-found path. Each cause must be closed separately.

Cause 1 of 4: AttachPolicy's unsupported-policy-target refusal message.
AWS: Smithy AttachPolicy models InvalidRequestException (httpError 400) and takes a required `target` of shape PolicyTarget; a target whose form is not a supported policy target (here a thing ARN) is refused 400 InvalidRequestException with the fixed template `Invalid Target: <target>` — the fixed text is `Invalid Target: ` and the only variable part is the `target` value echoed verbatim from the request. Recorded behavior confirms the status (400), the error class and that single-sentence message.
Emulator: same status and class, but the message is `Invalid policy target: <target>. A policy target is a certificate ARN (arn:aws:iot:region:accountId:cert/certificateId), a thing group ARN (arn:aws:iot:region:accountId:thinggroup/groupName) or a Cognito identity (region:id)` — emulator-authored prose that re-documents the accepted target forms, text AWS never sends.
Why: a client that asserts on, logs or pattern-matches the refusal message (the usual way an IoT policy-attachment failure is classified, since the error class is shared by many refusals on this operation) sees text that never occurs against AWS, so its handling is exercised against wording that is wrong in production.
Fix: the `Invalid` arm of the shared policy-target resolver that builds this sentence — `crate::state::resolve_policy_target` / `PolicyTargetRefusal::Invalid` in services/service-com-amazonaws-iot-awsiotservice/src/state.rs, surfaced by services/service-com-amazonaws-iot-awsiotservice/src/ops/attach_policy.rs (the InvalidRequestException arm around lines 199-217).
Traces: iot-vpce-traj-attach-target-mistakes#task#4 (and task#5, same cluster).

Cause 2 of 4: ListAttachedPolicies' unsupported-policy-target refusal message.
AWS: Smithy ListAttachedPolicies takes the required `target` as an httpLabel of shape PolicyTarget and models InvalidRequestException (httpError 400); a target of an unsupported form is refused 400 InvalidRequestException `Invalid Target: <target>`, same fixed text and same single request-derived value as cause 1.
Emulator: answers 400 InvalidRequestException with the same long emulator-authored "Invalid policy target: … A policy target is a certificate ARN …" prose.
Why: the read path diverges in the same way as the write path, so a client probing which targets are addressable gets non-AWS wording.
Fix: the same shared resolver arm as cause 1, as surfaced by services/service-com-amazonaws-iot-awsiotservice/src/ops/list_attached_policies.rs.
Traces: iot-vpce-traj-effective-policy-lookup-mistakes#task#10.

Cause 3 of 4: DetachPolicy's unsupported-policy-target refusal message.
AWS: Smithy DetachPolicy models InvalidRequestException (httpError 400) and takes the required `target` of shape PolicyTarget; an unsupported target form is refused 400 InvalidRequestException `Invalid Target: <target>`.
Emulator: again the "Invalid policy target: <target>. A policy target is a certificate ARN … or a Cognito identity (region:id)" prose.
Why: the rollback/detach path a client runs when tearing an attachment down reports a message AWS does not produce.
Fix: the same shared resolver arm, as surfaced by services/service-com-amazonaws-iot-awsiotservice/src/ops/detach_policy.rs.
Traces: iot-vpce-traj-rollback-order-refusals#task#9.

Cause 4 of 4: AttachPolicy's absent-policy refusal message names the policy AWS does not.
AWS: Smithy AttachPolicy models ResourceNotFoundException (httpError 404); attaching a policy name that does not exist is refused 404 ResourceNotFoundException whose message is the fixed sentence `Policy not found` — entirely fixed text, with NO variable part (the requested policy name is not echoed).
Emulator: 404 ResourceNotFoundException with `Policy <policyName> does not exist`, an emulator-authored template that both rewords the sentence and interpolates the requested name.
Why: clients matching AWS's fixed `Policy not found` string never match, and the emulator leaks the requested name into a message AWS keeps value-free.
Fix: AttachPolicy's not-found refusal, i.e. the message builder `crate::state::missing_policy_message` as used by services/service-com-amazonaws-iot-awsiotservice/src/ops/attach_policy.rs (around line 188); other callers of that shared builder need their own AWS wording rather than inheriting this one.
Traces: iot-vpce-traj-attach-target-mistakes#task#7.

Not covered by an existing issue: the open issues 20261107T022027Z-iot-refusal-envelopes-8-causes-wrong-x-amzn.md, 20261107T022027Z-iot-already-exists-conflicts-9-causes-reworded-conflictexception.md and 20261107T022027Z-iot-policy-document-plane-10-causes-document-validation.md concern other IoT refusal wordings (error-type classes, already-exists conflicts, policy-document validation) and carry none of the replay keys above. The open stub issue 20261106T201842Z-iot-attachpolicy-stub.md is stale with respect to this replay: AttachPolicy now answers modeled 400/404 refusals rather than 501.

## Evidence
Replayed wire corpus simparity-wire-aws-v1 (trace set simparity-wire-aws-v1), per cause — expected is the recorded AWS response, actual the emulator's:

Cause 1 — AttachPolicy, fixture iot-vpce-traj-attach-target-mistakes step task#4 (cluster also covers task#5): status 400 == 400, body $.message value_mismatch: expected 'Invalid Target: arn:aws:iot:us-west-2:000000000000:thing/<thingName>', got 'Invalid policy target: arn:aws:iot:us-west-2:000000000000:thing/<thingName>. A policy target is a certificate ARN (arn:aws:iot:region:accountId:cert/certificateId), a thing group ARN (arn:aws:iot:region:accountId:thinggroup/groupName) or a Cognito identity (region:id)'.

Cause 2 — ListAttachedPolicies, fixture iot-vpce-traj-effective-policy-lookup-mistakes step task#10: status 400 == 400, $.message expected 'Invalid Target: arn:aws:iot:us-west-2:000000000000:thing/<thingName>', got the same 'Invalid policy target: … Cognito identity (region:id)' prose.

Cause 3 — DetachPolicy, fixture iot-vpce-traj-rollback-order-refusals step task#9: status 400 == 400, $.message expected 'Invalid Target: arn:aws:iot:us-west-2:000000000000:thing/<thingName>', got the same prose.

Cause 4 — AttachPolicy, fixture iot-vpce-traj-attach-target-mistakes step task#7: status 404 == 404, $.message expected 'Policy not found', got 'Policy <policyName> does not exist'.

In every cause the status, the x-amzn-errortype class and the response shape already match; the divergence is the message text alone. The thing/policy names in the recordings are per-run generated values and are not part of the requirement — only the fixed text ('Invalid Target: ', 'Policy not found') and, for causes 1-3, the echoed request `target` value are.

## Steps to Reproduce
Replay the wire fixtures from trace set simparity-wire-aws-v1:
1. iot-vpce-traj-attach-target-mistakes, step task#4 (and task#5): CreatePolicy(policyName=P, policyDocument=…); CreateThing(thingName=T); AttachPolicy(policyName=P, target='arn:aws:iot:<region>:<account>:thing/T') — expect 400 InvalidRequestException with message exactly 'Invalid Target: arn:aws:iot:<region>:<account>:thing/T'.
2. iot-vpce-traj-effective-policy-lookup-mistakes, step task#10: ListAttachedPolicies(target='arn:aws:iot:<region>:<account>:thing/T') — expect 400 InvalidRequestException 'Invalid Target: <that arn>'.
3. iot-vpce-traj-rollback-order-refusals, step task#9: DetachPolicy(policyName=P, target='arn:aws:iot:<region>:<account>:thing/T') — expect 400 InvalidRequestException 'Invalid Target: <that arn>'.
4. iot-vpce-traj-attach-target-mistakes, step task#7: AttachPolicy(policyName=<a name never created>, target=<a valid certificate ARN>) — expect 404 ResourceNotFoundException with message exactly 'Policy not found'.
