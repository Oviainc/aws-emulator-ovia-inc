Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control__iam__logs.agentcore-team-session-list-agent-runtimes-denied-while-granted-twin-lists
Issue-Key: bedrock-agentcore-control__iam__logs.agentcore-team-session-list-agent-runtimes-denied-while-granted-twin-lists|_refusal#18|AssertionError|raise AssertionError("the call was expected to be refused, but it succeeded")
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# ListAgentRuntimes succeeds for an assumed-role session the identity policy denies

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAgentRuntimes

## Description
Cause 1 of 1: bedrock-agentcore-control performs no IAM authorization, so a denied session is served.
AWS: a call made with an assumed-role session whose identity policy does not allow bedrock-agentcore:ListAgentRuntimes is refused with AccessDeniedException (HTTP 403) naming the principal, the action and the resource; the granted twin session lists the runtimes. The recorded run shows the denied session refused and the granted one succeeding.
Emulator: the denied session's call returns 200 with the runtime list — 'the call was expected to be refused, but it succeeded' — so the denied and granted sessions are indistinguishable.
Why: a client cannot test or rely on least-privilege policies for AgentCore control-plane reads; an unauthorized caller sees every runtime in the account.
Fix: request authorization for the agentcore-control operations (the ListAgentRuntimes handler must consult the caller's identity policy before answering).
Traces: bedrock-agentcore-control__iam__logs.agentcore-team-session-list-agent-runtimes-denied-while-granted-twin-lists.

## Evidence
Expected: AccessDeniedException for the denied assumed-role session. Actual: 'AssertionError: the call was expected to be refused, but it succeeded'.

## Steps to Reproduce
Replay bedrock-agentcore-control__iam__logs.agentcore-team-session-list-agent-runtimes-denied-while-granted-twin-lists: it assumes a role whose policy denies bedrock-agentcore:ListAgentRuntimes and calls ListAgentRuntimes; the call returns 200.
