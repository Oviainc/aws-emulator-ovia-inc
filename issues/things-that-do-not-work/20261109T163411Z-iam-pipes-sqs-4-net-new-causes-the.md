Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__pipes__sqs.create-pipe-input-template-over-max-length-rejected iam__pipes__sqs.update-pipe-input-template-over-max-length-rejected-pipe-keeps-working iam__pipes__sqs.input-template-auto-quotes-strings-and-keeps-inlined-object-quotes-intact iam__pipes__sqs.input-template-reserved-pipe-variables-render-real-values iam__pipes__sqs.pipe-filter-on-poller-added-event-source-field-never-matches
Issue-Key: pipes|CreatePipe|ValidationException
Issue-Key: pipes|UpdatePipe|ValidationException
Issue-Key: iam__pipes__sqs.input-template-auto-quotes-strings-and-keeps-inlined-object-quotes-intact|run#14|AssertionError|assert rendered_raw == expected_raw, (
Issue-Key: iam__pipes__sqs.input-template-reserved-pipe-variables-render-real-values|run#11|AssertionError|assert first_fixed == expected_fixed, f"rendered fixed fields {first_fixed!r} != expected {expected_fixed!r}"
Issue-Key: iam__pipes__sqs.pipe-filter-on-poller-added-event-source-field-never-matches|run#27|AssertionError|assert others_a == [], (
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# iam__pipes__sqs: 4 net-new causes — the over-length InputTemplate refusal is misworded, templates render wrongly, reserved pipe variables are not substituted, and a filter on a poller-added field matches

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Four distinct root causes in the EventBridge Pipes input-template and filtering surface.

Cause 1 of 4: the over-length InputTemplate refusal uses the generic constraint message instead of AWS's input-template compiler error.
AWS: an InputTemplate longer than its maximum is refused by CreatePipe and UpdatePipe with code `ValidationException` whose message is the input-template compiler's error, which echoes the submitted template: it begins `[string "<the submitted input template>"` and continues with the compiler's complaint. The fixed part is the `[string "…"` framing and the compiler's trailing text; the variable part is the template submitted in the request. Smithy: pipes#InputTemplate is @length(min 0, max 8192) and the operations declare ValidationException — the model gives the limit, but the message AWS emits is the compiler error, not the constraint summary.
Emulator: the message is `1 validation error detected: Value at 'targetParameters.inputTemplate' failed to satisfy constraint: Member must have length less than or equal to 8192` — the generic Smithy constraint text, with none of the compiler framing.
Why: a client matching on the refusal (or showing it to the template's author) sees neither the offending template nor the compiler's reason; message-equality against AWS fails for both create and update.
Fix: the InputTemplate validation path shared by CreatePipe and UpdatePipe must emit the template-compiler error message.
Traces: iam__pipes__sqs.create-pipe-input-template-over-max-length-rejected, iam__pipes__sqs.update-pipe-input-template-over-max-length-rejected-pipe-keeps-working.

Cause 2 of 4: input-template rendering escapes and quotes values wrongly.
AWS: in an InputTemplate, an unquoted `<$.path>` whose extracted value is a string is auto-quoted; an object path inlined inside an existing string literal keeps the object's own JSON quotes intact (breaking the surrounding string rather than stripping them); and a quoted path's extracted value is escaped like any JSON string, quote and all. Smithy: the rendering rules are service behaviour — recorded behavior only; the recording pins the rendered target body byte-for-byte.
Emulator: the rendered body differs — the inlined object's quotes are escaped (`"x {\"key\":\"value\"}"`) instead of being emitted verbatim.
Why: the bytes the target receives are not the bytes AWS would send, so a consumer parsing the delivered payload sees different structure (an escaped string where AWS produces broken-but-literal JSON), and templates validated against AWS misbehave here.
Fix: the input-template renderer in the Pipes enrichment/target stage.
Traces: iam__pipes__sqs.input-template-auto-quotes-strings-and-keeps-inlined-object-quotes-intact.

Cause 3 of 4: reserved pipe variables are not substituted.
AWS: the reserved variables `<aws.pipes.pipe-arn>`, `<aws.pipes.pipe-name>`, `<aws.pipes.source-arn>` and `<aws.pipes.target-arn>` render to the pipe's own ARN and name and the configured source and target ARNs. Smithy: the reserved-variable set is service behaviour — recorded behavior only; the recording shows real values in the delivered body.
Emulator: the delivered body contains the placeholders verbatim (`'<aws.pipes.pipe-arn>'`, `'<aws.pipes.pipe-name>'`, `'<aws.pipes.source-arn>'`, `'<aws.pipes.target-arn>'`).
Why: a consumer that routes or attributes messages by the pipe/source/target identity receives literal placeholder text instead of ARNs — every message is unattributable, and downstream parsing of those fields fails.
Fix: reserved-variable substitution in the input-template renderer.
Traces: iam__pipes__sqs.input-template-reserved-pipe-variables-render-real-values.

Cause 4 of 4: a FilterCriteria pattern on a poller-added field matches.
AWS: fields the SQS poller adds to the event envelope (`eventSource`, `eventSourceARN`, `awsRegion`, …) are not part of the message as the source API delivered it, so a FilterCriteria pattern on `eventSource` can never match and no message reaches the target through such a filter — only the control filter does. Smithy: pipes#FilterCriteria.Filters[].Pattern is matched against the source event; which fields are visible to the filter is service behaviour — recorded behavior only, and the recording shows no delivery through the eventSource filter.
Emulator: a message IS delivered through the eventSource filter (the target received a full SQS record whose body itself contains `"eventSource": "aws:sqs"`).
Why: a filter the client believes blocks everything lets traffic through, so messages reach a target that was supposed to be unreachable — a correctness and isolation defect, and the inverse mistake (filters that should match) cannot be distinguished.
Fix: the filter-evaluation stage must match patterns against the source event only, before poller-added envelope fields are merged.
Traces: iam__pipes__sqs.pipe-filter-on-poller-added-event-source-field-never-matches.

## Evidence
Cause 1 — Expected ('ValidationException', '[string "{"v": "aaaa…"' …); Actual ('ValidationException', "1 validation error detected: Value at 'targetParameters.inputTemplate' failed to satisfy constraint: Member must have length less than or equal to 8192") — for both CreatePipe and UpdatePipe.
Cause 2 — Expected '{"unquoted_string": "hello", "obj_in_string": "x {"key":"value"}", …}'; Actual '{"unquoted_string":"hello","obj_in_string":"x {\\"key\\":\\"value\\"}", …}'.
Cause 3 — Expected {'pipeArn': 'arn:aws:pipes:…:pipe/…', 'pipeName': '…', 'sourceArn': 'arn:aws:sqs:…', 'targetArn': 'arn:aws:sqs:…'}; Actual {'pipeArn': '<aws.pipes.pipe-arn>', 'pipeName': '<aws.pipes.pipe-name>', 'sourceArn': '<aws.pipes.source-arn>', 'targetArn': '<aws.pipes.target-arn>'}.
Cause 4 — Expected: no message delivered through the eventSource filter; Actual: one full SQS record delivered.

## Steps to Reproduce
Cause 1: CreatePipe (then UpdatePipe) with a TargetParameters.InputTemplate longer than 8192 bytes; compare the ValidationException message.
Cause 2: CreatePipe with an InputTemplate mixing an unquoted string path, an object path inlined in a string literal and a quoted path whose value contains a quote; send a message and compare the raw target body.
Cause 3: CreatePipe with an InputTemplate referencing the four reserved aws.pipes variables; send a message and read the delivered body.
Cause 4: CreatePipe with FilterCriteria matching on eventSource, plus a control pipe; send messages and observe whether the eventSource-filtered target receives any.
