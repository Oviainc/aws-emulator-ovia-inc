Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-data-automation-runtime-async-document-extraction-writes-text
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-data-automation-runtime-async-document-extraction-writes-text
Issue-Shape-Id: com.amazonaws.bedrockdataautomationruntime#AmazonBedrockKeystoneRuntimeService
Issue-Key: bedrock-data-automation-runtime|GetDataAutomationStatus|2xx->2xx|$.jobCompletionTime
Issue-Key: bedrock-data-automation-runtime|GetDataAutomationStatus|2xx->2xx|$.jobSubmissionTime
Issue-Commit: 3d6b446e08b087d2665f5ca5dd389ba075b4c25a

# GetDataAutomationStatus serializes job timestamps as epoch-seconds numbers instead of modeled date-time strings

> _Note: this issue was identified against the code base at commit `3d6b446e08b087d2665f5ca5dd389ba075b4c25a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataAutomationStatus

## Description
Cause 1 of 1: GetDataAutomationStatus emits jobSubmissionTime and jobCompletionTime with the wrong wire type.

AWS: Smithy: GetDataAutomationStatus output members jobSubmissionTime and jobCompletionTime both target Timestamp carrying @timestampFormat("date-time"), so both are serialized as RFC 3339 / ISO-8601 date-time STRINGS (e.g. "2026-11-09T12:34:56.790Z"), not as the protocol-default epoch-seconds number. The recorded 200 response carries both members in that date-time string shape (the recording asserts the shape, not the instant, so the fixture shows them as '<shaped-not-asserted>'); the instant itself is per-run and is not a requirement.

Emulator: the same 200 response carries both members as JSON NUMBERS of fractional epoch seconds — $.jobSubmissionTime = 1794315764.990614 and $.jobCompletionTime = 1794315765.590614 (the digits are this run's clock and are not the point; the TYPE is). The handler builds the values correctly as aws_smithy_types::DateTime via crate::state::to_date_time, so the loss happens where the output is serialized: the member's @timestampFormat("date-time") override is not applied and the protocol default (epoch-seconds) is used instead.

Why: a client that relies on AWS's behavior cannot deserialize the response. An AWS SDK generated from this model expects a date-time string for these two members and errors out on a number, so a poll loop over an async data-automation job fails at parse time rather than observing the job's progress; a hand-written client that parses the field as a timestamp string sees a type error, and one that string-compares or sorts the value gets a value in a different domain (seconds since epoch) from AWS's.

Fix: the GetDataAutomationStatus output serializer / the generated output schema for this operation, so the per-member @timestampFormat("date-time") override is honored for jobSubmissionTime and jobCompletionTime (every other member of the response matched). Not the handler: it already yields DateTime values from the stored instants.

Traces: bedrock-data-automation-runtime-async-document-extraction-writes-text#task#6 (status 200 on both sides; only the two timestamp members diverge).

## Evidence
Fixture bedrock-data-automation-runtime-async-document-extraction-writes-text, step task#6, GetDataAutomationStatus, expected status 200, actual status 200.
$.jobCompletionTime: value_mismatch (expected '<shaped-not-asserted>' — a date-time string per @timestampFormat("date-time") — got the number 1794315765.590614)
$.jobSubmissionTime: value_mismatch (expected '<shaped-not-asserted>', got the number 1794315764.990614)
Model authority: smithy_query(query="operation", operation="GetDataAutomationStatus") on services/service-com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice/smithy-model/com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice.json reports jobSubmissionTime/jobCompletionTime -> {"target": "Timestamp", "timestampFormat": "date-time"}.
Handler side: services/.../src/ops/get_data_automation_status.rs lines 254-259 set the members from crate::state::to_date_time; services/.../src/state.rs lines 1389-1394 build the DateTime from epoch seconds.

## Steps to Reproduce
1. Replay the wire corpus; the diverging step is fixture bedrock-data-automation-runtime-async-document-extraction-writes-text step task#6.
2. Minimal repro: call InvokeDataAutomationAsync to mint a job, then call GetDataAutomationStatus with the returned invocationArn and inspect the JSON body. AWS returns "jobSubmissionTime": "<RFC3339 instant>" (and, once the job is terminal, "jobCompletionTime": "<RFC3339 instant>"); the emulator returns both as bare JSON numbers of epoch seconds.
