Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs.log-cwl-custom-log-group-shared-by-two-functions-keeps-streams-apart lambda__logs.log-cwl-role-without-logs-permission-keeps-the-tail-and-creates-no-group lambda__logs.log-cwl-log-group-update-redirects-the-next-invocation lambda__logs.log-cwl-missing-custom-log-group-is-created-on-first-invoke lambda__logs.log-cwl-node-text-multiline-console-log-is-one-event lambda__logs.log-cwl-self-structured-json-lines-are-filtered-by-their-level-key lambda__logs.log-cwl-system-debug-level-delivers-runtime-done-and-init-records
Issue-Key: lambda__logs.log-cwl-custom-log-group-shared-by-two-functions-keeps-streams-apart|run#9|AssertionError|assert body["group"] == shared, f"{fn} sees AWS_LAMBDA_LOG_GROUP_NAME={body['group']!r}"
Issue-Key: lambda__logs.log-cwl-role-without-logs-permission-keeps-the-tail-and-creates-no-group|run#9|AssertionError|assert f"START RequestId: {rid} Version: $LATEST" in lines, f"{label}: tail {tail!r}"
Issue-Key: lambda__logs.log-cwl-log-group-update-redirects-the-next-invocation|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: lambda__logs.log-cwl-missing-custom-log-group-is-created-on-first-invoke|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: lambda__logs.log-cwl-node-text-multiline-console-log-is-one-event|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: lambda__logs.log-cwl-self-structured-json-lines-are-filtered-by-their-level-key|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: lambda__logs.log-cwl-system-debug-level-delivers-runtime-done-and-init-records|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: 48b98241475aee57ea7962587dcd25c49f07ef51

# lambda__logs: 3 net-new causes — LoggingConfig.LogGroup ignored by the runtime, Invoke's request id does not match the Tail log's START/END/REPORT lines, and invocation output is never observable in the function's log group

> _Note: this issue was identified against the code base at commit `48b98241475aee57ea7962587dcd25c49f07ef51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Three distinct root causes in how a Lambda invocation's logs are produced and delivered. Related open issue 20261105T111951Z-lambda-logs-an-invocation-writes-nothing-to-its.md covers two other checks of this service; the causes below are carried by checks that issue does not claim, and cause 1 and cause 2 are independent of delivery.

Cause 1 of 3: LoggingConfig.LogGroup is ignored, so the runtime advertises the default group.
AWS: Smithy (lambda) LoggingConfig.LogGroup (@length 1..512, @pattern ^[\.\-_/#A-Za-z0-9]+$) is documented as "The name of the Amazon CloudWatch log group the function sends logs to. By default, Lambda functions send logs to a default log group named /aws/lambda/<function name>. To use a different log group, enter an existing log group or enter a new log group name." A function created with LoggingConfig={LogFormat:Text, LogGroup:<custom>} therefore runs with AWS_LAMBDA_LOG_GROUP_NAME equal to that custom group, and two functions sharing one custom group both report it while keeping separate streams; no /aws/lambda/<fn> group is created.
Emulator: the invoked handler reads AWS_LAMBDA_LOG_GROUP_NAME as '/aws/lambda/<function name>' even though CreateFunction was given a custom LoggingConfig.LogGroup — i.e. the stored LoggingConfig is not propagated into the execution environment (and, by implication, not used as the delivery target).
Why: a client that routes several functions into one application log group (the documented reason to set LogGroup) sees its code self-report the wrong destination; log routers, log-group-scoped IAM and per-group retention/subscription wiring built on AWS_LAMBDA_LOG_GROUP_NAME all address a group the function does not use.
Fix: the Lambda CreateFunction/UpdateFunctionConfiguration state for LoggingConfig and the invoke path that builds the execution environment variables (and chooses the delivery target) from it.
Traces: lambda__logs.log-cwl-custom-log-group-shared-by-two-functions-keeps-streams-apart.

Cause 2 of 3: the Invoke response's request id is not the request id written into the invocation's log lines.
AWS: an Invoke response carries the invocation's request id in x-amzn-RequestId (botocore surfaces it as ResponseMetadata.RequestId), and the same id is the one that appears in the runtime-framed lines of that invocation: 'START RequestId: <request id> Version: $LATEST', 'END RequestId: <request id>' and 'REPORT RequestId: <request id>\tDuration: ... ms\tBilled Duration: ... ms\tMemory Size: ... MB\tMax Memory Used: ... MB'. The fixed text is the 'START RequestId: ' / 'Version: ' / 'END RequestId: ' / 'REPORT RequestId: ' framing and the tab-separated REPORT field names; the variable parts are the request id of the call and that invocation's measured duration/memory. Recorded behavior: with LogType=Tail the base64 LogResult holds exactly those lines for the id the response returned.
Emulator: the decoded Tail body is well formed but keyed by a DIFFERENT uuid than ResponseMetadata.RequestId of the very same Invoke response, so 'START RequestId: <returned id> Version: $LATEST' is absent from the tail (observed tail: 'START RequestId: 1b709cd9-... Version: $LATEST\nout-<marker>\nEND RequestId: 1b709cd9-...\nREPORT RequestId: 1b709cd9-...\tDuration: 1052.60 ms\tBilled Duration: 1053 ms\tMemory Size: 128 MB\n' while the response advertised another id). The REPORT line additionally omits the trailing '\tMax Memory Used: <n> MB' field AWS always emits.
Why: request-id correlation is the only join between an API call and its logs. A client that captures ResponseMetadata.RequestId and then greps the tail, the log stream or Insights for that id finds nothing, so per-invocation tracing, retry diagnosis and support escalation all break.
Fix: the Invoke handler must mint one request id per invocation and use it both for the x-amzn-RequestId response header and for the runtime log framing, and the REPORT line serializer must emit the Max Memory Used field.
Traces: lambda__logs.log-cwl-role-without-logs-permission-keeps-the-tail-and-creates-no-group.

Cause 3 of 3: an invocation's events never become observable in its CloudWatch Logs group.
AWS: after a successful synchronous Invoke, the function's log group (the default /aws/lambda/<fn> or the configured LoggingConfig.LogGroup) receives, within seconds, one stream per execution environment holding the START/END/REPORT lines and the handler's stdout — a Text-format Node.js console.log with embedded newlines arriving as ONE event framed '<ISO-8601 ms timestamp>\t<request id>\tINFO\t<message>', JSON LogFormat applying ApplicationLogLevel filtering, and SystemLogLevel=DEBUG adding the runtime's init/done records.
Emulator: these checks never complete inside the suite window — the polled condition (the handler's printed marker, or a 'REPORT RequestId: <id>\t' line, in the group) is never satisfiable, so the content assertions downstream (one-event multiline framing, level filtering, system records, redirect after LoggingConfig update) are never reached.
Why: every Lambda observability workflow — tailing a function, alarming on its errors, metric filters, subscription filters to a log consumer — reads the function's log group, and here it stays empty, so the function appears never to have run.
Fix: the Lambda invoke path's log-delivery component that creates the log stream and writes START/stdout/END/REPORT into the configured log group, including the Text/JSON framing and the ApplicationLogLevel/SystemLogLevel filters.
Traces: lambda__logs.log-cwl-log-group-update-redirects-the-next-invocation, lambda__logs.log-cwl-missing-custom-log-group-is-created-on-first-invoke, lambda__logs.log-cwl-node-text-multiline-console-log-is-one-event, lambda__logs.log-cwl-self-structured-json-lines-are-filtered-by-their-level-key, lambda__logs.log-cwl-system-debug-level-delivers-runtime-done-and-init-records.

## Evidence
Cause 1 — expected: handler sees AWS_LAMBDA_LOG_GROUP_NAME == the custom LoggingConfig.LogGroup it was created with. Actual: AssertionError: sim2real-behav-l4-<id> sees AWS_LAMBDA_LOG_GROUP_NAME='/aws/lambda/sim2real-behav-l4-<id>' (check line 60).

Cause 2 — expected: 'START RequestId: <ResponseMetadata.RequestId> Version: $LATEST' present in the decoded LogResult. Actual: AssertionError: bare: tail 'START RequestId: 1b709cd9-fa60-427d-974c-cd78aaeb897f Version: $LATEST\nout-m-f4464d136256\nEND RequestId: 1b709cd9-fa60-427d-974c-cd78aaeb897f\nREPORT RequestId: 1b709cd9-fa60-427d-974c-cd78aaeb897f\tDuration: 1052.60 ms\tBilled Duration: 1053 ms\tMemory Size: 128 MB\n' — the tail is framed by an id other than the one the Invoke response returned, and REPORT has no 'Max Memory Used' field.

Cause 3 — expected: the awaited event (printed marker / REPORT line) readable from the function's log group within seconds. Actual: all five checks end with 'Error: Check timed out after 60.0s' while polling filter_log_events/get_log_events on that group.

## Steps to Reproduce
1. Create an IAM role trusted by lambda.amazonaws.com with AWSLambdaBasicExecutionRole.
2. Cause 1: logs.create_log_group(logGroupName='/sim2real/<custom>'); lambda.create_function(..., Runtime='python3.12', LoggingConfig={'LogFormat':'Text','LogGroup':'/sim2real/<custom>'}) with a handler returning os.environ['AWS_LAMBDA_LOG_GROUP_NAME']; Invoke and read the returned value — it is '/aws/lambda/<fn>' instead of the custom group.
3. Cause 2: lambda.invoke(FunctionName=<fn>, LogType='Tail'); compare resp['ResponseMetadata']['RequestId'] with the 'START RequestId:'/'REPORT RequestId:' ids in base64.b64decode(resp['LogResult']) — they differ, and REPORT carries no 'Max Memory Used' field.
4. Cause 3: after that same Invoke, poll logs.filter_log_events(logGroupName='/aws/lambda/<fn>') for the handler's printed marker or the REPORT line — it never appears.
