Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: gamelift.delete-game-session-queue-removes-it gamelift.delete-matchmaking-ruleset-in-use-rejected gamelift.matchmaking-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift.delete-game-session-queue-removes-it|run#12|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException updating deleted queue {name!r}, got {code!r}"
Issue-Key: gamelift.delete-matchmaking-ruleset-in-use-rejected|run#18|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: gamelift.matchmaking-configuration-lifecycle|run#21|AssertionError|assert update_after_delete_message == expected_update, (
Issue-Commit: 9bd7a6b4137af6f0c08881d7038871f02a639754

# GameLift refusal fidelity: 3 error-class/message divergences on UpdateGameSessionQueue, DeleteMatchmakingRuleSet and UpdateMatchmakingConfiguration not-found/in-use paths

> _Note: this issue was identified against the code base at commit `9bd7a6b4137af6f0c08881d7038871f02a639754`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateGameSessionQueue

## Description
Three independent root causes in the GameLift emulator's refusal paths. All three operations ARE implemented and refuse where AWS refuses, with HTTP 400 — what diverges is the error CLASS on one path and the Message text on all three. A fixer must close all THREE before submitting.

Note: these are NOT reopens of the fixed stub issues for these operations (20261029T185309Z-gamelift-updategamesessionqueue-stub.md, 20261029T235559Z-gamelift-updatematchmakingconfiguration-over-validates-backfillmode-flexmatchmode-refuses-invalidrequestexception.md): the operations answer and persist correctly, only the refusal representation is wrong. The still-open stub issues 20261029T185309Z-gamelift-deletegamesessionqueue-stub.md, 20261029T185309Z-gamelift-deletematchmakingruleset-stub.md and 20261029T185309Z-gamelift-updatematchmakingconfiguration-stub.md name the same operations but cover their absence, not these messages — the checks below exercised all of them successfully.

Cause 1 of 3 — UpdateGameSessionQueue on a queue name that no longer exists (deleted, or never created) is refused in the WRONG error class and with the wrong message.
AWS: HTTP 400 with error code InvalidRequestException and Message "Queue name <name> does not exist". Fixed text: "Queue name " and " does not exist" (no trailing period); the one variable part is the queue name/ARN copied from the request's Name member. Smithy: UpdateGameSessionQueue models both InvalidRequestException and NotFoundException (client, httpError 400 defaulted), so the status alone does not disambiguate — the code does; Name is required, GameSessionQueueNameOrArn pattern ^([a-zA-Z0-9-]+|arn:.*:gamesessionqueue\/[a-zA-Z0-9-]+)$.
Emulator: HTTP 400 with code NotFoundException and Message "Game session queue <name> does not exist." — the wrong modeled error shape and invented fixed text (service-noun prefix, trailing period).
Why: a client that branches on the error code (retry/recreate on InvalidRequestException, treat NotFoundException as a different class, or match the AWS message) takes the wrong branch for a deleted queue; code-based error handling and AWS-message assertions both break. DescribeGameSessionQueues correctly returns an empty list for the deleted queue, so only the update refusal diverges.
Fix: the UpdateGameSessionQueue handler's queue-resolution failure path (services/service-com-amazonaws-gamelift-gamelift/src/ops/update_game_session_queue.rs lines 215-224) must raise InvalidRequestException, and the shared message helper crate::state::game_session_queue_not_found_message (src/state.rs lines 998-1000) must produce AWS's template.
Traces: gamelift.delete-game-session-queue-removes-it.

Cause 2 of 3 — DeleteMatchmakingRuleSet while a matchmaking configuration references the rule set is correctly refused with InvalidRequestException, but the Message is invented and interpolates state.
AWS: HTTP 400, InvalidRequestException, Message "Matchmaking rule set is still in use, cannot delete" — the WHOLE message is fixed text; nothing from the request or from the emulator's state is interpolated (no rule-set name, no reference count). Smithy: DeleteMatchmakingRuleSet models InvalidRequestException and NotFoundException, both client/400; its doc states rule sets cannot be deleted while a matchmaking configuration uses them.
Emulator: same status and code, Message "Rule set <ruleSetName> is being used by <n> matchmaking configurations and cannot be deleted." — different fixed text, plus two variable parts AWS's template does not have (the rule-set name from the request and the referencing-configuration count from state), so the text is not even stable across environments.
Why: clients and operators matching the AWS in-use message never see it; the message also leaks an internal reference count. The rest of the path is right: the rule set stays describable after the rejection, and deletes once the configuration is removed.
Fix: the DeleteMatchmakingRuleSet handler's in-use refusal construction (services/service-com-amazonaws-gamelift-gamelift/src/ops/delete_matchmaking_rule_set.rs lines 191-203).
Traces: gamelift.delete-matchmaking-ruleset-in-use-rejected.

Cause 3 of 3 — UpdateMatchmakingConfiguration on a deleted configuration carries the wrong not-found Message.
AWS: HTTP 400, NotFoundException (the code the emulator already returns), Message "Configuration with name <name> does not exist". Fixed text: "Configuration with name " and " does not exist" (no trailing period); the one variable part is the configuration name/ARN from the request's Name member. Note AWS words the SAME missing resource differently per operation: DescribeMatchmakingConfigurations answers InvalidRequestException "Configuration not found: <name>" (the emulator already matches that one), so a single shared helper cannot serve both. Smithy: UpdateMatchmakingConfiguration models InvalidRequestException and NotFoundException, both client/400; Name required, MatchmakingConfigurationName pattern ^([a-zA-Z0-9-\.]*|arn:.*:matchmakingconfiguration\/[a-zA-Z0-9-\.]*)$.
Emulator: HTTP 400, NotFoundException, Message "The matchmaking configuration <name> was not found." — invented fixed text from the shared helper.
Why: clients that match or surface the AWS message for an updated-after-delete configuration see text AWS never emits. The rest of the lifecycle is correct: BackfillMode MANUAL->AUTOMATIC persists and reads back, Describe-after-delete matches, and the freed name is reusable.
Fix: the UpdateMatchmakingConfiguration handler's configuration-resolution failure path (services/service-com-amazonaws-gamelift-gamelift/src/ops/update_matchmaking_configuration.rs lines 243-252) and the shared message helper crate::state::matchmaking_configuration_not_found_message (src/state.rs lines 1648-1650), which must be split per operation rather than shared.
Traces: gamelift.matchmaking-configuration-lifecycle.

## Evidence
All three checks reached the refusal (status 400 in every case); the divergence is the error code and/or the Message member.

Cause 1 — gamelift.delete-game-session-queue-removes-it: after DeleteGameSessionQueue, UpdateGameSessionQueue(Name=<deleted queue>, TimeoutInSeconds=60) — AssertionError: expected InvalidRequestException updating deleted queue 'sim2real-gsq-del-<suffix>', got 'NotFoundException'. The message the emulator pairs with it, from src/state.rs:999, is "Game session queue {name} does not exist." where AWS returns "Queue name {name} does not exist".

Cause 2 — gamelift.delete-matchmaking-ruleset-in-use-rejected: DeleteMatchmakingRuleSet on a rule set referenced by one matchmaking configuration — AssertionError: expected message 'Matchmaking rule set is still in use, cannot delete', got 'Rule set sim2real-mmdel-inuse-<suffix> is being used by 1 matchmaking configurations and cannot be deleted.' (the name and the count "1" are the emulator's interpolations; AWS's template has neither). Error code InvalidRequestException matched.

Cause 3 — gamelift.matchmaking-configuration-lifecycle: UpdateMatchmakingConfiguration(Name=<deleted config>, BackfillMode=MANUAL) — AssertionError: expected message 'Configuration with name sim2real-mmlife-cfg-<suffix> does not exist', got 'The matchmaking configuration sim2real-mmlife-cfg-<suffix> was not found.' Error code NotFoundException matched, and the preceding Describe-after-delete assertion (InvalidRequestException, 'Configuration not found: <name>') passed.

## Steps to Reproduce
1. Boot the emulator and point a GameLift client at it.
2. Cause 1: CreateGameSessionQueue(Name=q); DeleteGameSessionQueue(Name=q); confirm DescribeGameSessionQueues(Names=[q]) is empty; then UpdateGameSessionQueue(Name=q, TimeoutInSeconds=60) and read the error Code and Message — expect InvalidRequestException / "Queue name q does not exist".
3. Cause 2: CreateMatchmakingRuleSet(Name=rs, ...); CreateMatchmakingConfiguration(Name=cfg, RuleSetName=rs, RequestTimeoutSeconds=30, AcceptanceRequired=false, FlexMatchMode=STANDALONE); then DeleteMatchmakingRuleSet(Name=rs) and read the Message — expect exactly "Matchmaking rule set is still in use, cannot delete" with no name or count interpolated.
4. Cause 3: with cfg created as above, DeleteMatchmakingConfiguration(Name=cfg); then UpdateMatchmakingConfiguration(Name=cfg, BackfillMode=MANUAL) and read the Message — expect NotFoundException / "Configuration with name cfg does not exist", while DescribeMatchmakingConfigurations(Names=[cfg]) must keep answering InvalidRequestException / "Configuration not found: cfg".
5. Re-replay the behavioral checks in trace_ids to confirm all three causes are closed.
