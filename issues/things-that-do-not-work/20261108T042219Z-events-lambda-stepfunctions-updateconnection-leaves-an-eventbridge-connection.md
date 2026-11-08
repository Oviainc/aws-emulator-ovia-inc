Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__stepfunctions.deep-sfn-3-http-task-deauthorized-connection-rejected
Issue-Key: events__lambda__stepfunctions.deep-sfn-3-http-task-deauthorized-connection-rejected|run#26|AssertionError|assert state == "AUTHORIZED", f"UpdateConnection must re-authorize the connection, got {state!r}"
Issue-Commit: 742d95e483c6290726dac54edb7cc16a76e76b19

# events__lambda__stepfunctions: UpdateConnection leaves an EventBridge connection stuck in UPDATING and it never reaches AUTHORIZED, so the HTTP-Task re-authorization path is unreachable

> _Note: this issue was identified against the code base at commit `742d95e483c6290726dac54edb7cc16a76e76b19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConnection

## Description
Cause 1 of 1: an EventBridge connection re-authorized with a new API key never settles — its ConnectionState stays `UPDATING` forever instead of converging to `AUTHORIZED`.
  AWS: UpdateConnection on an API_KEY connection returns a ConnectionState from the modeled set and the connection then converges to a terminal authorized state; DescribeConnection subsequently reports `ConnectionState: AUTHORIZED` (and refreshes `LastAuthorizedTime`). Smithy: eventbridge#UpdateConnection output member `ConnectionState` targets `ConnectionState`, whose values are CREATING, UPDATING, DELETING, AUTHORIZED, DEAUTHORIZED, AUTHORIZING, DEAUTHORIZING, ACTIVE, FAILED_CONNECTIVITY; `UPDATING`/`AUTHORIZING` are transitional and `AUTHORIZED` is the terminal state for a successfully authorized connection. DescribeConnection also carries `StateReason` for a non-authorized state.
  Emulator: after UpdateConnection with a new ApiKeyAuthParameters value, polled DescribeConnection keeps returning `ConnectionState: 'UPDATING'` and never transitions. The transitional state is written but no transition to AUTHORIZED is ever performed.
  Why: a client relying on AWS's state machine hangs. Any wait-for-AUTHORIZED loop around UpdateConnection never terminates, and every downstream behavior gated on an authorized connection is unreachable: a Step Functions HTTP Task over that connection can neither be shown to fail with `Events.ConnectionResource.InvalidConnectionState` while deauthorized nor to succeed with the rotated credential afterwards, so credential rotation cannot be tested at all.
  Fix: the EventBridge connection state machine in the UpdateConnection handler and the connection's stored state — the transition that must advance UPDATING/AUTHORIZING to AUTHORIZED (setting LastAuthorizedTime and clearing StateReason) once the new auth parameters are stored, so DescribeConnection reports a terminal state.
  Traces: events__lambda__stepfunctions.deep-sfn-3-http-task-deauthorized-connection-rejected.

Related open issue that does NOT cover this one (different service scope, and that one is about an OAUTH connection): 20261107T205257Z-events-4-net-new-causes-an-oauth-connection.md.

## Evidence
events__lambda__stepfunctions.deep-sfn-3-http-task-deauthorized-connection-rejected:
Expected: after UpdateConnection with a fresh API key, the connection's ConnectionState is 'AUTHORIZED' (terminal), enabling the HTTP Task to succeed with the new credential.
Actual: AssertionError: UpdateConnection must re-authorize the connection, got 'UPDATING'
The check therefore never reaches its downstream assertion that the deauthorized state machine fails with exactly 'Events.ConnectionResource.InvalidConnectionState'.

## Steps to Reproduce
1. git_clone(); run_behavioral_suite().
2. events.create_connection(AuthorizationType='API_KEY', AuthParameters={'ApiKeyAuthParameters': {'ApiKeyName': 'x-...-key', 'ApiKeyValue': <key1>}}) and wait for AUTHORIZED.
3. events.deauthorize_connection(...), then events.update_connection(Name=<conn>, AuthorizationType='API_KEY', AuthParameters={'ApiKeyAuthParameters': {'ApiKeyName': 'x-...-key', 'ApiKeyValue': <key2>}}).
4. Poll events.describe_connection(Name=<conn>)['ConnectionState']. The emulator returns 'UPDATING' indefinitely instead of 'AUTHORIZED'.
5. Re-replay the check id in trace_ids to confirm.
