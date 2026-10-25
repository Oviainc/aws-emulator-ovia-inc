Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3
Issue-Key: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3|run#40|AssertionError|assert called["result"].get("isError") is not True, (
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# bedrock-agentcore__lambda__s3: an AgentCore gateway MCP tools/call reports a NotImplemented tool error because its Lambda target is never executed (1 cause)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeGatewayTool (MCP tools/call)

## Description
Cause 1 of 1: a gateway tool backed by a Lambda target returns a tool error instead of the target's result, because Lambda Invoke never runs the deployment package.

AWS: an AgentCore gateway target of type Lambda invokes the function synchronously when the MCP client calls tools/call, and the tool result is the function's response payload; a failure is surfaced as the target's error, not as a control-plane 'not implemented' code. Recorded behavior only.

Emulator: the tools/call response carries a tool error whose body is the Lambda refusal: code 'NotImplemented', message 'Invoke resolved arn:aws:lambda:...:function:<fn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator' (recorded message, an example). The gateway therefore reports a tool failure for every Lambda-backed tool, and the S3 write the tool was supposed to perform never happens.

Why: a client wiring an MCP gateway to a Lambda tool sees every call fail, cannot distinguish a tool bug from a platform gap, and never observes the tool's side effects (here the S3 object the tool writes).

Fix: the Lambda Invoke execution path the gateway target calls into (lambda service Invoke / runtime execution); the gateway's tool-call handler should surface the target's own result once Invoke runs. The open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md records the underlying Invoke refusal but does not cover this gateway path.

Traces: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3.

## Evidence
Expected: tools/call for the gateway's PutObject tool returns the Lambda target's result and the object appears in S3.
Actual: AssertionError: tools/call for '<tool>' reported a tool error: NotImplemented: ... 'Invoke resolved arn:aws:lambda:us-west-2:000000000000:function:simparity-mcp-writer-... at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator'

## Steps to Reproduce
1. Create a Lambda function that writes an S3 object.
2. Create an AgentCore gateway with a Lambda target exposing that function as a tool.
3. Issue an MCP tools/call for the tool.
4. Observe the tool error carrying Lambda's NotImplemented, and no S3 object.
