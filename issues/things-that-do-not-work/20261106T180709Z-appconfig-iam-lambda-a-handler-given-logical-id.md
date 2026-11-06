Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__iam__lambda.handler-with-logical-id-identifiers-falls-back-to-default
Issue-Key: appconfig__iam__lambda.handler-with-logical-id-identifiers-falls-back-to-default|_invoke_until#20|Timeout|time.sleep(5.0)
Issue-Commit: f50d8ce9e37ffa71976cdbe00198995770403bcc

# appconfig__iam__lambda: a handler given logical-id-shaped AppConfig identifiers never converges on either answer, so the fallback-to-default check is killed at the harness cap

> _Note: this issue was identified against the code base at commit `f50d8ce9e37ffa71976cdbe00198995770403bcc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLatestConfiguration

## Description
One net-new root cause.

Cause 1 of 1: a Lambda handler that reads AppConfig through the appconfigdata data plane never produces either of the two answers the check distinguishes, so the invoke-until-converged probe runs to the harness's 60 s cap and the check is killed with nothing observed. Failing check id: appconfig__iam__lambda.handler-with-logical-id-identifiers-falls-back-to-default.

AWS: AppConfigData StartConfigurationSession takes ApplicationIdentifier, EnvironmentIdentifier and ConfigurationProfileIdentifier, each of which resolves either an id or a NAME; GetLatestConfiguration then serves the deployed document's bytes. An identifier that names nothing (the CloudFormation logical-id shape this check uses) resolves to no application/environment/profile and the data plane refuses it — the handler catches that refusal and returns its own default document — while the sibling function given the three real ids receives the deployed bytes verbatim. Lambda Invoke answers HTTP 200 with the handler's JSON and sets no FunctionError in both arms, because the handler catches its own AppConfig failure. Smithy: appconfigdata StartConfigurationSession/GetLatestConfiguration; the sibling check in the same group pins the refusal class for an unresolvable identifier as ResourceNotFoundException and the refusal class for a principal with no appconfig:* actions as AccessDeniedException. Recorded behavior only for the handler's own payload shape ({source, config, code}), which the check authors.

Emulator: the check never completes. The replay reports only `Error: Check timed out after 60.0s` — the probe invokes the function every 5 s for up to 240 s waiting for the logical-id function to answer source="default" and the real-id function to answer source="appconfig", and neither predicate ever holds, so the check is still polling when the 60 s harness budget expires. No status, no error code and no served bytes are ever observed.

Why: a client cannot tell an unreachable AppConfig identifier from a served configuration. Code that relies on AWS's behavior — fall back to a safe default when AppConfig cannot be reached, serve the deployed flag document when it can — gets neither outcome: the call never resolves to an answer the caller can branch on, so a feature-flag rollout is silently unobservable rather than either applied or defaulted.

Fix: the Lambda invoke path that runs the handler and the in-function appconfigdata client it uses, plus AppConfigData's identifier resolution (StartConfigurationSession/GetLatestConfiguration handlers and their state lookup of application/environment/configuration-profile by id or name). The two existing open issues in this group — 20261105T010503Z-appconfig-iam-lambda-cloudformation-ref-on-aws-appconfig-application-resolves-to.md and 20261106T144610Z-appconfig-iam-lambda-a-lambda-handler-s-in.md — do not carry this check's replay key.

Traces: appconfig__iam__lambda.handler-with-logical-id-identifiers-falls-back-to-default (verdict mismatch; detail `Error: Check timed out after 60.0s`).

## Evidence
Expected: both arms answer; the logical-id function returns {"source": "default", "config": {"useNewBackend": false}} (its own default, after the data plane refuses the unresolvable identifier) and the real-id function returns {"source": "appconfig", "config": {"useNewBackend": true}} (the deployed bytes), each over a 200 Invoke with no FunctionError, and the check completes.
Actual: neither predicate is ever satisfied; the check is terminated by the harness with `Error: Check timed out after 60.0s` and no AppConfig answer is produced at all.

## Steps to Reproduce
1. Create an AppConfig application, environment, hosted configuration profile and a 0-minute/100%-growth deployment strategy; create a hosted configuration version with {"useNewBackend": true} and deploy it to COMPLETE.
2. Create two python Lambda functions sharing one execution role granting the appconfig/appconfigdata actions; give one the real application/environment/profile ids in its environment variables and the other CloudFormation-logical-id-shaped identifiers.
3. Invoke each function repeatedly and read the JSON payload.
4. Observe that the real-id function never answers source="appconfig" with the deployed document and the logical-id function never answers source="default"; the check polls until the harness kills it at 60 s.
