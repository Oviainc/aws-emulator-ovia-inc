Issue-Source: trace-sweep-divergence
Issue-Trace: timestream-influxdb-create-db-parameter-group-success-and-get
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: timestream-influxdb-create-db-parameter-group-success-and-get
Issue-Shape-Id: com.amazonaws.timestreaminfluxdb#AmazonTimestreamInfluxDB
Issue-Key: timestream-influxdb|CreateDbParameterGroup|2xx->2xx|$.parameters
Issue-Key: timestream-influxdb|GetDbParameterGroup|2xx->2xx|$.parameters
Issue-Commit: 5dbb2ab27a0402c002f5157a9a56e61b81adc0a2

# timestream-influxdb: Parameters union member dropped from DB parameter group create and get responses

> _Note: this issue was identified against the code base at commit `5dbb2ab27a0402c002f5157a9a56e61b81adc0a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDbParameterGroup

## Description
The emulator does not round-trip the `Parameters` union of a DB parameter group. The request's `parameters` union is accepted (HTTP 200, no ValidationException) but the modeled `parameters` output member is absent from the response, and it is still absent when the stored group is read back. Both causes below are the same defect in the `Parameters` union's wire handling; they are listed separately because each is a distinct operation/step a fixer must re-replay.

Cause 1 of 2 — CreateDbParameterGroup omits `parameters` from its response (fixture timestream-influxdb-create-db-parameter-group-success-and-get, step task#1).
AWS: Smithy: CreateDbParameterGroup accepts input member `parameters` (target `Parameters`) and its output shape `CreateDbParameterGroupOutput` carries `parameters` (target `Parameters`) alongside the required `id`, `name`, `arn` and the optional `description`. `Parameters` is a union with members `InfluxDBv2` (InfluxDBv2Parameters), `InfluxDBv3Core` and `InfluxDBv3Enterprise`, documented as "The parameters that comprise the parameter group"; the create operation's `parameters` member is documented as "A list of the parameters that comprise the DB parameter group". AWS therefore echoes the union the caller supplied back on the create response, as the same single-member union object. Every member of `InfluxDBv2Parameters` is optional, so a union variant whose value object has no members present is a legal, meaningful value: it selects the InfluxDBv2 engine parameter family with every tunable left at its default. Status for the success case is 200.
Emulator: answers 200 with the right status and the other output members, but `$.parameters` is absent entirely (`None`) where the recording carries the union object selecting the `InfluxDBv2` variant with an empty value object. Nothing is reported to the caller — no error, no partial value; the member is simply not serialized.
Why: a client that creates a parameter group and reads `parameters` off the create response to confirm which engine parameter family and which tunables it just configured sees no parameters at all. It cannot distinguish "the group has no parameters" from "the service dropped them", and code that branches on which union variant is set (InfluxDBv2 vs InfluxDBv3Core vs InfluxDBv3Enterprise) takes the no-value path on a group that does have a variant selected. SDKs that model this as a required-to-be-present-when-configured union will surface an empty/absent union to application code.
Fix: the `Parameters` union's wire handling for this operation — the request deserializer that populates `CreateDbParameterGroupInput.parameters` and/or the response serializer that writes `CreateDbParameterGroupOutput.parameters`. The handler and state layer already carry the value through unchanged (the record holds `parameters: Option<Parameters>` and the projection copies it), so the loss is on one of the two wire edges, most likely the union codec. Hypothesis: a union variant whose value object has every member absent is treated as empty and collapsed to no-variant-selected, so the union is dropped rather than serialized as `{"InfluxDBv2": {}}`.
Traces: timestream-influxdb-create-db-parameter-group-success-and-get#task#1 (detail: "$.parameters: missing (expected {'InfluxDBv2': {}}, got None)", expected status 200, actual status 200).

Cause 2 of 2 — GetDbParameterGroup omits `parameters` when reading the group back (fixture timestream-influxdb-create-db-parameter-group-success-and-get, step task#2).
AWS: Smithy: GetDbParameterGroup takes `identifier` (required, `DbParameterGroupIdentifier`) and its output shape `GetDbParameterGroupOutput` carries `parameters` (target `Parameters`) beside the required `id`, `name`, `arn` and the optional `description`. A read of an existing group reports the union the group was created with; the success status is 200 and the only not-found path is the modeled `ResourceNotFoundException` (HTTP 404), which does not apply here because the group exists.
Emulator: answers 200 and resolves the group, but `$.parameters` is absent (`None`) where the recording carries the same union object (the `InfluxDBv2` variant) that the preceding create supplied.
Why: the parameters of a parameter group are the whole content of the resource. A client that reads a group back to decide whether to update it, to diff desired against actual configuration, or to pass the group's parameter family to a DB instance sees an empty resource and will either re-apply parameters needlessly or conclude the group is unconfigured. Combined with cause 1, there is no path by which a caller can observe the parameters it set.
Fix: the same `Parameters` union wire handling, on this operation's response serializer; the read path and the stored record are correct, so the fix is shared with cause 1 and both operations must be re-replayed.
Traces: timestream-influxdb-create-db-parameter-group-success-and-get#task#2 (detail: "$.parameters: missing (expected {'InfluxDBv2': {}}, got None)", expected status 200, actual status 200).

The only other open issue on this service (20261022T105412Z-timestream-influxdb-tagresource-stub-answers-501-notimplemented-no.md, about the TagResource stub, its authorization gate and its ValidationException message) does not cover this: it concerns a different operation and does not carry either replay key above.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, data listener accepting):

Step 1 — fixture timestream-influxdb-create-db-parameter-group-success-and-get, step task#1, operation CreateDbParameterGroup, verdict mismatch.
  expected_status 200, actual_status 200
  detail: $.parameters: missing (expected {'InfluxDBv2': {}}, got None)
  Expected response body member: "parameters": {"InfluxDBv2": {}} — the Parameters union with the InfluxDBv2 variant selected and its (all-optional) value object carrying no members.
  Actual response body: no "parameters" member at all.

Step 2 — same fixture, step task#2, operation GetDbParameterGroup, verdict mismatch.
  expected_status 200, actual_status 200
  detail: $.parameters: missing (expected {'InfluxDBv2': {}}, got None)
  Expected: the group reads back with the same "parameters": {"InfluxDBv2": {}} union it was created with.
  Actual: "parameters" absent.

Model evidence (smithy_query on services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/smithy-model/com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb.json):
  CreateDbParameterGroup.input  -> parameters: Parameters (optional)
  CreateDbParameterGroup.output -> id*, name*, arn*, description, parameters: Parameters
  GetDbParameterGroup.output    -> id*, name*, arn*, description, parameters: Parameters
  shape Parameters = union { InfluxDBv2: InfluxDBv2Parameters, InfluxDBv3Core: InfluxDBv3CoreParameters, InfluxDBv3Enterprise: InfluxDBv3EnterpriseParameters }, doc "The parameters that comprise the parameter group."

Code evidence that the loss is on a wire edge rather than in state: services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/src/ops/create_db_parameter_group.rs:211-222 stores `parameters: input.parameters` into the record and returns `record.to_create_output()`; services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/src/state.rs:267 holds `parameters: Option<Parameters>` and src/state.rs:278-301 projects `parameters: self.parameters.clone()` into both CreateDbParameterGroupOutput and GetDbParameterGroupOutput. No status or error-code divergence is involved: both steps returned the modeled 200.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: replay fixture timestream-influxdb-create-db-parameter-group-success-and-get, step task#1 — call CreateDbParameterGroup with a valid `name` (3..64, ^[a-zA-Z][a-zA-Z0-9]*(-[a-zA-Z0-9]+)*$) and `parameters` set to the Parameters union with the InfluxDBv2 variant selected, e.g. {"InfluxDBv2": {}}. Observe HTTP 200 whose body carries id/name/arn but no `parameters` member; AWS echoes the supplied union back.
3. Cause 2: replay the same fixture's step task#2 — call GetDbParameterGroup with the `id` returned by step 2 as `identifier`. Observe HTTP 200 whose body again omits `parameters`, where AWS reports the union the group was created with.
4. Both steps report verdict mismatch with detail "$.parameters: missing (expected {'InfluxDBv2': {}}, got None)". Re-replay BOTH steps to confirm a fix; a fix verified only on task#1 leaves the read-back path unexercised.
