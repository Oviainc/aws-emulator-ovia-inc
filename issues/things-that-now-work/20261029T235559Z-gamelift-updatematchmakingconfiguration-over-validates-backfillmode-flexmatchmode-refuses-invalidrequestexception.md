Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: gamelift.matchmaking-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|UpdateMatchmakingConfiguration|InvalidRequestException
Issue-Commit: beb9379647f6ddd981e0914c2206ccd791a0c1ce

# GameLift UpdateMatchmakingConfiguration over-validates BackfillMode/FlexMatchMode: refuses InvalidRequestException on a request AWS accepts

> _Note: this issue was identified against the code base at commit `beb9379647f6ddd981e0914c2206ccd791a0c1ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateMatchmakingConfiguration

## Description
REOPENS 20261029T183132Z-gamelift-update-operations-return-success-but-do-not.md: its fix left the call refused.

That issue's Cause 3 asked for UpdateMatchmakingConfiguration to persist the members a request supplies. The handler is now implemented and does merge-and-persist, but the same change added a pre-write cross-member rejection that now refuses a request AWS accepts, so the operation is unusable for the case the behavioral corpus records.

One cause.

Cause 1 of 1 — UpdateMatchmakingConfiguration rejects BackfillMode=AUTOMATIC whenever the resulting configuration's FlexMatchMode is STANDALONE.

AWS: UpdateMatchmakingConfiguration accepts BackfillMode=AUTOMATIC on a configuration whose FlexMatchMode is STANDALONE and answers HTTP 200 with Configuration.BackfillMode == "AUTOMATIC"; an independent DescribeMatchmakingConfigurations then reports BackfillMode AUTOMATIC. Smithy: UpdateMatchmakingConfiguration input members BackfillMode (enum BackfillMode, AUTOMATIC | MANUAL) and FlexMatchMode (enum FlexMatchMode, STANDALONE | WITH_QUEUE), both OPTIONAL and both unconstrained by any model trait; the modeled errors are InternalServiceException (500), InvalidRequestException (400), NotFoundException (400), UnsupportedRegionException (400). The only statement about this pair is prose in BackfillMode's documentation — "Automatic backfill is not available when FlexMatchMode is set to STANDALONE" — which is guidance about the backfill feature having no effect in STANDALONE mode, and the model attaches NO constraint or required-combination trait expressing it. The recorded behavior is the authority here: the recording shows CreateMatchmakingConfiguration(FlexMatchMode=STANDALONE, BackfillMode=MANUAL) succeeding and the subsequent UpdateMatchmakingConfiguration(Name=<name>, BackfillMode=AUTOMATIC) ALSO succeeding with the new value readable on Describe. AWS does not enforce the prose note as a request-validation rule on this operation.

Emulator: the request is refused with HTTP 400 and error code InvalidRequestException, message "BackfillMode AUTOMATIC is not available when FlexMatchMode is STANDALONE." In that message the whole text is fixed except the two enum tokens, which the emulator substitutes from its BACKFILL_MODE_AUTOMATIC / FLEX_MATCH_MODE_STANDALONE constants — i.e. it is not an AWS message template at all, because AWS emits no error for this request class. Nothing is written, so the stored configuration keeps BackfillMode MANUAL. The check that records this is an error, not a mismatch, because botocore raises on the 400 and the rest of the recorded lifecycle (Describe of the updated value, DeleteMatchmakingConfiguration, Describe-after-delete, Update-after-delete, and re-create under the freed name) never runs.

Why: a client that flips a STANDALONE matchmaker to automatic backfill — a normal, AWS-accepted configuration change — gets a hard 400 instead of a 200, and the configuration silently stays on MANUAL. Because the refusal happens over the MERGED values (the stored FlexMatchMode is consulted even when the request does not mention FlexMatchMode), any later update of an already-AUTOMATIC STANDALONE configuration is refused too, so such a configuration becomes un-updatable for every unrelated member (Description, RequestTimeoutSeconds, RuleSetName, ...). Clients and IaC tools that reconcile matchmaking configurations see a permanent, un-retryable failure.

Fix: the cross-member validation block in the UpdateMatchmakingConfiguration handler — the STANDALONE-plus-AUTOMATIC guard must be removed so the merged BackfillMode is written through and reported, since no model constraint and no recorded AWS response refuses that combination. The handler's merge-and-persist and its response-from-stored-record logic are correct and should be kept. The sibling guards in the same block (more than one GameSessionQueueArns, and queue ARNs set under STANDALONE) ARE backed by explicit model prose stating the request fails with InvalidRequestException and are out of scope for this issue. Check whether CreateMatchmakingConfiguration carries the same guard; the recorded create path only exercises MANUAL, so if a guard exists there it must not be removed on the strength of this recording.

Traces: gamelift.matchmaking-configuration-lifecycle (fails on its UpdateMatchmakingConfiguration call). Note the open issue 20261029T185309Z-gamelift-updatematchmakingconfiguration-stub.md still carries this same check id under a NotImplemented replay key; the operation is no longer a scaffold stub, so that issue does not cover this refusal.

## Evidence
Replaying gamelift.matchmaking-configuration-lifecycle:

  botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the UpdateMatchmakingConfiguration operation: BackfillMode AUTOMATIC is not available when FlexMatchMode is STANDALONE.

Expected (recorded AWS): HTTP 200, UpdateMatchmakingConfiguration output Configuration.BackfillMode == "AUTOMATIC", and a following DescribeMatchmakingConfigurations(Names=[<config>]) reporting Configurations[0].BackfillMode == "AUTOMATIC".
Actual (emulator): HTTP 400, Error.Code = "InvalidRequestException", Error.Message = "BackfillMode AUTOMATIC is not available when FlexMatchMode is STANDALONE." — fixed text with the two enum tokens as its only variable parts; no AWS error exists for this request class. State unchanged (BackfillMode stays MANUAL).

Source of the refusal: services/service-com-amazonaws-gamelift-gamelift/src/ops/update_matchmaking_configuration.rs lines 328-335 — the guard `if standalone && backfill_mode.as_deref() == Some(BACKFILL_MODE_AUTOMATIC)`, evaluated at lines 300-307 over the merged (request-or-stored) FlexMatchMode and BackfillMode, which is why it also fires for requests that do not mention FlexMatchMode. Its comment cites BackfillMode's documentation prose as the authority; the Smithy model expresses no such constraint (smithy_query doc shape=UpdateMatchmakingConfigurationInput member=BackfillMode returns only that prose note, and the member carries no constraint traits).

Because the call raises, these further recorded expectations in the same check were never reached and are not separately evidenced here: DeleteMatchmakingConfiguration removal, Describe-after-delete answering InvalidRequestException "Configuration not found: <name>", Update-after-delete answering NotFoundException "Configuration with name <name> does not exist", and reuse of the freed configuration name.

## Steps to Reproduce
1. Boot the gamelift emulator.
2. CreateMatchmakingRuleSet with a minimal valid RuleSetBody (ruleLanguageVersion 1.0, one team).
3. CreateMatchmakingConfiguration with RuleSetName=<that rule set>, RequestTimeoutSeconds=30, AcceptanceRequired=false, BackfillMode="MANUAL", FlexMatchMode="STANDALONE". Observe HTTP 200 with Configuration.BackfillMode == "MANUAL".
4. Call UpdateMatchmakingConfiguration(Name=<that configuration>, BackfillMode="AUTOMATIC").
5. Observe HTTP 400 InvalidRequestException "BackfillMode AUTOMATIC is not available when FlexMatchMode is STANDALONE." where AWS answers HTTP 200 with Configuration.BackfillMode == "AUTOMATIC".
6. DescribeMatchmakingConfigurations(Names=[<that configuration>]) and observe BackfillMode still "MANUAL".
7. Re-replay gamelift.matchmaking-configuration-lifecycle to confirm the whole lifecycle (update, describe, delete, describe-after-delete, update-after-delete, name reuse) now passes.
