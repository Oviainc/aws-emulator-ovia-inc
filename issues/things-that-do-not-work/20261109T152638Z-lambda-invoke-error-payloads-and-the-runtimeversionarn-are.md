Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-wsbe-traj-invoke-test-event-shapes-then-endpoint-fix
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-wsbe-traj-invoke-test-event-shapes-then-endpoint-fix lambda-lvau-traj-key-rename-breaks-handler lambda-lsrno-traj-invoke-reproduces-timeout update-function-code-s3-source-round-trip
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|Invoke|2xx->2xx|$.errorMessage
Issue-Key: lambda|Invoke|2xx->2xx|$.stackTrace[*]
Issue-Key: lambda|UpdateFunctionCode|2xx->2xx|$.RuntimeVersionConfig.RuntimeVersionArn
Issue-Related: 20261107T030408Z-lambda-nodejs-runtime-serializes-handler-exceptions-with-pre.md
Issue-Commit: 8d55b39ab66a28bf4265bc0ae130d6078a9d6cb6

# Lambda invoke error payloads and the RuntimeVersionArn are emulator-authored strings, not AWS's

> _Note: this issue was identified against the code base at commit `8d55b39ab66a28bf4265bc0ae130d6078a9d6cb6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Shared fault: values the emulator's own runtime shim produces are returned where AWS returns text/identifiers produced by the managed runtime — an unhandled-error payload, a stack-trace frame, the task-timeout message, and the runtime-version ARN stamped on a function. Four net-new clusters, all with the recorded status.

Cause 1 of 4 — the Node.js unhandled-error payload is worded by a different engine.
AWS: recorded behavior only (the Invoke response payload is the runtime's error document, not a modelled shape; Smithy Invoke returns an opaque Payload blob with FunctionError in the `X-Amz-Function-Error` header). For a Node.js 18+/20+ handler that dereferences an undefined object, the managed runtime's V8 wording is `Cannot read properties of undefined (reading '<property>')`, errorType `TypeError`, and the first trace line is `TypeError: ` + that same message. The frame lines name the handler symbol and the file/line/column in `/var/task/<file>` — the property name comes from the code, the file and position from the deployed package.
Emulator: it answers the legacy/other-engine wording `cannot read property '<property>' of undefined` (lower-case, different clause order), the trace[0] line repeats that wrong text, the handler frame is attributed to `<anonymous>` at a different column instead of the exported handler symbol, and two extra frames of the emulator's own harness (`at callHandler (eval_script:1394:46)`, `at <anonymous> (eval_script:1430:58)`) are appended where the recording has none.
Why: the error payload is what a client sees for every handler bug. A caller (or a test, or an error-tracking integration) that matches on the message or inspects the top frame mis-identifies the error class, and the leaked `eval_script` frames disclose the emulator's harness as if it were customer code.
Fix: the Node invoke runtime's error serializer — adopt the modern V8 message wording, attribute the handler frame to the exported symbol, and stop appending harness frames to the trace array.
Traces: lambda-wsbe-traj-invoke-test-event-shapes-then-endpoint-fix#task#4.

Cause 2 of 4 — a Python stack-trace frame does not have the recorded frame shape.
AWS: recorded behavior only. The Python runtime's `stackTrace` entries are `traceback`-formatted frames: a `  File "<path>", line <n>, in <symbol>` header FOLLOWED by the source line of that frame, for frames inside the managed runtime's own standard library as well as `/var/task` ones.
Emulator: the frame it emits at that position is the header alone (`  File "/var/lang/lib/python3.12/os.py", line 709, in __getitem__\n`), which does not match the recorded frame's shape. Hypothesis: the shim formats frames itself instead of using traceback.format_exception, so the source line is dropped. The path and line number of the runtime's own os.py are environment values and not the requirement; the frame's two-part shape is.
Why: a client that parses stackTrace (every error-reporting tool does) sees frames missing their source line, so a KeyError on an environment variable cannot be attributed to the statement that raised it.
Fix: the Python invoke runtime's error serializer — format stackTrace with the standard traceback formatting so each frame carries its source line.
Traces: lambda-lvau-traj-key-rename-breaks-handler#task#7.

Cause 3 of 4 — the task-timeout errorMessage drops AWS's framing.
AWS: recorded behavior only. When an invocation exceeds the function's Timeout, the errorMessage is `RequestId: <requestId> Error: Task timed out after <N>.<NN> seconds` — the fixed words `RequestId: `, ` Error: Task timed out after ` and ` seconds`, with the invocation's request id and the function's Timeout filling the variable slots. (The same expectation is recorded in the behavioral suite and is described in open issue 20261107T212447Z-lambda-6-net-new-causes-timeout-error-text-oversize; that issue carries behavioral check ids, not this wire cluster's replay key, so the wire cluster is filed here.)
Emulator: it emits `<requestId> Task timed out after 3.00 seconds` — the `RequestId: ` prefix and the ` Error:` separator are missing, so the message does not have the recorded shape.
Why: the timeout message is the standard way to separate timeouts from other unhandled errors in logs, alarms and client retry logic; a client matching AWS's framing never classifies a timeout.
Fix: the invoke timeout path's error-payload serializer — the errorMessage template.
Traces: lambda-lsrno-traj-invoke-reproduces-timeout#task#5.

Cause 4 of 4 — UpdateFunctionCode returns a short, non-AWS RuntimeVersionArn.
AWS: Smithy RuntimeVersionConfig.RuntimeVersionArn is @length 26..2048 with pattern `^arn:(aws[a-zA-Z-]*):lambda:<region>::runtime:.+$`; AWS fills the suffix with the runtime build's full 64-hex-character digest, and that digest is what identifies the patched runtime a function is pinned to.
Emulator: UpdateFunctionCode's response carries a 16-hex-character suffix (`arn:aws:lambda:us-west-2::runtime:a5e88e82e3fdc7cd`) in place of the 64-character digest form. The same defect on other operations is filed as 20261102T111455Z-lambda-runtimeversionarn-carries-a-short-non-aws-identifier.md; that issue does not carry UpdateFunctionCode's replay key, so this cluster is filed here and should be fixed in the same place.
Why: a client that records or compares runtime-version ARNs across operations (to detect a runtime patch, or to pin a version) sees an identifier of the wrong form that no AWS runtime ARN can match.
Fix: the shared runtime-version projection used by the function serializers (so UpdateFunctionCode's response goes through the same corrected form as the function-configuration reads).
Traces: update-function-code-s3-source-round-trip#task#7.

## Evidence
Replay cluster details (status equals the recorded status in all four):
Cause 1 — lambda-wsbe-traj-invoke-test-event-shapes-then-endpoint-fix#task#4, Invoke 200: `$.errorMessage: value_mismatch (expected "Cannot read properties of undefined (reading 'connectionId')", got "cannot read property 'connectionId' of undefined")`; `$.trace[0]: value_mismatch (expected "TypeError: Cannot read properties of undefined (reading 'connectionId')", got "TypeError: cannot read property 'connectionId' of undefined")`; `$.trace[1]: value_mismatch (expected '    at exports.handler (/var/task/agent-message.js:16:47)', got '    at <anonymous> (/var/task/agent-message.js:16:26)')`; `$.trace[2]` -> `'    at callHandler (eval_script:1394:46)'`; `$.trace[3]: extra (got '    at <anonymous> (eval_script:1430:58)')`.
Cause 2 — lambda-lvau-traj-key-rename-breaks-handler#task#7, Invoke 200: `$.stackTrace[1]: value_mismatch (expected '<shaped-not-asserted>', got '  File "/var/lang/lib/python3.12/os.py", line 709, in __getitem__\n')`.
Cause 3 — lambda-lsrno-traj-invoke-reproduces-timeout#task#5, Invoke 200: `$.errorMessage: value_mismatch (expected '<shaped-not-asserted>', got '24f78d7a-8729-4012-9e5c-a5531265b214 Task timed out after 3.00 seconds')`; the recorded framing is `RequestId: <id> Error: Task timed out after <N>.<NN> seconds`.
Cause 4 — update-function-code-s3-source-round-trip#task#7, UpdateFunctionCode 200: `$.RuntimeVersionConfig.RuntimeVersionArn: value_mismatch (expected 'arn:aws:lambda:us-west-2::runtime:a8b19198a828662ade07d066a1dce047429a5869166346747ab7d57435583a29', got 'arn:aws:lambda:us-west-2::runtime:a5e88e82e3fdc7cd')` — the 64-hex digest is the form, not that particular digest.
Smithy: RuntimeVersionConfig.RuntimeVersionArn @length 26..2048, pattern `arn:(aws[a-zA-Z-]*):lambda:<region>::runtime:.+`; Invoke returns an opaque Payload blob with FunctionError in X-Amz-Function-Error.

## Steps to Reproduce
Cause 1: deploy a Node.js handler that reads a property of an undefined object (e.g. event.requestContext.connectionId with no requestContext) and Invoke it; read errorMessage and trace[] from the payload.
Cause 2: deploy a Python handler that reads os.environ['MISSING_KEY'] and Invoke it; read stackTrace[] — each frame should carry its source line under the File header.
Cause 3: CreateFunction with Timeout=3 and a handler that sleeps longer, then Invoke; read errorMessage (expect the `RequestId: ... Error: Task timed out after 3.00 seconds` framing).
Cause 4: UpdateFunctionCode on an existing function (S3 source) and read RuntimeVersionConfig.RuntimeVersionArn in the 200 body — the suffix is 16 hex characters, not the 64-character runtime digest form.
