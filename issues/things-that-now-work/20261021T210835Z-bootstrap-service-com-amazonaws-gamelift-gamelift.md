Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the GameLift emulator

_Filed: 2026-10-21T21:08:35Z_
_Operation: service-com-amazonaws-gamelift-gamelift | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 120 GameLift operations in `services/service-com-amazonaws-gamelift-gamelift/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow can run. Implement a first, small, stateful slice covering the core GameLift hosting loop: provision a fleet, start a game session on it, join a player, read both back, and tear the fleet down.

### Happy path to implement (in order)

1. **CreateFleet** — input `Name` (required), `Description`, `BuildId`, `ScriptId`, `EC2InstanceType`, `FleetType`, `ComputeType`, `RuntimeConfiguration`, `EC2InboundPermissions`, `NewGameSessionProtectionPolicy`, `Locations`, `Tags`. Mint a `FleetId` matching the modeled pattern `^[a-z]*fleet-[a-zA-Z0-9\-]+$` (e.g. `fleet-<uuid>`) plus a matching `FleetArn` (`arn:aws:gamelift:<region>::fleet/<FleetId>`), persist the fleet, and return `FleetAttributes` (FleetId, FleetArn, Name, Description, Status, FleetType, InstanceType, ComputeType, CreationTime, BuildId/ScriptId, NewGameSessionProtectionPolicy) plus `LocationStates` built from the stored record — not from echoed input only. `Status` should be `ACTIVE` (or `NEW` then `ACTIVE`) so game sessions can be created against it.
2. **DescribeFleetAttributes** — input `FleetIds`, `Limit`, `NextToken`; output `FleetAttributes` (list) + `NextToken`. With no `FleetIds`, return every stored fleet; with `FleetIds`, return only those and raise `NotFoundException` for an unknown id. Honor `Limit`/`NextToken` (modeled `@paginated`).
3. **CreateGameSession** — input `FleetId` or `AliasId`, `MaximumPlayerSessionCount` (required), `Name`, `GameProperties`, `CreatorId`, `GameSessionId`, `IdempotencyToken`, `GameSessionData`, `Location`. Reject an unknown fleet with `NotFoundException`. Mint a full-ARN `GameSessionId` (`arn:aws:gamelift:<region>::gamesession/<FleetId>/<id>`), persist it against the fleet, and return `GameSession` with Status `ACTIVE`, `CurrentPlayerSessionCount=0`, the requested `MaximumPlayerSessionCount`, `IpAddress`, `Port`, `DnsName`, `CreationTime`, `PlayerSessionCreationPolicy=ACCEPT_ALL`, and the stored `GameProperties`/`GameSessionData`.
4. **CreatePlayerSession** — input `GameSessionId` (required), `PlayerId` (required), `PlayerData`. Look the game session up in state; `NotFoundException` when absent, `GameSessionFullException` once `CurrentPlayerSessionCount == MaximumPlayerSessionCount`, `InvalidGameSessionStatusException` when the session is not ACTIVE. Mint `PlayerSessionId` matching `^psess-\S+$`, persist it, increment the game session's `CurrentPlayerSessionCount`, and return `PlayerSession` (PlayerSessionId, PlayerId, GameSessionId, FleetId, FleetArn, Status=RESERVED, CreationTime, IpAddress, Port, DnsName, PlayerData).
5. **DescribeGameSessions** — input `FleetId` / `GameSessionId` / `AliasId` / `Location` / `StatusFilter` / `Limit` / `NextToken`; output `GameSessions` + `NextToken`. Must return the persisted sessions filtered by the supplied selector (and reflect the player-count increment from step 4), with `Limit`/`NextToken` pagination.
6. **DeleteFleet** — input `FleetId` (required). Remove the fleet (and its game/player sessions) from state; a subsequent `DescribeFleetAttributes` with that id must raise `NotFoundException` rather than still returning the fleet.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the request back, and no minting an id that is never stored: a create followed by a describe must return exactly what was created, and a delete must be observable by the next read.

## State Model

In `src/state.rs` (`ComAmazonawsGameliftGameliftState`, reached via `crate::state::service_state(&*self.engine)`):

- `fleets: HashMap<String /*FleetId*/, Fleet>` where `Fleet { fleet_id, fleet_arn, name, description, status: FleetStatus, fleet_type, instance_type, compute_type, build_id, script_id, runtime_configuration, ec2_inbound_permissions, new_game_session_protection_policy, metric_groups, locations: Vec<LocationState>, tags, creation_time, termination_time }`.
- `game_sessions: HashMap<String /*GameSessionId ARN*/, GameSessionRecord>` where `GameSessionRecord { game_session_id, name, fleet_id, fleet_arn, status: GameSessionStatus, creation_time, termination_time, current_player_session_count, maximum_player_session_count, game_properties, game_session_data, creator_id, ip_address, port, dns_name, location, player_session_creation_policy }`.
- `player_sessions: HashMap<String /*PlayerSessionId*/, PlayerSessionRecord>` where `PlayerSessionRecord { player_session_id, player_id, game_session_id, fleet_id, fleet_arn, status: PlayerSessionStatus, creation_time, termination_time, ip_address, port, dns_name, player_data }`.
- Optional helper indexes for the list/describe filters: `game_sessions_by_fleet: HashMap<FleetId, Vec<GameSessionId>>`, `player_sessions_by_game_session: HashMap<GameSessionId, Vec<PlayerSessionId>>`, and an `idempotency_tokens: HashMap<String, GameSessionId>` map so a repeated `CreateGameSession` with the same `IdempotencyToken` returns the same session (and `IdempotentParameterMismatchException` on a conflicting one).
- A monotonic counter or UUID source for id minting, plus opaque pagination tokens (e.g. an encoded offset) shared by the paginated describes.

Done when a realistic integration script runs the six calls above end-to-end against the emulator and every response body is derived from stored state (fleet attributes match what was created, the game session shows the right max/current player counts, the player session points at the right fleet and game session, and the fleet is gone after DeleteFleet). All other GameLift operations (builds, scripts, aliases, queues, matchmaking, game server groups, container fleets, scaling, compute, tagging) stay 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-gamelift-gamelift/OPERATIONS.md` lists 120 operations for `awsJson1_1` / `X-Amz-Target: GameLift.<Operation>` and gives the exact contracts cited above: CreateFleet (line 63-67: `Name` required, output `FleetAttributes` + `LocationStates`), DescribeFleetAttributes (line 285-290: `@paginated(inputToken=NextToken, outputToken=NextToken, items=FleetAttributes)`), CreateGameSession (line 81-85: `MaximumPlayerSessionCount` required, output `GameSession`, errors include `NotFoundException`, `IdempotentParameterMismatchException`, `FleetCapacityExceededException`), CreatePlayerSession (line 111-115: `GameSessionId`+`PlayerId` required, output `PlayerSession`, errors include `GameSessionFullException`, `InvalidGameSessionStatusException`, `NotFoundException`), DescribeGameSessions (line 383-388: `@paginated`, output `GameSessions`), DeleteFleet (line 165-168: `FleetId` required, `NotFoundException`). Shape members for `FleetAttributes`, `GameSession` (incl. `CurrentPlayerSessionCount`, `MaximumPlayerSessionCount`, `Status`) and `PlayerSession` (incl. `PlayerSessionId` pattern `^psess-\S+$`, `Status` RESERVED/ACTIVE) come from `smithy_query shape=FleetAttributes,GameSession,PlayerSession` on `smithy-model/com-amazonaws-gamelift-gamelift.json`. A grep for `^//! SCAFFOLDED, NOT IMPLEMENTED:` in `src/ops/` matches every operation file, including `create_fleet.rs`, `create_game_session.rs`, `create_player_session.rs`, `describe_fleet_attributes.rs`, `describe_game_sessions.rs` and `delete_fleet.rs` — all currently return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). No tests exercise any of this.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI GameLift client at it.
2. `aws gamelift create-fleet --name demo-fleet --ec2-instance-type c5.large --compute-type EC2 --build-id build-demo` — expect a `FleetAttributes` with a `fleet-...` FleetId, matching FleetArn and an ACTIVE/NEW status; currently HTTP 501 NotImplemented.
3. `aws gamelift describe-fleet-attributes --fleet-ids <FleetId>` — expect the fleet just created, with the same Name/Description/InstanceType.
4. `aws gamelift create-game-session --fleet-id <FleetId> --maximum-player-session-count 4 --name demo-session` — expect a `GameSession` with Status ACTIVE, MaximumPlayerSessionCount 4, CurrentPlayerSessionCount 0 and a gamesession ARN id.
5. `aws gamelift create-player-session --game-session-id <GameSessionId> --player-id player-1` — expect a `PlayerSession` with a `psess-...` id, Status RESERVED, and the fleet/game-session ids of the parent.
6. `aws gamelift describe-game-sessions --fleet-id <FleetId>` — expect exactly the one session, now with CurrentPlayerSessionCount 1.
7. `aws gamelift delete-fleet --fleet-id <FleetId>`, then re-run `describe-fleet-attributes --fleet-ids <FleetId>` — expect `NotFoundException`, not the deleted fleet.
8. Repeat `create-game-session` with the same `--idempotency-token` twice — expect the same GameSessionId back, not a second session.
