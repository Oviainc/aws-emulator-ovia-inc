Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-lsrno-traj-invoke-reproduces-timeout
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-lsrno-traj-invoke-reproduces-timeout lambda-poller-handlers-invoked-directly lambda-lvau-traj-key-rename-breaks-handler lambda-sstr-traj-handler-module-mismatch-then-fix
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|Invoke|2xx->2xx|$.errorMessage
Issue-Key: lambda|Invoke|2xx->2xx|$.errorType
Issue-Key: lambda|Invoke|2xx->2xx|$.stackTrace[*]
Issue-Key: lambda|Invoke|2xx->2xx|$.requestId
Issue-Commit: 44501b87781dd7ce256ff2df137ae9a71e892337

# Lambda Invoke function-error payloads diverge: missing errorType, mis-formatted stackTrace, populated requestId

> _Note: this issue was identified against the code base at commit `44501b87781dd7ce256ff2df137ae9a71e892337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
FOUR net-new root causes, all in the document the Lambda Invoke data plane returns as its response Payload when the invocation fails (HTTP 200 with X-Amz-Function-Error set). In every case the emulator does run the handler and answers 200 — the divergence is in the shape/content of the function-error document, so none of these is a "did not execute" cause. No open issue carries any of these four replay keys. The nearest open issues own other causes and not these: 20261105T111951Z-lambda-10-net-new-root-causes-in-the.md (missing x-amzn-RequestId header, no AWS SDK, no filesystem, interpreter version, layers, extensions, concurrency, weighted aliases), 20261103T165236Z-lambda-invoke-does-not-execute-the-deployment-package.md and 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md.

Smithy note that applies to all four causes: Invoke is POST /2015-03-31/functions/{FunctionName}/invocations (restJson1), and its output member Payload targets Blob with @httpPayload. The function-error document inside that blob (errorType / errorMessage / stackTrace / requestId) is therefore NOT modeled, so each cause below is grounded in "recorded behavior only" plus the recorded status and payload.

Cause 1 of 4 — a timed-out invocation's payload omits errorType "Sandbox.Timedout".
AWS: recorded behavior only. An invocation that runs past the function's configured Timeout is answered HTTP 200 with FunctionError set, and the payload document carries BOTH "errorType": "Sandbox.Timedout" and "errorMessage". In that message the fixed text is "Task timed out after " ... " seconds"; the variable parts are the invocation's request id (prefix) and the function's configured Timeout rendered with two decimals. "Sandbox.Timedout" is fixed text — it is a class name, not a per-run value.
Emulator: HTTP 200, payload has errorMessage only ('<request id> Task timed out after 3.00 seconds'); errorType is absent (got None).
Why: SDK users and retry/alarm logic branch on errorType to tell a sandbox timeout from an unhandled handler exception; with the field absent every failure looks like the same untyped error and timeout-specific handling never fires.
Fix: the Invoke handler's function-error payload builder, on the timeout path — it must emit the errorType for the timeout class alongside the message.
Traces: lambda-lsrno-traj-invoke-reproduces-timeout#task#5. (That same step also reports errorMessage differing; its leading request id is a per-run value and is NOT part of the requirement — only the fixed text and the Timeout-derived number are.)

Cause 2 of 4 — stackTrace frames carry CPython's fine-grained error-location anchor lines.
AWS: recorded behavior only. Each element of the payload's stackTrace array is ONE traceback frame rendered as exactly two lines: '  File "<path>", line <n>, in <name>' followed by the indented source line. No third "anchor" line of ~ and ^ characters appears. Fixed text is the 'File "...", line ..., in ...' framing; the path, line number, function name and source line are values from the deployed package.
Emulator: the frame carries a third anchor line appended to the source line (e.g. the source line followed by '                     ~~~~~~~~~~^^^^^^^^^^^^^^^^^^'), so the frame string does not match the frame AWS returns.
Why: a client or log pipeline that parses stackTrace frames (or diffs them between runs/environments) sees an extra, interpreter-version-specific line that AWS never emits, and error fingerprinting on the frame text does not match production.
Fix: the Invoke handler's traceback serializer for the Python execution environment — format frames without fine-grained error-location anchors.
Traces: lambda-poller-handlers-invoked-directly#task#11.

Cause 3 of 4 — a stdlib traceback frame names the bare module instead of its interpreter file path.
AWS: recorded behavior only. When the raised exception propagates through a standard-library function, the frame for it is a real interpreter source path (an absolute path to the runtime's own module file) with its line number and function name — a file path, not a module name.
Emulator: emits '  File "os", line 709, in __getitem__' — the file field is the bare module name "os", which is not a path; the same step also shows the Cause-2 anchor line on the handler frame.
Why: anything that maps a frame's file field to a path (symbolization, grouping stdlib vs user frames, log parsers) cannot classify the frame, and the trace does not look like the one AWS returns for the same handler fault.
Fix: the Invoke handler's traceback serializer / the Python execution environment's module loader — frames must report the module's file path.
Traces: lambda-lvau-traj-key-rename-breaks-handler#task#7.

Cause 4 of 4 — an init-phase (handler import) failure payload fills requestId instead of leaving it empty.
AWS: recorded behavior only. A handler-module import failure is raised during initialization, before any invocation request id is bound to it, and the recorded payload's "requestId" is the empty string. The empty string is the fixed value for this error class, not a per-run id.
Emulator: puts a freshly generated UUID in requestId on that same import-failure payload.
Why: a client that uses requestId to decide whether a failure has an invocation context (and to correlate the payload with a request/log line) is told an init-time failure belongs to an invocation that never got one, so init vs invoke failures cannot be distinguished.
Fix: the Invoke handler's init/import-error payload serializer — do not synthesize a request id for a failure raised before the invocation is bound.
Traces: lambda-sstr-traj-handler-module-mismatch-then-fix#task#5 (also task#8 in the same fixture).

## Evidence
Cause 1 — fixture lambda-lsrno-traj-invoke-reproduces-timeout, step task#5, Invoke, status 200 expected and 200 observed:
  $.errorType: missing (expected 'Sandbox.Timedout', got None)
  $.errorMessage: expected the shaped '<request id> Task timed out after <Timeout>.00 seconds' form; observed 'da09cb8b-680e-4db2-8d26-d4d23dd75ecc Task timed out after 3.00 seconds' (the uuid is per-run and not asserted as a requirement).

Cause 2 — fixture lambda-poller-handlers-invoked-directly, step task#11, Invoke, status 200 -> 200:
  $.stackTrace[0]: expected '  File "/var/task/index.py", line 2, in main\n    if event["status"] == "SUCCEEDED":\n'
                 got '  File "/var/task/index.py", line 2, in main\n    if event["status"] == "SUCCEEDED":\n       ~~~~~^^^^^^^^^^\n'
  i.e. one extra anchor line appended to the frame.

Cause 3 — fixture lambda-lvau-traj-key-rename-breaks-handler, step task#7, Invoke, status 200 -> 200:
  $.stackTrace[0]: expected '  File "/var/task/index.py", line 5, in handler\n    return {"table": os.environ["OLD_TABLE_NAME"]}\n'
                 got the same two lines plus '                     ~~~~~~~~~~^^^^^^^^^^^^^^^^^^\n'
  $.stackTrace[1]: expected a traceback frame whose file field is an interpreter source path; got '  File "os", line 709, in __getitem__\n' (bare module name as the file field).

Cause 4 — fixture lambda-sstr-traj-handler-module-mismatch-then-fix, steps task#5 and task#8, Invoke, status 200 -> 200:
  $.requestId: expected '' (empty string), got '0e1cdb2c-8e3c-4996-a13a-0fc2fe09160d'.

All four clusters were reported by the replay of trace set simparity-wire-aws-v1 with stub_path empty and owned_by empty (no open issue carries their replay key).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: replay fixture lambda-lsrno-traj-invoke-reproduces-timeout and inspect step task#5 — Invoke a python function whose handler sleeps past its configured Timeout; read the response Payload JSON. Expect errorType 'Sandbox.Timedout' next to the 'Task timed out after <Timeout>.00 seconds' message; the emulator returns no errorType.
3. Cause 2: replay fixture lambda-poller-handlers-invoked-directly step task#11 — Invoke a handler that raises a KeyError on a subscript; read Payload.stackTrace[0]. Expect a two-line frame; the emulator adds a '~~~^^^' anchor line.
4. Cause 3: replay fixture lambda-lvau-traj-key-rename-breaks-handler step task#7 — Invoke a handler that reads a missing os.environ key; read Payload.stackTrace. Expect the stdlib frame's file field to be an interpreter source path; the emulator reports the bare module name 'os' (and again appends the anchor line to frame 0).
5. Cause 4: replay fixture lambda-sstr-traj-handler-module-mismatch-then-fix steps task#5 and task#8 — Invoke a function whose configured Handler names a module that is not in the package; read Payload.requestId. Expect '' for this init-phase failure; the emulator returns a generated uuid.
6. A fix is confirmed when all four fixture steps above replay as passed.
