Issue-Source: trace-sweep-divergence
Issue-Trace: gamelift-game-session-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gamelift-game-session-lifecycle
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|CreateGameSession|4xx->2xx|$.Message
Issue-Key: gamelift|CreateGameSession|4xx->2xx|$.__type
Issue-Key: gamelift|CreateGameSession|4xx->2xx|status
Issue-Key: gamelift|DescribeGameSessions|2xx->2xx|$.GameSessions[*]
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# GameLift CreateGameSession ignores fleet server-process capacity and creates a session

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGameSession

## Description
The emulator does not model fleet process capacity, so it accepts a CreateGameSession that AWS rejects, and the phantom session it creates then leaks into the describe path. TWO CAUSES MUST BE FIXED.

Cause 1 of 2 — CreateGameSession succeeds where AWS raises FleetCapacityExceededException.
AWS: Smithy (services/service-com-amazonaws-gamelift-gamelift) CreateGameSession models FleetCapacityExceededException (client error, HTTP 400), documented as "The specified fleet has no available instances to fulfill a CreateGameSession request. Clients can retry such requests immediately or after a waiting period." The recording shows exactly that case: an Anywhere fleet with no registered/active server process answers status 400 with __type 'FleetCapacityExceededException' and a message reporting that no process could be reserved (recorded example: 'Unable to reserve a process(No active and available server processes found on the fleet)').
Emulator: status 200 and a fully-formed GameSession in ACTIVATING (replay: "status: expected 400, got 200", "$.__type: missing (expected 'FleetCapacityExceededException')").
Why: the no-capacity path is unreachable in the emulator, so a client's retry/backoff and fleet-scaling logic — the whole reason FleetCapacityExceededException is modeled — is never exercised, and a placement that must fail succeeds, handing the caller a session id that corresponds to no server process. The requirement is the 400 status and the FleetCapacityExceededException error code; the recorded message text is an example.
Fix: the CreateGameSession handler's capacity check against fleet/compute state (no active, available server process on the target fleet/location must raise FleetCapacityExceededException).
Traces: gamelift-game-session-lifecycle#task#3.

Cause 2 of 2 — the phantom session is then returned by DescribeGameSessions.
AWS: Smithy DescribeGameSessions -> DescribeGameSessionsOutput.GameSessions : GameSessionList. Because the create in cause 1 is rejected, the recording's following DescribeGameSessions returns no entry for it (status 200, the session absent).
Emulator: status 200 but GameSessions carries the session that should never have existed (replay: "$.GameSessions[0]: extra (expected None, got {... 'Status': 'ACTIVATING', 'Location': 'custom-...' ...})").
Why: session enumeration reports game sessions that no fleet capacity backs, so a client that counts active sessions per fleet, or reconciles its own records against DescribeGameSessions, sees an entry it can never connect to. Fixing cause 1 removes the write; the describe path must also not surface sessions whose create was rejected.
Fix: the game-session state write performed by CreateGameSession (must not persist on the rejected path) and the DescribeGameSessions read path.
Traces: gamelift-game-session-lifecycle#task#4.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, state_reset_failures=0).
Cause 1 — gamelift-game-session-lifecycle#task#3, CreateGameSession: expected status 400 with $.__type = 'FleetCapacityExceededException' and a Message reporting no active/available server process (recorded example 'Unable to reserve a process(No active and available server processes found on the fleet)'); got status 200 with neither $.__type nor $.Message, i.e. a created session.
Cause 2 — gamelift-game-session-lifecycle#task#4, DescribeGameSessions, 200/200: $.GameSessions[0] extra — the emulator returns the session created in task#3 (Status 'ACTIVATING', MaximumPlayerSessionCount 10, Location 'custom-sim2real-probe-gamelift-loc-...') where AWS returns no such entry.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout.
2. Fixture gamelift-game-session-lifecycle, step task#3: CreateGameSession against the Anywhere fleet created earlier in the fixture, which has no active/available server process. Expect HTTP 400 with error code FleetCapacityExceededException; the emulator returns 200 and a GameSession.
3. Same fixture, step task#4: DescribeGameSessions for that fleet. Expect no game session in GameSessions; the emulator lists the session created in step task#3.
