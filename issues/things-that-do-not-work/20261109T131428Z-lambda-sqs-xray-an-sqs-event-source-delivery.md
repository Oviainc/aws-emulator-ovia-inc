Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__xray.sqs-delivered-invocation-trace-links-back-to-the-messages-trace-header
Issue-Key: lambda__sqs__xray.sqs-delivered-invocation-trace-links-back-to-the-messages-trace-header|_poll_until#13|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: fb003e0a3fdd2a31392ea928ed9540deb9fe4c34

# lambda__sqs__xray: an SQS event-source delivery to a TracingConfig Active function publishes no X-Ray segment, so nothing links back to the message's AWSTraceHeader (1 cause)

> _Note: this issue was identified against the code base at commit `fb003e0a3fdd2a31392ea928ed9540deb9fe4c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTraceSummaries

## Description
One net-new root cause behind 1 failing check.

Cause 1 of 1: a message delivered by an Enabled SQS event source mapping to a function created with TracingConfig Mode=Active produces no X-Ray trace for that function at all, so no segment ever carries a links entry back to the trace id in the message's AWSTraceHeader.

AWS: when Lambda consumes an SQS message whose AWSTraceHeader system attribute carries Root=1-<8 hex>-<24 hex>, and the function's tracing mode is Active, Lambda publishes the invocation under a NEW trace id and records a link to the message's trace: the segment named after the function carries a links entry whose trace_id is the message's Root and whose attributes["aws.xray.reserved.reference_type"] is "parent". The invocation's segments become retrievable through GetTraceSummaries with FilterExpression service("<function name>") over the delivery window and then BatchGetTraces (Smithy xray: GetTraceSummaries POST /TraceSummaries with StartTime/EndTime required and FilterExpression; BatchGetTraces POST /Traces). Two messages carrying two distinct caller-chosen Root ids must each link only to their own. Trace ids, the function name and the queue name are per-run values, not requirements; the requirement is the existence of the new trace and the single parent link back to the message's Root.

Emulator: after SendMessage with the AWSTraceHeader system attribute on a queue whose event source mapping reports State=Enabled, polling GetTraceSummaries for service("<function>") and BatchGetTraces never returns any segment with such a link: `TimeoutError: message 'a''s delivery to '<function>' to produce a segment linking back to trace '1-<8 hex>-<24 hex>' (from AWSTraceHeader 'Root=1-...;Parent=...;Sampled=1'): the run budget ended first; not graded`. The first message never gets past this wait, so the per-message isolation arm is never reached. No trace is published for the delivery, and consequently no link, no parent reference_type, and no distinct trace per message.

Why: a client relying on AWS's behavior loses the SQS -> Lambda hop of every distributed trace. The producer's trace cannot be joined to the consumer's invocation, so a trace map shows the queue as a dead end, latency cannot be attributed across the boundary, and tooling that walks links to find downstream work finds none — with no error to indicate anything is missing.

Fix: the Lambda invoke path's X-Ray publication for event-source (poller) invocations in the lambda service crate, together with X-Ray's segment store: when TracingConfig Mode=Active, mint a new trace for the invocation, write the function's segment with a links entry {trace_id: <message AWSTraceHeader Root>, attributes: {"aws.xray.reserved.reference_type": "parent"}}, and make it retrievable via GetTraceSummaries' service() filter and BatchGetTraces.

Traces: lambda__sqs__xray.sqs-delivered-invocation-trace-links-back-to-the-messages-trace-header.

## Evidence
Expected (AWS): SendMessage with MessageSystemAttributes AWSTraceHeader="Root=1-<8 hex>-<24 hex>;Parent=<16 hex>;Sampled=1" to a queue mapped to a TracingConfig Active function yields, within the polling window, a trace (id different from the message's Root) whose segment name equals the function name and whose links are exactly {<message Root>: "parent"}; a second message with a different Root links only to its own.

Actual (emulator): TimeoutError: message 'a''s delivery to 'sim2real-behav-sqsxray-07958d82e7a8' to produce a segment linking back to trace '1-6af1c255-823acaec0eb1136fe5db5073' (from AWSTraceHeader 'Root=1-6af1c255-823acaec0eb1136fe5db5073;Parent=ae60e65a7062dd18;Sampled=1'): the run budget ended first; not graded — GetTraceSummaries/BatchGetTraces return no segment for the function carrying any link.

## Steps to Reproduce
1. CreateQueue (VisibilityTimeout 60) and read its QueueArn.
2. Create an execution role allowing the SQS poller actions, then CreateFunction (python3.13, TracingConfig {Mode: "Active"}).
3. CreateEventSourceMapping from the queue to the function and poll GetEventSourceMapping until State=Enabled.
4. SendMessage with MessageSystemAttributes {"AWSTraceHeader": {"DataType": "String", "StringValue": "Root=1-<8 hex>-<24 hex>;Parent=<16 hex>;Sampled=1"}}.
5. Poll GetTraceSummaries(FilterExpression='service("<function name>")') over the delivery window, then BatchGetTraces on the returned ids.
6. Expected: a segment named after the function whose links are {<message Root>: "parent"}. Observed: no trace/segment for the function is ever returned.
