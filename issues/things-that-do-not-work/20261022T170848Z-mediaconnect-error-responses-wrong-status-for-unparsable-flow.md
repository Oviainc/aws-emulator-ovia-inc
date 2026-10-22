Issue-Source: trace-sweep-divergence
Issue-Trace: mediaconnect-flow-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediaconnect-flow-error-paths mediaconnect-flow-lifecycle-basic mediaconnect-flow-list-pagination mediaconnect-flow-media-streams-and-outputs mediaconnect-flow-sources-entitlements-update
Issue-Shape-Id: com.amazonaws.mediaconnect#MediaConnect
Issue-Key: mediaconnect|DescribeFlow|4xx->4xx|$.message
Issue-Key: mediaconnect|DescribeFlow|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mediaconnect|DescribeFlow|4xx->4xx|status
Issue-Key: mediaconnect|CreateFlow|4xx->4xx|$.message
Issue-Key: mediaconnect|AddFlowOutputs|4xx->4xx|$.message
Issue-Key: mediaconnect|GrantFlowEntitlements|4xx->4xx|$.message
Issue-Key: mediaconnect|DeleteFlow|4xx->4xx|$.message
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# MediaConnect error responses: wrong status for unparsable flow ARN plus non-AWS message envelope

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFlow

## Description
Two distinct root causes on MediaConnect's error path, both in service `mediaconnect`, carried by six replayed divergence clusters. Both must be closed before this issue is submitted.

=== Cause 1 of 2: a flow ARN path label that is not an ARN is refused 400 BadRequestException instead of 404 NotFoundException ===
AWS: Smithy DescribeFlow (GET /v1/flows/{FlowArn}, success 200) binds `FlowArn` as an `@httpLabel` with pattern `^arn:.+:mediaconnect.+:flow:.+$` and models NotFoundException (404) as well as BadRequestException (400). Recorded behavior: a GET whose `{FlowArn}` label is not an ARN at all is answered with HTTP 404 (the recorded message is the generic 'Not found: 404 Not Found'), i.e. the request resolves to no flow rather than being rejected as a malformed member. The requirement is the status 404 and the NotFoundException error code; the recorded message text is an example of Cause 2's envelope.
Emulator: the request is refused with HTTP 400 and `x-amzn-errortype: BadRequestException`, produced by a pre-handler pattern check whose message cites a regex that is NOT the modeled one — "Value 'not-a-valid-flow-arn' at 'flowArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:flow:[a-zA-Z0-9-]+:[a-zA-Z0-9_-]+$". That pattern is strictly narrower than the model's `^arn:.+:mediaconnect.+:flow:.+$`, so well-formed ARNs the model admits (other partitions, non-12-digit or non-numeric account segments, flow names or ids outside the narrowed character classes) are also rejected 400 instead of being looked up.
Why: clients and SDK retry/branching logic key on the error code. Code that catches NotFoundException to handle "this flow is gone" (the standard teardown/poll idiom) instead sees an unexpected BadRequestException and aborts; and because the enforced pattern is narrower than the modeled one, legitimate cross-partition or non-standard-account ARNs can never reach a handler at all.
Fix: the flow-ARN label validation on the flow operations — enforce the MODELED pattern only (or defer entirely to the store lookup) and let an unmatched/unknown flow ARN fall through to the handler's NotFoundException 404.
Traces: mediaconnect-flow-error-paths#task#3.

=== Cause 2 of 2: client-error messages do not use MediaConnect's error-message envelope ===
AWS: `BadRequestException` and `NotFoundException` both model a required `Message` member, and the model states the precise meaning of each "is documented in the error message" — so the message envelope is part of what clients read. The envelope is a request-derived rule visible across independent recordings of this service: NotFoundException messages are `Not found: <the requested ARN>`; a request-validation refusal is `Validation failed:\n<detail>` (e.g. `Validation failed:\nInvalid Protocol` for a source protocol outside the `Protocol` enum, and `Validation failed:\nWhen specifying an Entitlements list, it must contain at least one Entitlement.` for an empty required `Entitlements` list). HTTP statuses and error codes already match on all five clusters below (404/NotFoundException, 400/BadRequestException) — only the message envelope diverges. The prose AFTER the prefix is an example; the envelope and the request-derived ARN/member are the requirement.
Emulator: emits its own prose with no envelope. NotFoundException: `Flow with ARN <arn> was not found.` where AWS returns `Not found: <arn>` (the literal is formatted in several handlers, e.g. `src/ops/update_flow.rs` line 343). Validation refusals: the generic schema-constraint text `Value 'not-a-real-protocol' at 'sources.1.member.protocol' failed to satisfy constraint: Member must satisfy enum value set: [...]` where AWS returns `Validation failed:\nInvalid Protocol`, and the hand-written `A grant flow entitlements request must include at least one entitlement.` where AWS returns the `Validation failed:\n...` envelope.
Why: operators and clients that distinguish a validation refusal from a semantic one by the `Validation failed:` / `Not found:` prefix, or that parse the ARN back out of a not-found message, misread every MediaConnect refusal; log-based alerting keyed on the envelope never matches.
Fix: the service's error construction — render modeled BadRequestException/NotFoundException messages through a shared envelope helper used by every MediaConnect op handler, and route modeled-constraint rejections for this service through the `Validation failed:\n` form.
Related open issue: the behavioral issue `20261022T132818Z-mediaconnect-router-interface-union-payload-rejected-no-flow.md` describes this same envelope defect as its Cause 6, but it is grounded in behavioral check ids only and carries NO wire replay key, so the replay report reports these six clusters as unowned. They are filed here so the fix is gated on re-replaying them.
Traces: mediaconnect-flow-error-paths#task#2 (CreateFlow, invalid protocol), mediaconnect-flow-error-paths#task#4 (AddFlowOutputs, unknown flow), mediaconnect-flow-error-paths#task#5 (GrantFlowEntitlements, empty entitlements), mediaconnect-flow-error-paths#task#6 (DeleteFlow, unknown flow), mediaconnect-flow-lifecycle-basic#teardown#9 (DescribeFlow after delete; the same cluster covers the teardown of mediaconnect-flow-list-pagination, mediaconnect-flow-media-streams-and-outputs and mediaconnect-flow-sources-entitlements-update).

## Evidence
Replay of trace set simparity-wire-aws-v1: 16/50 steps passed (32%), 18 divergence clusters, all service=mediaconnect. None of the clusters below carried a stub_path or an owned_by.

Cause 1 — mediaconnect-flow-error-paths#task#3 (DescribeFlow with FlowArn='not-a-valid-flow-arn'):
  expected: HTTP 404 (NotFoundException), message 'Not found: 404 Not Found'
  actual:   HTTP 400, x-amzn-errortype: BadRequestException, message "Value 'not-a-valid-flow-arn' at 'flowArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:flow:[a-zA-Z0-9-]+:[a-zA-Z0-9_-]+$"
  (modeled pattern is the looser ^arn:.+:mediaconnect.+:flow:.+$)

Cause 2 — status and error code match on all of these; only $.message diverges:
  mediaconnect-flow-error-paths#task#2 (CreateFlow, 400/400): expected 'Validation failed:\nInvalid Protocol', got "Value 'not-a-real-protocol' at 'sources.1.member.protocol' failed to satisfy constraint: Member must satisfy enum value set: [zixi-push, rtp-fec, rtp, zixi-pull, rist, st2110-jpegxs, cdi, srt-listener, srt-caller, fujitsu-qos, udp, ndi-speed-hq]"
  mediaconnect-flow-error-paths#task#4 (AddFlowOutputs, 404/404): expected 'Not found: <flow arn>', got 'Flow with ARN <flow arn> was not found.'
  mediaconnect-flow-error-paths#task#5 (GrantFlowEntitlements, 400/400): expected 'Validation failed:\nWhen specifying an Entitlements list, it must contain at least one Entitlement.', got 'A grant flow entitlements request must include at least one entitlement.'
  mediaconnect-flow-error-paths#task#6 (DeleteFlow, 404/404): expected 'Not found: <flow arn>', got 'Flow with ARN <flow arn> was not found.'
  mediaconnect-flow-lifecycle-basic#teardown#9 (DescribeFlow, 404/404): expected 'Not found: <flow arn>', got 'Flow with ARN <flow arn> was not found.' (cluster also covers mediaconnect-flow-list-pagination#teardown#6, mediaconnect-flow-media-streams-and-outputs#teardown#6, mediaconnect-flow-sources-entitlements-update#teardown#9)

## Steps to Reproduce
Boot the emulator and replay the wire corpus (replay_wire_corpus), or drive a botocore mediaconnect client at the emulator endpoint:

Cause 1: issue GET /v1/flows/not-a-valid-flow-arn (or mc.describe_flow(FlowArn='not-a-valid-flow-arn')) -> HTTP 400 BadRequestException with a regex-constraint message instead of HTTP 404 NotFoundException. Also try a well-formed ARN the MODEL admits but the emulator's narrower regex does not, e.g. arn:aws-cn:mediaconnect:cn-north-1:acct:flow:1-00000001:f1 -> rejected 400 instead of looked up and answered 404.

Cause 2: (a) mc.describe_flow(FlowArn=<well-formed but unknown flow ARN>) -> compare Error.Message to 'Not found: <arn>'. (b) mc.delete_flow / mc.add_flow_outputs on that same unknown ARN -> same comparison. (c) mc.create_flow(Name='f', Sources=[{...},{'Name':'s2','Protocol':'not-a-real-protocol','WhitelistCidr':'10.24.43.0/24'}]) -> compare Error.Message to 'Validation failed:\nInvalid Protocol'. (d) mc.grant_flow_entitlements(FlowArn=<existing flow>, Entitlements=[]) -> compare Error.Message to the 'Validation failed:\n...' envelope.
