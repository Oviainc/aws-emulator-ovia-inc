Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-sap-register-application-missing-instance
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-sap-register-application-missing-instance
Issue-Shape-Id: com.amazonaws.ssmsap#SsmSap
Issue-Key: ssm-sap|RegisterApplication|4xx->2xx|$.Message
Issue-Key: ssm-sap|RegisterApplication|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: ssm-sap|RegisterApplication|4xx->2xx|status
Issue-Key: ssm-sap|GetApplication|4xx->2xx|$.Message
Issue-Key: ssm-sap|GetApplication|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: ssm-sap|GetApplication|4xx->2xx|status
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# RegisterApplication accepts HANA application with no SID, instance number or credentials

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterApplication

## Description
Two causes in one fixture: the emulator skips RegisterApplication's conditional requirement for HANA applications, so a registration AWS rejects succeeds, and the bogus application is then retrievable.

Cause 1 of 2: RegisterApplication does not enforce the HANA SID/instance-number/credentials requirement.
AWS: RegisterApplication models ApplicationId, ApplicationType and Instances as @required and ValidationException (client, httpError 400); Sid, SapInstanceNumber and Credentials are conditionally required by the service when ApplicationType is HANA — recorded behavior: the request returns HTTP 400 with an x-amzn-errortype error header and $.Message "SID, instance number, and credentials of HANA instance must be specified for SAP applications with type HANA." (message shown as an example; the requirement is the 400 and the modeled error type).
Emulator: returns HTTP 200 with a success body — no $.Message and no x-amzn-errortype header — i.e. it registers the application.
Why: a client that relies on AWS rejecting an under-specified HANA registration gets a silent success and a half-formed application in its inventory; error-path tests and retry/validation logic built on the 400 never fire, and subsequent operations run against state AWS would never have created.
Fix: RegisterApplication's input validation in the ssm-sap handler — add the conditional check that ApplicationType==HANA requires Sid, SapInstanceNumber and Credentials, failing with the modeled ValidationException at HTTP 400 before any state is written.
Traces: ssm-sap-register-application-missing-instance#task#1.

Cause 2 of 2: GetApplication returns 200 for an application that was never validly registered.
AWS: GetApplication models only InternalServerException (500) and ValidationException (client, httpError 400); recorded behavior for an application id that is not registered in the account is HTTP 400 with $.Message "No registered application found for account: <account>, and application: <applicationId>" and an x-amzn-errortype header — a 400 ValidationException, not a 404.
Emulator: returns HTTP 200 with an Application body for that same id, with no error header.
Why: lookups of unregistered applications appear to succeed, so a client cannot distinguish a registered application from an absent one. Note this step runs after Cause 1 in the same fixture, so part of the 200 is downstream of the bad registration; the lookup path must still answer 400 for an id with no registered application, and the step must be re-replayed after Cause 1 is fixed to confirm it.
Fix: GetApplication's lookup in the ssm-sap handler/state — an unknown application id must return the modeled ValidationException at HTTP 400 with the x-amzn-errortype header set, instead of synthesising a 200 body.
Traces: ssm-sap-register-application-missing-instance#task#2.

## Evidence
Replay of simparity-wire-aws-v1, fixture ssm-sap-register-application-missing-instance (both clusters verdict=mismatch):
- step task#1, divergence_key service=ssm-sap|RegisterApplication|status=400->200|keys=$.Message,$headers.x-amzn-errortype,status — expected 400, got 200; $.Message missing (expected 'SID, instance number, and credentials of HANA instance must be specified for SAP applications with type HANA.', got None); $headers.x-amzn-errortype missing.
- step task#2, divergence_key service=ssm-sap|GetApplication|status=400->200|keys=$.Message,$headers.x-amzn-errortype,status — expected 400, got 200; $.Message missing (expected 'No registered application found for account: 000000000000, and application: sim2real-probe-ssm-sap-app-8d37e33f', got None); $headers.x-amzn-errortype missing. Account id and application id in that message are environment values, cited only as examples.
Model cited: smithy_query operation RegisterApplication -> ApplicationId/ApplicationType/Instances required, Sid pattern ^[A-Z][A-Z0-9]{2}$, SapInstanceNumber pattern ^[0-9]{2}$, Credentials list, errors ValidationException httpError 400 / ConflictException 409 / ResourceNotFoundException 404; operation GetApplication -> errors ValidationException httpError 400, InternalServerException 500 (no ResourceNotFoundException).
state_reset_failures for the run was 0, so these results are not contaminated by a previous fixture's state.

## Steps to Reproduce
1. Boot the emulator from mainline and replay simparity-wire-aws-v1.
2. Fixture ssm-sap-register-application-missing-instance, step task#1 — POST /register-application with ApplicationType=HANA and no Sid, SapInstanceNumber or Credentials. Observe HTTP 200 instead of the recorded 400 with an x-amzn-errortype header.
3. Step task#2 — POST /get-application for that same application id. Observe HTTP 200 with an Application body instead of the recorded 400 error response.
4. After fixing, re-replay both steps of the fixture.
