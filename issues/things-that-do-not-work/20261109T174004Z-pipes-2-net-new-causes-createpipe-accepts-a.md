Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes.create-pipe-malformed-input-template-json-rejected pipes.input-template-unresolved-path-empty-string-and-whole-event-json
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes.create-pipe-malformed-input-template-json-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: pipes.input-template-unresolved-path-empty-string-and-whole-event-json|_receive_until#11|TimeoutError|raise TimeoutError(f"no message carrying {marker!r} reached the queue within {timeout_s:.0f}s; saw {seen!r}")
Issue-Commit: 6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4

# pipes: 2 net-new causes — CreatePipe accepts a malformed InputTemplate JSON, and a pipe with an unresolved template path delivers nothing

> _Note: this issue was identified against the code base at commit `6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Two distinct defects. Neither replay key is carried by an open issue.

Cause 1 of 2: a syntactically invalid TargetParameters.InputTemplate is accepted.
AWS: CreatePipe parses the InputTemplate's literal JSON at create time and refuses a malformed one with HTTP 400 ValidationException (Smithy: CreatePipe declares ValidationException, httpError 400) whose message is the Pipes field-report form, fixed text and all: {"ValidationErrorFields":[{"Name":"InputTemplate","Message":"Invalid JSON format for Input Template."}]} — the only variable part of this message class is the offending member's name ("InputTemplate"). Nothing is created: the refused name does not appear in ListPipes. The same request with well-formed template JSON is accepted and the template is echoed verbatim by DescribePipe.
Emulator: the call returns HTTP 200 and creates the pipe (DesiredState=RUNNING, CurrentState=CREATING). The template is stored unparsed, so the error surfaces, if at all, only at delivery time.
Why: the client's authoring-time guard rail disappears: a broken template deploys clean and the failure moves from a synchronous 400 at create time to silent data loss at runtime. A negative test for a bad template passes against the emulator and fails in AWS.
Fix: the Pipes CreatePipe (and UpdatePipe) handler's TargetParameters.InputTemplate validation — parse the template and raise ValidationException with the ValidationErrorFields message, persisting nothing.
Traces: pipes.create-pipe-malformed-input-template-json-rejected.

Cause 2 of 2: a pipe whose template contains an unresolved path and the whole-event pseudo path delivers nothing.
AWS: an InputTemplate path that does not resolve against the event (here <$.body.missing>, the body carrying no "missing" key) renders as the empty string rather than failing the record, and <aws.pipes.event.json> renders the whole event as JSON. The record is still delivered to the target, carrying the run's marker.
Emulator: no message carrying the marker ever reaches the target queue within 52 s (the target was read repeatedly and saw []). The record is dropped instead of being delivered with an empty substitution.
Why: an unresolved path is a routine condition in heterogeneous event streams; treating it as fatal loses every such record silently, and a client cannot distinguish "path missing" from "pipe broken". The documented <aws.pipes.event.json> escape hatch is also unusable.
Fix: the Pipes target input-transformer's path-resolution (an unresolved JSONPath must substitute the empty string, and <aws.pipes.event.json> must render the full event) together with the target stage that must still deliver the rendered record. Related open issue 20261103T144201Z-pipes-a-filter-parsed-json-body-is-not.md describes a different non-render path and does not carry this replay key.
Traces: pipes.input-template-unresolved-path-empty-string-and-whole-event-json.

## Evidence
Cause 1 — expected a refusal with Error.Code 'ValidationException' and Error.Message '{"ValidationErrorFields":[{"Name":"InputTemplate","Message":"Invalid JSON format for Input Template."}]}' and the name absent from ListPipes; got HTTP 200 and a created pipe: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Arn': 'arn:aws:pipes:...:pipe/<name>', 'Name': '<name>', 'DesiredState': 'RUNNING', 'CurrentState': 'CREATING', ...}. InputTemplate submitted: '{"broken": <$.body.marker'. The control leg with '{"ok": "<$.body.marker>"}' is accepted and echoed.

Cause 2 — TimeoutError: no message carrying '<marker>' reached the queue within 52s; saw [].

## Steps to Reproduce
1. CreatePipe (SQS source, SQS target, valid role) with TargetParameters.InputTemplate='{"broken": <$.body.marker' — the emulator answers 200 and creates the pipe; AWS answers 400 ValidationException with the ValidationErrorFields message and creates nothing.
2. CreatePipe with TargetParameters.InputTemplate containing an unresolvable path (<$.body.missing>) and <aws.pipes.event.json>, DesiredState=RUNNING; send one message carrying a marker in its body and poll the target — nothing arrives. AWS delivers the record with the unresolved path rendered as an empty string.
