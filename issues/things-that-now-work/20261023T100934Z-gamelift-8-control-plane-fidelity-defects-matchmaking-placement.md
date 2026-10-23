Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: gamelift.matchmaking-configuration-unknown-ruleset-rejected gamelift.start-game-session-placement-unknown-queue-rejected gamelift.start-matchmaking-unknown-configuration-rejected gamelift.validate-matchmaking-ruleset-semantic-violation-returns-invalid gamelift.concurrent-create-location-only-one-wins gamelift.create-alias-simple-missing-fleet-id-rejected gamelift.create-matchmaking-ruleset-malformed-json-rejected gamelift.game-session-queue-invalid-destination-rejected
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift.matchmaking-configuration-unknown-ruleset-rejected|run#5|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r}"
Issue-Key: gamelift.start-game-session-placement-unknown-queue-rejected|run#5|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Key: gamelift.start-matchmaking-unknown-configuration-rejected|run#5|AssertionError|assert code == "NotFoundException", f"unexpected code: {code!r}"
Issue-Key: gamelift.validate-matchmaking-ruleset-semantic-violation-returns-invalid|run#4|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: gamelift.concurrent-create-location-only-one-wins|run#9|AssertionError|assert len(successes) == 1, f"expected exactly 1 winner, got {results}"
Issue-Key: gamelift.create-alias-simple-missing-fleet-id-rejected|run#4|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: gamelift.create-matchmaking-ruleset-malformed-json-rejected|run#4|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: gamelift.game-session-queue-invalid-destination-rejected|run#4|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r}"
Issue-Commit: 3b7cd50bec204a941815a8ace6b2a1778291c65c

# GameLift: 8 control-plane fidelity defects — matchmaking, placement, queue, alias and location validation all answer 501 NotImplemented instead of modeled behaviour

> _Note: this issue was identified against the code base at commit `3b7cd50bec204a941815a8ace6b2a1778291c65c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMatchmaking

## Description
The behavioral suite exercised 27 GameLift checks and none passed. Eight of them are NOT covered by the auto-filed scaffold-stub issues (those are indexed by the operation whose [error] verdict names them); these eight were reported as [mismatch] because the check caught the ClientError and asserted on its code, so their replay keys are carried by no open issue. Each is a distinct behavioural requirement of the GameLift control plane that the emulator does not meet. There are EIGHT separate root causes bundled here; all eight must be closed before this issue is submitted.

--- Cause 1 of 8: CreateMatchmakingConfiguration does not validate that RuleSetName resolves, and DescribeMatchmakingConfigurations does not reject unknown names ---
Failing check: gamelift.matchmaking-configuration-unknown-ruleset-rejected
AWS: Smithy (services/service-com-amazonaws-gamelift-gamelift/smithy-model/com-amazonaws-gamelift-gamelift.json) models CreateMatchmakingConfiguration with required members Name, RequestTimeoutSeconds (@range 1..43200), AcceptanceRequired and RuleSetName (MatchmakingRuleSetName, @length 1..256, pattern ^([a-zA-Z0-9-\.]*|arn:.*:matchmakingruleset\/[a-zA-Z0-9-\.]*)$), and declares InvalidRequestException (HTTP 400) — 'One or more parameter values in the request are invalid.' Recorded behavior: a RuleSetName that does not exist is refused with HTTP 400 / InvalidRequestException; the recorded message, as an example, is "RuleSet with name <name> does not exist". The configuration is then absent, and DescribeMatchmakingConfigurations for that never-created name is ALSO refused with InvalidRequestException (example message "Configuration not found: <name>") rather than returning an empty list.
Emulator: CreateMatchmakingConfiguration answers HTTP 501 with error code NotImplemented ("operation CreateMatchmakingConfiguration is scaffolded but not implemented in this emulator"). No cross-resource rule-set existence check and no configuration state exist.
Why: a client that provisions a matchmaker by creating a rule set and then a configuration cannot do so at all, and a client whose negative path depends on InvalidRequestException to detect a typo'd rule set instead sees a 501 it will treat as a transient server fault and retry forever.
Fix: the CreateMatchmakingConfiguration handler and its validation, plus matchmaking-configuration state and the DescribeMatchmakingConfigurations lookup path.
Traces: gamelift.matchmaking-configuration-unknown-ruleset-rejected.

--- Cause 2 of 8: StartGameSessionPlacement does not resolve GameSessionQueueName, and DescribeGameSessionPlacement does not reject unknown PlacementIds ---
Failing check: gamelift.start-game-session-placement-unknown-queue-rejected
AWS: Smithy models StartGameSessionPlacement with required PlacementId (@length 1..48), GameSessionQueueName (GameSessionQueueNameOrArn, @length 1..256) and MaximumPlayerSessionCount, and declares NotFoundException (HTTP 400) — 'The requested resource was not found. The resource was either not created yet or deleted.' Recorded behavior: a GameSessionQueueName that does not exist is refused with HTTP 400 / NotFoundException (example message "Game session queue <name> does not exist."), no placement is created for that PlacementId, and DescribeGameSessionPlacement for it is also refused with NotFoundException.
Emulator: StartGameSessionPlacement answers HTTP 501 / NotImplemented.
Why: placement is the normal data path for session allocation; a client gets a 501 for both the happy path and the dangling-queue path, so it can neither place a session nor distinguish a configuration error from a server outage.
Fix: the StartGameSessionPlacement handler and its queue-resolution validation, plus placement state and the DescribeGameSessionPlacement lookup path.
Traces: gamelift.start-game-session-placement-unknown-queue-rejected.

--- Cause 3 of 8: StartMatchmaking does not resolve ConfigurationName, and DescribeMatchmaking does not reject unknown TicketIds ---
Failing check: gamelift.start-matchmaking-unknown-configuration-rejected
AWS: Smithy models StartMatchmaking with required ConfigurationName (MatchmakingConfigurationName, @length 1..256) and Players, and declares NotFoundException (HTTP 400) and InvalidRequestException (HTTP 400). Recorded behavior: an unknown ConfigurationName is refused with HTTP 400 / NotFoundException (example message "Specified configuration name does not exist. configurationName=<name>"); no ticket is persisted for the supplied TicketId, and DescribeMatchmaking for that ticket is refused with HTTP 400 / InvalidRequestException (example message "Matchmaking ticket not found: <id>") rather than returning an empty TicketList.
Emulator: StartMatchmaking answers HTTP 501 / NotImplemented (services/service-com-amazonaws-gamelift-gamelift/src/ops/start_matchmaking.rs:191).
Why: FlexMatch clients cannot start matchmaking at all, and the two distinct negative classes AWS separates (NotFoundException for the configuration, InvalidRequestException for the ticket) collapse into one non-modeled 501 that botocore classifies as retryable.
Fix: the StartMatchmaking handler and its configuration-resolution validation, plus matchmaking-ticket state and the DescribeMatchmaking lookup path.
Traces: gamelift.start-matchmaking-unknown-configuration-rejected.

--- Cause 4 of 8: ValidateMatchmakingRuleSet never evaluates a rule set ---
Failing check: gamelift.validate-matchmaking-ruleset-semantic-violation-returns-invalid
AWS: Smithy models ValidateMatchmakingRuleSet with required RuleSetBody (@length 1..65535) and output Valid (boolean), and declares InvalidRequestException (HTTP 400). Recorded behavior: a schema-conformant body returns HTTP 200 with Valid=true; a body that is valid JSON but violates the rule-set schema (no 'teams' element) is REFUSED with HTTP 400 / InvalidRequestException rather than returning Valid=false (example message "Rule set does not define team definitions; location: top level").
Emulator: ValidateMatchmakingRuleSet answers HTTP 501 / NotImplemented (services/service-com-amazonaws-gamelift-gamelift/src/ops/validate_matchmaking_rule_set.rs:161).
Why: a client that pre-validates a rule set before CreateMatchmakingRuleSet gets neither Valid=true nor the 400 that signals a bad rule set, so every rule set looks un-validatable; the important contract is that semantic violations are a 400, not a Valid=false payload.
Fix: the ValidateMatchmakingRuleSet handler and a rule-set body parser/validator shared with CreateMatchmakingRuleSet.
Traces: gamelift.validate-matchmaking-ruleset-semantic-violation-returns-invalid.

--- Cause 5 of 8: CreateLocation does not enforce LocationName uniqueness (no ConflictException), including under concurrency ---
Failing check: gamelift.concurrent-create-location-only-one-wins
AWS: Smithy models CreateLocation with required LocationName (CustomInputLocationStringModel, @length 8..64, pattern ^custom-[A-Za-z0-9\-]+$) and declares ConflictException (HTTP 400). Recorded behavior: N concurrent CreateLocation calls for the same custom location name yield exactly ONE success and N-1 HTTP 400 / ConflictException, and ListLocations then shows that name exactly once.
Emulator: all 8 concurrent calls answered HTTP 501 / NotImplemented, so neither the single-winner guarantee nor ConflictException exists. Note: a scaffold issue naming CreateLocation (src/ops/create_location.rs) is already filed for the [error] checks it blocks; it records only that the operation is a stub and does NOT carry this uniqueness/ConflictException requirement or this check's replay key, so this cause is tracked here.
Why: idempotent provisioning code that races to create a shared custom location relies on exactly one winner and on ConflictException to recognise the loser; without it either both branches believe they created the location or neither can proceed.
Fix: the CreateLocation handler plus location state — the uniqueness check and insert must be atomic under the state lock so a race cannot produce two winners.
Traces: gamelift.concurrent-create-location-only-one-wins.

--- Cause 6 of 8: CreateAlias does not validate RoutingStrategy consistency (SIMPLE without FleetId must be rejected) ---
Failing check: gamelift.create-alias-simple-missing-fleet-id-rejected
AWS: Smithy models CreateAlias with required Name and RoutingStrategy and declares InvalidRequestException (HTTP 400). Recorded behavior: RoutingStrategy.Type=SIMPLE with no FleetId is refused with HTTP 400 / InvalidRequestException and nothing is persisted (ListAliases by that name returns []), while Type=TERMINAL with a Message and no FleetId succeeds and is readable via DescribeAlias. The target FleetId itself is NOT validated for existence — that cross-resource check is deferred past the Alias surface.
Emulator: CreateAlias answers HTTP 501 / NotImplemented, so the per-Type conditional requirement is unimplemented in both directions. Note: the already-filed scaffold issue for CreateAlias (src/ops/create_alias.rs) records only that the operation is a stub; it does not carry this conditional-validation requirement or this check's replay key.
Why: a client that mistypes a SIMPLE routing strategy gets a retryable 501 instead of a 400 naming the bad request, and once implemented an alias created without a FleetId would silently fail at resolve time.
Fix: the CreateAlias handler's validation (RoutingStrategy Type/FleetId cross-member rule) and alias state.
Traces: gamelift.create-alias-simple-missing-fleet-id-rejected.

--- Cause 7 of 8: CreateMatchmakingRuleSet does not reject a malformed RuleSetBody, and DescribeMatchmakingRuleSets does not reject unknown names ---
Failing check: gamelift.create-matchmaking-ruleset-malformed-json-rejected
AWS: Smithy models CreateMatchmakingRuleSet with required Name (@length 0..128, pattern ^[a-zA-Z0-9-\.]*$) and RuleSetBody (@length 1..65535) and declares InvalidRequestException (HTTP 400). Recorded behavior: a RuleSetBody that is not valid JSON is refused with HTTP 400 / InvalidRequestException and nothing is persisted (example message "Encountered IOException attempting to extract version; location: top level"); DescribeMatchmakingRuleSets for a never-created name is ALSO refused with HTTP 400 / InvalidRequestException (example message "Failed to find rule set: <name>") rather than returning an empty list; a well-formed body is accepted and visible on an independent describe.
Emulator: CreateMatchmakingRuleSet answers HTTP 501 / NotImplemented. Note: the already-filed scaffold issue for CreateMatchmakingRuleSet (src/ops/create_matchmaking_rule_set.rs) records only the stub, not this body-validation requirement, the non-empty-list describe semantics, or this check's replay key.
Why: callers that validate operator-supplied rule sets at create time depend on the 400; and a describe that returns [] for an unknown rule set instead of a 400 makes 'absent' indistinguishable from 'empty result page'.
Fix: the CreateMatchmakingRuleSet handler's RuleSetBody parse/validation (shared with ValidateMatchmakingRuleSet, Cause 4), rule-set state, and the DescribeMatchmakingRuleSets lookup path.
Traces: gamelift.create-matchmaking-ruleset-malformed-json-rejected.

--- Cause 8 of 8: CreateGameSessionQueue does not range-check TimeoutInSeconds, and must still accept a dangling destination ARN ---
Failing check: gamelift.game-session-queue-invalid-destination-rejected
AWS: Smithy models CreateGameSessionQueue with required Name (@length 1..128, pattern ^[a-zA-Z0-9-]+$), TimeoutInSeconds (WholeNumber, @range min 0 — the model alone does NOT express the service's real lower bound) and Destinations, and declares InvalidRequestException (HTTP 400) and NotFoundException (HTTP 400). Recorded behavior: TimeoutInSeconds=5 is refused with HTTP 400 / InvalidRequestException (example message "TimeoutInSeconds must be between 10 and 600 seconds") and no queue is persisted — DescribeGameSessionQueues by name returns an empty list for a missing queue, it does not raise. TimeoutInSeconds=10 (the boundary) succeeds. Separately, a Destinations entry naming a nonexistent fleet ARN is NOT eagerly validated: the queue is created and the dangling DestinationArn persists verbatim on an independent describe.
Emulator: CreateGameSessionQueue answers HTTP 501 / NotImplemented, so neither the 10..600 range rule, the boundary acceptance, nor the deliberate non-validation of destinations exists. Note: the already-filed scaffold issue for CreateGameSessionQueue (src/ops/create_game_session_queue.rs) records only the stub and carries none of these requirements nor this check's replay key.
Why: a client sizing a queue timeout gets a retryable 501 instead of a 400 naming the member and its bound; and if destinations were later validated eagerly, the documented deferred-validation behaviour (dangling ARN stored as-is) would break queue provisioning that is ordered fleet-after-queue.
Fix: the CreateGameSessionQueue handler's TimeoutInSeconds validation and queue state, plus the DescribeGameSessionQueues by-name lookup (empty list, not an error).
Traces: gamelift.game-session-queue-invalid-destination-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, gamelift 0/27 passed (0%), 8 mismatched + 19 errored.

Expected vs actual, per cause:
1. CreateMatchmakingConfiguration(RuleSetName=<nonexistent>) -> expected HTTP 400 InvalidRequestException; actual HTTP 501 NotImplemented. AssertionError: "expected InvalidRequestException, got 'NotImplemented'".
2. StartGameSessionPlacement(GameSessionQueueName=<nonexistent>) -> expected HTTP 400 NotFoundException; actual HTTP 501 NotImplemented. AssertionError: "expected NotFoundException, got 'NotImplemented'".
3. StartMatchmaking(ConfigurationName=<nonexistent>) -> expected HTTP 400 NotFoundException; actual HTTP 501 NotImplemented. AssertionError: "unexpected code: 'NotImplemented'".
4. ValidateMatchmakingRuleSet(RuleSetBody={"name":...,"ruleLanguageVersion":"1.0"} with no teams) -> expected HTTP 400 InvalidRequestException; actual HTTP 501 NotImplemented. AssertionError: "expected InvalidRequestException for a rule set missing the required 'teams' element, got 'NotImplemented'". The conformant body is expected to return 200 Valid=true.
5. 8x concurrent CreateLocation(LocationName=custom-...) -> expected 1 'ok' + 7 ConflictException; actual ['NotImplemented' x8]. AssertionError: "expected exactly 1 winner, got [...]".
6. CreateAlias(RoutingStrategy={'Type':'SIMPLE'}) -> expected HTTP 400 InvalidRequestException; actual HTTP 501 NotImplemented. AssertionError: "CreateAlias with RoutingStrategy.Type=SIMPLE and no FleetId was refused with 'NotImplemented', not InvalidRequestException".
7. CreateMatchmakingRuleSet(RuleSetBody='{not valid json at all') -> expected HTTP 400 InvalidRequestException; actual HTTP 501 NotImplemented. AssertionError: "expected InvalidRequestException creating a rule set with malformed RuleSetBody, got 'NotImplemented'".
8. CreateGameSessionQueue(TimeoutInSeconds=5) -> expected HTTP 400 InvalidRequestException; actual HTTP 501 NotImplemented. AssertionError: "expected InvalidRequestException, got 'NotImplemented'".

Common emulator response body for all eight: error code NotImplemented, "operation <Op> is scaffolded but not implemented in this emulator". Handler stubs confirmed in the checkout, e.g. services/service-com-amazonaws-gamelift-gamelift/src/ops/start_matchmaking.rs:191 and services/service-com-amazonaws-gamelift-gamelift/src/ops/validate_matchmaking_rule_set.rs:161, both returning crate::scaffolded::Scaffolded::not_implemented(..).

Note on recorded messages: the quoted messages are examples. The requirements are the HTTP status and error code, plus (for the validation cases) that the message names the offending member and its constraint.

## Steps to Reproduce
Boot the emulator and point a botocore gamelift client at it, then run the eight checks:

1. gl.create_matchmaking_configuration(Name=<new>, RuleSetName=<nonexistent>, RequestTimeoutSeconds=30, AcceptanceRequired=False, FlexMatchMode='STANDALONE') -> expect ClientError InvalidRequestException (HTTP 400); then gl.describe_matchmaking_configurations(Names=[<new>]) -> expect InvalidRequestException.
2. gl.start_game_session_placement(PlacementId=<new>, GameSessionQueueName=<nonexistent>, MaximumPlayerSessionCount=4) -> expect NotFoundException; then gl.describe_game_session_placement(PlacementId=<new>) -> expect NotFoundException.
3. gl.start_matchmaking(TicketId=<new>, ConfigurationName=<nonexistent>, Players=[{'PlayerId':'player-1'}]) -> expect NotFoundException; then gl.describe_matchmaking(TicketIds=[<new>]) -> expect InvalidRequestException.
4. gl.validate_matchmaking_rule_set(RuleSetBody=json.dumps({'name':'missing_teams','ruleLanguageVersion':'1.0'})) -> expect InvalidRequestException; gl.validate_matchmaking_rule_set(RuleSetBody=json.dumps({'name':'well_formed','ruleLanguageVersion':'1.0','teams':[{'name':'solo','minPlayers':1,'maxPlayers':2}]})) -> expect 200 with Valid=True.
5. Fire 8 parallel gl.create_location(LocationName='custom-<same-name>') from a ThreadPoolExecutor -> expect exactly 1 success and 7 ConflictException; gl.list_locations() must show that name exactly once.
6. gl.create_alias(Name=<new>, RoutingStrategy={'Type':'SIMPLE'}) -> expect InvalidRequestException and gl.list_aliases(Name=<new>)['Aliases'] == []; gl.create_alias(Name=<other>, RoutingStrategy={'Type':'TERMINAL','Message':'control alias'}) -> expect success, and describe_alias must report the TERMINAL strategy.
7. gl.create_matchmaking_rule_set(Name=<new>, RuleSetBody='{not valid json at all') -> expect InvalidRequestException; gl.describe_matchmaking_rule_sets(Names=[<new>]) -> expect InvalidRequestException; the same call with a well-formed body -> expect success and the rule set visible on an independent describe.
8. gl.create_game_session_queue(Name=<new>, TimeoutInSeconds=5) -> expect InvalidRequestException and describe_game_session_queues(Names=[<new>]) == []; TimeoutInSeconds=10 -> expect success; create a queue with Destinations=[{'DestinationArn': 'arn:aws:gamelift:<region>::fleet/fleet-0000...'}] -> expect success with the dangling ARN persisted verbatim on an independent describe.

Replay shortcut: re-run the eight check ids listed in trace_ids.
