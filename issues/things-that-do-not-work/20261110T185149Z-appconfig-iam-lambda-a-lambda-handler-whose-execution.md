Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__iam__lambda.execution-role-without-appconfig-actions-denied-in-handler
Issue-Key: appconfig__iam__lambda.execution-role-without-appconfig-actions-denied-in-handler|_invoke_until#27|Timeout|resp = lam.invoke(FunctionName=name, Payload=b'{"probe": "sim2real"}')
Issue-Commit: 6c534d81bd78f33a29d39ee989f5affa38b22780

# appconfig__iam__lambda: a Lambda handler whose execution role lacks AppConfig actions never produces a verdict — the under-privileged-handler check never completes (1 cause)

> _Note: this issue was identified against the code base at commit `6c534d81bd78f33a29d39ee989f5affa38b22780`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLatestConfiguration

## Description
Cause 1 of 1: the in-handler AppConfig configuration read never reaches a decidable answer, so the behaviour that an execution role without appconfig/appconfigdata actions is denied inside the handler is never observable. Failing check: appconfig__iam__lambda.execution-role-without-appconfig-actions-denied-in-handler.

AWS: recorded behavior only for the end-to-end shape (the model states the surface, not the timing). The check deploys a configuration, then invokes a function whose execution role is missing the AppConfig grants and reads the handler's own report. On AWS the handler's StartConfigurationSession/GetLatestConfiguration call is answered promptly — either with the deployed configuration bytes when the role is permitted, or refused AccessDeniedException when it is not — so the handler returns within the invoke and the caller sees a decidable result (the deployed document, or the documented fallback the handler applies after the denial). Either outcome is an answer; neither is an unbounded wait.

Emulator: the check never completes. The only recorded result is `Error: Check timed out after 60.0s` — the invoke/read sequence produces no verdict inside the 60 s budget, so neither the permitted read nor the AccessDeniedException refusal is ever observed.

Why: a client that gates a feature flag on an AppConfig read from inside Lambda gets no answer at all instead of either the configuration or an authorization refusal. Code that depends on the denial being fast and explicit (fall back to a default on AccessDeniedException) hangs instead of falling back, and an authorization regression — a role with no AppConfig grant being served the configuration — cannot be detected.

Fix: the AppConfig data-plane read path reached from the in-process Lambda runtime (the appconfigdata StartConfigurationSession/GetLatestConfiguration handlers and the Lambda invoke path that carries the handler's response back to the caller) must terminate with a response — the configuration for a permitted role, AccessDeniedException for a role without the action — rather than leaving the invocation outstanding. Related open issues describe the same family for other appconfig__iam__lambda checks (20261110T161251Z-appconfig-iam-lambda-a-lambda-handler-that-reads.md, 20261107T182959Z-appconfig-iam-lambda-a-lambda-handler-s-in-990472.md, 20261106T144610Z-appconfig-iam-lambda-a-lambda-handler-s-in.md); none of them carries this check's replay key, so it is filed here.

Traces: appconfig__iam__lambda.execution-role-without-appconfig-actions-denied-in-handler.

## Evidence
Expected (AWS): the invoked handler's AppConfig read returns promptly — the deployed configuration bytes for a permitted execution role, or an AccessDeniedException the handler converts into its documented fallback answer — so the check reads a verdict from the invoke response.

Actual (emulator): no verdict is ever produced. The suite reports for appconfig__iam__lambda.execution-role-without-appconfig-actions-denied-in-handler:
  Error: Check timed out after 60.0s
The 60 s figure is the harness budget, not a requirement; the defect is that the in-handler AppConfig read/invoke never settles into either answer.

## Steps to Reproduce
1. Create an AppConfig application, environment and configuration profile, add a hosted configuration version and StartDeployment it to COMPLETE.
2. Create a Lambda function whose execution role has no appconfig:* / appconfigdata:* grants, with a handler that calls StartConfigurationSession + GetLatestConfiguration and reports what it got.
3. Invoke the function and wait for the handler's report.
4. Observe no answer arrives — neither the configuration nor an AccessDeniedException — and the check ends on its timeout.
