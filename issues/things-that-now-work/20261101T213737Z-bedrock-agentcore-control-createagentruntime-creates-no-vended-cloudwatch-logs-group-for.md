Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events
Issue-Key: bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events|_poll#4|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s; last={last!r}")
Issue-Commit: 5fa99114f4f1697b9b790323162b082bd594102a

# CreateAgentRuntime creates no vended CloudWatch Logs group for the runtime, so every runtime-log read answers empty

> _Note: this issue was identified against the code base at commit `5fa99114f4f1697b9b790323162b082bd594102a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgentRuntime

## Description
Cause 1 of 1: CreateAgentRuntime mints and persists the runtime but performs no CloudWatch Logs side effect, so the runtime's vended log group never exists.

NOTE for the fixer: open issue 20261024T085022Z-bedrock-agentcore-control-createagentruntime-skips-its-s3-code-artifact-pre-condition.md describes the same missing log group as its cause 2 and lists this check in its Issue-Trace-Ids, but the replay did not report this check as OWNED or ALREADY FILED, so it is re-filed here with the current evidence. Its cause 1 (the S3 code-artifact pre-condition) IS now present in the handler — services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_agent_runtime.rs calls crate::state::code_object_is_absent(...) and crate::state::execution_role_is_unassumable(...) before writing — so the log-group effect described below is the part that still diverges.

AWS:      recorded behavior only — the Smithy model (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json) binds no logging member on CreateAgentRuntime (its output is agentRuntimeArn/agentRuntimeId/agentRuntimeVersion/createdAt/status/workloadIdentityDetails, SUCCESS_STATUS 202). The recording shows that from a successful CreateAgentRuntime onward a service-made log group exists whose name is derived from the response: /aws/bedrock-agentcore/runtimes/<agentRuntimeId>-DEFAULT (fixed prefix and -DEFAULT suffix; the only variable part is the agentRuntimeId the create returned). DescribeLogGroups(logGroupNamePrefix=<that name>) returns exactly that one group and the group carries NO retentionInDays member (it never expires). The agent's stdout later lands in a stream under that group whose name names the runtimeSessionId, and the group and its events SURVIVE DeleteAgentRuntime — the runtime delete must not cascade into CloudWatch Logs.
Emulator: the group is never created. create_agent_runtime.rs writes only this service's own store (runtime_names / client-token memo / agent_runtimes) and makes no logs-side effect, so DescribeLogGroups for that prefix keeps answering an empty logGroups list. The check's bounded wait for 'runtime log group exists at create' can therefore never satisfy and the check exhausts its whole budget there; the replay reports it as 'Check timed out after 60.0s'. That wait is the one that burns the budget: the only other wait before it is the CREATING->READY settle, and state.rs RUNTIME_SETTLE_DELAY is sub-second, while the next wait after it is sized to 240 s.
Why:      every client, console view and troubleshooting workflow addresses a runtime's logs by that derived group name. With no group, DescribeLogGroups answers empty and FilterLogEvents answers empty or ResourceNotFoundException, agent stdout has nowhere to land, and a caller that creates a runtime and then tails its logs waits forever on a resource AWS guarantees exists from create. The retained-after-delete state that real operators diagnose (and that this check asserts) cannot be reproduced at all.
Fix:      the CreateAgentRuntime handler's side effects / state (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_agent_runtime.rs, after the record is inserted in handle()): on a successful create, create the vended log group /aws/bedrock-agentcore/runtimes/<agentRuntimeId>-DEFAULT in the CloudWatch Logs service state with no retention set; and ensure DeleteAgentRuntime does NOT delete it.
Traces:   bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events.

## Evidence
Check bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events, verdict [mismatch], detail: "Check timed out after 60.0s".

The check's first assertion after create is:
  found = _poll(lambda: logs.describe_log_groups(logGroupNamePrefix=rt['group'])['logGroups'], lambda g: len(g) >= 1, timeout_s=60.0, description='runtime log group exists at create')
  assert [g['logGroupName'] for g in found] == [rt['group']]
  assert 'retentionInDays' not in found[0]
with rt['group'] = f"/aws/bedrock-agentcore/runtimes/{agentRuntimeId}-DEFAULT".

Expected: DescribeLogGroups for that prefix returns exactly that one group, with no retentionInDays, immediately after CreateAgentRuntime; and after DeleteAgentRuntime the group and its events are still readable.
Actual: the list stays empty for the whole 60 s budget, so the harness cap fires.

Code: services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_agent_runtime.rs handle() — after the artifact and role pre-conditions it builds AgentRuntimeVersionRecord/AgentRuntimeRecord and performs only store.runtime_names.insert(...), store.remember_client_token(...), store.agent_runtimes.insert(...). No CloudWatch Logs state is touched anywhere in the file. src/state.rs RUNTIME_SETTLE_DELAY is sub-second, so the preceding CREATING->READY poll cannot account for the exhausted budget.

## Steps to Reproduce
1. Boot the emulator.
2. Create an S3 bucket and upload a valid agent code zip to agent/code.zip; create an IAM role trusted by bedrock-agentcore.amazonaws.com with logs:CreateLogGroup/CreateLogStream/PutLogEvents/DescribeLogStreams/DescribeLogGroups and s3:GetObject on that bucket.
3. bedrock-agentcore-control.create_agent_runtime(agentRuntimeName=<n>, roleArn=<that role>, agentRuntimeArtifact={'codeConfiguration': {'code': {'s3': {'bucket': <b>, 'prefix': 'agent/code.zip'}}, 'runtime': 'PYTHON_3_13', 'entryPoint': ['main.py']}}, networkConfiguration={'networkMode': 'PUBLIC'}, protocolConfiguration={'serverProtocol': 'HTTP'}) and note agentRuntimeId; poll get_agent_runtime until status == 'READY'.
4. logs.describe_log_groups(logGroupNamePrefix=f'/aws/bedrock-agentcore/runtimes/{agentRuntimeId}-DEFAULT') -> logGroups is [] and stays [] indefinitely. AWS returns exactly that one group with no retentionInDays member.
5. bedrock-agentcore-control.delete_agent_runtime(agentRuntimeId=<id>), then repeat step 4: AWS still returns the group (and its events); the emulator has nothing to return either way.
6. Replay bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events.
