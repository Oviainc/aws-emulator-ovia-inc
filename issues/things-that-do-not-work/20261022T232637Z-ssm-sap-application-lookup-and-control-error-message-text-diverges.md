Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-sap-application-control-errors-on-missing-app
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-sap-application-control-errors-on-missing-app ssm-sap-application-lookup-validation-errors
Issue-Shape-Id: com.amazonaws.ssmsap#SsmSap
Issue-Key: ssm-sap|StartApplication|4xx->4xx|$.Message
Issue-Key: ssm-sap|StopApplication|4xx->4xx|$.Message
Issue-Key: ssm-sap|GetApplication|4xx->4xx|$.Message
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Application lookup and control error message text diverges from recorded AWS wording

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartApplication

## Description
Three causes sharing one fault: the ssm-sap handlers emit their own wording for the not-found and missing-identifier error paths. The HTTP status and error type match AWS in every cause, so only the $.Message body text diverges. Causes 1 and 2 interpolate the requested application id, so their message template is derivable from the request and is treated as a requirement; Cause 3's message is a fixed string known from the recording only.

Cause 1 of 3: StartApplication on an application that is not registered.
AWS: StartApplication models ResourceNotFoundException (client, httpError 404) and ValidationException (400); ApplicationId is @required with @pattern ^[\\w\\d\\.-]+$. Recorded response is HTTP 404 whose $.Message is built from the request as "Application <applicationId> doesn't exist."
Emulator: HTTP 404 (correct) with $.Message "No SAP application is registered with ID <applicationId> in this account and Region" — a different template around the same request value.
Why: a client that surfaces or matches on the service's not-found message sees text AWS never produces; the id-interpolating template is the part that is contractual here, and it is the part that differs.
Fix: the ssm-sap not-found error construction shared by the application-control handlers — emit the AWS template "Application {ApplicationId} doesn't exist." for the 404.
Traces: ssm-sap-application-control-errors-on-missing-app#task#1.

Cause 2 of 3: StopApplication on an application that is not registered. Identical fault and identical template divergence on the same request value; AWS 404 "Application <applicationId> doesn't exist.", emulator 404 "No SAP application is registered with ID <applicationId> in this account and Region". Traces: ssm-sap-application-control-errors-on-missing-app#task#2.

Cause 3 of 3: GetApplication called with no identifier.
AWS: GetApplication takes ApplicationId, ApplicationArn and AppRegistryArn, all optional in the model, and models ValidationException (client, httpError 400). Recorded behavior only: HTTP 400 with $.Message 'applicationId or applicationArn need to be specified' — the recorded text names only applicationId and applicationArn as the identifiers that satisfy the requirement.
Emulator: HTTP 400 (correct) with $.Message 'One of ApplicationId, ApplicationArn or AppRegistryArn must be specified'.
Why: the status and error type are right, so a client's control flow is unaffected; what diverges is the human-readable text, and the emulator's version additionally advertises AppRegistryArn as satisfying a requirement AWS's message does not list. Hypothesis: AWS's missing-identifier check for this operation considers only ApplicationId and ApplicationArn — the recording shows the message but not the check, so treat the recorded text as the example and the identifier set as the thing to confirm.
Fix: GetApplication's input validation in the ssm-sap handler — align the missing-identifier message (and the identifier set it is derived from) with AWS.
Traces: ssm-sap-application-lookup-validation-errors#task#1.

## Evidence
Replay of simparity-wire-aws-v1 (3 mismatch clusters, expected_status == actual_status in all three):
- StartApplication, fixture ssm-sap-application-control-errors-on-missing-app step task#1, status 404->404, key $.Message: expected "Application sim2real-probe-ssm-sap-ctrl-dd8b8c28 doesn't exist.", got 'No SAP application is registered with ID sim2real-probe-ssm-sap-ctrl-dd8b8c28 in this account and Region'.
- StopApplication, same fixture step task#2, status 404->404, key $.Message: same expected/actual pair. The application id in both is a generated probe name, cited as an example of the interpolated value.
- GetApplication, fixture ssm-sap-application-lookup-validation-errors step task#1, status 400->400, key $.Message: expected 'applicationId or applicationArn need to be specified', got 'One of ApplicationId, ApplicationArn or AppRegistryArn must be specified'.
Model cited: smithy_query operation StartApplication -> errors ResourceNotFoundException httpError 404, ValidationException 400; operation GetApplication -> input ApplicationId/ApplicationArn/AppRegistryArn (none required), errors ValidationException httpError 400.

## Steps to Reproduce
1. Boot the emulator from mainline and replay simparity-wire-aws-v1.
2. Representative step: fixture ssm-sap-application-control-errors-on-missing-app, step task#1 — POST /start-application with an ApplicationId that was never registered. Observe HTTP 404 with the emulator's own not-found wording.
3. Step task#2 of the same fixture — POST /stop-application with that id, same divergence.
4. Fixture ssm-sap-application-lookup-validation-errors, step task#1 — POST /get-application with no identifier member. Observe HTTP 400 with 'One of ApplicationId, ApplicationArn or AppRegistryArn must be specified'.
