Issue-Source: trace-sweep-divergence
Issue-Trace: ram-resource-policies-lookup-error
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ram-resource-policies-lookup-error
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram|GetResourcePolicies|4xx->4xx|$.__type
Issue-Key: ram|GetResourcePolicies|4xx->4xx|$.message
Issue-Key: ram|GetResourcePolicies|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: f836b492e790c3b6e56806350eb24f389e34b620

# RAM GetResourcePolicies answers ResourceArnNotFoundException for a non-shareable resource type instead of MalformedArnException

> _Note: this issue was identified against the code base at commit `f836b492e790c3b6e56806350eb24f389e34b620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicies

## Description
One cause.

Cause 1 of 1: GetResourcePolicies classifies an unusable `resourceArns` entry by state (does the resource exist in my store?) instead of by kind (is this ARN a shareable resource ARN at all?), so the request is refused with the wrong modeled error shape, error code and message.

AWS: GetResourcePolicies takes `resourceArns` (required list) of ARNs of *shareable resources*. When an entry is not an ARN of a resource type RAM can share — e.g. a RAM resource-share ARN (`arn:aws:ram:<region>:<account>:resource-share/<id>`), which identifies a share rather than a shared resource — RAM refuses the whole request with 400 `MalformedArnException`, message `You cannot share the selected resource type.` Smithy: GetResourcePolicies errors include `MalformedArnException` (client, httpError 400, awsQueryError `InvalidArn.Malformed`, doc: "the specified Amazon Resource Name (ARN) has a format that isn't valid") and, separately, `ResourceArnNotFoundException` (client, httpError 400, awsQueryError `InvalidResourceArn.NotFound`, doc: "the specified Amazon Resource Name (ARN) was not found"). The two are distinct conditions: the first is about the ARN's shape/type, the second about a well-formed shareable-resource ARN that has no entry. The recorded AWS response is the first. Message anatomy for this class: the text `You cannot share the selected resource type.` is entirely FIXED — it interpolates nothing from the request, no ARN, id, account or region.

Emulator: answers 400 with `__type` = `ResourceArnNotFoundException`, header `x-amzn-errortype: ResourceArnNotFoundException`, and message `Resource <arn> could not be found.` — a template the emulator authored, whose variable part is the offending ARN taken from `resourceArns`. The HTTP status happens to match (400 -> 400), so only the error class and the message body diverge.

Why: a client that branches on the error type cannot tell "you asked about something RAM cannot share" from "that share/resource does not exist here". The first is a permanent, non-retryable request-construction bug the caller must fix; the second invites a create-then-retry or a poll for eventual consistency. Callers keyed on `MalformedArnException` (or the awsQuery code `InvalidArn.Malformed`) see an unexpected error and either retry forever or surface the wrong diagnosis; callers keyed on `ResourceArnNotFoundException` are told a resource is missing when the real problem is that the ARN names a non-shareable type. Error-message assertions also fail, since the emulator leaks the request's ARN into text AWS keeps fixed.

Fix: the ARN classification step in the ram service crate's GetResourcePolicies handler validation, which must run BEFORE the state lookup. Each `resourceArns` entry should first be checked for being a well-formed ARN of a RAM-shareable resource type; a RAM `resource-share/...` ARN (and any other non-shareable service/resource-type pair) must short-circuit to `MalformedArnException` with the fixed message `You cannot share the selected resource type.`, and only entries that pass that check may fall through to the store lookup that raises `ResourceArnNotFoundException`.

Traces: fixture `ram-resource-policies-lookup-error`, step `task#2` (GetResourcePolicies).

## Evidence
Fixture ram-resource-policies-lookup-error, step task#2, operation GetResourcePolicies, verdict mismatch (expected_status 400, actual_status 400).

Expected (recorded AWS):
  HTTP 400
  $.__type                      = "MalformedArnException"
  $headers.x-amzn-errortype     = "MalformedArnException"
  $.message                     = "You cannot share the selected resource type."

Actual (emulator):
  HTTP 400
  $.__type                      = "ResourceArnNotFoundException"
  $headers.x-amzn-errortype     = "ResourceArnNotFoundException"
  $.message                     = "Resource arn:aws:ram:us-west-2:000000000000:resource-share/f581604e-e6be-88ab-82f1-c82793469f1c could not be found."

Replay detail: "$.__type: value_mismatch (expected 'MalformedArnException', got 'ResourceArnNotFoundException'); $.message: value_mismatch; $headers.x-amzn-errortype: value_mismatch". divergence_key: service=ram|GetResourcePolicies|status=400->400|keys=$.__type,$.message,$headers.x-amzn-errortype. Cluster size 1 (affected: ram-resource-policies-lookup-error#task#2).

In the actual message the only variable part is the ARN echoed from the request (account 000000000000, region us-west-2 and the share uuid are per-run values, not requirements); the expected message has no variable part at all. Smithy model (services/service-com-amazonaws-ram-amazonresourcesharing/smithy-model/com-amazonaws-ram-amazonresourcesharing.json) binds BOTH MalformedArnException and ResourceArnNotFoundException to GetResourcePolicies, each client/400, so the emulator can raise the modeled shape without any new error type.

## Steps to Reproduce
1. git_clone() and boot the emulator built from the checkout.
2. Replay fixture `ram-resource-policies-lookup-error`, step `task#2`: POST /getresourcepolicies against the ram endpoint with a `resourceArns` list whose single entry is a RAM resource-share ARN, i.e. an ARN of the form arn:aws:ram:<region>:<account>:resource-share/<uuid> (a non-shareable resource type), as recorded in the fixture.
3. Observe the response: 400 with __type / x-amzn-errortype = ResourceArnNotFoundException and message "Resource <arn> could not be found.".
4. Expected: 400 with __type / x-amzn-errortype = MalformedArnException and the fixed message "You cannot share the selected resource type.".
5. Re-run replay_wire_corpus() after the fix; the step must pass.
