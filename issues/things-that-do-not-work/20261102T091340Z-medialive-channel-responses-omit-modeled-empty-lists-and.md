Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-cmc-traj-attached-input-conflicts-and-delete-order
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-cmc-traj-attached-input-conflicts-and-delete-order medialive-cmc-traj-role-mistakes-and-idempotent-retry
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|DescribeChannel|2xx->2xx|$.destinations[*].logicalInterfaceNames
Issue-Key: medialive|DescribeChannel|2xx->2xx|$.destinations[*].mediaPackageSettings
Issue-Key: medialive|DescribeChannel|2xx->2xx|$.destinations[*].srtSettings
Issue-Key: medialive|DescribeChannel|2xx->2xx|$.encoderSettings.captionDescriptions
Issue-Key: medialive|DescribeChannel|2xx->2xx|$.encoderSettings.outputGroups[*].outputGroupSettings.hlsGroupSettings.adMarkers
Issue-Key: medialive|DescribeChannel|2xx->2xx|$.encoderSettings.outputGroups[*].outputGroupSettings.hlsGroupSettings.captionLanguageMappings
Issue-Key: medialive|DescribeChannel|2xx->2xx|$.encoderSettings.outputGroups[*].outputs[*].captionDescriptionNames
Issue-Key: medialive|DescribeChannel|2xx->2xx|$.inputAttachments[*].logicalInterfaceNames
Issue-Key: medialive|DescribeChannel|2xx->2xx|$.logLevel
Issue-Key: medialive|DescribeChannel|2xx->2xx|$.maintenanceStatus
Issue-Key: medialive|DescribeChannel|2xx->2xx|$.maintenanceWindow
Issue-Key: medialive|DeleteChannel|2xx->2xx|$.destinations[*].logicalInterfaceNames
Issue-Key: medialive|DeleteChannel|2xx->2xx|$.destinations[*].mediaPackageSettings
Issue-Key: medialive|DeleteChannel|2xx->2xx|$.destinations[*].srtSettings
Issue-Key: medialive|DeleteChannel|2xx->2xx|$.encoderSettings.captionDescriptions
Issue-Key: medialive|DeleteChannel|2xx->2xx|$.encoderSettings.outputGroups[*].outputGroupSettings.hlsGroupSettings.adMarkers
Issue-Key: medialive|DeleteChannel|2xx->2xx|$.encoderSettings.outputGroups[*].outputGroupSettings.hlsGroupSettings.captionLanguageMappings
Issue-Key: medialive|DeleteChannel|2xx->2xx|$.encoderSettings.outputGroups[*].outputs[*].captionDescriptionNames
Issue-Key: medialive|DeleteChannel|2xx->2xx|$.inputAttachments[*].logicalInterfaceNames
Issue-Key: medialive|DeleteChannel|2xx->2xx|$.logLevel
Issue-Key: medialive|DeleteChannel|2xx->2xx|$.maintenanceStatus
Issue-Key: medialive|DeleteChannel|2xx->2xx|$.maintenanceWindow
Issue-Key: medialive|CreateChannel|2xx->2xx|$.channel.destinations[*].logicalInterfaceNames
Issue-Key: medialive|CreateChannel|2xx->2xx|$.channel.destinations[*].mediaPackageSettings
Issue-Key: medialive|CreateChannel|2xx->2xx|$.channel.destinations[*].srtSettings
Issue-Key: medialive|CreateChannel|2xx->2xx|$.channel.egressEndpoints[*]
Issue-Key: medialive|CreateChannel|2xx->2xx|$.channel.encoderSettings.captionDescriptions
Issue-Key: medialive|CreateChannel|2xx->2xx|$.channel.encoderSettings.outputGroups[*].outputGroupSettings.hlsGroupSettings.adMarkers
Issue-Key: medialive|CreateChannel|2xx->2xx|$.channel.encoderSettings.outputGroups[*].outputGroupSettings.hlsGroupSettings.captionLanguageMappings
Issue-Key: medialive|CreateChannel|2xx->2xx|$.channel.encoderSettings.outputGroups[*].outputs[*].captionDescriptionNames
Issue-Key: medialive|CreateChannel|2xx->2xx|$.channel.inputAttachments[*].logicalInterfaceNames
Issue-Key: medialive|CreateChannel|2xx->2xx|$.channel.logLevel
Issue-Key: medialive|CreateChannel|2xx->2xx|$.channel.maintenanceStatus
Issue-Key: medialive|CreateChannel|2xx->2xx|$.channel.maintenanceWindow
Issue-Commit: 54f388b9499ec3dd7b64bc533fb6b62161b4e3d7

# medialive channel responses omit modeled empty lists and defaulted members

> _Note: this issue was identified against the code base at commit `54f388b9499ec3dd7b64bc533fb6b62161b4e3d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeChannel

## Description
Every MediaLive operation that serializes a Channel (DescribeChannel, DeleteChannel, CreateChannel) drops the members whose value is an empty list or a service-applied default, so a client sees `null`/absent where AWS sends `[]` or the default. This bundle has 4 causes; all four must be closed.

Cause 1 of 4 - DescribeChannel omits empty lists and defaulted scalars (fixture medialive-cmc-traj-attached-input-conflicts-and-delete-order, step task#7).
AWS: DescribeChannel (Smithy: DescribeChannel -> DescribeChannelResponse, GET /prod/channels/{ChannelId}, code 200) serializes the whole Channel shape. For a channel created without them AWS still emits the collection members as empty JSON arrays - inputAttachments[].logicalInterfaceNames, destinations[].srtSettings, destinations[].mediaPackageSettings, destinations[].logicalInterfaceNames, encoderSettings.captionDescriptions, encoderSettings.outputGroups[].outputGroupSettings.hlsGroupSettings.adMarkers, .captionLanguageMappings, encoderSettings.outputGroups[].outputs[].captionDescriptionNames - and emits the service-applied defaults logLevel (Smithy: DescribeChannel output member LogLevel -> enum LogLevel, jsonName "logLevel") with the value "DISABLED" when the request did not set one, plus the maintenance members (maintenanceWindow, a service-chosen window string, and maintenanceStatus, the empty string when no maintenance is scheduled).
Emulator: all of those members are absent from the body (replay: "$.inputAttachments[0].logicalInterfaceNames: missing (expected [], got None)", same for the other seven lists, "$.logLevel: missing (expected 'DISABLED', got None)", "$.maintenanceWindow: missing", "$.maintenanceStatus: missing (expected '', got None)"). Status is 200 on both sides, so only the body diverges.
Why: an SDK client that iterates channel.encoderSettings.captionDescriptions or channel.inputAttachments[i].logicalInterfaceNames gets a null collection instead of an empty one and either NPEs or has to special-case the emulator; code reading channel.logLevel to decide whether logging is on sees no value at all even though AWS guarantees DISABLED.
Fix: the Channel serializer shared by the channel handlers - empty collections must be emitted as `[]` rather than skipped, and the create path must persist the defaults (logLevel DISABLED, the maintenance window/status) so the describe path has them to emit.
Traces: medialive-cmc-traj-attached-input-conflicts-and-delete-order#task#7 (cluster also covers task#12, task#14 of the same fixture and task#10, teardown#14 of medialive-cmc-traj-role-mistakes-and-idempotent-retry).

Cause 2 of 4 - DeleteChannel's response body has the same omissions (fixture medialive-cmc-traj-attached-input-conflicts-and-delete-order, step task#11).
AWS: DeleteChannel (Smithy: DELETE /prod/channels/{ChannelId}, code 200) returns the same Channel projection as DescribeChannel - DeleteChannelResponse carries the identical member set - so the empty collections and defaults listed in cause 1 are present in the delete acknowledgement too.
Emulator: identical set of eleven missing members on the 200 body ("$.destinations[0].srtSettings: missing (expected [], got None)" ... "$.maintenanceStatus: missing (expected '', got None)").
Why: a client that reads the delete response as its last snapshot of the channel (common, since the channel then disappears) records a differently shaped object than AWS returned.
Fix: same Channel serializer, reached from the DeleteChannel handler.
Traces: medialive-cmc-traj-attached-input-conflicts-and-delete-order#task#11 (cluster also covers medialive-cmc-traj-role-mistakes-and-idempotent-retry#teardown#13).

Cause 3 of 4 - CreateChannel's 201 body omits the same members and additionally reports egress endpoints that do not exist yet (fixture medialive-cmc-traj-role-mistakes-and-idempotent-retry, step task#9).
AWS: CreateChannel (Smithy: POST /prod/channels, code 201, output member Channel jsonName "channel") returns the freshly created channel with the empty collections and the defaults of cause 1 under $.channel. At creation time the channel is still CREATING and its egressEndpoints list is empty - the source addresses are assigned later, so the create response carries no egressEndpoints entries.
Emulator: the thirteen members under $.channel are missing as in cause 1, and two egressEndpoints entries are present that AWS did not send ("$.channel.egressEndpoints[0]: extra (expected None, got {'sourceIp': ...})", likewise [1]). Status 201 on both sides.
Why: a client that waits for egressEndpoints to appear as its readiness signal treats a channel that has not been provisioned as ready, and the same null-collection problem as cause 1 applies to the create path.
Fix: the CreateChannel handler and the Channel serializer - emit empty collections, persist the defaults, and leave egressEndpoints empty until the channel leaves CREATING.
Traces: medialive-cmc-traj-role-mistakes-and-idempotent-retry#task#9.

Cause 4 of 4 - the second CreateChannel in the same fixture repeats the omissions (fixture medialive-cmc-traj-role-mistakes-and-idempotent-retry, step task#11).
AWS: as cause 3, for a channel whose egress endpoints are not asserted; the eleven empty-collection/default members are still present under $.channel.
Emulator: same eleven members missing on the 201 body.
Why: as cause 3 - the defect is in the create path, not in one request's shape, so every CreateChannel caller is affected.
Fix: as cause 3.
Traces: medialive-cmc-traj-role-mistakes-and-idempotent-retry#task#11.

## Evidence
Expected vs actual, from the replay of simparity-wire-aws-v1:

(1) DescribeChannel 200 vs 200, fixture medialive-cmc-traj-attached-input-conflicts-and-delete-order step task#7:
  $.inputAttachments[0].logicalInterfaceNames: missing (expected [], got None)
  $.destinations[0].srtSettings / .mediaPackageSettings / .logicalInterfaceNames: missing (expected [], got None)
  $.encoderSettings.captionDescriptions: missing (expected [], got None)
  $.encoderSettings.outputGroups[0].outputGroupSettings.hlsGroupSettings.adMarkers: missing (expected [], got None)
  $.encoderSettings.outputGroups[0].outputGroupSettings.hlsGroupSettings.captionLanguageMappings: missing (expected [], got None)
  $.encoderSettings.outputGroups[0].outputs[0].captionDescriptionNames: missing (expected [], got None)
  $.logLevel: missing (expected 'DISABLED', got None)
  $.maintenanceWindow: missing (expected a service-chosen window, got None)
  $.maintenanceStatus: missing (expected '', got None)

(2) DeleteChannel 200 vs 200, same fixture step task#11: identical eleven missing members.

(3) CreateChannel 201 vs 201, fixture medialive-cmc-traj-role-mistakes-and-idempotent-retry step task#9: same members missing under $.channel, plus
  $.channel.egressEndpoints[0]: extra (expected None, got {'sourceIp': '198.18.0.6'})
  $.channel.egressEndpoints[1]: extra (expected None, got {'sourceIp': '198.18.0.7'})
(the recorded sourceIp values are per-run and are not themselves the requirement; the requirement is that the list is empty at creation.)

(4) CreateChannel 201 vs 201, same fixture step task#11: same eleven members missing under $.channel.

Model references: Smithy CreateChannel POST /prod/channels code 201 output {Channel -> "channel"}; DescribeChannel GET /prod/channels/{ChannelId} code 200; DeleteChannel DELETE /prod/channels/{ChannelId} code 200, all three projecting the same Channel member set including LogLevel (enum LogLevel) and the list members named above.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: fixture medialive-cmc-traj-attached-input-conflicts-and-delete-order, step task#7 - create an input security group, an RTMP_PUSH input and a channel attaching it, then GET /prod/channels/{id}. Compare the JSON body against the recorded AWS body: the eight list members, logLevel, maintenanceWindow and maintenanceStatus are absent.
3. Cause 2: same fixture, step task#11 - DELETE /prod/channels/{id} and inspect the 200 body; the same eleven members are absent.
4. Cause 3: fixture medialive-cmc-traj-role-mistakes-and-idempotent-retry, step task#9 - POST /prod/channels and inspect $.channel in the 201 body: eleven members absent and two egressEndpoints entries present that AWS omits.
5. Cause 4: same fixture, step task#11 - POST /prod/channels again and inspect $.channel: the eleven members are absent.
