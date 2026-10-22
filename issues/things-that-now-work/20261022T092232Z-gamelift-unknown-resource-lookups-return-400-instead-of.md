Issue-Source: trace-sweep-divergence
Issue-Trace: gamelift-fleet-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gamelift-fleet-error-paths gamelift-game-session-errors-invalid-fleet gamelift-player-session-error-paths
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|DescribeFleetAttributes|2xx->4xx|$.FleetAttributes
Issue-Key: gamelift|DescribeFleetAttributes|2xx->4xx|status
Issue-Key: gamelift|DescribeGameSessions|2xx->4xx|$.GameSessions
Issue-Key: gamelift|DescribeGameSessions|2xx->4xx|status
Issue-Key: gamelift|DeleteFleet|4xx->4xx|$.Message
Issue-Key: gamelift|CreateGameSession|4xx->4xx|$.Message
Issue-Key: gamelift|CreatePlayerSession|4xx->4xx|$.Message
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# GameLift unknown-resource lookups return 400 instead of an empty result list

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFleetAttributes

## Description
The gamelift not-found paths diverge in two ways: two list/describe operations turn an unknown id into a 400 where AWS answers 200 with an empty list, and three NotFoundException messages use a different template from the recorded one. FIVE CAUSES MUST BE FIXED.

Cause 1 of 5 — DescribeFleetAttributes on an unknown fleet id returns 400 instead of 200 with an empty list.
AWS: Smithy DescribeFleetAttributes -> DescribeFleetAttributesOutput{FleetAttributes: FleetAttributesList, NextToken}; it is @paginated over FleetAttributes. NotFoundException is modeled (client, HTTP 400), but the recording shows that describing a well-formed but non-existent fleet id answers status 200 with FleetAttributes: [] — the empty-collection response, not the error.
Emulator: status 400 and no FleetAttributes member at all (replay: "status: expected 200, got 400", "$.FleetAttributes: missing (expected [])").
Why: a client that polls describe to learn whether a fleet exists (or that iterates the paginated result) gets a thrown modeled exception instead of an empty page, so existence checks and teardown-confirmation loops fail hard against the emulator where they succeed against AWS.
Fix: the DescribeFleetAttributes handler's unknown-id branch — return 200 with an empty FleetAttributes list rather than raising NotFoundException.
Traces: gamelift-fleet-error-paths#task#3.

Cause 2 of 5 — DescribeGameSessions on an unknown fleet id returns 400 instead of 200 with an empty list.
AWS: Smithy DescribeGameSessions -> DescribeGameSessionsOutput{GameSessions: GameSessionList, NextToken}, @paginated over GameSessions; NotFoundException is modeled but the recording shows an unknown FleetId answering status 200 with GameSessions: [].
Emulator: status 400 with GameSessions absent (replay: "status: expected 200, got 400", "$.GameSessions: missing (expected [])").
Why: same class of breakage one level down — session-enumeration code paths that tolerate an empty page against AWS raise against the emulator.
Fix: the DescribeGameSessions handler's unknown-fleet branch — empty GameSessions list with status 200.
Traces: gamelift-game-session-errors-invalid-fleet#task#2.

Cause 3 of 5 — DeleteFleet not-found message does not use the recorded template.
AWS: Smithy DeleteFleet, modeled NotFoundException (client, HTTP 400, member Message : NonEmptyString). Recorded behavior only: the message is built from the request and names the requested fleet id in single quotes, template "Unable to find fleet '<FleetId>'." — e.g. "Unable to find fleet 'fleet-00000000-0000-0000-0000-000000000000'."
Emulator: correct status 400 and error shape, but the message uses its own template: "The fleet fleet-00000000-0000-0000-0000-000000000000 was not found."
Why: only message-level — clients that surface or match on the AWS wording see different text. The requirement is the template built from the request (the requested id, quoted), not the literal recorded id.
Fix: the DeleteFleet handler's NotFoundException message construction.
Traces: gamelift-fleet-error-paths#task#2.

Cause 4 of 5 — CreateGameSession not-found message does not use the recorded template.
AWS: Smithy CreateGameSession, modeled NotFoundException (client, HTTP 400). Recorded behavior only: message built from the request FleetId, template "Cannot find fleet with ID <FleetId>".
Emulator: status 400 with message "The fleet fleet-00000000-0000-0000-0000-000000000000 was not found."
Why: message-level only; status and error code already match.
Fix: the CreateGameSession handler's NotFoundException message construction.
Traces: gamelift-game-session-errors-invalid-fleet#task#1.

Cause 5 of 5 — CreatePlayerSession not-found message does not use the recorded template.
AWS: Smithy CreatePlayerSession, modeled NotFoundException (client, HTTP 400); input GameSessionId is required. Recorded behavior only: message built from the request GameSessionId, template "GameSession <GameSessionId> does not exist".
Emulator: status 400 with message "The game session <GameSessionId> was not found."
Why: message-level only.
Fix: the CreatePlayerSession handler's NotFoundException message construction.
Traces: gamelift-player-session-error-paths#task#1.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, state_reset_failures=0).
Cause 1 — gamelift-fleet-error-paths#task#3, DescribeFleetAttributes: expected status 200 with $.FleetAttributes = [], got status 400 with $.FleetAttributes missing.
Cause 2 — gamelift-game-session-errors-invalid-fleet#task#2, DescribeGameSessions: expected status 200 with $.GameSessions = [], got status 400 with $.GameSessions missing.
Cause 3 — gamelift-fleet-error-paths#task#2, DeleteFleet, 400/400: $.Message expected "Unable to find fleet 'fleet-00000000-0000-0000-0000-000000000000'.", got 'The fleet fleet-00000000-0000-0000-0000-000000000000 was not found.'
Cause 4 — gamelift-game-session-errors-invalid-fleet#task#1, CreateGameSession, 400/400: $.Message expected 'Cannot find fleet with ID fleet-00000000-0000-0000-0000-000000000000', got 'The fleet fleet-00000000-0000-0000-0000-000000000000 was not found.'
Cause 5 — gamelift-player-session-error-paths#task#1, CreatePlayerSession, 400/400: $.Message expected 'GameSession arn:aws:gamelift:us-west-2::gamesession/fleet-00000000-0000-0000-0000-000000000000/sim2real-probe-gamelift-gs-c274c643 does not exist', got 'The game session <same id> was not found.'

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout.
2. Cause 1: fixture gamelift-fleet-error-paths, step task#3 — DescribeFleetAttributes with FleetIds=[fleet-00000000-0000-0000-0000-000000000000]; expect 200 and FleetAttributes=[].
3. Cause 2: fixture gamelift-game-session-errors-invalid-fleet, step task#2 — DescribeGameSessions with that unknown FleetId; expect 200 and GameSessions=[].
4. Cause 3: fixture gamelift-fleet-error-paths, step task#2 — DeleteFleet on the unknown fleet; compare the NotFoundException Message template.
5. Cause 4: fixture gamelift-game-session-errors-invalid-fleet, step task#1 — CreateGameSession on the unknown fleet; compare the Message template.
6. Cause 5: fixture gamelift-player-session-error-paths, step task#1 — CreatePlayerSession on a non-existent game session; compare the Message template.
