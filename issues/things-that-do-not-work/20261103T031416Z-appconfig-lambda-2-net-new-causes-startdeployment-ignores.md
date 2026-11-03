Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__lambda.validator-lambda-rejects-noncompliant-content-rejected appconfig__lambda.extension-action-point-invokes-lambda-during-deployment
Issue-Key: appconfig__lambda.validator-lambda-rejects-noncompliant-content-rejected|_error_of#41|AssertionError|raise AssertionError(
Issue-Key: appconfig__lambda.extension-action-point-invokes-lambda-during-deployment|_drain_one#46|Timeout|msgs = sqs.receive_message(
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# appconfig__lambda: 2 net-new causes — StartDeployment ignores a LAMBDA validator and deploys content AWS refuses, and an extension action point never invokes its Lambda (check hangs)

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
Two distinct root causes under the suite heading `appconfig__lambda`. A fixer must close both before submitting.

Cause 1 of 2: StartDeployment does not invoke the configuration profile's LAMBDA validator, so a deployment of content the validator rejects succeeds instead of being refused.
AWS: a configuration profile created with `Validators=[{"Type": "LAMBDA", "Content": <function-arn>}]` has that function invoked synchronously by StartDeployment; when the validator handler raises, AppConfig refuses the deployment with HTTP 400 `BadRequestException` and relays the Lambda error payload as the message, of the form `Function Error: Unhandled, Log Result: null, Payload: {"errorMessage": "<the handler's own message>", "errorType": "<the handler's exception class>", "requestId": "<per-invocation id>", "stackTrace": ["<frames>"]}` — the fixed text is `Function Error: Unhandled, Log Result: null, Payload: ` and the payload's keys; the errorMessage/errorType/stackTrace come from the validator, and requestId is a per-run value. Smithy: appconfig StartDeployment models BadRequestException (400) among its errors; the validator contract is AppConfig's documented Validators behaviour.
Emulator: StartDeployment of the non-compliant version SUCCEEDS. Replay detail: `AssertionError: the call under test SUCCEEDED where real AWS refuses it — this is an over-permissiveness failure, not an error-code mismatch`. Consequently the rejected bytes are also served by the data plane and a deployment record is created where AWS creates none.
Why: AppConfig's only customer-code safety gate is inert. A client that relies on a validator to stop bad configuration from reaching its fleet gets it deployed and served, and sees no error to branch on.
Fix: appconfig's StartDeployment handler — validator dispatch (LAMBDA type) plus the BadRequestException mapping and the "no deployment record, nothing served" effect-absence on refusal.
Traces: appconfig__lambda.validator-lambda-rejects-noncompliant-content-rejected

Cause 2 of 2: a Lambda-backed extension action point never invokes its function, so the check hangs to the 60 s harness cap.
AWS: an extension declaring an action with a `Uri` naming a Lambda and a `RoleArn`, associated to a resource, has that Lambda invoked at the action point during the deployment lifecycle. Recorded behavior only: the recording shows the observation side-channel never receiving the event and the check making no progress.
Emulator: `Error: Check timed out after 60.0s` — no invocation, and no error either.
Why: a client wiring extension action points observes neither the action's effect nor a refusal; a hang leaves no error path.
Fix: appconfig's extension action-point dispatcher (the deployment lifecycle hook that resolves an action's Uri and invokes it under RoleArn). Related open issues `20261027T010737Z-appconfig-lambda-backed-extension-action-points-still-never.md` and `20261101T232607Z-appconfig-sqs-appconfig-s-aws-authored-deployment-event-notification-extensions.md` describe neighbouring extension-dispatch gaps but do not carry this check's replay key.
Traces: appconfig__lambda.extension-action-point-invokes-lambda-during-deployment

## Evidence
Cause 1 — expected: `StartDeployment` with a version the LAMBDA validator rejects -> HTTP 400 `BadRequestException`, message `Function Error: Unhandled, Log Result: null, Payload: {"errorMessage": "<validator message>", "errorType": "<validator exception class>", "requestId": "<id>", "stackTrace": [...]}`; no deployment record created and the previous version still served. Actual: StartDeployment returns success; the non-compliant version deploys and is served.
Cause 2 — expected: the PRE_* / deployment-lifecycle action point invokes the associated Lambda. Actual: nothing is invoked; replay reports `Error: Check timed out after 60.0s`.

## Steps to Reproduce
1. Replay the behavioral suite and re-run the two checks in trace_ids.
2. Cause 1: create an AppConfig application, environment and a configuration profile with `Validators=[{Type:LAMBDA, Content:<fn-arn>}]` where the function raises for non-compliant JSON; create a hosted configuration version with non-compliant content (which AWS stores) and call StartDeployment for it. Observe success instead of BadRequestException.
3. Cause 2: create an extension with a `PRE_CREATE_HOSTED_CONFIGURATION_VERSION` action naming a Lambda Uri + RoleArn, associate it to the configuration profile ARN, create a hosted configuration version, and watch for the Lambda's side effect. Observe no invocation and a 60 s timeout.
