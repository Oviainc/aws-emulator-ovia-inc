Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.concurrent-lambda-ddb-writes-no-lost-updates dynamodb__lambda.ddb-leading-keys-condition-scopes-lambda-partition-access dynamodb__lambda.ddb-resource-policy-alone-grants-lambda-role-get-item dynamodb__lambda.function-sdk-put-to-unpermitted-table-denied-as-function-error dynamodb__lambda.lambda-env-var-update-redirects-the-next-invoke-to-the-new-table dynamodb__lambda.lambda-published-version-pins-its-env-while-latest-moves dynamodb__lambda.conc-limits-and-fanout-reserved-concurrency-admission-transitions-rejected
Issue-Key: dynamodb__lambda.concurrent-lambda-ddb-writes-no-lost-updates|run#9|AssertionError|assert val == 5, (
Issue-Key: dynamodb__lambda.ddb-leading-keys-condition-scopes-lambda-partition-access|_invoke_payload#7|AssertionError|assert "error" not in result, f"handler faulted instead of returning a result: {result!r}"
Issue-Key: dynamodb__lambda.ddb-resource-policy-alone-grants-lambda-role-get-item|_invoke_payload#8|AssertionError|assert "error" not in result, f"handler faulted instead of returning a result: {result!r}"
Issue-Key: dynamodb__lambda.function-sdk-put-to-unpermitted-table-denied-as-function-error|run#9|AssertionError|assert err is None, f"the permitted PutItem failed the invocation: {err!r} {out!r}"
Issue-Key: dynamodb__lambda.lambda-env-var-update-redirects-the-next-invoke-to-the-new-table|_invoke_with_iam_retry#10|AssertionError|raise AssertionError(f"invoking {fn} with {payload!r} failed: {last!r}")
Issue-Key: dynamodb__lambda.lambda-published-version-pins-its-env-while-latest-moves|_invoke_with_iam_retry#22|AssertionError|raise AssertionError(f"invoking {fn} (qualifier {qualifier!r}) failed: {last!r}")
Issue-Key: dynamodb__lambda.conc-limits-and-fanout-reserved-concurrency-admission-transitions-rejected|run#33|Timeout|resp = lam_ctl.invoke(FunctionName=fn, Payload=json.dumps(
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# dynamodb__lambda: the Lambda python runtime has no AWS SDK, and reserved-concurrency admission never settles

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 2 — the managed python runtime ships no boto3/botocore.
AWS: the python3.x managed runtimes bundle boto3/botocore; a handler may `import boto3` and call dynamodb:GetItem/PutItem/UpdateItem under its execution role, returning its own result — or the service's AccessDeniedException, surfaced as a handled payload, when an identity policy, a leading-key condition or a table resource policy denies it.
Emulator: Invoke answers HTTP 200 with FunctionError='Unhandled' and payload {"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError"} — fixed text "Unable to import module '<module>': No module named 'boto3'" with the configured handler module substituted.
Why: no handler can read or write a table, so the env-var repointing, per-version env pinning, leading-key scoping and resource-policy grant behaviours are all unobservable and their negative controls prove nothing.
Fix: the python runtime's bundled site-packages / module search path in the lambda runtime component.
Traces: dynamodb__lambda.concurrent-lambda-ddb-writes-no-lost-updates, .ddb-leading-keys-condition-scopes-lambda-partition-access, .ddb-resource-policy-alone-grants-lambda-role-get-item, .function-sdk-put-to-unpermitted-table-denied-as-function-error, .lambda-env-var-update-redirects-the-next-invoke-to-the-new-table, .lambda-published-version-pins-its-env-while-latest-moves.

Cause 2 of 2 — reserved-concurrency admission transitions never settle.
AWS: PutFunctionConcurrency takes effect immediately: with ReservedConcurrentExecutions=0 every invoke is refused with TooManyRequestsException (429, Reason=ReservedFunctionConcurrentInvocationLimitExceeded), and raising the value admits invocations again; the transition is observable within a second or two.
Emulator: recorded behavior only — the check never observes the admission transition and the harness reports "Check timed out after 60.0s".
Why: a client cannot rely on reserved concurrency either to protect a downstream or to release traffic again, and the control plane reports a setting the data plane does not honour.
Fix: the concurrency admission control in the invoke path and its coupling to PutFunctionConcurrency.
Traces: dynamodb__lambda.conc-limits-and-fanout-reserved-concurrency-admission-transitions-rejected.

## Evidence
Cause 1: AssertionError: handler faulted instead of returning a result / the permitted PutItem failed the invocation: 'Unhandled' {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError'}.
Cause 2: Error: Check timed out after 60.0s. Expected: with ReservedConcurrentExecutions=0 Invoke is refused 429 TooManyRequestsException, and raising the reservation admits it again, both within the window.

## Steps to Reproduce
1. CreateFunction (python3.12) importing boto3 and calling dynamodb.put_item; Invoke (cause 1).
2. PutFunctionConcurrency ReservedConcurrentExecutions=0 and Invoke; then raise it and Invoke again (cause 2).
