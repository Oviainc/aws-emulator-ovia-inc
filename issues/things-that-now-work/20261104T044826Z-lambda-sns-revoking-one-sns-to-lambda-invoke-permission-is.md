Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns.snstrig-revoked-invoke-permission-stops-delivery-while-the-granted-twin-still-fires
Issue-Key: lambda__sns.snstrig-revoked-invoke-permission-stops-delivery-while-the-granted-twin-still-fires|_poll#9|Timeout|time.sleep(interval_s)
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# Revoking one SNS-to-Lambda invoke permission is unobservable: neither the revoked nor the granted twin delivers

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: with two functions subscribed to one SNS topic, removing the sns.amazonaws.com invoke permission from one must stop its deliveries while the other keeps firing.
AWS: recorded behavior only — the recording subscribes both functions, grants lambda:InvokeFunction to sns.amazonaws.com for each, publishes to prove both fire, calls lambda:RemovePermission on one, publishes again, and then reads each function's /aws/lambda/<name> log group to see one marker arrive and the other not; on AWS this settles inside the suite's 60s budget.
Emulator: the check is cut off with "Error: Check timed out after 60.0s" — Subscribe/AddPermission/Publish are accepted but neither function is ever invoked, so the positive control never fires and the revocation has nothing to suppress.
Why: a client cannot verify that revoking a resource-policy statement actually stops SNS deliveries; a dead delivery path and a correctly enforced denial look the same, so an over-broad or missing grant cannot be detected.
Fix: the SNS Publish delivery path to Lambda subscriptions and the resource-policy evaluation it must perform per subscription.
Traces: lambda__sns.snstrig-revoked-invoke-permission-stops-delivery-while-the-granted-twin-still-fires.

## Evidence
Expected (AWS): after RemovePermission on one function, a Publish invokes only the still-granted function.
Actual (emulator): 'Error: Check timed out after 60.0s' — neither function is ever invoked.

## Steps to Reproduce
1. CreateTopic; Subscribe two Lambdas; AddPermission for sns.amazonaws.com on both.
2. Publish and poll both functions' log groups for markers. Observe neither is invoked.
3. RemovePermission on one, Publish again, and poll. Observe the scenario never settles within 60s.
