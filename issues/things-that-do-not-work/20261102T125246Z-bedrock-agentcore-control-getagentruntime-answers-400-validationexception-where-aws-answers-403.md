Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-agentcore-traj-runtime-lookup-from-log-group-name
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-agentcore-traj-runtime-lookup-from-log-group-name
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetAgentRuntime|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|GetAgentRuntime|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|GetAgentRuntime|4xx->4xx|status
Issue-Commit: e3e78cf83f0a145a8ff37896813888a6025b7749

# GetAgentRuntime answers 400 ValidationException where AWS answers 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `e3e78cf83f0a145a8ff37896813888a6025b7749`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAgentRuntime

## Description
GetAgentRuntime on an agentRuntimeId the caller is not permitted to read is answered as an input-validation failure instead of an authorization failure: the emulator runs the path label's @pattern check first and emits 400 ValidationException, where AWS answers 403 AccessDeniedException for the same request. The status class, the x-amzn-errortype error code and the message all diverge. 1 cause must be fixed before this issue is closed.

Cause 1 of 1: GetAgentRuntime reports a pattern/validation refusal instead of the modeled access-denied refusal.
AWS: GetAgentRuntime is GET /runtimes/{agentRuntimeId}/ and models both AccessDeniedException (httpError 403) and ValidationException (httpError 400) (Smithy: GetAgentRuntime errors = AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400; input member agentRuntimeId is @required @httpLabel with @pattern ^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$). Recorded behavior: for a request whose agentRuntimeId does NOT satisfy that @pattern, the recorded response is still 403 with the AccessDeniedException body — i.e. the identity-based authorization decision is reached and returned before the httpLabel constraint is evaluated, so a caller without bedrock-agentcore:GetAgentRuntime never sees a constraint error. The 403 message's fixed text is "User: " + "<callerArn>" + " is not authorized to perform: bedrock-agentcore:GetAgentRuntime"; the only variable part is the caller's principal ARN, taken from the signing identity of the request (the recorded run's value, arn:aws:iam::000000000000:user/emulator, is that run filling the slot, not part of the requirement). Hypothesis: the service evaluates the operation's IAM authorization in request dispatch, ahead of per-member constraint validation, which is why a malformed id still yields AccessDeniedException.
Emulator: answers 400 with x-amzn-errortype: ValidationException and the constraint message "1 validation error detected: Value '<agentRuntimeId>' at 'agentRuntimeId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}". No authorization decision is taken on this path at all, and the pattern check is ordered first.
Why: a client cannot distinguish "you may not read this runtime" from "your identifier is malformed". Callers branch on exactly that distinction — an AccessDeniedException is surfaced to an operator as a permissions problem (or drives a role/credential refresh), while a ValidationException is treated as a caller bug and the identifier is rewritten or the call abandoned. Least-privilege tests that assert 403/AccessDeniedException on a read path pass against AWS and fail here, and anything that derives a runtime id from another source (here, a CloudWatch log-group name) is told its derivation is invalid when the real failure is permissions.
Fix: the bedrock-agentcore-control request dispatch / GetAgentRuntime handler's refusal ordering and error selection — the identity-based authorization gate must run before the agentRuntimeId @httpLabel constraint check, and must emit the modeled AccessDeniedException (403) with AWS's "User: <callerArn> is not authorized to perform: bedrock-agentcore:GetAgentRuntime" sentence.
Traces: bedrock-agentcore-control-agentcore-traj-runtime-lookup-from-log-group-name#task#4 (and the same cluster's task#5).

Related but distinct: open issue 20261029T212239Z-bedrock-agentcore-control-accepts-four-requests-aws-refuses records, as its cause 3, that the service has no identity-based authorization gate at all (CreateAgentRuntime and DeleteGateway from a denied principal). That issue is grounded only in behavioral check ids and carries no replay key for this wire fixture step, and its causes are write operations; this divergence is the read path GetAgentRuntime, where the missing gate additionally shows up as a wrong refusal ORDER (the @pattern check pre-empting the 403). It is not covered by that issue's replay pointer.

## Evidence
Fixture bedrock-agentcore-control-agentcore-traj-runtime-lookup-from-log-group-name, step task#4, operation GetAgentRuntime (cluster also covers step task#5 of the same fixture, affected_count 2):
- status: expected 403, got 400.
- $headers.x-amzn-errortype: expected the modeled AccessDeniedException shape, got 'ValidationException'.
- $.message: expected 'User: <callerArn> is not authorized to perform: bedrock-agentcore:GetAgentRuntime' (recorded run's callerArn: arn:aws:iam::000000000000:user/emulator), got "1 validation error detected: Value 'sim2real_probe_agmcp_ed5cafbc' at 'agentRuntimeId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}".
The requirement is the status (403), the error code (AccessDeniedException) and the fixed text 'User: ... is not authorized to perform: bedrock-agentcore:GetAgentRuntime'; the principal ARN is the request's signing identity and the runtime id shown is the recorded run's value filling the emulator's message, not part of any requirement. Note the recorded request's agentRuntimeId violates the modeled @pattern and AWS still answers 403, which is what pins the ordering.

## Steps to Reproduce
1. Clone the repo and boot the emulator, then replay the wire corpus (replay_wire_corpus).
2. Fixture bedrock-agentcore-control-agentcore-traj-runtime-lookup-from-log-group-name, step task#4 (and task#5): call GetAgentRuntime (GET /runtimes/{agentRuntimeId}/) with an agentRuntimeId derived from a runtime log-group name, i.e. one that does not match ^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$, using credentials that are not authorized for bedrock-agentcore:GetAgentRuntime.
3. Observe HTTP 400 with x-amzn-errortype: ValidationException and the '1 validation error detected: ... failed to satisfy constraint' message.
4. Expected: HTTP 403 with x-amzn-errortype: AccessDeniedException and message 'User: <callerArn> is not authorized to perform: bedrock-agentcore:GetAgentRuntime', the authorization decision taking precedence over the agentRuntimeId pattern check.
