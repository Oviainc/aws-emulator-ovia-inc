Issue-Source: trace-sweep-divergence
Issue-Trace: gamelift-anywhere-fleet-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gamelift-anywhere-fleet-lifecycle gamelift-compute-registration-and-access gamelift-ec2-only-fleet-ops-against-anywhere-fleet gamelift-ec2-only-ops-against-anywhere-fleet gamelift-fleet-locations-management gamelift-multi-compute-isolation gamelift-search-and-filter-game-sessions gamelift-fleet-events-and-ec2-limits gamelift-game-session-lifecycle gamelift-game-session-queue-lifecycle
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|CreateFleet|2xx->2xx|$.FleetAttributes.CertificateConfiguration
Issue-Key: gamelift|CreateFleet|2xx->2xx|$.FleetAttributes.MetricGroups[*]
Issue-Key: gamelift|CreateFleet|2xx->2xx|$.FleetAttributes.PlayerGatewayMode
Issue-Key: gamelift|DescribeFleetAttributes|2xx->2xx|$.FleetAttributes[Name="sim2real-probe-gamelift-fleet-1c8e8527"].CertificateConfiguration
Issue-Key: gamelift|DescribeFleetAttributes|2xx->2xx|$.FleetAttributes[Name="sim2real-probe-gamelift-fleet-1c8e8527"].FleetArn
Issue-Key: gamelift|DescribeFleetAttributes|2xx->2xx|$.FleetAttributes[Name="sim2real-probe-gamelift-fleet-1c8e8527"].MetricGroups[*]
Issue-Key: gamelift|DescribeFleetAttributes|2xx->2xx|$.FleetAttributes[Name="sim2real-probe-gamelift-fleet-1c8e8527"].PlayerGatewayMode
Issue-Key: gamelift|DescribeFleetAttributes|2xx->2xx|$.FleetAttributes[Name="sim2real-probe-gamelift-fleet-1c8e8527"].TerminationTime
Issue-Key: gamelift|CreateFleet|2xx->2xx|$.FleetAttributes.FleetArn
Issue-Key: gamelift|DescribeFleetAttributes|2xx->2xx|$.FleetAttributes[Name="sim2real-probe-gamelift-fleet-1c8e8527"].Description
Issue-Key: gamelift|DescribeFleetAttributes|2xx->2xx|$.FleetAttributes[Name="sim2real-probe-gamelift-fleet-1c8e8527"].NewGameSessionProtectionPolicy
Issue-Key: gamelift|DescribeFleetAttributes|2xx->2xx|$.FleetAttributes[*]
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# GameLift fleet attribute responses omit modeled fields and persisted updates

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFleetAttributes

## Description
The emulator's FleetAttributes projection is incomplete and the fleet record it serves does not track mutation. Five distinct causes, all in the gamelift fleet handlers/state. FIVE CAUSES MUST BE FIXED.

Cause 1 of 5 — CreateFleet response omits modeled FleetAttributes members.
AWS: Smithy (services/service-com-amazonaws-gamelift-gamelift) CreateFleet -> CreateFleetOutput.FleetAttributes : FleetAttributes. FleetAttributes models CertificateConfiguration (structure CertificateConfiguration with required member CertificateType, enum CertificateType), MetricGroups (MetricGroupList, @length min 0 max 1) and PlayerGatewayMode (enum PlayerGatewayMode, values DISABLED|ENABLED|REQUIRED). The recording shows a newly created Anywhere fleet carrying CertificateConfiguration={"CertificateType":"DISABLED"}, MetricGroups=["default"] and PlayerGatewayMode="DISABLED" even when the request did not set them — i.e. AWS populates these defaults server-side. Status 200.
Emulator: status 200, but all three members are absent from the serialized FleetAttributes (replay: "$.FleetAttributes.CertificateConfiguration: missing", "$.FleetAttributes.MetricGroups[0]: missing", "$.FleetAttributes.PlayerGatewayMode: missing"). This cluster covers 7 fixtures.
Why: a client that reads fleet.CertificateConfiguration.CertificateType to decide whether to use TLS, or reads PlayerGatewayMode to choose a connection path, dereferences a null; a client that reads the fleet's metric group gets an empty list and cannot resolve the CloudWatch group a real fleet is in.
Fix: the CreateFleet handler's fleet-record construction (server-side defaults for CertificateConfiguration, MetricGroups and PlayerGatewayMode) and the FleetAttributes serializer.
Traces: gamelift-anywhere-fleet-lifecycle#task#2 (representative; also gamelift-compute-registration-and-access#setup#2, gamelift-ec2-only-fleet-ops-against-anywhere-fleet#setup#2, gamelift-ec2-only-ops-against-anywhere-fleet#setup#2, gamelift-fleet-locations-management#setup#3, gamelift-multi-compute-isolation#setup#2, gamelift-search-and-filter-game-sessions#setup#2).

Cause 2 of 5 — DescribeFleetAttributes omits the same members plus TerminationTime.
AWS: Smithy DescribeFleetAttributes -> DescribeFleetAttributesOutput.FleetAttributes : FleetAttributesList of FleetAttributes; FleetAttributes.TerminationTime : Timestamp ("a time stamp indicating when this data object was terminated"). The recording shows the described fleet carrying CertificateConfiguration, MetricGroups=["default"], PlayerGatewayMode and a TerminationTime value (shape asserted, value not). Status 200.
Emulator: status 200 with CertificateConfiguration, MetricGroups[0], PlayerGatewayMode and TerminationTime all missing on the described fleet.
Why: the describe path returns a differently-shaped fleet than the create path, so a client that round-trips create -> describe loses the same fields again, and anything that keys off TerminationTime to detect a terminating fleet never sees one.
Fix: the DescribeFleetAttributes handler and the shared FleetAttributes serializer (same projection as cause 1, plus TerminationTime from fleet state).
Traces: gamelift-anywhere-fleet-lifecycle#task#3.

Cause 3 of 5 — fleet ARNs are emitted without the account-id segment.
AWS: Smithy FleetAttributes.FleetArn : FleetArn, @pattern ^arn:.*:[a-z]*fleet\/[a-z]*fleet-[a-zA-Z0-9\-]+$. The recording shows the fleet ARN carrying the account id of the calling account, e.g. arn:aws:gamelift:us-west-2:000000000000:fleet/fleet-<id> — the fifth ARN segment is populated. (The model's prose example renders the account segment empty; the recorded wire response is the authority here and it is populated, so this is recorded behavior.)
Emulator: emits arn:aws:gamelift:us-west-2::fleet/fleet-<id> — the account segment is empty. Replay: "$.FleetAttributes.FleetArn: value_mismatch (expected 'arn:aws:gamelift:us-west-2:000000000000:fleet/...', got 'arn:aws:gamelift:us-west-2::fleet/...')". Same defect on the describe path (gamelift-anywhere-fleet-lifecycle#task#3/#task#5) and on the game-session ARNs built from it.
Why: a client that splits the ARN on ':' to recover the owning account reads an empty string, and any ARN it re-submits or compares against an IAM resource pattern that expects an account will not match. The specific account digits are environment-dependent; the requirement is that the segment is populated with the caller's account, not the literal recorded value.
Fix: the gamelift ARN construction helper used by the fleet (and game-session) serializers, which must interpolate the configured account id.
Traces: gamelift-fleet-events-and-ec2-limits#setup#2 (representative; also gamelift-game-session-lifecycle#task#2, gamelift-game-session-queue-lifecycle#setup#2).

Cause 4 of 5 — UpdateFleetAttributes mutations are not reflected in fleet state.
AWS: Smithy UpdateFleetAttributes accepts Description (NonZeroAndMaxString) and NewGameSessionProtectionPolicy (enum ProtectionPolicy, NoProtection|FullProtection) and returns FleetId/FleetArn; the subsequent DescribeFleetAttributes in the recording returns Description='updated by probe' and NewGameSessionProtectionPolicy='FullProtection', i.e. the update is persisted on the fleet record. Status 200.
Emulator: the update call succeeds but the following DescribeFleetAttributes still returns the pre-update record — Description missing entirely and NewGameSessionProtectionPolicy still 'NoProtection' (replay: "Description: missing (expected 'updated by probe')", "NewGameSessionProtectionPolicy: value_mismatch (expected 'FullProtection', got 'NoProtection')").
Why: fleet reconfiguration is silently a no-op, so a client that sets FullProtection to stop scale-down from killing active sessions believes protection is on while the emulator still reports and behaves as NoProtection.
Fix: the UpdateFleetAttributes handler's write into fleet state (and the describe read path that must serve it).
Traces: gamelift-anywhere-fleet-lifecycle#task#5.

Cause 5 of 5 — a deleted fleet is still returned by DescribeFleetAttributes.
AWS: Smithy DeleteFleet (errors include NotFoundException, InvalidFleetStatusException) removes the fleet; the recording's teardown DescribeFleetAttributes returns an empty FleetAttributes list after the delete. Status 200 with no fleet entries.
Emulator: status 200 but FleetAttributes still contains the fleet, Status='DELETING' (replay: "$.FleetAttributes[0]: extra (expected None, got {... 'Status': 'DELETING' ...})").
Why: a client that polls describe to confirm teardown never observes the fleet disappear, so cleanup loops hang and a re-create that checks for name/id absence sees a phantom fleet.
Fix: the DeleteFleet handler's fleet-state removal (or the describe filter that must exclude fully deleted fleets once the DELETING transition completes).
Traces: gamelift-anywhere-fleet-lifecycle#teardown#9.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, state_reset_failures=0).
Cause 1 — gamelift-anywhere-fleet-lifecycle#task#2, CreateFleet, expected 200 / got 200: $.FleetAttributes.CertificateConfiguration missing (expected {'CertificateType': 'DISABLED'}); $.FleetAttributes.MetricGroups[0] missing (expected 'default'); $.FleetAttributes.PlayerGatewayMode missing (expected 'DISABLED'). 7 fixtures affected.
Cause 2 — gamelift-anywhere-fleet-lifecycle#task#3, DescribeFleetAttributes, 200/200: CertificateConfiguration, MetricGroups[0], PlayerGatewayMode, TerminationTime all missing on FleetAttributes[Name="sim2real-probe-gamelift-fleet-1c8e8527"].
Cause 3 — gamelift-fleet-events-and-ec2-limits#setup#2, CreateFleet, 200/200: FleetArn expected 'arn:aws:gamelift:us-west-2:000000000000:fleet/...', got 'arn:aws:gamelift:us-west-2::fleet/...' (account segment empty). 3 fixtures affected.
Cause 4 — gamelift-anywhere-fleet-lifecycle#task#5, DescribeFleetAttributes, 200/200: Description missing (expected 'updated by probe'); NewGameSessionProtectionPolicy expected 'FullProtection', got 'NoProtection'.
Cause 5 — gamelift-anywhere-fleet-lifecycle#teardown#9, DescribeFleetAttributes, 200/200: FleetAttributes[0] extra, got the deleted fleet with Status 'DELETING' where AWS returns an empty list.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout.
2. Cause 1: fixture gamelift-anywhere-fleet-lifecycle, step task#2 — CreateFleet (ComputeType ANYWHERE); inspect FleetAttributes for CertificateConfiguration, MetricGroups and PlayerGatewayMode.
3. Cause 2: same fixture, step task#3 — DescribeFleetAttributes on that fleet; inspect the same members plus TerminationTime.
4. Cause 3: fixture gamelift-fleet-events-and-ec2-limits, step setup#2 — CreateFleet; inspect FleetAttributes.FleetArn for the account-id segment.
5. Cause 4: fixture gamelift-anywhere-fleet-lifecycle, step task#5 — UpdateFleetAttributes (Description, NewGameSessionProtectionPolicy=FullProtection) then DescribeFleetAttributes.
6. Cause 5: same fixture, step teardown#9 — DeleteFleet then DescribeFleetAttributes; the fleet must no longer be listed.
