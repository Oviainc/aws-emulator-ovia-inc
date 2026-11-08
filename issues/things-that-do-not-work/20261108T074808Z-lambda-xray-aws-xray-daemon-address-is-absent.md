Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__xray.lambda-udp-daemon-datagram-reaches-trace-store lambda__xray.lambda-udp-daemon-malformed-framing-datagram-rejected
Issue-Key: lambda__xray.lambda-udp-daemon-datagram-reaches-trace-store|run#13|AssertionError|assert daemon and _DAEMON_RE.match(daemon), (
Issue-Key: lambda__xray.lambda-udp-daemon-malformed-framing-datagram-rejected|run#17|AssertionError|assert daemon and _DAEMON_RE.match(daemon), (
Issue-Commit: 3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e

# lambda__xray: AWS_XRAY_DAEMON_ADDRESS is absent from the Lambda invocation environment, so no handler can address the X-Ray daemon over UDP (1 cause)

> _Note: this issue was identified against the code base at commit `3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the Lambda execution environment does not set AWS_XRAY_DAEMON_ADDRESS, so a handler has no address to send X-Ray segment datagrams to and neither well-formed nor malformed UDP framing can be exercised. Failing checks: lambda__xray.lambda-udp-daemon-datagram-reaches-trace-store, lambda__xray.lambda-udp-daemon-malformed-framing-datagram-rejected.

AWS: recorded behavior only — the execution-environment variables are not described by a Smithy shape. AWS sets AWS_XRAY_DAEMON_ADDRESS in every Lambda invocation environment to `<ipv4>:<port>` — the fixed form being a dotted-quad address, a colon and a port number, with both values supplied by the platform. A handler (or the X-Ray SDK) sends the daemon's two-line UDP framing (a header line then a segment document) to that address; a well-formed datagram's segment becomes retrievable from the trace store, and a datagram with broken framing is rejected without a segment appearing.

Emulator: the variable is simply not there. Both checks report AWS_XRAY_DAEMON_ADDRESS as None — `AssertionError: AWS_XRAY_DAEMON_ADDRESS in the invocation environment was None; the handler contract is <ipv4>:<port>. Environment keys the handler saw: ['daemon', 'parent', 'raw_trace_header', 'root', 'sampled', 'send_error', 'sent']` and `AssertionError: AWS_XRAY_DAEMON_ADDRESS in the invocation environment was None, expected <ipv4>:<port>`. With no address, nothing about the datagram path — acceptance or rejection — is measurable.

Why: every X-Ray-instrumented function reads this variable (directly or through the X-Ray SDK) to emit segments. A client gets no segments from Lambda at all, so service maps, subsegments and trace-store queries are empty, and a malformed-emitter bug cannot be detected because the emitter has nowhere to send.

Fix: the Lambda invoke path's execution-environment construction — it must publish AWS_XRAY_DAEMON_ADDRESS as `<ipv4>:<port>` pointing at a UDP listener that accepts the X-Ray daemon framing and forwards accepted segments to the trace store (rejecting malformed framing without storing a segment).

Note: 20261105T095706Z-lambda-xray-the-lambda-python-runtime-has-no.md is an open issue in this service about the runtime lacking the _socket extension module so the datagram handlers died with Runtime.ImportModuleError. It does not cover this failure: the handler now imports and runs, and the remaining defect is the missing daemon-address environment variable.

Traces: both checks listed above.

## Evidence
Expected (AWS): the handler reads os.environ['AWS_XRAY_DAEMON_ADDRESS'] and it matches `<ipv4>:<port>`; a well-formed two-line daemon datagram sent there results in the segment being retrievable from the trace store, and a malformed-framing datagram results in no segment.

Actual (emulator): the variable is absent.
  - lambda-udp-daemon-datagram-reaches-trace-store: `AssertionError: AWS_XRAY_DAEMON_ADDRESS in the invocation environment was None; the handler contract is <ipv4>:<port>. Environment keys the handler saw: ['daemon', 'parent', 'raw_trace_header', 'root', 'sampled', 'send_error', 'sent']`
  - lambda-udp-daemon-malformed-framing-datagram-rejected: `AssertionError: AWS_XRAY_DAEMON_ADDRESS in the invocation environment was None, expected <ipv4>:<port>`

## Steps to Reproduce
1. Create a Python Lambda with TracingConfig Mode=Active whose handler returns os.environ.get('AWS_XRAY_DAEMON_ADDRESS') and, if present, sends an X-Ray daemon UDP datagram to it.
2. Invoke the function. Observe the returned daemon address is None instead of `<ipv4>:<port>`, so no datagram can be sent.
3. Replay lambda__xray.lambda-udp-daemon-datagram-reaches-trace-store and lambda__xray.lambda-udp-daemon-malformed-framing-datagram-rejected.
