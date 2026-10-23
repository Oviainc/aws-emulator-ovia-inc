Issue-Source: trace-sweep-divergence
Issue-Trace: nova-act-workflow-definition-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: nova-act-workflow-definition-lifecycle
Issue-Shape-Id: com.amazonaws.novaact#AmazonNovaAgentsDataPlane
Issue-Key: nova-act|GetWorkflowDefinition|2xx->2xx|$.createdAt
Issue-Commit: 0f903c65ef17614b948ed7843da2bc7f944d5d1a

# nova-act workflow definition createdAt serialized as epoch number not date-time

> _Note: this issue was identified against the code base at commit `0f903c65ef17614b948ed7843da2bc7f944d5d1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkflowDefinition

## Description
ONE cause.

Cause 1 of 1: GetWorkflowDefinition emits the required createdAt timestamp in the wrong wire form.
AWS: Smithy: GetWorkflowDefinitionResponse.createdAt targets DateTimestamp, is required, and carries timestampFormat 'date-time' under the restJson1 protocol — so it must be serialized as an RFC 3339 / ISO-8601 date-time STRING (e.g. "2024-05-01T12:34:56.789Z"), not as a number. The recording asserts the field's shape rather than its value (expected '<shaped-not-asserted>'), because the instant itself is environment-specific.
Emulator: serializes createdAt as a bare epoch-seconds floating point number (observed 1792729913.9078014), i.e. the default epoch-seconds timestamp form instead of the member's date-time override.
Why: a generated AWS SDK deserializing a date-time-formatted member reads a string; given a JSON number it raises a deserialization/parse error, so GetWorkflowDefinition fails in the client even though the emulator answered 200. The field is required, so there is no path for a client to skip it, and every consumer of workflow-definition metadata (CLI output, lifecycle polling, anything ordering definitions by creation time) breaks rather than merely showing an odd value.
Fix: the GetWorkflowDefinition response serializer (and the shared timestamp serialization for DateTimestamp members in this service) must honour the member's timestampFormat='date-time' trait and render an ISO-8601 string. Audit the other DateTimestamp members in the service for the same default, since the fault is in how the serializer picks the timestamp format rather than in this one field.
Traces: nova-act-workflow-definition-lifecycle#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture nova-act-workflow-definition-lifecycle, step task#2 (GetWorkflowDefinition), status 200 expected and 200 actual:

  $.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1792729913.9078014)

Expected (AWS, per Smithy GetWorkflowDefinitionResponse.createdAt -> DateTimestamp, required, timestampFormat 'date-time', restJson1): a JSON string holding an ISO-8601 date-time. Actual (emulator): a JSON number holding epoch seconds with fractional part. The step's other asserted members (name, arn, status) matched, so only the timestamp encoding diverges.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Fixture nova-act-workflow-definition-lifecycle: step task#1 creates a workflow definition, step task#2 issues GET /workflow-definitions/{workflowDefinitionName}.
3. Inspect the 200 body: createdAt is a JSON number (epoch seconds, e.g. 1792729913.9078014) where the restJson1 date-time format requires a quoted ISO-8601 string.
4. Re-replay nova-act-workflow-definition-lifecycle#task#2 to confirm the fix.
