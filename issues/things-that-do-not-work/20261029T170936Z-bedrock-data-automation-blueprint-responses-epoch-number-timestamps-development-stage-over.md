Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-data-automation-blueprint-error-paths
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-data-automation-blueprint-error-paths bedrock-data-automation-blueprint-lifecycle bedrock-data-automation-blueprint-optimization-invoke bedrock-data-automation-project-with-custom-blueprint bedrock-data-automation-blueprint-tagging
Issue-Shape-Id: com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService
Issue-Key: bedrock-data-automation|CreateBlueprint|2xx->2xx|$.blueprint.blueprintStage
Issue-Key: bedrock-data-automation|CreateBlueprint|2xx->2xx|$.blueprint.creationTime
Issue-Key: bedrock-data-automation|CreateBlueprint|2xx->2xx|$.blueprint.lastModifiedTime
Issue-Key: bedrock-data-automation|GetBlueprint|2xx->2xx|$.blueprint.blueprintStage
Issue-Key: bedrock-data-automation|GetBlueprint|2xx->2xx|$.blueprint.creationTime
Issue-Key: bedrock-data-automation|GetBlueprint|2xx->2xx|$.blueprint.lastModifiedTime
Issue-Key: bedrock-data-automation|UpdateBlueprint|2xx->2xx|$.blueprint.blueprintStage
Issue-Key: bedrock-data-automation|UpdateBlueprint|2xx->2xx|$.blueprint.creationTime
Issue-Key: bedrock-data-automation|UpdateBlueprint|2xx->2xx|$.blueprint.lastModifiedTime
Issue-Key: bedrock-data-automation|CreateBlueprintVersion|2xx->2xx|$.blueprint.blueprintStage
Issue-Key: bedrock-data-automation|CreateBlueprintVersion|2xx->2xx|$.blueprint.creationTime
Issue-Key: bedrock-data-automation|CreateBlueprintVersion|2xx->2xx|$.blueprint.lastModifiedTime
Issue-Key: bedrock-data-automation|ListBlueprints|2xx->2xx|$.blueprints[*]
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Blueprint responses: epoch-number timestamps, DEVELOPMENT stage, over-wide list projection

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBlueprint

## Description
Every door that serves a Blueprint projection diverges the same two ways: the timestamp members are emitted as raw epoch-second numbers instead of the modelled date-time strings, and blueprintStage is reported as DEVELOPMENT where the service reports LIVE. ListBlueprints additionally serves a wider item than the service's list item. Six net-new causes, all in the blueprint response projection. (The only open bedrock-data-automation* issues are for the separate bedrock-data-automation-runtime service -- e.g. 20261024T024752Z-bedrock-data-automation-runtime-getdataautomationstatus-wrong-behavior -- and none of them cover this build-time surface.)

=== Cause 1 of 6: CreateBlueprint serves numeric timestamps and the wrong stage ===
AWS: Smithy: Blueprint.creationTime and Blueprint.lastModifiedTime both target DateTimestamp, which carries @timestampFormat("date-time"); the service protocol is restJson1, so both members serialize as ISO-8601 date-time STRINGS with millisecond precision and a trailing Z. Smithy: Blueprint.blueprintStage targets enum BlueprintStage with values [DEVELOPMENT, LIVE], and CreateBlueprintRequest.blueprintStage is optional. The recorded 201 for a CreateBlueprint reports blueprintStage LIVE and both timestamps as date-time strings. Hypothesis: a blueprint created without an explicit blueprintStage settles at LIVE, so the stage the create response reports is LIVE for this request class.
Emulator: status 201 (correct) but $.blueprint.creationTime and $.blueprint.lastModifiedTime are JSON numbers (epoch seconds with a fractional part) and $.blueprint.blueprintStage is "DEVELOPMENT".
Why: a client that reads blueprintStage to decide whether a blueprint is servable sees a development-stage blueprint it never asked for and will refuse to use (or will re-stage) a blueprint AWS already made live. A strict date-time reader rejects the numeric timestamp outright, and any client that round-trips the response as a date-time string gets a type error rather than a creation time.
Fix: the CreateBlueprint handler's response serializer (timestamp encoding for the Blueprint projection) and the stage default its handler/state applies when the request omits blueprintStage.
Traces: bedrock-data-automation-blueprint-error-paths#setup#1 (also bedrock-data-automation-blueprint-lifecycle#task#1, bedrock-data-automation-blueprint-optimization-invoke#setup#1, bedrock-data-automation-project-with-custom-blueprint#setup#1).

=== Cause 2 of 6: GetBlueprint serves numeric timestamps and the wrong stage ===
AWS: Smithy: GetBlueprint -> GetBlueprintResponse.blueprint is the same Blueprint shape, so creationTime/lastModifiedTime are date-time strings and blueprintStage is the stage the blueprint actually holds. The recorded 200 reports blueprintStage LIVE and both timestamps as date-time strings equal to the create's.
Emulator: status 200 (correct) with $.blueprint.blueprintStage "DEVELOPMENT" and both timestamps as epoch-second numbers.
Why: the read-back of a blueprint contradicts what AWS stored, so a reconcile loop that compares the read stage with the desired stage never converges.
Fix: the shared Blueprint projection used by the GetBlueprint handler -- same serializer and stage source as Cause 1.
Traces: bedrock-data-automation-blueprint-lifecycle#task#2 (also #task#4, #task#6, bedrock-data-automation-blueprint-tagging#task#2).

=== Cause 3 of 6: UpdateBlueprint serves numeric timestamps and the wrong stage ===
AWS: Smithy: UpdateBlueprint -> UpdateBlueprintResponse.blueprint (Blueprint). The recorded 200 reports blueprintStage LIVE, creationTime unchanged from the create and lastModifiedTime advanced to the update's instant -- both as date-time strings.
Emulator: status 200 (correct); blueprintStage "DEVELOPMENT", creationTime and lastModifiedTime numeric. The two numbers do differ (creation vs modification), so only the encoding and the stage are wrong here, not the ordering.
Why: a client cannot tell an updated blueprint from a stale one, because it cannot parse the lastModifiedTime it is handed.
Fix: the UpdateBlueprint handler's Blueprint projection.
Traces: bedrock-data-automation-blueprint-lifecycle#task#3.

=== Cause 4 of 6: CreateBlueprintVersion serves numeric timestamps and the wrong stage ===
AWS: Smithy: CreateBlueprintVersion -> 201, CreateBlueprintVersionResponse.blueprint (Blueprint). The recorded 201 reports blueprintStage LIVE and date-time-string timestamps.
Emulator: status 201 (correct); blueprintStage "DEVELOPMENT" and both timestamps numeric.
Why: the newly cut version is reported as a development artefact, so a pipeline that promotes only LIVE versions stalls on a version AWS already published.
Fix: the CreateBlueprintVersion handler's Blueprint projection and the stage it carries over from the source blueprint.
Traces: bedrock-data-automation-blueprint-lifecycle#task#5.

=== Cause 5 of 6: GetBlueprint on a pinned version still serves numeric timestamps ===
AWS: Smithy: Blueprint.creationTime/lastModifiedTime are date-time strings (as Cause 1). The recorded 200 for a version-scoped read reports both as date-time strings.
Emulator: status 200 (correct), both timestamps numeric. On this step the stage matches, so the timestamp encoding is the whole divergence.
Why: same as Cause 1 -- the timestamps are unparseable as modelled, and here they are the only fields that identify which version snapshot was read.
Fix: the Blueprint projection's timestamp encoding (single-home it so the version-scoped read cannot drift from the unscoped one).
Traces: bedrock-data-automation-blueprint-lifecycle#task#8.

=== Cause 6 of 6: ListBlueprints serves a wider item than the service's list projection ===
AWS: Smithy: ListBlueprintsResponse.blueprints is a list of BlueprintItem, whose ONLY members are blueprintArn (required), blueprintVersion and blueprintStage -- blueprintName and lastModifiedTime are not members of it at all. Recorded behavior only: the recorded 200 item carries exactly blueprintArn, blueprintVersion and a creationTime date-time string, and carries NO blueprintStage, blueprintName or lastModifiedTime. So the list item the service serves is {blueprintArn, blueprintVersion, creationTime-as-date-time-string}.
Emulator: status 200 (correct) but the item carries blueprintStage "DEVELOPMENT", blueprintName (the blueprint's name), lastModifiedTime, and creationTime as an epoch-second number -- three members the service omits plus the wrong encoding on the one it does serve.
Why: a client that enumerates versions and diffs the list item against its own record sees spurious members that never change and a creationTime it cannot parse; worse, blueprintName is @sensitive in the model and the list projection is where AWS deliberately does not carry it, so the emulator widens the blast radius of a list call.
Fix: the ListBlueprints handler's item projection -- build the list item from its own field set rather than reusing the full Blueprint projection, and encode creationTime as a date-time string.
Traces: bedrock-data-automation-blueprint-lifecycle#task#9.

## Evidence
Cause 1 (bedrock-data-automation-blueprint-error-paths#setup#1, CreateBlueprint, 201 vs 201): expected $.blueprint.blueprintStage 'LIVE', got 'DEVELOPMENT'; expected $.blueprint.creationTime a date-time string ('2026-09-06T04:59:43.224Z' in the recording), got the number 1793293472.207068; expected $.blueprint.lastModifiedTime a date-time string, got the same number. 4 steps in the cluster.
Cause 2 (bedrock-data-automation-blueprint-lifecycle#task#2, GetBlueprint, 200 vs 200): expected blueprintStage 'LIVE', got 'DEVELOPMENT'; creationTime/lastModifiedTime expected date-time strings, got 1793293472.886649. 4 steps in the cluster.
Cause 3 (bedrock-data-automation-blueprint-lifecycle#task#3, UpdateBlueprint, 200 vs 200): blueprintStage 'LIVE' -> 'DEVELOPMENT'; creationTime -> 1793293472.886649, lastModifiedTime -> 1793293472.9084613, both expected as date-time strings.
Cause 4 (bedrock-data-automation-blueprint-lifecycle#task#5, CreateBlueprintVersion, 201 vs 201): blueprintStage 'LIVE' -> 'DEVELOPMENT'; creationTime -> 1793293472.886649, lastModifiedTime -> 1793293472.9424963.
Cause 5 (bedrock-data-automation-blueprint-lifecycle#task#8, GetBlueprint, 200 vs 200): creationTime -> 1793293472.886649, lastModifiedTime -> 1793293472.9084613, both expected as date-time strings.
Cause 6 (bedrock-data-automation-blueprint-lifecycle#task#9, ListBlueprints, 200 vs 200): expected blueprints[0] == {blueprintArn, blueprintVersion, creationTime(date-time string)}; got {blueprintArn, blueprintVersion, blueprintStage 'DEVELOPMENT', blueprintName <the created name>, creationTime 1793293472.886649, lastModifiedTime 1793293472.9084613}.
Model citations: Smithy Blueprint.creationTime/lastModifiedTime -> DateTimestamp @timestampFormat("date-time"); Blueprint.blueprintStage -> enum BlueprintStage [DEVELOPMENT, LIVE]; BlueprintItem members = {blueprintArn (required), blueprintVersion, blueprintStage}; protocol restJson1. (The literal names, ARNs, epoch values and recorded dates above are this run's values, not requirements -- the requirements are the date-time string encoding, the LIVE stage, and the narrow list item.)

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator (replay_wire_corpus).
2. Cause 1: fixture bedrock-data-automation-blueprint-error-paths, step setup#1 -- CreateBlueprint; inspect $.blueprint.blueprintStage, $.blueprint.creationTime, $.blueprint.lastModifiedTime in the 201 body.
3. Cause 2: fixture bedrock-data-automation-blueprint-lifecycle, step task#2 -- GetBlueprint on the blueprint created in that fixture; same three members.
4. Cause 3: same fixture, step task#3 -- UpdateBlueprint; same three members.
5. Cause 4: same fixture, step task#5 -- CreateBlueprintVersion; same three members.
6. Cause 5: same fixture, step task#8 -- GetBlueprint (version-scoped); creationTime/lastModifiedTime only.
7. Cause 6: same fixture, step task#9 -- ListBlueprints; compare the member SET of blueprints[0] against BlueprintItem plus the recorded creationTime.
