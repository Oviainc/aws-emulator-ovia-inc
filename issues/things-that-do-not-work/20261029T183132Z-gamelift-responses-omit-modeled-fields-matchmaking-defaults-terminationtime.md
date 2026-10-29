Issue-Source: trace-sweep-divergence
Issue-Trace: gamelift-matchmaking-configuration-lifecycle
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gamelift-matchmaking-configuration-lifecycle gamelift-ruleset-in-use-cannot-be-deleted gamelift-start-match-backfill-without-live-session gamelift-anywhere-fleet-lifecycle gamelift-catalog-list-locations gamelift-create-alias-simple-missing-fleet-id-rejected
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|CreateMatchmakingConfiguration|2xx->2xx|$.Configuration.AdditionalPlayerCount
Issue-Key: gamelift|CreateMatchmakingConfiguration|2xx->2xx|$.Configuration.BackfillMode
Issue-Key: gamelift|DescribeFleetAttributes|2xx->2xx|$.FleetAttributes[Name="sim2real-probe-gamelift-fleet-1c8e8527"].TerminationTime
Issue-Key: gamelift|CreateAlias|2xx->2xx|$.Alias.AliasArn
Issue-Related: 20261029T183132Z-gamelift-delete-operations-do-not-remove-state-resources.md
Issue-Related: 20261029T183132Z-gamelift-update-operations-return-success-but-do-not.md
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# GameLift responses omit modeled fields: matchmaking defaults, TerminationTime, managed locations

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMatchmakingConfiguration

## Description
Five independent causes, one shared fault: the emulator serves 200 responses that leave out members AWS populates (service-applied defaults, lifecycle timestamps, the service's own catalog) or populates one in the wrong form. A fixer must close all FIVE before submitting.

Cause 1 of 5 - CreateMatchmakingConfiguration omits service-applied defaults.
AWS: when the request does not supply them, AWS still returns Configuration.AdditionalPlayerCount = 0 and Configuration.BackfillMode = "MANUAL" in the created configuration. Smithy: MatchmakingConfiguration.AdditionalPlayerCount (WholeNumber, range min 0) and BackfillMode (enum; its documentation defines MANUAL as "the game makes backfill requests or does not use the match backfill feature", the behaviour of a configuration that asked for nothing); neither member is required on input, so the service supplies the default.
Emulator: both members are absent from the response.
Why: a client that reads BackfillMode to decide whether to issue StartMatchBackfill itself, or AdditionalPlayerCount to size a match, gets null and must guess; round-tripping the configuration back into UpdateMatchmakingConfiguration then drops the setting.
Fix: the CreateMatchmakingConfiguration handler's defaulting (apply AdditionalPlayerCount=0 and BackfillMode=MANUAL when the input omits them) before the configuration is stored.
Traces: gamelift-matchmaking-configuration-lifecycle#setup#2 (also gamelift-ruleset-in-use-cannot-be-deleted#task#2, gamelift-start-match-backfill-without-live-session#setup#2).

Cause 2 of 5 - DescribeMatchmakingConfigurations omits the same defaults.
AWS: the describe response carries AdditionalPlayerCount = 0 and BackfillMode = "MANUAL" for such a configuration.
Emulator: both absent.
Why: the gap is in stored state, not only in the create serializer - every later read is incomplete too.
Fix: the defaults must be persisted by create (cause 1) and serialized by the describe handler.
Traces: gamelift-matchmaking-configuration-lifecycle#task#3.

Cause 3 of 5 - DescribeFleetAttributes omits TerminationTime for a deleted fleet.
AWS: once a fleet has been deleted/terminated, its FleetAttributes carries TerminationTime, a Unix-time timestamp recording when the fleet was terminated (the recording asserts the member's presence and shape, not a particular clock value). Smithy: FleetAttributes.TerminationTime (Timestamp) - "A time stamp indicating when this data object was terminated."
Emulator: TerminationTime is missing from the FleetAttributes entry.
Why: a client that distinguishes a live fleet from a terminated one by the presence of TerminationTime (or that reports when a fleet went away) cannot.
Fix: the fleet state transition to DELETING/TERMINATED must stamp TerminationTime, and DescribeFleetAttributes must serialize it.
Traces: gamelift-anywhere-fleet-lifecycle#task#3 (also #task#5).

Cause 4 of 5 - ListLocations returns only the local region, not the GameLift location catalog.
AWS: ListLocations with no filters returns the full set of GameLift-supported locations for the account - AWS regions plus local zones (e.g. ap-south-1, us-west-1, us-east-1-mia-2, us-east-1-lim-1, ap-southeast-2, us-east-1-chi-2, us-west-2-phx-2, us-east-1-iah-1, eu-central-1, us-east-1-mci-1, ap-southeast-5, ... 46 entries in the recording) - and each managed location carries PingBeacon.UDPEndpoint with a Domain and Port (the recorded beacons use the gamelift-ping.<region>.api.aws / gamelift-ping-<zone>.<region>.api.aws form on port 7770). Smithy: ListLocations output Locations: LocationModelList, paginated (Limit range 1-200, NextToken); LocationModel members LocationName, LocationArn (custom locations only) and PingBeacon.
Emulator: returns a single element {"LocationName": "us-west-2"} - one managed location, the emulator's own region, with no PingBeacon at all.
Why: a client that enumerates locations to build a multi-location fleet, or reads PingBeacon to measure player latency, has nothing to work with; the exact catalog contents are environment data, but a managed-location list of one entry with no ping beacon is not AWS's shape.
Fix: the ListLocations handler's catalog source (serve the GameLift managed-location set with PingBeacon.UDPEndpoint populated, plus the account's custom locations) and its pagination.
Traces: gamelift-catalog-list-locations#task#1.

Cause 5 of 5 - CreateAlias returns an Alias.AliasArn that fails the recorded ARN shape.
AWS: CreateAlias returns Alias.AliasArn in the GameLift alias ARN form; the model documents it as arn:aws:gamelift:<region>::alias/alias-<uuid>, i.e. partition and service literal, the request's region, the account-id segment EMPTY, and a resource id that matches the AliasId value exactly. Smithy: Alias.AliasArn, pattern ^(alias-\S+|arn:.*:alias\/alias-\S+)$ with that documented format. The recording asserts this member by shape only.
Emulator: emits an ARN with a populated account-id segment (arn:aws:gamelift:<region>:<accountId>:alias/alias-<uuid>), which does not match the asserted shape. Hypothesis: the ARN builder is the generic account-qualified one rather than the GameLift alias form.
Why: clients that parse the alias ARN (to recover the region/resource id, or to compare an ARN they were given) mis-segment it.
Fix: the CreateAlias handler's ARN construction for Alias.AliasArn. The region and the uuid itself are per-environment values and are not the requirement; the segment layout is.
Traces: gamelift-create-alias-simple-missing-fleet-id-rejected#task#3.

## Evidence
Cause 1: gamelift-matchmaking-configuration-lifecycle#setup#2 CreateMatchmakingConfiguration 200/200 - $.Configuration.AdditionalPlayerCount expected 0, got missing; $.Configuration.BackfillMode expected 'MANUAL', got missing.
Cause 2: gamelift-matchmaking-configuration-lifecycle#task#3 DescribeMatchmakingConfigurations 200/200 - same two members missing on the configuration entry.
Cause 3: gamelift-anywhere-fleet-lifecycle#task#3 DescribeFleetAttributes 200/200 - $.FleetAttributes[...].TerminationTime expected present (shape-asserted), got missing.
Cause 4: gamelift-catalog-list-locations#task#1 ListLocations 200/200 - expected 46 Locations entries each with PingBeacon.UDPEndpoint {Domain, Port 7770}; got exactly one entry {'LocationName': 'us-west-2'} and no PingBeacon ($.Locations[0] through $.Locations[45] reported missing/mismatched).
Cause 5: gamelift-create-alias-simple-missing-fleet-id-rejected#task#3 CreateAlias 200/200 - $.Alias.AliasArn shape-asserted and not matched; emulator returned an account-qualified alias ARN.

## Steps to Reproduce
1. Replay the wire corpus (simparity-wire-aws-v1).
2. Cause 1/2: CreateMatchmakingConfiguration without AdditionalPlayerCount or BackfillMode, then DescribeMatchmakingConfigurations by that name; check both members.
3. Cause 3: create an Anywhere fleet, DeleteFleet, then DescribeFleetAttributes and check TerminationTime.
4. Cause 4: ListLocations with no filters and count the managed locations and their PingBeacon members.
5. Cause 5: CreateAlias and inspect the segment layout of Alias.AliasArn.
6. Re-replay gamelift-matchmaking-configuration-lifecycle#setup#2 and every fixture#step in trace_ids to confirm all five causes are closed.
