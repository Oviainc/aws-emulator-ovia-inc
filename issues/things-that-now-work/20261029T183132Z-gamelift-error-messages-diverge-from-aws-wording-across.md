Issue-Source: trace-sweep-divergence
Issue-Trace: gamelift-build-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gamelift-build-error-paths gamelift-create-alias-simple-missing-fleet-id-rejected gamelift-game-session-lifecycle gamelift-location-error-paths gamelift-ruleset-lifecycle-and-validation
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|DescribeBuild|4xx->4xx|$.Message
Issue-Key: gamelift|CreateAlias|4xx->4xx|$.Message
Issue-Key: gamelift|CreateGameSession|4xx->4xx|$.Message
Issue-Key: gamelift|CreateLocation|4xx->4xx|$.Message
Issue-Key: gamelift|ValidateMatchmakingRuleSet|4xx->4xx|$.Message
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# GameLift error messages diverge from AWS wording across five operations

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBuild

## Description
Five independent causes, one shared fault: the emulator picks the right HTTP status (400) and the right error code, but writes its own English in the Message member instead of the message AWS returns for that error class. The message is part of the wire contract - clients log, match and branch on it. A fixer must close all FIVE before submitting.

Cause 1 of 5 - DescribeBuild on an unknown build id.
AWS: HTTP 400, NotFoundException, Message "Build (<buildId>) not found". Fixed text: "Build (" and ") not found"; the single variable part is the build id copied from the request's BuildId. Smithy: DescribeBuild models NotFoundException (client, httpError 400 defaulted); BuildIdOrArn pattern ^(build-\S+|arn:.*:build\/build-\S+)$.
Emulator: same status and code, but Message "The build <buildId> was not found." - different fixed text, different punctuation, id not parenthesised.
Why: clients and test suites that match the AWS message (or surface it to operators) see text AWS never emits.
Fix: the DescribeBuild handler's not-found error construction.
Traces: gamelift-build-error-paths#task#1.

Cause 2 of 5 - CreateAlias with a SIMPLE routing strategy and no FleetId.
AWS: HTTP 400 with Message "SIMPLE RoutingStrategy must contain a valid FleetId." - the whole message is fixed text, with no value interpolated from the request. Smithy: CreateAlias.RoutingStrategy is required and models InvalidRequestException at 400.
Emulator: same status and code, Message "RoutingStrategy of type SIMPLE must name the fleet it resolves to in RoutingStrategy.FleetId." - invented wording.
Why: validation-error assertions and user-facing error text diverge from AWS.
Fix: the CreateAlias input-validation path for RoutingStrategy.
Traces: gamelift-create-alias-simple-missing-fleet-id-rejected#task#1.

Cause 3 of 5 - CreateGameSession when no server process is available.
AWS: HTTP 400 with Message "Unable to reserve a process(No active and available server processes found on the fleet)" - entirely fixed text, including the parenthesised clause with no space before the parenthesis and no fleet or location identifier inside it. Smithy: CreateGameSession models FleetCapacityExceededException / InvalidFleetStatusException / InvalidRequestException, all 400.
Emulator: same status and code, but interpolates request/state values into the parenthesis: "Unable to reserve a process(no active and available server processes found at location <locationName> of fleet <fleetId>)" - lower-cased, and with a location name and fleet id that are not part of the AWS message template.
Why: the message is no longer stable across environments (it now embeds generated ids), and does not match what AWS returns for this class.
Fix: the CreateGameSession placement path's error construction.
Traces: gamelift-game-session-lifecycle#task#3.

Cause 4 of 5 - CreateLocation for a custom location that already exists.
AWS: HTTP 400 with Message "A Location with name: <locationName> already exists." Fixed text "A Location with name: " and " already exists."; the variable part is the LocationName from the request. Smithy: CreateLocation models ConflictException (400); LocationName pattern ^custom-[A-Za-z0-9\-]+$.
Emulator: same status and code, Message "Location <locationName> already exists; a custom location name identifies one location." - invented wording with an extra clause.
Why: duplicate-create handling keyed on the AWS message fails.
Fix: the CreateLocation conflict path's error construction.
Traces: gamelift-location-error-paths#task#3.

Cause 5 of 5 - ValidateMatchmakingRuleSet with a malformed rule set body.
AWS: HTTP 400 with Message "Encountered IOException attempting to extract version; location: top level" - fixed text; the "location: " suffix names the position in the rule set, which for a body that cannot be parsed at all is "top level". No parser-internal detail is exposed. Smithy: ValidateMatchmakingRuleSet.RuleSetBody required (length 1-65535); errors InvalidRequestException / UnsupportedRegionException / InternalServiceException.
Emulator: same status and code, Message "RuleSetBody is not valid JSON (key must be a string at line 1 column 2); location: top level" - leaks its own JSON parser's diagnostic (line/column) instead of AWS's fixed text.
Why: the message carries implementation detail of the emulator's parser, so it differs from AWS for every malformed body and cannot be matched.
Fix: the ValidateMatchmakingRuleSet validation path's error construction.
Traces: gamelift-ruleset-lifecycle-and-validation#task#4.

## Evidence
Every cause below matched on status and error code; only $.Message diverged.
Cause 1: gamelift-build-error-paths#task#1 DescribeBuild 400/400 - expected "Build (build-...) not found", got "The build build-... was not found.".
Cause 2: gamelift-create-alias-simple-missing-fleet-id-rejected#task#1 CreateAlias 400/400 - expected "SIMPLE RoutingStrategy must contain a valid FleetId.", got "RoutingStrategy of type SIMPLE must name the fleet it resolves to in RoutingStrategy.FleetId.".
Cause 3: gamelift-game-session-lifecycle#task#3 CreateGameSession 400/400 - expected "Unable to reserve a process(No active and available server processes found on the fleet)", got "Unable to reserve a process(no active and available server processes found at location <location> of fleet <fleetId>)".
Cause 4: gamelift-location-error-paths#task#3 CreateLocation 400/400 - expected "A Location with name: <locationName> already exists.", got "Location <locationName> already exists; a custom location name identifies one location.".
Cause 5: gamelift-ruleset-lifecycle-and-validation#task#4 ValidateMatchmakingRuleSet 400/400 - expected "Encountered IOException attempting to extract version; location: top level", got "RuleSetBody is not valid JSON (key must be a string at line 1 column 2); location: top level".

## Steps to Reproduce
1. Replay the wire corpus (simparity-wire-aws-v1).
2. Cause 1: DescribeBuild with build-00000000-0000-0000-0000-000000000000. Cause 2: CreateAlias with RoutingStrategy {Type: SIMPLE} and no FleetId. Cause 3: CreateGameSession against an Anywhere fleet with no registered server process. Cause 4: CreateLocation twice with the same custom-* name. Cause 5: ValidateMatchmakingRuleSet with a body that is not valid JSON.
3. Compare the Message member of each 400 response with the AWS template above.
4. Re-replay gamelift-build-error-paths#task#1 and every fixture#step in trace_ids to confirm all five causes are closed.
