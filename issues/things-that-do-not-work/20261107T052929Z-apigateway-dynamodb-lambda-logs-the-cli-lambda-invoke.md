Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.cli-lambda-invoke-missing-permission-negative
Issue-Key: apigateway__dynamodb__lambda__logs.cli-lambda-invoke-missing-permission-negative|_wait_for_log_marker#35|Timeout|time.sleep(interval_s)
Issue-Commit: f3ca7ead92e65d3eb9c80914c7f5792563c2115c

# apigateway__dynamodb__lambda__logs: the CLI lambda-invoke permission-boundary check reaches no verdict — an Invoke from an unauthorized assumed-role session is not refused and the control invocation's log line never arrives, so the check runs to the 60 s cap

> _Note: this issue was identified against the code base at commit `f3ca7ead92e65d3eb9c80914c7f5792563c2115c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1 — `aws lambda invoke` under an assumed-role session that holds no lambda permissions is not refused, and the positive-control invocation's stdout never reaches /aws/lambda/<function-name>, so the check never reaches a terminal verdict and the harness reports `Check timed out after 60.0s`.

AWS: Lambda's Invoke is an IAM-authorized call. A caller whose identity-based policy allows nothing but sts:GetCallerIdentity is refused by the authorization layer, not by the handler: HTTP 403 with error code `AccessDeniedException`. Smithy (service-com-amazonaws-lambda-awsgirapiservice, operation Invoke, POST /2015-03-31/functions/{FunctionName}/invocations, success code 200) models no AccessDeniedException among Invoke's errors — the refusal is the service-common IAM one, which the AWS CLI surfaces on stderr as `An error occurred (AccessDeniedException) when calling the Invoke operation: User: <caller ARN> is not authorized to perform: lambda:InvokeFunction on resource: <function ARN> because no identity-based policy allows the lambda:InvokeFunction action`. Fixed text in that message is `is not authorized to perform:`, `on resource:` and `because no identity-based policy allows the ... action`; the variable parts are the caller's assumed-role ARN, the action name `lambda:InvokeFunction` and the target function ARN, all taken from the request and the emulator's IAM state — never the literal ARNs of any one run. A refused Invoke must also leave the function unexecuted, so nothing is written to its log group for that call. For the authorized caller, the same Invoke returns 200 with the handler's own JSON return value and no FunctionError, and that invocation creates /aws/lambda/<function-name> (unless LoggingConfig.LogGroup names another), opens a dated log stream and writes START/END/REPORT framing plus the handler's stdout, readable through FilterLogEvents within seconds.

Emulator: recorded behavior only — the check never completes. The replay reports no status, no error code and no payload for either leg: `Error: Check timed out after 60.0s`. Two observable sub-behaviors are consistent with that and are what the check is asserting on: (a) the restricted-session CLI invoke is not refused — no `An error occurred (AccessDeniedException)` ever appears on stderr, i.e. Lambda Invoke performs no IAM authorization of the calling identity, and (b) the full-permission control invoke's marker line is never readable from /aws/lambda/<function-name>, so the check's 60 s poll for it consumes the whole budget. Hypothesis: (b) is the dominant cost — the log-marker poll is the only 60 s wait in the check, and the same symptom is already filed for other checks of this service as cause 2 of open issue 20261105T111951Z-apigateway-dynamodb-lambda-logs-the-lambda-python-runtime.md (which lists this check id but no longer owns its replay key, because the check now times out instead of failing the `returncode != 0` assertion).

Why: a client cannot rely on Lambda's permission boundary at all. Code that depends on an unprivileged principal being unable to invoke a function sees the invocation succeed, so an emulator-backed test of a least-privilege deployment passes while the real deployment is refused — and conversely the authorized invocation produces no observable log output, so neither the deny nor the allow side of the boundary can be verified. The check yields no verdict, so this whole permission-boundary class is ungraded.

Fix: the Lambda Invoke handler's authorization step (reject a caller with no lambda:InvokeFunction grant with 403 AccessDeniedException before dispatching to the runtime, and do not execute the function on refusal), and the Lambda→CloudWatch Logs delivery path in the invoke path (implicit /aws/lambda/<function-name> creation, dated stream naming, START/END/REPORT plus handler stdout emission).

Traces: apigateway__dynamodb__lambda__logs.cli-lambda-invoke-missing-permission-negative.

## Evidence
Failing check: apigateway__dynamodb__lambda__logs.cli-lambda-invoke-missing-permission-negative — verdict `mismatch`, detail `Error: Check timed out after 60.0s`.

Expected (AWS):
- restricted assumed-role session: `aws lambda invoke --function-name <fn> --payload fileb://<f> <out>` exits non-zero; stderr carries `An error occurred (AccessDeniedException) when calling the Invoke operation: User: <assumed-role ARN> is not authorized to perform: lambda:InvokeFunction on resource: <function ARN> because no identity-based policy allows the lambda:InvokeFunction action` (HTTP 403, code AccessDeniedException); FilterLogEvents on /aws/lambda/<fn> shows no `INVOKED:` line for that call.
- full-permission caller: same command exits 0, the output file holds the handler's return value `{"ok": true}` with no FunctionError, and FilterLogEvents on /aws/lambda/<fn> yields the handler's printed marker within seconds.

Actual (emulator): neither leg produces an asserted-on observation within the check's 60 s budget — the run is cut off with `Check timed out after 60.0s`, so no status code, error code or payload is returned for the sequence. No AccessDeniedException is reported for the unauthorized caller and no log event for the authorized one.

Related open issue (does not cover this check — the suite did not mark it OWNED): 20261105T111951Z-apigateway-dynamodb-lambda-logs-the-lambda-python-runtime.md.

## Steps to Reproduce
1. CreateFunction (python3.12) with a handler that prints `INVOKED:<marker from the event>` and returns `{"ok": True}`; wait for State=Active.
2. CreateRole trusting arn:aws:iam::<account>:root, attach an inline policy allowing only sts:GetCallerIdentity, and AssumeRole it.
3. With only those session credentials in the environment run `aws lambda invoke --function-name <fn> --payload fileb://<payload.json> <out.json>`. Expected: non-zero exit and stderr `An error occurred (AccessDeniedException) ... is not authorized to perform: lambda:InvokeFunction on resource: <function ARN> because no identity-based policy allows the lambda:InvokeFunction action`. Observed: no such refusal.
4. FilterLogEvents(logGroupName='/aws/lambda/<fn>', filterPattern='"INVOKED:DENIED-CALL"') — expected: no events (function never ran).
5. Re-run the same CLI invoke with full-permission credentials. Expected: exit 0, out.json == {"ok": true}, and FilterLogEvents(filterPattern='"INVOKED:ALLOWED-CALL"') returns the line within seconds. Observed: the marker never appears, the poll runs to its deadline and the check is cut off at 60 s with no verdict.
