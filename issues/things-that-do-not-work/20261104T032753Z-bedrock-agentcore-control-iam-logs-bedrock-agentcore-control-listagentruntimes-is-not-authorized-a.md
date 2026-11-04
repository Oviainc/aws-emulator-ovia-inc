Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control__iam__logs.agentcore-team-session-list-agent-runtimes-denied-while-granted-twin-lists
Issue-Key: bedrock-agentcore-control__iam__logs.agentcore-team-session-list-agent-runtimes-denied-while-granted-twin-lists|_refusal#15|AssertionError|raise AssertionError("the call was expected to be refused, but it succeeded")
Issue-Commit: eb1ede2cbce186d0b7cae6d474f923ddb0f91b8a

# bedrock-agentcore-control ListAgentRuntimes is not authorized: a session granted only bedrock-agent reads lists every agent runtime instead of being refused AccessDeniedException (1 cause)

> _Note: this issue was identified against the code base at commit `eb1ede2cbce186d0b7cae6d474f923ddb0f91b8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAgentRuntimes

## Description
Cause 1 of 1: ListAgentRuntimes is served without consulting the caller's identity-based policy, so a session that holds no bedrock-agentcore grant at all lists the account's agent runtimes.

AWS: Smithy ListAgentRuntimes (POST /runtimes/, output agentRuntimes @required) models AccessDeniedException with errorKind client and httpError 403. bedrock-agent:* (Agents for Bedrock) is a different action namespace from bedrock-agentcore:*, so a role whose only agent grant is Allow bedrock-agent:ListAgents, bedrock-agent:GetAgent on Resource "*" grants nothing on this operation. The recording shows the call refused 403 AccessDeniedException with the message "User: <callerArn> is not authorized to perform: bedrock-agentcore:ListAgentRuntimes on resource: <resourceArn> because no identity-based policy allows the bedrock-agentcore:ListAgentRuntimes action" — the fixed text exactly as shown, with <callerArn> the caller's assumed-role ARN (arn:aws:sts::<account>:assumed-role/<roleName>/<sessionName>) and <resourceArn> the list operation's resource pattern arn:aws:bedrock-agentcore:<region>:<account>:runtime/* — and no runtimes disclosed.

Emulator: the identical call from that session succeeds and returns the runtime list (the check's refusal helper reports "the call was expected to be refused, but it succeeded"). The check's own positive controls pass first — the session's CloudWatch Logs reads of the runtime group work, and a twin role identical except for one added Allow of bedrock-agentcore:ListAgentRuntimes is polled to allowed and lists exactly what the account owner lists — so this is not propagation lag: the denied session and the granted session get the same answer because the policy is never read.

Why: a client cannot scope an AgentCore session at all. A read-only troubleshooting role intended to see only Bedrock agent metadata enumerates every agent runtime in the account, and any least-privilege test of AgentCore passes against the emulator and fails against AWS. Because the emulator answers 200 instead of 403, code that catches AccessDeniedException to fall back or to report a permissions problem never runs.

Fix: the identity-policy authorization gate in the bedrock-agentcore-control request dispatch / ListAgentRuntimes handler entry — evaluate the signing principal's identity-based policies against action bedrock-agentcore:ListAgentRuntimes and resource arn:aws:bedrock-agentcore:<region>:<account>:runtime/* before the listing is produced, and emit the modeled AccessDeniedException (403) with the message template above.

Traces: bedrock-agentcore-control__iam__logs.agentcore-team-session-list-agent-runtimes-denied-while-granted-twin-lists. Related but not covering it: open issue 20261029T212239Z-bedrock-agentcore-control-accepts-four-requests-aws-refuses.md names the missing authorization gate for CreateAgentRuntime and DeleteGateway (its Cause 3); this check exercises the read path ListAgentRuntimes and was not marked as owned by it, so it is filed separately and should be fixed by the same service-wide gate.

## Evidence
Check agentcore-team-session-list-agent-runtimes-denied-while-granted-twin-lists, at the refusal step: ctx.client("bedrock-agentcore-control", creds=team).list_agent_runtimes() raised nothing — AssertionError: "the call was expected to be refused, but it succeeded". Expected ('AccessDeniedException', 'User: arn:aws:sts::<account>:assumed-role/<teamRole>/sim2real-team is not authorized to perform: bedrock-agentcore:ListAgentRuntimes on resource: arn:aws:bedrock-agentcore:<region>:<account>:runtime/* because no identity-based policy allows the bedrock-agentcore:ListAgentRuntimes action') with HTTP 403.
The check asserts immediately before this that the team role's agent policy grants exactly ['bedrock-agent:GetAgent', 'bedrock-agent:ListAgents'] and nothing else, and that the twin role (same policy plus one Allow of bedrock-agentcore:ListAgentRuntimes on "*") is already allowed and lists what the account owner lists — so the grant difference is the only variable.
Account number, region and the generated role name are per-run values filling the message template.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (service bedrock-agentcore-control__iam__logs).
2. Create a log group /aws/bedrock-agentcore/runtimes/<tok>-DEFAULT with one seeded event (the check's liveness control).
3. Create two roles trusted by arn:aws:iam::<account>:root, each with an inline logs read policy and an inline agent policy of Allow [bedrock-agent:ListAgents, bedrock-agent:GetAgent] on "*"; add to the second (twin) role one extra statement Allow bedrock-agentcore:ListAgentRuntimes on "*".
4. sts:AssumeRole both as sim2real-team and call bedrock-agentcore-control ListAgentRuntimes with each session.
5. The twin session must list the account's runtimes; the team session must be refused 403 AccessDeniedException with the message above. The emulator serves both.
