Issue-Source: trace-sweep-divergence
Issue-Trace: gamelift-alias-lifecycle-terminal-routing
Issue-Step: teardown#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gamelift-alias-lifecycle-terminal-routing gamelift-alias-list-and-tags gamelift-build-error-paths gamelift-build-lifecycle-and-tags gamelift-matchmaking-configuration-lifecycle gamelift-ruleset-lifecycle-and-validation gamelift-script-lifecycle gamelift-game-session-queue-lifecycle gamelift-location-lifecycle-and-tags
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|DescribeAlias|4xx->2xx|$
Issue-Key: gamelift|DescribeAlias|4xx->2xx|$.Message
Issue-Key: gamelift|DescribeAlias|4xx->2xx|$.__type
Issue-Key: gamelift|DescribeAlias|4xx->2xx|status
Issue-Key: gamelift|DescribeBuild|4xx->2xx|$.Message
Issue-Key: gamelift|DescribeBuild|4xx->2xx|$.__type
Issue-Key: gamelift|DescribeBuild|4xx->2xx|status
Issue-Key: gamelift|DescribeBuild|4xx->2xx|$
Issue-Key: gamelift|DescribeMatchmakingConfigurations|4xx->2xx|$.Message
Issue-Key: gamelift|DescribeMatchmakingConfigurations|4xx->2xx|$.__type
Issue-Key: gamelift|DescribeMatchmakingConfigurations|4xx->2xx|status
Issue-Key: gamelift|DescribeMatchmakingRuleSets|4xx->2xx|$.Message
Issue-Key: gamelift|DescribeMatchmakingRuleSets|4xx->2xx|$.__type
Issue-Key: gamelift|DescribeMatchmakingRuleSets|4xx->2xx|status
Issue-Key: gamelift|DescribeScript|4xx->2xx|$
Issue-Key: gamelift|DescribeScript|4xx->2xx|$.Message
Issue-Key: gamelift|DescribeScript|4xx->2xx|$.__type
Issue-Key: gamelift|DescribeScript|4xx->2xx|status
Issue-Key: gamelift|DescribeGameSessionQueues|2xx->2xx|$.GameSessionQueues[*]
Issue-Key: gamelift|ListLocations|2xx->2xx|$.Locations[*]
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# GameLift Delete* operations do not remove state: resources remain readable after deletion

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlias

## Description
Eight independent causes, one shared fault: every GameLift Delete* operation in the corpus answers success but leaves the resource in the emulator's state, so the following read still returns the deleted resource instead of the modeled not-found error (or still lists it). A fixer must close all EIGHT before submitting.

Cause 1 of 8 - DeleteAlias / DescribeAlias.
AWS: after DeleteAlias succeeds, DescribeAlias on that alias id fails. Smithy: DescribeAlias errors include NotFoundException (client error, httpError 400 defaulted). Recorded response: HTTP 400 with body {"__type": "NotFoundException", "Message": "Alias (<aliasId>) not found"}. Message template: fixed text "Alias (" ... ") not found"; the only variable part is the alias id taken from the request's AliasId.
Emulator: HTTP 200 with a full Alias body for the deleted alias, and no __type/Message. The fixture polled 29 times and the alias never disappeared.
Why: a client that deletes an alias and then waits for the 400 NotFoundException to confirm removal never observes it - teardown/poll loops hang and idempotent re-delete logic is never reached.
Fix: the DeleteAlias handler's state mutation (the alias must be removed from the service state store) and DescribeAlias's not-found path.
Traces: gamelift-alias-lifecycle-terminal-routing#teardown#7 (also gamelift-alias-list-and-tags#teardown#9, #teardown#11).

Cause 2 of 8 - DeleteBuild / DescribeBuild (error-paths fixture).
AWS: DescribeBuild on a deleted build returns HTTP 400 {"__type": "NotFoundException", "Message": "Build (<buildId>) not found"} - fixed text "Build (" ... ") not found" with the build id from the request. Smithy: DescribeBuild/DeleteBuild both model NotFoundException at 400.
Emulator: HTTP 200 with the Build body; __type and Message absent.
Why: same as cause 1 for builds - deletion is unobservable.
Fix: DeleteBuild handler state removal + DescribeBuild not-found path.
Traces: gamelift-build-error-paths#task#4.

Cause 3 of 8 - DeleteBuild / DescribeBuild (lifecycle fixture, polled).
AWS: as cause 2; the recorded poll terminates on NotFoundException.
Emulator: HTTP 200 with the Build body (Status INITIALIZED) after 29 poll attempts.
Why: lifecycle teardown never converges.
Fix: same component as cause 2 - the delete must be durable, not just acknowledged.
Traces: gamelift-build-lifecycle-and-tags#teardown#9.

Cause 4 of 8 - DeleteMatchmakingConfiguration / DescribeMatchmakingConfigurations.
AWS: describing a deleted configuration by name fails with HTTP 400 {"__type": "InvalidRequestException", "Message": "Configuration not found: <name>"} - note the error class here is InvalidRequestException, not NotFoundException, and the fixed text is "Configuration not found: " followed by the requested configuration name. Smithy: DescribeMatchmakingConfigurations models InvalidRequestException (400) and does NOT model NotFoundException.
Emulator: HTTP 200, no error at all (the deleted configuration is still resolvable).
Why: clients cannot tell a deleted configuration from a live one, and matchmaking code that keys off InvalidRequestException never fires.
Fix: DeleteMatchmakingConfiguration state removal + the describe handler's unknown-name path, which must raise InvalidRequestException for this operation.
Traces: gamelift-matchmaking-configuration-lifecycle#teardown#7.

Cause 5 of 8 - DeleteMatchmakingRuleSet / DescribeMatchmakingRuleSets.
AWS: HTTP 400 {"__type": "InvalidRequestException", "Message": "Failed to find rule set: <ruleSetName>"} - fixed text "Failed to find rule set: " plus the requested rule set name.
Emulator: HTTP 200 with no error.
Why: deleted rule sets stay referenceable, so configuration validation against a removed rule set wrongly succeeds.
Fix: DeleteMatchmakingRuleSet state removal + DescribeMatchmakingRuleSets unknown-name path.
Traces: gamelift-matchmaking-configuration-lifecycle#teardown#9 (also gamelift-ruleset-lifecycle-and-validation#teardown#6).

Cause 6 of 8 - DeleteScript / DescribeScript.
AWS: HTTP 400 {"__type": "NotFoundException", "Message": "Script (<scriptId>) not found."} - fixed text "Script (" ... ") not found." (this message ends with a period, unlike the alias and build messages) with the script id from the request. Smithy: DescribeScript models NotFoundException at 400.
Emulator: HTTP 200 with the Script body after 29 poll attempts.
Why: Realtime script teardown never completes for a client that waits for the not-found error.
Fix: DeleteScript state removal + DescribeScript not-found path.
Traces: gamelift-script-lifecycle#teardown#7.

Cause 7 of 8 - DeleteGameSessionQueue / DescribeGameSessionQueues.
AWS: after DeleteGameSessionQueue succeeds, DescribeGameSessionQueues no longer returns that queue; the recorded response has an empty GameSessionQueues list. Smithy: DeleteGameSessionQueue models NotFoundException, so the name must stop resolving.
Emulator: HTTP 200 whose GameSessionQueues list still contains the deleted queue (extra element with its Name, GameSessionQueueArn, TimeoutInSeconds, PlayerLatencyPolicies and Destinations).
Why: listings never shrink, so clients that enumerate queues to pick a placement target route sessions at a queue that no longer exists.
Fix: DeleteGameSessionQueue handler's state mutation (removal from the queue collection the describe handler reads).
Traces: gamelift-game-session-queue-lifecycle#task#8.

Cause 8 of 8 - DeleteLocation / ListLocations.
AWS: after DeleteLocation succeeds, the custom location is gone from ListLocations output. Smithy: DeleteLocation takes a custom-* location name and models NotFoundException.
Emulator: HTTP 200 whose Locations list still contains the deleted custom location (extra element with LocationName and LocationArn).
Why: a client that deletes a custom location and re-lists still sees it, and a subsequent CreateLocation of the same name can then collide.
Fix: DeleteLocation handler's state mutation (removal from the custom-location collection ListLocations reads).
Traces: gamelift-location-lifecycle-and-tags#task#10.

## Evidence
Cause 1: step gamelift-alias-lifecycle-terminal-routing#teardown#7 DescribeAlias - expected 400 / {"__type":"NotFoundException","Message":"Alias (<aliasId>) not found"}; actual 200 with the Alias body, __type and Message missing, poll gave up after 29 attempts.
Cause 2: gamelift-build-error-paths#task#4 DescribeBuild - expected 400 NotFoundException "Build (<buildId>) not found"; actual 200 with Build body.
Cause 3: gamelift-build-lifecycle-and-tags#teardown#9 DescribeBuild - expected 400 NotFoundException; actual 200 Build body after 29 poll attempts.
Cause 4: gamelift-matchmaking-configuration-lifecycle#teardown#7 DescribeMatchmakingConfigurations - expected 400 InvalidRequestException "Configuration not found: <name>"; actual 200.
Cause 5: gamelift-matchmaking-configuration-lifecycle#teardown#9 DescribeMatchmakingRuleSets - expected 400 InvalidRequestException "Failed to find rule set: <name>"; actual 200.
Cause 6: gamelift-script-lifecycle#teardown#7 DescribeScript - expected 400 NotFoundException "Script (<scriptId>) not found."; actual 200 Script body after 29 poll attempts.
Cause 7: gamelift-game-session-queue-lifecycle#task#8 DescribeGameSessionQueues - expected GameSessionQueues empty; actual contains the deleted queue ($.GameSessionQueues[0]: extra).
Cause 8: gamelift-location-lifecycle-and-tags#task#10 ListLocations - expected the custom location absent; actual still present ($.Locations[0]: extra).

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. For each cause, run the fixture's create -> delete -> describe sequence, e.g. CreateAlias, DeleteAlias, then DescribeAlias with the returned AliasId.
3. Observe HTTP 200 with the resource body instead of HTTP 400 with the modeled error code and message (causes 1-6), or the resource still present in the list response (causes 7-8).
4. Re-replay gamelift-alias-lifecycle-terminal-routing#teardown#7 plus every fixture#step in trace_ids to confirm all eight causes are closed.
