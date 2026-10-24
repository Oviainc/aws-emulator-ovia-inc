Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-microvms-tag-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-microvms-tag-error-paths
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda|TagResource|4xx->4xx|$.message
Issue-Key: lambda|ListTags|4xx->4xx|$.message
Issue-Key: lambda|UntagResource|4xx->4xx|$.message
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# lambda tag ARN validation pattern omits the layer alternative and echoes regex anchors

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three distinct causes — one per tag operation bound at /2017-03-31/tags/{Resource} — all produced by the same defect in the resource-ARN constraint used to validate the Resource path label. Each operation constructs the refusal independently, so each is listed and each must be verified. All three must be closed before this issue is submitted.

Shared defect, stated once: AWS refuses a malformed Resource with HTTP 400 and the standard constraint text '1 validation error detected: Value '<value>' at 'resource' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>'. Two things about <pattern> are wrong in the emulator. First, the alternation is narrower than AWS's: AWS's pattern admits a layer ARN via the alternative layer:([a-zA-Z0-9-_]+), and the emulator's pattern has no layer alternative at all. Second, AWS renders the constraint with its anchoring ^ and $ stripped, while the emulator echoes them. The message text here IS a requirement rather than an example, because it is built from the request (the offending value and member name) and the member's own constraint — that construction rule is what diverges, not incidental wording.

AWS: recorded behavior is the reference for the pattern itself (the tag operations are bound in the lambda-microvms model at POST/GET/DELETE /2017-03-31/tags/{Resource} with InvalidParameterValueException and ValidationException among their modeled errors, but the model does not state the ARN alternation used for the 'resource' label). The recording shows the pattern as: arn:(aws[a-zA-Z-]*):lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:(function:[a-zA-Z0-9-_]+(:(\$LATEST|[a-zA-Z0-9-_]+))?|layer:([a-zA-Z0-9-_]+)|code-signing-config:csc-[a-z0-9]{17}|event-source-mapping:[0-9a-fA-F]{8}-...|(capacity-provider|network-connector):[a-zA-Z0-9-_]{1,64}) — unanchored, with the layer alternative present.
Emulator: emits the same message shell but with a pattern that is anchored (^...$) and from which the layer:([a-zA-Z0-9-_]+) alternative is missing.
Why: the missing alternative is not cosmetic — the pattern is the acceptance predicate for the Resource label, so tagging, listing tags on, or untagging a Lambda *layer* ARN is rejected with 400 even though AWS accepts it. Layer tagging is simply unavailable through the emulator. The anchor difference additionally breaks any client or test that matches the documented constraint text or re-compiles the echoed pattern to explain the failure to a user.
Fix: the shared resource-ARN constraint used by the /2017-03-31/tags/{Resource} handlers' validation, plus the constraint-violation message formatter — add the layer ARN alternative to the accepted set, and strip the anchoring ^ and $ when rendering a pattern into the message.

Cause 1 of 3: TagResource (POST /2017-03-31/tags/{Resource}, success code 204) refuses 'totally-not-an-arn' with 400 but echoes the anchored, layer-less pattern; a layer ARN is rejected outright.
Traces: lambda-microvms-tag-error-paths#task#1.

Cause 2 of 3: ListTags (GET /2017-03-31/tags/{Resource}) — same divergence, built in its own handler.
Traces: lambda-microvms-tag-error-paths#task#2.

Cause 3 of 3: UntagResource (DELETE /2017-03-31/tags/{Resource}, success code 204) — same divergence, built in its own handler.
Traces: lambda-microvms-tag-error-paths#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200). All three steps agree on status (400 expected, 400 actual); the divergence is in $.message, specifically the echoed pattern.

Cause 1 — lambda-microvms-tag-error-paths#task#1, TagResource, status 400->400, $.message value_mismatch:
  expected: "1 validation error detected: Value 'totally-not-an-arn' at 'resource' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:(aws[a-zA-Z-]*):lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\\d{1}:\\d{12}:(function:[a-zA-Z0-9-_]+(:(\\$LATEST|[a-zA-Z0-9-_]+))?|layer:([a-zA-Z0-9-_]+)|code-signing-config:csc-[a-z0-9]{17}|event-source-mapping:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|(capacity-provider|network-connector):[a-zA-Z0-9-_]{1,64})"
  actual:   "1 validation error detected: Value 'totally-not-an-arn' at 'resource' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws[a-zA-Z-]*):lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\\d{1}:\\d{12}:(function:[a-zA-Z0-9-_]+(:(\\$LATEST|[a-zA-Z0-9-_]+))?|code-signing-config:csc-[a-z0-9]{17}|event-source-mapping:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|(capacity-provider|network-connector):[a-zA-Z0-9-_]{1,64})$"
  Diff: actual adds leading ^ and trailing $, and drops the alternative 'layer:([a-zA-Z0-9-_]+)|'.

Cause 2 — lambda-microvms-tag-error-paths#task#2, ListTags, status 400->400: byte-identical $.message divergence (same two differences).

Cause 3 — lambda-microvms-tag-error-paths#task#3, UntagResource, status 400->400: byte-identical $.message divergence (same two differences).

Model grounding: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json — TagResource (POST /2017-03-31/tags/{Resource}, code 204), ListTags (GET /2017-03-31/tags/{Resource}), UntagResource (DELETE /2017-03-31/tags/{Resource}, code 204); each models InvalidParameterValueException and ResourceNotFoundException, with ValidationException @httpError 400 among the service-common errors. The 'resource' ARN alternation itself is recorded behavior only.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and read the three clusters whose operations are TagResource, ListTags and UntagResource.
3. Reproduce the echoed-pattern divergence (all three return 400; compare $.message):
   - POST /2017-03-31/tags/totally-not-an-arn  -> fixture lambda-microvms-tag-error-paths#task#1
   - GET  /2017-03-31/tags/totally-not-an-arn  -> fixture lambda-microvms-tag-error-paths#task#2
   - DELETE /2017-03-31/tags/totally-not-an-arn -> fixture lambda-microvms-tag-error-paths#task#3
   In each, observe the echoed pattern carrying ^ and $ and lacking the 'layer:([a-zA-Z0-9-_]+)' alternative.
4. Confirm the functional half of the defect: call the same three operations with a well-formed Lambda layer ARN (arn:aws:lambda:us-west-2:000000000000:layer:my-layer) -> the emulator rejects it with 400 because the layer alternative is missing from its pattern, where AWS accepts it.
5. Re-replay the two fixture#step bindings in trace_ids plus the representative to confirm a fix.
