Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__sts.nodejs22-execution-role-credentials-in-environment iam__lambda__sts.nodejs22-environment-keys-without-session-token-rejected
Issue-Key: iam__lambda__sts.nodejs22-execution-role-credentials-in-environment|run#7|AssertionError|assert present == {k: True for k in _KEYS}, (
Issue-Key: iam__lambda__sts.nodejs22-environment-keys-without-session-token-rejected|run#6|AssertionError|assert present == {k: True for k in _KEYS}, (
Issue-Commit: a0fae0073c5a57a2ef84b70eabff7d12afa58f91

# iam__lambda__sts: a nodejs22.x function's handler environment carries no execution-role credentials — AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY and AWS_SESSION_TOKEN are all unset

> _Note: this issue was identified against the code base at commit `a0fae0073c5a57a2ef84b70eabff7d12afa58f91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
1 net-new root cause (2 failing checks share it).

Cause 1 of 1: the in-process Lambda runtime does not inject the function's execution-role session credentials into the handler's process environment, so a nodejs22.x handler sees none of the three reserved credential variables.
AWS: the managed Lambda runtimes set the reserved environment variables `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and `AWS_SESSION_TOKEN` for every invocation (AWS documents them as reserved runtime environment variables: https://docs.aws.amazon.com/lambda/latest/dg/configuration-envvars.html). They are the TEMPORARY credentials of an STS session of the function's execution role, so (a) the AWS SDK's default credential chain inside the handler picks them up unchanged, (b) `sts:GetCallerIdentity` from the handler reports `arn:<partition>:sts::<account>:assumed-role/<execution-role-name>/<function-name>` — the session is named for the function — and (c) because they are temporary keys, signing with the key and secret but WITHOUT the session token is refused by STS with HTTP 403, error code `InvalidClientTokenId` and the message `The security token included in the request is invalid.` (fixed text; no variable part). Smithy (lambda Invoke, com-amazonaws-lambda-awsgirapiservice): POST /2015-03-31/functions/{FunctionName}/invocations, `StatusCode` is @httpResponseCode, `FunctionError` is bound to header `X-Amz-Function-Error` and is set only when the handler itself faulted, `Payload` is the @httpPayload blob — the model has no error for a runtime that fails to vend credentials, because AWS always vends them.
Emulator: `Invoke` answers 200 with no FunctionError and the handler's payload, but the handler reports `{"AWS_ACCESS_KEY_ID": false, "AWS_SECRET_ACCESS_KEY": false, "AWS_SESSION_TOKEN": false}` — all three variables are absent (or empty) in `process.env` for a nodejs22.x function. Because the variables are missing, everything downstream of them is unreachable in the same run: the SDK v3 default chain has nothing to sign with, `GetCallerIdentity` cannot confirm the `assumed-role/<role>/<function>` session ARN, and the token-less-signing refusal (403 `InvalidClientTokenId` / `The security token included in the request is invalid.`) is never exercised.
Why: a client cannot run any handler that uses the AWS SDK's default credential provider — the near-universal way function code calls AWS. Code that reads the variables directly gets undefined, code that relies on the default chain is unsigned/anonymous, and the function's identity (which execution role it runs as, and that it runs as a SESSION of that role rather than as long-term keys) is unobservable, so an over-broad execution role is indistinguishable from a correctly scoped one.
Fix: the Invoke execution path's runtime environment for nodejs runtimes — before handing control to the handler, mint an STS session of the function's execution role named for the function and export its key, secret and session token as `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`/`AWS_SESSION_TOKEN`, and have the signing/authorization path treat those keys as temporary credentials (so a request signed with key+secret and no token is refused 403 `InvalidClientTokenId` with STS's exact message).
Related, NOT the same cause: open issue 20261105T085348Z-iam-lambda-sts-the-function-s-own-identity.md reports the python runtime having no boto3 on the import path. Here the nodejs22.x module loaded and the invoke succeeded; what is missing is the credential environment itself.
Traces: iam__lambda__sts.nodejs22-execution-role-credentials-in-environment, iam__lambda__sts.nodejs22-environment-keys-without-session-token-rejected.

## Evidence
Expected (AWS): the handler's `process.env` has all three of AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_SESSION_TOKEN set to non-empty strings; SDK v3's default credentials equal them; GetCallerIdentity from the handler returns `arn:<partition>:sts::<account>:assumed-role/<role-name>/<function-name>`; and a GetCallerIdentity signed with the same key+secret but no session token is refused HTTP 403 `InvalidClientTokenId` with message `The security token included in the request is invalid.`

Actual (emulator), identically in both checks (Invoke returned 200 with no FunctionError, so the nodejs22.x module did load):
- iam__lambda__sts.nodejs22-execution-role-credentials-in-environment — `AssertionError: credential variables set in the nodejs22.x environment: {'AWS_ACCESS_KEY_ID': False, 'AWS_SECRET_ACCESS_KEY': False, 'AWS_SESSION_TOKEN': False}, expected all three`
- iam__lambda__sts.nodejs22-environment-keys-without-session-token-rejected — `AssertionError: credential variables set in the nodejs22.x environment: {'AWS_ACCESS_KEY_ID': False, 'AWS_SECRET_ACCESS_KEY': False, 'AWS_SESSION_TOKEN': False}, expected all three`
(The handlers report only booleans, never a credential value, so nothing here depends on a per-run secret.)

## Steps to Reproduce
1. iam:CreateRole with a trust policy allowing `lambda.amazonaws.com` to `sts:AssumeRole` (no permissions policy needed).
2. lambda:CreateFunction with Runtime=`nodejs22.x`, Handler=`index.handler`, that role as Role, and an inline zip whose `index.js` reports for each of AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY / AWS_SESSION_TOKEN whether `process.env` has it as a non-empty string; wait for State=Active.
3. lambda:Invoke (RequestResponse) with payload `{}`.
4. Observe 200, no FunctionError, and all three variables reported absent. With them present, AWS additionally yields: SDK v3 default credentials equal the environment's key and token; GetCallerIdentity returns `arn:<partition>:sts::<account>:assumed-role/<role>/<function>`; and the same key+secret signed WITHOUT the token is refused 403 `InvalidClientTokenId` / `The security token included in the request is invalid.`
