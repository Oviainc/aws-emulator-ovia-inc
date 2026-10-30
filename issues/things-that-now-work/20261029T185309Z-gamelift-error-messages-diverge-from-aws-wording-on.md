Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: gamelift.create-matchmaking-ruleset-malformed-json-rejected gamelift.game-session-queue-invalid-destination-rejected gamelift.start-game-session-placement-unknown-queue-rejected
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift.create-matchmaking-ruleset-malformed-json-rejected|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: gamelift.game-session-queue-invalid-destination-rejected|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: gamelift.start-game-session-placement-unknown-queue-rejected|run#11|AssertionError|assert describe_message == expected_describe, (
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# GameLift error messages diverge from AWS wording on three refusal paths (CreateMatchmakingRuleSet, CreateGameSessionQueue, DescribeGameSessionPlacement)

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMatchmakingRuleSet

## Description
Three independent causes, one shared fault: the emulator picks the right HTTP status (400) and the right error code, but writes its own English into the Message member instead of the text AWS returns for that error class. The message is part of the wire contract — clients log it, match on it and surface it to operators. A fixer must close all THREE before submitting.

RELATED (does not cover these): open issue 20261029T183132Z-gamelift-error-messages-diverge-from-aws-wording-across.md files the same fault on five OTHER operations (DescribeBuild, CreateAlias, CreateGameSession, CreateLocation, ValidateMatchmakingRuleSet). Its cause 5 is the malformed-rule-set message on ValidateMatchmakingRuleSet; cause 1 below is the same defect on the CreateMatchmakingRuleSet path, which that issue's keys do not carry.

Cause 1 of 3 — CreateMatchmakingRuleSet with a RuleSetBody that is not valid JSON.
AWS: HTTP 400, InvalidRequestException, Message "Encountered IOException attempting to extract version; location: top level". The whole string is fixed text; the "location: " suffix names the position in the rule set, which for a body that cannot be parsed at all is "top level". No parser-internal detail (line/column, expected token) is ever exposed. Smithy (com.amazonaws.gamelift#GameLift): CreateMatchmakingRuleSet.RuleSetBody required, @length 1–65535; modeled errors InvalidRequestException / LimitExceededException / TaggingFailedException / UnsupportedRegionException (client, httpError 400 defaulted) and InternalServiceException (500).
Emulator: same status and code, but Message "RuleSetBody is not valid JSON (key must be a string at line 1 column 2); location: top level" — it leaks its own JSON parser's diagnostic. The parenthesised clause varies with the body, so no client can match it.
Why: a client or test asserting AWS's fixed refusal text never matches, and the emulator's message carries implementation detail of its parser rather than GameLift's version-extraction failure.
Fix: the CreateMatchmakingRuleSet handler's rule-set-body validation path must build the same fixed-text InvalidRequestException the ValidateMatchmakingRuleSet path owes, with the position suffix ("top level" when the body is unparseable) as its only variable part.
Traces: gamelift.create-matchmaking-ruleset-malformed-json-rejected.

Cause 2 of 3 — CreateGameSessionQueue with TimeoutInSeconds below the minimum.
AWS: HTTP 400, InvalidRequestException, Message "TimeoutInSeconds must be between 10 and 600 seconds". The entire message is fixed text: the two numbers are the documented bounds of the member, not values copied from the request, and the rejected value is NOT interpolated. Smithy: CreateGameSessionQueue.TimeoutInSeconds targets WholeNumber @range min 0 (the 10–600 service rule is enforced by the service, not the model); modeled errors include InvalidRequestException at 400.
Emulator: same status and code, Message "TimeoutInSeconds must be between 10 and 600; the request asked for 5." — the fixed text is truncated ("600" with no trailing " seconds") and an extra clause echoing the request value is appended, so the message differs for every rejected value.
Why: boundary-validation assertions keyed on AWS's stable sentence fail, and the message becomes request-dependent where AWS's is constant.
Fix: the CreateGameSessionQueue input-validation path's error construction for TimeoutInSeconds.
Traces: gamelift.game-session-queue-invalid-destination-rejected.

Cause 3 of 3 — DescribeGameSessionPlacement for a PlacementId that was never created.
AWS: HTTP 400, NotFoundException, Message "No GameSessionReservation found for reservation <placementId> accountId <awsAccountId>". Fixed text: "No GameSessionReservation found for reservation " and " accountId "; the two variable parts are the PlacementId copied from the request and the caller's AWS account id taken from the request context (no trailing period). Smithy: DescribeGameSessionPlacement.PlacementId required (IdStringModel, @length 1–48, pattern ^[a-zA-Z0-9-]+$); modeled errors NotFoundException / InvalidRequestException / UnauthorizedException at 400.
Emulator: same status and code, Message "Game session placement <placementId> does not exist." — invented wording that drops the GameSessionReservation phrasing and the account-id slot entirely. (The sibling refusal in the same check, StartGameSessionPlacement against an unknown queue, matched AWS exactly, so this is specifically the Describe not-found path.)
Why: clients that distinguish "reservation absent for this account" from other not-found conditions on the message lose that information, and message assertions diverge.
Fix: the DescribeGameSessionPlacement handler's not-found error construction (it must interpolate the request's PlacementId and the caller account id into AWS's template).
Traces: gamelift.start-game-session-placement-unknown-queue-rejected.

## Evidence
All three causes matched AWS on HTTP status (400) and error code; only the Message member diverged.

Cause 1 — gamelift.create-matchmaking-ruleset-malformed-json-rejected: CreateMatchmakingRuleSet(Name=<unique>, RuleSetBody="{not valid json at all") → InvalidRequestException (correct). Expected Message 'Encountered IOException attempting to extract version; location: top level'; got 'RuleSetBody is not valid JSON (key must be a string at line 1 column 2); location: top level'. The parenthesised parser diagnostic is the divergence; everything after the semicolon happens to agree.

Cause 2 — gamelift.game-session-queue-invalid-destination-rejected: CreateGameSessionQueue(Name=<unique>, TimeoutInSeconds=5) → InvalidRequestException (correct). Expected Message 'TimeoutInSeconds must be between 10 and 600 seconds'; got 'TimeoutInSeconds must be between 10 and 600; the request asked for 5.' The '; the request asked for <n>.' clause is emulator-added and tracks the request value (5 here is this run's value, not part of any template); the word 'seconds' is missing from the fixed text.

Cause 3 — gamelift.start-game-session-placement-unknown-queue-rejected: DescribeGameSessionPlacement(PlacementId=<never-created id>) → NotFoundException (correct). Expected Message 'No GameSessionReservation found for reservation <placementId> accountId <accountId>'; got 'Game session placement <placementId> does not exist.' (observed with placement id sim2real-placement-bad-dac02f59477d and account 000000000000 — both template slots filled from the request/context in that run, not literals of the requirement).

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (simparity-behavioral-aws-v1), gamelift checks.
2. Cause 1: call CreateMatchmakingRuleSet with any RuleSetBody that is not parseable JSON (e.g. "{not valid json at all") and read Error.Message of the 400.
3. Cause 2: call CreateGameSessionQueue with TimeoutInSeconds=5 (any value below 10) and read Error.Message of the 400.
4. Cause 3: call DescribeGameSessionPlacement with a PlacementId that was never created and read Error.Message of the 400.
5. Compare each Message against the AWS template above, treating the placement id, account id and bounds as described (fixed text vs. request/context values).
6. Re-replay gamelift.create-matchmaking-ruleset-malformed-json-rejected, gamelift.game-session-queue-invalid-destination-rejected and gamelift.start-game-session-placement-unknown-queue-rejected to confirm all three causes are closed.
