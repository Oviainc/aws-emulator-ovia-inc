Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events
Issue-Key: bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events|_poll#4|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s; last={last!r}")
Issue-Commit: a4cd200a918caca0ae8b79fc6e58307dfb0eac4f

# CreateAgentRuntime creates no vended CloudWatch Logs group, so the runtime's log group is never found at create and cannot outlive the runtime

> _Note: this issue was identified against the code base at commit `a4cd200a918caca0ae8b79fc6e58307dfb0eac4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgentRuntime

## Description
Cause 1 of 1: a successful CreateAgentRuntime performs no CloudWatch Logs side effect, so the runtime's service-vended log group never exists and every read addressed by its derived name answers empty.

NOTE for the fixer: two OPEN issues already describe this same root cause — 20261101T213737Z-bedrock-agentcore-control-createagentruntime-creates-no-vended-cloudwatch-logs-group-for.md (which lists exactly this check in its Issue-Trace-Ids) and 20261102T090835Z-bedrock-agentcore-control-logs-agentcore-runtime-vended-log-group-is-never-created.md (its cause 1, grounded in bedrock-agentcore-control__logs.orphaned-runtime-log-group-names-runtime-not-found). This replay did NOT report this check as OWNED or ALREADY FILED — its replay key moved from `_poll#4|Timeout` to a `_poll` AssertionError now that the bounded wait expires inside the check's own budget rather than the harness cap — so it is re-filed here with current evidence against this checkout. Closing it closes the same defect those issues name; there is one fix, not three.

AWS:      recorded behavior only. The Smithy model (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json) binds no logging member on CreateAgentRuntime — its output is agentRuntimeArn/agentRuntimeId/agentRuntimeVersion/createdAt/status/workloadIdentityDetails with SUCCESS_STATUS 202 — so the log-group effect is model-silent and recording-stated. The recording shows that from a successful CreateAgentRuntime onward a service-made log group exists whose name is derived from the create's own response: the fixed prefix `/aws/bedrock-agentcore/runtimes/`, then the returned agentRuntimeId, then the fixed suffix `-DEFAULT` (the agentRuntimeId is the ONLY variable part; this run's id is not part of the requirement). DescribeLogGroups(logGroupNamePrefix=<that name>) returns exactly that one group, and that group carries NO retentionInDays member — the service-made group never expires. Agent stdout from an invocation lands in a stream under that group whose name names the runtimeSessionId, and both the group and its events SURVIVE DeleteAgentRuntime: the runtime delete must not cascade into CloudWatch Logs, which is why an orphaned group whose runtime no longer resolves is a reachable state in AWS.
Emulator: the group is never created. CreateAgentRuntime touches only this service's own store, so DescribeLogGroups for that prefix keeps answering an empty logGroups list for the whole bounded wait, and the check fails on its first assertion after create with `AssertionError: runtime log group exists at create not met within 60.0s; last=[]`. Nothing downstream in the check is reachable: there is no group for the invocation's marker to land in and no group to still be readable after the runtime is deleted.
Why:      every client, console view and troubleshooting workflow addresses a runtime's logs by that derived group name. With no group, DescribeLogGroups answers an empty list and FilterLogEvents answers empty or ResourceNotFoundException, so a caller that creates a runtime and then tails its logs waits forever on a resource AWS guarantees exists from create. The retained-after-delete lifecycle operators rely on (logs of a runtime that is gone) cannot be reproduced at all, and the absence is silent — the create succeeds with 202 and gives the caller no signal that its logs will never appear.
Fix:      the CreateAgentRuntime handler's side effects / state — services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_agent_runtime.rs, in handle() after the record is inserted: on a successful create, register the vended log group named from the minted agentRuntimeId in the CloudWatch Logs service state with no retention set; and ensure DeleteAgentRuntime's handler does NOT remove it, so the group and its events outlive the runtime.
Traces:   bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events. Code read in this checkout: create_agent_runtime.rs handle() writes only store.runtime_names.insert(...), store.remember_client_token(...) and store.agent_runtimes.insert(...) (lines ~336-341) and no CloudWatch Logs state is touched anywhere in the file.

## Evidence
From the simparity-behavioral-aws-v1 replay against this checkout.

Check bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events, verdict [mismatch]:
  File "check_bedrock_agentcore__bedrock_agentcore_control__logs_deleted_runtime_retains_runtime_log_group_and_events.py", line 32, in run
    found = _poll(lambda: logs.describe_log_groups(logGroupNamePrefix=rt["group"])["logGroups"], lambda g: len(g) >= 1, timeout_s=60.0, description="runtime log group exists at create")
  AssertionError: runtime log group exists at create not met within 60.0s; last=[]

The check builds rt["group"] as f"/aws/bedrock-agentcore/runtimes/{agentRuntimeId}-DEFAULT" from the agentRuntimeId CreateAgentRuntime returned, after polling GetAgentRuntime to status READY (so the create itself succeeded).

Expected (recorded AWS): DescribeLogGroups(logGroupNamePrefix=<that name>) -> exactly one group whose logGroupName equals that name, with no retentionInDays member; after the invocation the group holds a stream naming the runtimeSessionId with the agent's stdout line; after DeleteAgentRuntime the group lookup still returns that one group and the same events are still readable.
Actual (emulator): logGroups stays [] for the whole 60 s bounded wait, so the first assertion after create never becomes satisfiable and the marker/retention/after-delete assertions are never reached.

The group name's agentRuntimeId, the session id and the marker are this run's values filling the request; only the fixed prefix `/aws/bedrock-agentcore/runtimes/` and suffix `-DEFAULT`, the single-group result, and the absent retentionInDays are the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create an S3 bucket and upload a valid agent code zip to key agent/code.zip; create an IAM role trusted by bedrock-agentcore.amazonaws.com with logs:CreateLogGroup/CreateLogStream/PutLogEvents/DescribeLogStreams/DescribeLogGroups and s3:GetObject on that bucket.
3. bedrock-agentcore-control create-agent-runtime --agent-runtime-name <n> --role-arn <that role> --agent-runtime-artifact '{"codeConfiguration":{"code":{"s3":{"bucket":"<b>","prefix":"agent/code.zip"}},"runtime":"PYTHON_3_13","entryPoint":["main.py"]}}' --network-configuration '{"networkMode":"PUBLIC"}' --protocol-configuration '{"serverProtocol":"HTTP"}'; note agentRuntimeId; poll get-agent-runtime until status == READY.
4. aws logs describe-log-groups --log-group-name-prefix /aws/bedrock-agentcore/runtimes/<agentRuntimeId>-DEFAULT -> logGroups is [] and stays []. AWS returns exactly that one group, with no retentionInDays member.
5. Invoke the runtime and repeat step 4's group with filter-log-events --filter-pattern '"<marker>"' -> no events. AWS returns the agent's stdout line in a stream whose name names the runtimeSessionId.
6. aws bedrock-agentcore-control delete-agent-runtime --agent-runtime-id <id>, then repeat step 4: AWS still returns the group and its events; the emulator has nothing to return either way.
7. Re-replay: bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events
