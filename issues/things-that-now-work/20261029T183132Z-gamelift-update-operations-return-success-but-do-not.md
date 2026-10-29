Issue-Source: trace-sweep-divergence
Issue-Trace: gamelift-alias-lifecycle-terminal-routing
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gamelift-alias-lifecycle-terminal-routing gamelift-build-lifecycle-and-tags gamelift-matchmaking-configuration-lifecycle gamelift-script-lifecycle
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|DescribeAlias|2xx->2xx|$.Alias.Description
Issue-Key: gamelift|DescribeAlias|2xx->2xx|$.Alias.RoutingStrategy.Message
Issue-Key: gamelift|DescribeBuild|2xx->2xx|$.Build.Version
Issue-Key: gamelift|DescribeMatchmakingConfigurations|2xx->2xx|$.Configurations[Name="sim2real-probe-gamelift-mmconfig-116e38f0"].AcceptanceRequired
Issue-Key: gamelift|DescribeMatchmakingConfigurations|2xx->2xx|$.Configurations[Name="sim2real-probe-gamelift-mmconfig-116e38f0"].AcceptanceTimeoutSeconds
Issue-Key: gamelift|DescribeMatchmakingConfigurations|2xx->2xx|$.Configurations[Name="sim2real-probe-gamelift-mmconfig-116e38f0"].AdditionalPlayerCount
Issue-Key: gamelift|DescribeMatchmakingConfigurations|2xx->2xx|$.Configurations[Name="sim2real-probe-gamelift-mmconfig-116e38f0"].BackfillMode
Issue-Key: gamelift|DescribeMatchmakingConfigurations|2xx->2xx|$.Configurations[Name="sim2real-probe-gamelift-mmconfig-116e38f0"].Description
Issue-Key: gamelift|DescribeMatchmakingConfigurations|2xx->2xx|$.Configurations[Name="sim2real-probe-gamelift-mmconfig-116e38f0"].RequestTimeoutSeconds
Issue-Key: gamelift|DescribeScript|2xx->2xx|$.Script.StorageLocation.Bucket
Issue-Key: gamelift|DescribeScript|2xx->2xx|$.Script.StorageLocation.Key
Issue-Key: gamelift|DescribeScript|2xx->2xx|$.Script.Version
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# GameLift Update* operations return success but do not persist changes

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlias

## Description
Four independent causes, one shared fault: GameLift Update* operations are accepted (HTTP 200) but the mutated fields are not written to the emulator's state, so the following Describe* returns the pre-update values. A fixer must close all FOUR before submitting.

Cause 1 of 4 - UpdateAlias not persisted.
AWS: UpdateAlias replaces the alias's Description and RoutingStrategy, and the next DescribeAlias returns the updated Alias. Smithy: UpdateAlias input members Description (NonZeroAndMaxString, length 1-1024) and RoutingStrategy, output Alias; DescribeAlias returns the same Alias shape.
Emulator: DescribeAlias after UpdateAlias returns the values supplied at CreateAlias - Alias.Description is still the creation description and Alias.RoutingStrategy.Message is still the creation routing message, not the updated ones.
Why: a client that updates a TERMINAL alias's routing message (the text players are shown) and reads it back sees the stale text, so configuration changes silently do not take effect.
Fix: the UpdateAlias handler's state mutation - the updated Description and RoutingStrategy (including RoutingStrategy.Message) must be written back to the stored alias that DescribeAlias reads.
Traces: gamelift-alias-lifecycle-terminal-routing#task#4.

Cause 2 of 4 - UpdateBuild Version not persisted.
AWS: UpdateBuild sets Build.Version and DescribeBuild returns the new value. Smithy: Build.Version / UpdateBuild input Version (NonZeroAndMaxString).
Emulator: DescribeBuild returns the Version supplied at CreateBuild, not the value passed to UpdateBuild.
Why: build-version bookkeeping (promotion, fleet selection by version) reads stale data.
Fix: the UpdateBuild handler's state mutation for Name/Version.
Traces: gamelift-build-lifecycle-and-tags#task#4.

Cause 3 of 4 - UpdateMatchmakingConfiguration not persisted.
AWS: UpdateMatchmakingConfiguration applies Description, RequestTimeoutSeconds, AcceptanceRequired and AcceptanceTimeoutSeconds, and DescribeMatchmakingConfigurations returns the updated configuration. Smithy: all four are UpdateMatchmakingConfiguration input members (RequestTimeoutSeconds range 1-43200, AcceptanceTimeoutSeconds range 1-600, AcceptanceRequired BooleanModel).
Emulator: the describe response still carries the creation-time values - Description unchanged, RequestTimeoutSeconds unchanged, AcceptanceRequired still false although the update set it true, and AcceptanceTimeoutSeconds absent although the update supplied one (when AcceptanceRequired is turned on, AcceptanceTimeoutSeconds must be present in the configuration).
Why: FlexMatch behaviour (whether players must accept a match, and how long they have) is read from this configuration; the emulator reports a configuration the client did not ask for.
Fix: the UpdateMatchmakingConfiguration handler's state mutation, which must merge every supplied member into the stored MatchmakingConfiguration.
Traces: gamelift-matchmaking-configuration-lifecycle#task#5.

Cause 4 of 4 - UpdateScript Version not persisted.
AWS: UpdateScript sets Script.Version and DescribeScript returns the new value. Smithy: UpdateScript input Version (NonZeroAndMaxString, length 1-1024), output Script.
Emulator: DescribeScript returns the creation Version after a successful UpdateScript.
Why: Realtime script versioning is unobservable, so clients cannot verify a script rollout.
Fix: the UpdateScript handler's state mutation for Name/Version/StorageLocation. (The same step also reports a wrong Script.StorageLocation - that aspect belongs to the separate StorageLocation issue, not to this cause.)
Traces: gamelift-script-lifecycle#task#4.

## Evidence
Cause 1: gamelift-alias-lifecycle-terminal-routing#task#4 DescribeAlias 200/200 - $.Alias.Description expected the updated description, got the creation description; $.Alias.RoutingStrategy.Message expected the updated routing message, got the creation routing message.
Cause 2: gamelift-build-lifecycle-and-tags#task#4 DescribeBuild 200/200 - $.Build.Version expected the updated version ('1.0.1' in that recording), got the creation version ('1.0.0').
Cause 3: gamelift-matchmaking-configuration-lifecycle#task#5 DescribeMatchmakingConfigurations 200/200 - AcceptanceRequired expected true, got false; AcceptanceTimeoutSeconds expected present, got missing; Description expected the updated description, got the creation one; RequestTimeoutSeconds expected the updated value, got the creation value; AdditionalPlayerCount/BackfillMode missing (that omission is the separate defaults issue).
Cause 4: gamelift-script-lifecycle#task#4 DescribeScript 200/200 - $.Script.Version expected the updated version, got the creation version.

## Steps to Reproduce
1. Replay the wire corpus (simparity-wire-aws-v1).
2. For each cause: create the resource, call the Update* operation with changed fields, then call the matching Describe*.
3. Observe HTTP 200 whose body still carries the creation-time field values.
4. Re-replay gamelift-alias-lifecycle-terminal-routing#task#4 and every fixture#step in trace_ids to confirm all four causes are closed.
