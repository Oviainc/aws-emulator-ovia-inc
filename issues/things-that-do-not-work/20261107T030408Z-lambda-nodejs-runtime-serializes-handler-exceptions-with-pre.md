Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-wsbe-traj-invoke-test-event-shapes-then-endpoint-fix
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-wsbe-traj-invoke-test-event-shapes-then-endpoint-fix
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|Invoke|2xx->2xx|$.trace[*]
Issue-Related: 20261105T141451Z-lambda-invoke-function-error-payloads-diverge-missing-errortype.md
Issue-Commit: a89f2b297f2a633390e709c28e627328b3f2666a

# Lambda nodejs runtime serializes handler exceptions with pre-V8 wording and emulator-internal stack frames

> _Note: this issue was identified against the code base at commit `a89f2b297f2a633390e709c28e627328b3f2666a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new cause in the lambda service.

Cause 1 of 1 - Invoke of a Node.js function whose handler throws a TypeError returns an error payload whose errorMessage uses obsolete V8 wording and whose trace is the emulator's own evaluator stack (fixture lambda-wsbe-traj-invoke-test-event-shapes-then-endpoint-fix, step task#4).
AWS: Smithy: Invoke POST /2015-03-31/functions/{FunctionName}/invocations, success code 200, output Payload (httpPayload blob) with the failure flagged by the X-Amz-Function-Error header; the payload's internal shape is produced by the runtime and is not modeled, so this is recorded behavior only. The recording shows the standard Node.js (V8 10+/nodejs18.x and later) shape: errorMessage is V8's modern property-access message "Cannot read properties of undefined (reading '<property>')", trace[0] is the error header line "<errorType>: <errorMessage>" (here "TypeError: Cannot read properties of undefined (reading 'connectionId')"), and the frames that follow are the handler's own frames in the deployed package, formatted "    at <function> (/var/task/<module>.js:<line>:<column>)". Fixed text: the V8 message template and the "<errorType>: <message>" header; variable parts: the property name taken from the failing expression, and the module path/line/column taken from the deployed code.
Emulator: errorMessage is the pre-V8-6 wording "cannot read property '<property>' of undefined" (lower-cased, different template); trace carries no "TypeError: ..." header line at all - trace[0] is already a frame "    at <anonymous> (<input>:18:26)" - the following frames name the emulator's own evaluator ("    at callHandler (eval_script:662:46)", "    at <anonymous> (eval_script:698:58)") instead of /var/task paths, and an extra trailing empty-string element is appended to the array.
Why: handler code and client-side tests routinely match on the error message text (a Node 18 upgrade is detected exactly by the "Cannot read properties of undefined" wording) and parse trace[0] for the error type and the later frames to locate the failing source line. Against this emulator the message matches nothing AWS emits, the error type is unavailable because the header line is missing, and the frames point at emulator internals and a file named "<input>" that does not exist in the deployment package, so a diagnostic flow that works on AWS yields nothing here.
Fix: the Node.js runtime shim's error serialization in the Invoke execution path - build the payload as {errorType, errorMessage, trace} with V8's current message wording, the "<errorType>: <errorMessage>" first trace element, handler frames rewritten to the /var/task module path, and no trailing empty element.
Traces: lambda-wsbe-traj-invoke-test-event-shapes-then-endpoint-fix#task#4.

No open issue owns this step (the replay reports owned_by empty). The open issue 20261106T062901Z-lambda-nodejs-runtime-8-net-new-execution-fidelity.md covers other nodejs execution-fidelity gaps (missing core modules, bundled SDK, globals) and does not carry this replay key.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout, fixture lambda-wsbe-traj-invoke-test-event-shapes-then-endpoint-fix step task#4 (Invoke, status 200 on both sides):

- $.errorMessage: expected "Cannot read properties of undefined (reading 'connectionId')", got "cannot read property 'connectionId' of undefined".
- $.trace[0]: expected "TypeError: Cannot read properties of undefined (reading 'connectionId')", got "    at <anonymous> (<input>:18:26)".
- $.trace[1]: expected "    at exports.handler (/var/task/agent-message.js:16:47)", got "    at callHandler (eval_script:662:46)".
- $.trace[2]: expected a shaped frame, got "    at <anonymous> (eval_script:698:58)".
- $.trace[3]: extra element present - expected no further element, got "".

The property name 'connectionId', the module name agent-message.js and the line/column numbers are this recording's values filling the templates; the requirement is the message template, the "<errorType>: <message>" header line, /var/task-rooted handler frames and the absence of a trailing empty element.

## Steps to Reproduce
1. Clone this branch and call replay_wire_corpus().
2. Replay fixture lambda-wsbe-traj-invoke-test-event-shapes-then-endpoint-fix, step task#4: Invoke a nodejs function with a test event that omits the field the handler dereferences, so the handler throws a TypeError on a property of undefined.
3. Inspect the 200 response payload: compare errorMessage with V8's "Cannot read properties of undefined (reading '<property>')" wording and trace with the "<errorType>: <message>" header plus /var/task-rooted handler frames.
