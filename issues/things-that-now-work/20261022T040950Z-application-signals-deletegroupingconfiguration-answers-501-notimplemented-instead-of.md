Issue-Source: trace-sweep-divergence
Issue-Trace: application-signals-grouping-configuration
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-signals-grouping-configuration
Issue-Shape-Id: com.amazonaws.applicationsignals#ApplicationSignals
Issue-Key: application-signals|DeleteGroupingConfiguration|2xx->5xx|$
Issue-Key: application-signals|DeleteGroupingConfiguration|2xx->5xx|status
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# application-signals DeleteGroupingConfiguration answers 501 NotImplemented instead of 200 with an empty body

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteGroupingConfiguration

## Description
One cause: the grouping-configuration delete operation is not served at all.
AWS: Smithy DeleteGroupingConfiguration is bound as DELETE /grouping-configuration with an empty input (Unit), an empty output structure (DeleteGroupingConfigurationOutput) and success code 200; it is marked @idempotent, and its only modeled errors are AccessDeniedException (403), ThrottlingException (429) and ValidationException (400). The recorded response is HTTP 200 with an empty JSON body {}.
Emulator: HTTP 501 with body {"__type": "NotImplemented", "message": "operation DELETE /grouping-configuration is not implemented in this emulator"} — a status and error type the model does not declare for this operation.
Why: 501 NotImplemented is outside the operation's modeled error set, so SDK clients surface it as an unmodeled/unretryable transport failure rather than any Application Signals error. The grouping configuration cannot be removed, so any teardown or reconfigure flow that deletes it fails, and because the operation is @idempotent callers legitimately expect a bare 200 even when nothing is configured.
Fix: implement the DeleteGroupingConfiguration handler in the application-signals service — clear the account/region grouping-configuration state and return 200 with the empty DeleteGroupingConfigurationOutput, idempotently when no configuration exists.
Traces: application-signals-grouping-configuration#task#3.
Note: this failure is reported with no stub_path and is not owned by any open issue (no application-signals issue exists in the open set), so it is filed here rather than covered by an auto-filed scaffold-stub issue.

## Evidence
application-signals-grouping-configuration#task#3 DeleteGroupingConfiguration: status expected 200, got 501; body $ value_mismatch — expected {}, got {'__type': 'NotImplemented', 'message': 'operation DELETE /grouping-configuration is not implemented in this emulator'}.
Smithy citation: DeleteGroupingConfiguration — http {method: DELETE, uri: /grouping-configuration, code: 200}, input Unit, output DeleteGroupingConfigurationOutput (empty), @idempotent, errors AccessDeniedException(403)/ThrottlingException(429)/ValidationException(400); NotImplemented/501 is not among them.

## Steps to Reproduce
1. Replay fixture application-signals-grouping-configuration, step task#3: send DELETE /grouping-configuration (empty body) to the emulator.
2. Observe HTTP 501 with __type NotImplemented instead of HTTP 200 with an empty JSON body.
