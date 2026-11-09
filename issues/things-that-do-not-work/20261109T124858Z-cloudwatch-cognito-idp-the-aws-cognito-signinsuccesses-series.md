Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__cognito-idp.obs-cognito-idp-2-sign-in-rejected-for-disabled-flow-is-a-zero-sample
Issue-Key: cloudwatch__cognito-idp.obs-cognito-idp-2-sign-in-rejected-for-disabled-flow-is-a-zero-sample|_poll_until#23|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 68689e45c55a2a3c2aeeec91dff9a251e0f23b15

# cloudwatch__cognito-idp: the AWS/Cognito SignInSuccesses series is never published, so a refused sign-in can never be read back as a zero sample

> _Note: this issue was identified against the code base at commit `68689e45c55a2a3c2aeeec91dff9a251e0f23b15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: user-pool sign-in traffic produces no AWS/Cognito metric datapoints.
AWS: recorded behavior only (metric emission is not Smithy-modelled; the read is GetMetricStatistics/GetMetricData over namespace AWS/Cognito). A sign-in attempt against a user pool publishes a SignInSuccesses datapoint dimensioned by UserPool/UserPoolClient; an attempt refused because the flow is disabled publishes a zero sample for that period, and the later repaired sign-in publishes a non-zero one, so the series shows the refused attempt and the repaired one.
Emulator: the series never appears — the metric read polls to the end of the check's budget without ever seeing the repaired sign-in or the earlier attempts, so no datapoint for the pool is ever readable.
Why: a customer cannot observe or alarm on Cognito sign-in volume or failure: dashboards stay empty and an alarm on SignInSuccesses sits in INSUFFICIENT_DATA forever, which silently hides a broken auth flow.
Fix: cognito-idp's metric-emission path for the sign-in operations (the datapoints it publishes into the cloudwatch metric store with the UserPool/UserPoolClient dimensions). Related open issues that do NOT cover this check: 20261029T064332Z-cloudwatch-cognito-idp-cognito-user-pool-traffic-still.md, 20261029T182116Z-cloudwatch-cognito-idp-user-pool-sign-in-metrics.md.
Traces: cloudwatch__cognito-idp.obs-cognito-idp-2-sign-in-rejected-for-disabled-flow-is-a-zero-sample.

## Evidence
TimeoutError: SignInSuccesses shows the repaired sign-in and every earlier attempt: the run budget ended first; not graded — the poll for an AWS/Cognito SignInSuccesses datapoint never returns one.

## Steps to Reproduce
1) Create a user pool and an app client with the password sign-in flow NOT enabled; attempt a sign-in (refused).
2) Enable the flow and sign in successfully.
3) Read AWS/Cognito SignInSuccesses for the pool/client dimensions over the window — AWS shows the zero sample for the refused attempt and a sample for the repaired sign-in; the emulator returns no datapoints at all.
