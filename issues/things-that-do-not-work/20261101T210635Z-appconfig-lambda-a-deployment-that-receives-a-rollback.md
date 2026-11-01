Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__lambda.at-deployment-tick-directive-rollback-reverts-the-deployment
Issue-Key: appconfig__lambda.at-deployment-tick-directive-rollback-reverts-the-deployment|_wait_terminal#43|Timeout|return last
Issue-Commit: 62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3

# appconfig__lambda: a deployment that receives a rollback directive at a growth tick never reverts, so the rollback lifecycle never reaches a terminal state

> _Note: this issue was identified against the code base at commit `62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopDeployment

## Description
Cause 1 of 1: an in-flight AppConfig deployment told to roll back at a growth tick never converges on the reverted state.

AWS: recorded behavior only (the Smithy model carries the Deployment shape and its State enum but no tick/rollback timing semantics). AWS behaviour exercised by the check: while a deployment is DEPLOYING, a rollback directive moves it through ROLLING_BACK to ROLLED_BACK, the growth stops advancing PercentageComplete, and the hosted configuration served to the application reverts to the previously deployed version — GetDeployment/GetLatestConfiguration reflect the reverted content once the deployment reaches its terminal rolled-back state.

Emulator: the check never completes — `Error: Check timed out after 60.0s`. The deployment never reaches the terminal rolled-back state (or the reverted content is never served), so the poller that waits for the rollback verdict blocks until the harness budget expires.

Why: a caller that rolls back a bad configuration has no observable completion: the deployment state never settles, so deployment pipelines and the application's configuration reads both hang on a state that never arrives, and a rolled-back configuration can never be confirmed as reverted.

Fix: the AppConfig deployment state machine (growth-tick advancement plus the rollback transition to ROLLING_BACK/ROLLED_BACK) and the configuration-serving path that must revert to the previous version at that transition.

Traces: appconfig__lambda.at-deployment-tick-directive-rollback-reverts-the-deployment.

## Evidence
Replay verdict [mismatch] for appconfig__lambda.at-deployment-tick-directive-rollback-reverts-the-deployment; detail: `Error: Check timed out after 60.0s`. Expected: deployment reaches ROLLED_BACK and the served configuration is the previous version. Actual: no terminal state within 60s.

## Steps to Reproduce
1. Create an AppConfig application, environment, configuration profile and two hosted configuration versions.
2. StartDeployment of version 2 with a deployment strategy that grows in ticks.
3. While the deployment is DEPLOYING, issue the rollback directive (StopDeployment with rollback).
4. Poll GetDeployment: AWS moves it to ROLLING_BACK then ROLLED_BACK and the served configuration reverts to version 1.
5. Against the emulator the poll never observes the rolled-back terminal state; the check dies at the 60s budget.
