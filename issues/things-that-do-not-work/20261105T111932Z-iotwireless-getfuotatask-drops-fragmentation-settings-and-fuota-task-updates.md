Issue-Source: trace-sweep-divergence
Issue-Trace: iot-wireless-fuota-task-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-wireless-fuota-task-lifecycle
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless|GetFuotaTask|2xx->2xx|$.FragmentIntervalMS
Issue-Key: iotwireless|GetFuotaTask|2xx->2xx|$.FragmentSizeBytes
Issue-Key: iotwireless|GetFuotaTask|2xx->2xx|$.ProtocolType
Issue-Key: iotwireless|GetFuotaTask|2xx->2xx|$.RedundancyPercent
Issue-Key: iotwireless|GetFuotaTask|2xx->2xx|$.Name
Issue-Key: iotwireless|GetFuotaTask|4xx->2xx|$.Message
Issue-Key: iotwireless|GetFuotaTask|4xx->2xx|$.ResourceId
Issue-Key: iotwireless|GetFuotaTask|4xx->2xx|$.ResourceType
Issue-Key: iotwireless|GetFuotaTask|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iotwireless|GetFuotaTask|4xx->2xx|status
Issue-Commit: dc856e880220c47012eb41a5b29cc1ddaf120576

# GetFuotaTask drops fragmentation settings, and FUOTA task updates and deletes never apply

> _Note: this issue was identified against the code base at commit `dc856e880220c47012eb41a5b29cc1ddaf120576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFuotaTask

## Description
Three distinct causes in the iotwireless FUOTA-task lifecycle. A fixer must close ALL THREE before submitting; each names its own fixture and step.

Cause 1 of 3: GetFuotaTask reports no fragmentation settings for a freshly created task (fixture iot-wireless-fuota-task-lifecycle, step task#3).
AWS: Smithy model services/service-com-amazonaws-iotwireless-iotwireless/smithy-model/com-amazonaws-iotwireless-iotwireless.json — GetFuotaTask (GET /fuota-tasks/{Id}, 200) output shape GetFuotaTaskResponse models RedundancyPercent (@range 0..100), FragmentSizeBytes (@range min 1) and FragmentIntervalMS (@range min 1); the same three are optional members of CreateFuotaTaskRequest. The model declares no @default for them, so the fact that AWS reports values for a task whose create did not pin them is recorded behavior only: the recording shows GetFuotaTask answering 200 with RedundancyPercent, FragmentSizeBytes and FragmentIntervalMS all populated (the service fills its own fragmentation defaults at create time) plus a top-level ProtocolType of 'LoRaWAN'. ProtocolType is recorded behavior only in a second sense: the Smithy model in this checkout is an older API revision whose GetFuotaTaskResponse has no ProtocolType member at all, so the output shape itself cannot carry it.
Emulator: answers 200 but omits all four keys — RedundancyPercent, FragmentSizeBytes, FragmentIntervalMS and ProtocolType are absent from the JSON body rather than present with the task's effective values.
Why: a client that reads back a task to learn how its firmware image will be fragmented (fragment size, send interval, redundancy, protocol) gets nulls, so it cannot report or validate the task's transfer plan and any round-trip (read-modify-write) of a task silently erases settings AWS had applied.
Fix: CreateFuotaTask's handler/state must apply the service's fragmentation defaults when the request omits RedundancyPercent/FragmentSizeBytes/FragmentIntervalMS instead of storing none, and GetFuotaTask's serializer must emit the task's protocol type; carrying ProtocolType on the wire also requires the member to exist on the response shape.
Traces: iot-wireless-fuota-task-lifecycle#task#3.

Cause 2 of 3: UpdateFuotaTask never applies — a renamed task still reads back under its old name (fixture iot-wireless-fuota-task-lifecycle, step task#6).
AWS: Smithy: UpdateFuotaTask (PATCH /fuota-tasks/{Id}, success 204) takes Name, Description, LoRaWAN, FirmwareUpdateImage, FirmwareUpdateRole, RedundancyPercent, FragmentSizeBytes, FragmentIntervalMS and Descriptor, and a following GetFuotaTask must report the updated values; the recording shows Name reading back as the updated name after the patch.
Emulator: GetFuotaTask after the update answers 200 with the pre-update Name, and still omits FragmentIntervalMS, FragmentSizeBytes, RedundancyPercent and ProtocolType as in cause 1. The underlying handler services/service-com-amazonaws-iotwireless-iotwireless/src/ops/update_fuota_task.rs is still the scaffold: its handle body returns Scaffolded::not_implemented("UpdateFuotaTask"), so no mutation ever reaches the store.
Why: a client that renames or re-parameterizes a FUOTA task sees the call accepted-looking yet observes the old state forever, so any workflow that configures a task in two steps (create, then update) runs against stale configuration.
Fix: implement UpdateFuotaTask's handler to mutate the stored task in the iotwireless state (and to refuse an unknown Id with the modeled ResourceNotFoundException), so GetFuotaTask reflects the patch.
Traces: iot-wireless-fuota-task-lifecycle#task#6.

Cause 3 of 3: a deleted FUOTA task stays readable — GetFuotaTask answers 200 instead of the modeled 404 (fixture iot-wireless-fuota-task-lifecycle, step task#9).
AWS: Smithy: DeleteFuotaTask (DELETE /fuota-tasks/{Id}, success 204) removes the task; GetFuotaTask models ResourceNotFoundException with httpError 404, whose members are Message, ResourceId ("Id of the not found resource") and ResourceType. The recording shows the post-delete read answering HTTP 404 with error type ResourceNotFoundException (header x-amzn-errortype), ResourceType "FuotaTask", ResourceId equal to the deleted task's id, and Message "Cannot complete the step because resource is not found, with resource type FuotaTask, resource Id <Id>". In that message the fixed text is "Cannot complete the step because resource is not found, with resource type " + <ResourceType> + ", resource Id " + <ResourceId>; the only variable parts are the resource type ("FuotaTask" for this resource) and the id taken from the request's Id label — the id in the recording is one run's value and is not part of the template.
Emulator: answers HTTP 200 with the task body still intact — no status change, no Message, no ResourceId, no ResourceType and no x-amzn-errortype header. services/service-com-amazonaws-iotwireless-iotwireless/src/ops/delete_fuota_task.rs is still the scaffold returning Scaffolded::not_implemented("DeleteFuotaTask"), so the task is never removed from the store; GetFuotaTask itself already has the right refusal path (it maps a missing key to ResourceNotFoundException), it is simply never taken.
Why: a client cannot tell a live FUOTA task from a deleted one. Cleanup loops that poll for 404 never terminate, and tests that assert a resource is gone pass against a resource that still exists, so state leaks across runs.
Fix: implement DeleteFuotaTask's handler to remove the task from the iotwireless state (and to refuse an unknown Id with ResourceNotFoundException), so a subsequent GetFuotaTask takes its existing 404 path and the message/ResourceId/ResourceType template above is produced from the request and state.
Traces: iot-wireless-fuota-task-lifecycle#task#9.

Related but NOT covering these: open issues 20261030T072116Z-iotwireless-getdeviceprofile-drops-lorawan-configuration-and-ignores-deletion.md, 20261030T115132Z-iotwireless-getmulticastgroup-drops-group-state-and-ignores-deletion.md, 20261103T042841Z-iotwireless-getwirelessgateway-drops-beaconing-ignores-renames-and-deletion.md and 20261104T232607Z-iotwireless-wirelessgatewaytaskdefinition-lifecycle-unenforced-missing-update-accepted-and-deleted.md describe the same shape of fault on the DeviceProfile, MulticastGroup, WirelessGateway and WirelessGatewayTaskDefinition resources; none of them touches the FuotaTask resource or these fixture steps.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false), fixture iot-wireless-fuota-task-lifecycle:

Cause 1 — step task#3, GetFuotaTask, expected 200 / actual 200:
  $.FragmentIntervalMS: missing (expected 1000, got None)
  $.FragmentSizeBytes: missing (expected 50, got None)
  $.ProtocolType: missing (expected 'LoRaWAN', got None)
  $.RedundancyPercent: missing (expected 50, got None)

Cause 2 — step task#6, GetFuotaTask after UpdateFuotaTask, expected 200 / actual 200:
  $.Name: value_mismatch (expected '<task-name>-renamed', got '<task-name>' — the pre-update name)
  $.FragmentIntervalMS / $.FragmentSizeBytes / $.ProtocolType / $.RedundancyPercent: missing, as in cause 1

Cause 3 — step task#9, GetFuotaTask after DeleteFuotaTask:
  status: expected 404, got 200
  $.Message: missing (expected 'Cannot complete the step because resource is not found, with resource type FuotaTask, resource Id <Id>', got None)
  $.ResourceId: missing (expected the deleted task's Id, got None)
  $.ResourceType: missing (expected 'FuotaTask', got None)
  $headers.x-amzn-errortype: missing (expected the ResourceNotFoundException error type, got None)

Code evidence: src/ops/update_fuota_task.rs and src/ops/delete_fuota_task.rs both end their handle bodies with Scaffolded::not_implemented(...), while src/ops/get_fuota_task.rs already maps an absent store key to ResourceNotFoundException via crate::state::fuota_task_not_found and already serializes redundancy_percent/fragment_size_bytes/fragment_interval_ms straight from the stored task; src/ops/create_fuota_task.rs copies those three members from the request with no default applied.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture iot-wireless-fuota-task-lifecycle from trace set simparity-wire-aws-v1.
2. Cause 1: CreateFuotaTask with FirmwareUpdateImage and FirmwareUpdateRole only (no RedundancyPercent/FragmentSizeBytes/FragmentIntervalMS), then GET /fuota-tasks/{Id} (step task#3) — observe the 200 body carries no RedundancyPercent, FragmentSizeBytes, FragmentIntervalMS or ProtocolType.
3. Cause 2: PATCH /fuota-tasks/{Id} with a new Name, then GET /fuota-tasks/{Id} (step task#6) — observe Name is still the pre-update value.
4. Cause 3: DELETE /fuota-tasks/{Id}, then GET /fuota-tasks/{Id} (step task#9) — observe HTTP 200 with the task body instead of HTTP 404 ResourceNotFoundException carrying Message/ResourceId/ResourceType and the x-amzn-errortype header.
