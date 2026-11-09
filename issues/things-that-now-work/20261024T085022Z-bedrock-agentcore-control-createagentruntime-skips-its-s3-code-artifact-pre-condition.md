Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control__s3.agent-runtime-code-artifact-requires-existing-s3-object bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events
Issue-Key: bedrock-agentcore-control__s3.agent-runtime-code-artifact-requires-existing-s3-object|run#11|AssertionError|raise AssertionError(
Issue-Key: bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events|_poll#4|Timeout|time.sleep(interval_s)
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# CreateAgentRuntime skips its S3 code-artifact pre-condition and never creates the runtime's vended log group

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgentRuntime

## Description
The AgentCore control plane's CreateAgentRuntime handler mints and persists a runtime from the request alone: it checks the name for conflict and the store for quota, and nothing else. Two pre-conditions/effects AWS has are therefore absent. 2 causes.

Cause 1 of 2: CreateAgentRuntime accepts a codeConfiguration naming an S3 object that does not exist.
AWS: Smithy (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json) models CreateAgentRuntime with ValidationException (httpError 400) among its errors; agentRuntimeArtifact is @required and carries codeConfiguration.code.s3 {bucket, prefix}. The model does not itself state the existence rule, so for that part: recorded behavior only — the recording shows CreateAgentRuntime whose codeConfiguration.code.s3 points at a key that HeadObject reports absent (HTTP 404) is refused SYNCHRONOUSLY with a 400-class ValidationException, no runtime of that name is ever listed, and the byte-identical create succeeds and reaches READY once that exact object is uploaded. The recorded message "S3 object not found in bucket" is an example; the requirement is the 400 / ValidationException refusal plus the absent side effect.
Emulator: the call is ACCEPTED — it answers 202 with an agentRuntimeId (observed runtime id example 'sim2reals3art390a0e71d8a7-dxnm28l2lp') and persists the record, so the runtime is also listed by ListAgentRuntimes. src/ops/create_agent_runtime.rs performs no lookup of the referenced S3 object at all.
Why: a client whose deployment package was mistyped or not yet uploaded is told the create succeeded. It then waits for a runtime that can never run, and the misconfiguration surfaces (if at all) much later and in a different place, instead of as the modeled client-side ValidationException it would branch on to correct the request. Downstream enumeration also diverges: ListAgentRuntimes reports a runtime AWS never created.
Fix: the CreateAgentRuntime handler's pre-condition validation (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_agent_runtime.rs, handle(), before the record is minted at the mint_runtime_id/insert block) must resolve codeConfiguration.code.s3 against S3 state and return ValidationException (400) when the object is absent, storing nothing. NOTE: open issue 20261022T135641Z-bedrock-agentcore-control-s3-createagentruntime-naming-a-missing-s3-code-object-answers.md describes the SAME check failing with 500 AmbiguousService from the router; that router cause is no longer what the replay shows (the request now reaches the handler and is accepted), so the remaining defect is the missing artifact validation named here.
Traces: bedrock-agentcore-control__s3.agent-runtime-code-artifact-requires-existing-s3-object.

Cause 2 of 2: no CloudWatch Logs log group is created for the runtime, so the group that must exist from CreateAgentRuntime on is never there.
AWS: recorded behavior only (the Smithy model binds no logging member on CreateAgentRuntime). The recording shows that from CreateAgentRuntime onward a vended log group named /aws/bedrock-agentcore/runtimes/<agentRuntimeId>-DEFAULT exists and is returned by DescribeLogGroups for that prefix, with NO retentionInDays member set (never expires); the agent's stdout later lands in a stream under that group, and the group and its events SURVIVE DeleteAgentRuntime (the delete must not cascade into logs).
Emulator: the group is never created. DescribeLogGroups(logGroupNamePrefix='/aws/bedrock-agentcore/runtimes/<id>-DEFAULT') keeps answering an empty logGroups list, so the check's 60.0s wait for 'runtime log group exists at create' never satisfies and the check exhausts its whole budget there (reported as 'Check timed out after 60.0s'; the runtime itself settles CREATING->READY in 500 ms per state.rs RUNTIME_SETTLE_DELAY and the only other wait is sized to 240 s, so the exhausted wait is this one). create_agent_runtime.rs writes only to this service's own store and performs no logs-side effect.
Why: every client and troubleshooting workflow that reads a runtime's logs addresses that group by its derived name. With no group, DescribeLogGroups/FilterLogEvents answer empty or ResourceNotFoundException, agent stdout has nowhere to land, and the orphaned-group-after-delete state that real operators diagnose cannot be reproduced at all.
Fix: the CreateAgentRuntime handler's side effects / state — on successful create, create the vended log group /aws/bedrock-agentcore/runtimes/<agentRuntimeId>-DEFAULT in the CloudWatch Logs service state with no retention set, and ensure DeleteAgentRuntime does NOT remove it.
Traces: bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events.

## Evidence
Cause 1 (bedrock-agentcore-control__s3.agent-runtime-code-artifact-requires-existing-s3-object, [mismatch]): expected a synchronous refusal with Error.Code 'ValidationException' (HTTP 400) and no runtime listed. Actual: AssertionError — "CreateAgentRuntime accepted the missing code object s3://sim2real-bacs3art-11f482051da9/sim2real/agent-runtime-code-artifact-requires-existing-s3-object/deployment_package.zip (runtime 'sim2reals3art390a0e71d8a7-dxnm28l2lp'); expected a synchronous ValidationException". The premise asserts held first: HeadObject on the key returned Code '404' / Message 'Not Found'. Model: CreateAgentRuntime.errors includes ValidationException (HTTP 400). Code: src/ops/create_agent_runtime.rs handle() validates only name-uniqueness (ConflictException) and MAX_AGENT_RUNTIMES (ServiceQuotaExceededException) before inserting the record.

Cause 2 (bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events, [mismatch]): expected DescribeLogGroups(logGroupNamePrefix='/aws/bedrock-agentcore/runtimes/<runtimeId>-DEFAULT') to return exactly that one group, with 'retentionInDays' absent, immediately after CreateAgentRuntime. Actual: "Check timed out after 60.0s" — the only wait with a 60.0s budget is _poll(..., description='runtime log group exists at create'), i.e. the group never appeared. Code: src/ops/create_agent_runtime.rs performs no CloudWatch Logs side effect; src/state.rs RUNTIME_SETTLE_DELAY = 500 ms, so the preceding status poll cannot account for the elapsed budget.

## Steps to Reproduce
Cause 1:
1. Boot the emulator.
2. s3.create_bucket(Bucket=<b>); do NOT upload the key. Confirm s3.head_object(Bucket=<b>, Key='sim2real/.../deployment_package.zip') raises 404 'Not Found'.
3. bedrock-agentcore-control.create_agent_runtime(agentRuntimeName=<n>, roleArn=<assumable role>, agentRuntimeArtifact={'codeConfiguration': {'code': {'s3': {'bucket': <b>, 'prefix': <that key>}}, 'runtime': 'PYTHON_3_12', 'entryPoint': ['main.py']}}, networkConfiguration={'networkMode': 'PUBLIC'}).
4. Observe 202 + agentRuntimeId (and the runtime in list_agent_runtimes) instead of 400 ValidationException with nothing created.
5. Replay bedrock-agentcore-control__s3.agent-runtime-code-artifact-requires-existing-s3-object.

Cause 2:
1. Boot the emulator.
2. Upload a valid code zip, create an AgentCore execution role, and create_agent_runtime(...) until it reports status READY; note agentRuntimeId.
3. logs.describe_log_groups(logGroupNamePrefix=f'/aws/bedrock-agentcore/runtimes/{agentRuntimeId}-DEFAULT') -> logGroups is empty; AWS returns that one group with no retentionInDays.
4. Replay bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events.
