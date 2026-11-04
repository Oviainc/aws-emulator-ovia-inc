Issue-Source: trace-sweep-divergence
Issue-Trace: networkflowmonitor-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: networkflowmonitor-error-paths
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor|GetMonitor|4xx->4xx|$.message
Issue-Key: networkflowmonitor|DeleteMonitor|4xx->4xx|$.message
Issue-Key: networkflowmonitor|GetScope|4xx->4xx|$.message
Issue-Key: networkflowmonitor|CreateMonitor|4xx->4xx|$.message
Issue-Commit: e8571d025aeac7270f8ce9c804fecdb89f8ea73b

# networkflowmonitor error messages omit AWS's service/status/request-id envelope suffix (4 causes)

> _Note: this issue was identified against the code base at commit `e8571d025aeac7270f8ce9c804fecdb89f8ea73b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMonitor

## Description
Four net-new wire divergences in the networkflowmonitor emulator, all four from fixture networkflowmonitor-error-paths. They share ONE fault: on every modelled client-error path the emulator serves the bare refusal sentence as the error structure's `message` member and stops there, where NetworkFlowMonitor's recorded `message` continues with the exception envelope that names the service, the status code and the id of the request being refused. HTTP status and error shape are correct in all four; only the `message` text diverges. A fixer must close all FOUR causes below before submitting.

Neither open networkflowmonitor issue covers this: 20261029T205920Z-networkflowmonitor-two-causes-the-one-scope-per-account.md is about the one-scope-per-account/Region quota not being enforced by CreateScope, and 20261103T191055Z-networkflowmonitor-deletescope-deletes-a-scope-that-a-monitor.md is about DeleteScope lacking a referencing-monitor guard. Neither names the error-message envelope, GetMonitor, DeleteMonitor, GetScope or CreateMonitor. The same family of defect was filed and accepted for another service as 20261102T095923Z-controlcatalog-listcommoncontrols-the-invalid-filter-validationexception-message-omits-aws.md (ListCommonControls dropping the same envelope), so this is the networkflowmonitor instance of that class, not a duplicate of it.

The envelope's SHAPE, which is the requirement shared by all four causes:
  Fixed text (must be reproduced literally): a single space after the refusal sentence, then the literal "(Service: ", the literal ", Status Code: ", the literal ", Request ID: ", a ")", a space, and the trailing literal "(SDK Attempt Count: " ... ")".
  Variable parts, taken from the request/response being served and NEVER copied literally from a recording: the service name as this API reports it (recorded as NetworkFlowMonitor), the numeric HTTP status of the refusal being served (404 / 400 / 403 for the causes below), the request id of the request being refused (one id per request, never a value from a recorded run), and the attempt count.
So the general template is: "<refusal sentence> (Service: NetworkFlowMonitor, Status Code: <this response's status>, Request ID: <this request's id>) (SDK Attempt Count: <n>)". The recorded ids (a730f233-aee7-404a-b5e4-72633ce93312, 9380bb0a-4f46-4977-89fc-6ca6bad8d628, e3eb05d4-6c80-47ce-b279-93ffed116e9e, 896402a1-214b-4ee8-8f5a-9c2b8b006834) and the recorded monitor/scope names fill the template; they are not part of it.

Cause 1 of 4: GetMonitor on an unknown monitor returns the bare not-found sentence (fixture networkflowmonitor-error-paths, step task#1).
  AWS: GetMonitor is GET /monitors/{monitorName} and declares ResourceNotFoundException (Smithy: com.amazonaws.networkflowmonitor#ResourceNotFoundException, errorKind client, @httpError 404, doc "The request specifies a resource that doesn't exist."; its only member is `message` -> String). Recorded behavior only for the wording: HTTP 404 with message = "Monitor [<monitor arn>] doesn't exist" followed by the envelope above. Fixed text: "Monitor [" ... "] doesn't exist" plus the envelope literals; variable: the monitor ARN built from the requested monitorName, plus the envelope's status (404) and request id.
  Emulator: HTTP 404 and ResourceNotFoundException are right, but message is exactly "Monitor [arn:aws:networkflowmonitor:us-west-2:000000000000:monitor/<name>] doesn't exist" — the string ends at "exist" and the envelope is absent entirely.
  Why: a client that reads the message to recover the request id for a support case, or that matches AWS's full message text, gets nothing to parse and no correlation id; code that works against AWS silently finds no match against the emulator.
  Fix: the GetMonitor handler's not-found refusal path / this service's shared error serializer, wherever the ResourceNotFoundException `message` is assembled — the envelope is per-request (it carries the id of the request being served) so it cannot be a compile-time constant and must be appended where the error response is built.
  Traces: networkflowmonitor-error-paths#task#1.

Cause 2 of 4: DeleteMonitor on an unknown monitor returns the bare not-found sentence (fixture networkflowmonitor-error-paths, step task#2).
  AWS: DeleteMonitor is DELETE /monitors/{monitorName} and declares the same ResourceNotFoundException (Smithy: @httpError 404, member `message`). Recorded behavior only: HTTP 404, message = "Monitor [<monitor arn>] doesn't exist" + envelope, with the envelope's status 404 and this request's own id.
  Emulator: HTTP 404 and the error shape are right; message stops at "Monitor [...] doesn't exist" with no envelope.
  Why: same as cause 1 — the delete path is the one most likely to be retried or escalated, and the refusal carries no correlatable request id in its message.
  Fix: the DeleteMonitor handler's not-found refusal path, through the same shared error-message assembly as cause 1; fixing the shared assembly should cover both monitor causes at once.
  Traces: networkflowmonitor-error-paths#task#2.

Cause 3 of 4: GetScope with a malformed scope identifier returns the bare invalid-ARN sentence (fixture networkflowmonitor-error-paths, step task#3).
  AWS: GetScope is GET /scopes/{scopeId} with scopeId @required @length(min 1), and declares ValidationException (Smithy: com.amazonaws.networkflowmonitor#ValidationException, errorKind client, @httpError 400, doc "Invalid request."; member `message`). Recorded behavior only for the wording: HTTP 400, message = "The provided Scope ARN is not valid" followed by the envelope, with status 400 and this request's id.
  Emulator: HTTP 400 and ValidationException are right; message is exactly "The provided Scope ARN is not valid" with no envelope. Here the entire refusal sentence is fixed text — nothing in it comes from the request — so the ONLY divergence is the missing envelope.
  Why: a validation refusal is what a client surfaces to a user or logs; without the service/status/request-id envelope the log line cannot be joined back to the request, and a text assertion on AWS's message fails.
  Fix: the GetScope input-validation refusal path and the same ValidationException message assembly.
  Traces: networkflowmonitor-error-paths#task#3.

Cause 4 of 4: CreateMonitor against a non-existent scope returns the bare access-denied sentence (fixture networkflowmonitor-error-paths, step task#4).
  AWS: CreateMonitor is POST /monitors with scopeArn @required, and declares AccessDeniedException (Smithy: com.amazonaws.networkflowmonitor#AccessDeniedException, errorKind client, @httpError 403, doc "You don't have sufficient permission to perform this action."; member `message`). Recorded behavior only: HTTP 403 and message = "scope supplied doesn't exist" followed by the envelope, with status 403 and this request's id. (Note the recorded status for an unresolvable scopeArn is 403 AccessDeniedException, not 404 — the emulator already matches that part.)
  Emulator: HTTP 403 and AccessDeniedException are right; message is exactly "scope supplied doesn't exist" with no envelope. As in cause 3 the whole sentence is fixed text, so the envelope is the sole delta.
  Why: same as above — the only machine-readable correlation AWS offers on this refusal lives in the message, and the emulator drops it.
  Fix: the CreateMonitor scope-resolution refusal path and the same AccessDeniedException message assembly. Because all four causes are the same missing suffix, the right fix is one place in the service's error-response construction (every modelled client error of this service), not four independent string edits.
  Traces: networkflowmonitor-error-paths#task#4.

## Evidence
Replay of trace_set simparity-wire-aws-v1 against the emulator built from this checkout: 14 steps, 10 passed, 4 mismatched (score 71.4%), emulator_up=true, bootstrap_signal=false, not_implemented=0. All 4 mismatches are in fixture networkflowmonitor-error-paths and all 4 are keyed on $.message only (keys=$.message in every divergence_key); expected_status == actual_status in every one.

Cause 1 — networkflowmonitor-error-paths#task#1, GetMonitor, status 404 -> 404, divergence_key "service=networkflowmonitor|GetMonitor|status=404->404|keys=$.message".
  Expected $.message: "Monitor [arn:aws:networkflowmonitor:us-west-2:000000000000:monitor/sim2real-probe-networkflowmonitor-missing-monitor-d303928e] doesn't exist (Service: NetworkFlowMonitor, Status Code: 404, Request ID: a730f233-aee7-404a-b5e4-72633ce93312) (SDK Attempt Count: 1)"
  Actual   $.message: "Monitor [arn:aws:networkflowmonitor:us-west-2:000000000000:monitor/sim2real-probe-networkflowmonitor-missing-monitor-d303928e] doesn't exist"

Cause 2 — networkflowmonitor-error-paths#task#2, DeleteMonitor, status 404 -> 404, divergence_key "service=networkflowmonitor|DeleteMonitor|status=404->404|keys=$.message".
  Expected $.message: "Monitor [arn:aws:networkflowmonitor:us-west-2:000000000000:monitor/sim2real-probe-networkflowmonitor-missing-monitor-d303928e] doesn't exist (Service: NetworkFlowMonitor, Status Code: 404, Request ID: 9380bb0a-4f46-4977-89fc-6ca6bad8d628) (SDK Attempt Count: 1)"
  Actual   $.message: "Monitor [arn:aws:networkflowmonitor:us-west-2:000000000000:monitor/sim2real-probe-networkflowmonitor-missing-monitor-d303928e] doesn't exist"

Cause 3 — networkflowmonitor-error-paths#task#3, GetScope, status 400 -> 400, divergence_key "service=networkflowmonitor|GetScope|status=400->400|keys=$.message".
  Expected $.message: "The provided Scope ARN is not valid (Service: NetworkFlowMonitor, Status Code: 400, Request ID: e3eb05d4-6c80-47ce-b279-93ffed116e9e) (SDK Attempt Count: 1)"
  Actual   $.message: "The provided Scope ARN is not valid"

Cause 4 — networkflowmonitor-error-paths#task#4, CreateMonitor, status 403 -> 403, divergence_key "service=networkflowmonitor|CreateMonitor|status=403->403|keys=$.message".
  Expected $.message: "scope supplied doesn't exist (Service: NetworkFlowMonitor, Status Code: 403, Request ID: 896402a1-214b-4ee8-8f5a-9c2b8b006834) (SDK Attempt Count: 1)"
  Actual   $.message: "scope supplied doesn't exist"

In all four the expected value is the emulator's string plus the envelope " (Service: NetworkFlowMonitor, Status Code: <status>, Request ID: <id>) (SDK Attempt Count: <n>)". The fixed literals of that envelope ("(Service: ", ", Status Code: ", ", Request ID: ", "(SDK Attempt Count: ") and the service name are absent from the emulator's output, so the gap is not merely a differing request id: the emulator emits no envelope at all. The request ids and monitor/scope names quoted above are per-run fill-ins for the template, not required values.

## Steps to Reproduce
1. Clone this repo and boot the emulator built from it (data listener answered on :46603; /__admin/health returned 200).
2. Replay trace set simparity-wire-aws-v1, fixture networkflowmonitor-error-paths (4 task steps). Equivalently, by hand against the emulator's networkflowmonitor endpoint:
   a. GET /monitors/sim2real-probe-networkflowmonitor-missing-monitor-<suffix> for a monitor that was never created -> HTTP 404 ResourceNotFoundException. Observe $.message ends at "doesn't exist" instead of continuing " (Service: NetworkFlowMonitor, Status Code: 404, Request ID: <this request's id>) (SDK Attempt Count: 1)". [step task#1]
   b. DELETE /monitors/<that same unknown monitor name> -> HTTP 404 ResourceNotFoundException, same truncated message. [step task#2]
   c. GET /scopes/<a malformed scope identifier> -> HTTP 400 ValidationException with $.message exactly "The provided Scope ARN is not valid", no envelope. [step task#3]
   d. POST /monitors with a valid monitorName/localResources and a scopeArn that resolves to no scope -> HTTP 403 AccessDeniedException with $.message exactly "scope supplied doesn't exist", no envelope. [step task#4]
3. In each case the status code and error shape match the recording; only the message envelope is missing. Re-replay networkflowmonitor-error-paths steps task#1, task#2, task#3 and task#4 to confirm a fix — all four must pass.
