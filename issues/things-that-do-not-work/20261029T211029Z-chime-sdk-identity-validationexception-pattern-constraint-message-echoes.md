Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chime-sdk-identity.create-app-instance-bot-malformed-lex-arn-rejected
Issue-Shape-Id: com.amazonaws.chimesdkidentity#ChimeIdentityService
Issue-Key: chime-sdk-identity.create-app-instance-bot-malformed-lex-arn-rejected|run#17|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# chime-sdk-identity: ValidationException pattern-constraint message echoes the Smithy @pattern verbatim, including the ^ and $ anchors AWS strips

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppInstanceBot

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: the request-shape validator's `ValidationException` message for a @pattern violation reproduces the Smithy pattern string VERBATIM, including the leading `^` and trailing `$` regex anchors. AWS renders the same clause with those anchors removed.

AWS: Smithy model services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/smithy-model/com-amazonaws-chimesdkidentity-chimeidentityservice.json declares `LexConfiguration$LexBotAliasArn` -> `com.amazonaws.chimesdkidentity#LexBotAliasArn`, a string with @required, @length(min:15, max:2048) and @pattern "^arn:aws:lex:[a-z]{2}-[a-z]+-\\d{1}:\\d{12}:bot-alias/[A-Z0-9]{10}/[A-Z0-9]{10}$". A CreateAppInstanceBot whose Configuration.Lex.LexBotAliasArn does not match is refused by the request-shape validator (not by a business rule, so NOT the modeled BadRequestException) with HTTP 400 and error code `ValidationException`. The recorded message follows AWS's standard constraint-validation template, where the fixed text is:
  "<N> validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>"
and the variable parts are: <N> = the number of violations in this request; <value> = the rejected value taken verbatim from the request; <memberPath> = the lowerCamel dotted path of the offending member in the input; <pattern> = the member's modeled @pattern WITH the anchoring `^` and `$` removed. For this request the recording shows:
  "1 validation error detected: Value 'not-a-valid-lex-bot-alias-arn' at 'configuration.lex.lexBotAliasArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws:lex:[a-z]{2}-[a-z]+-\\d{1}:\\d{12}:bot-alias/[A-Z0-9]{10}/[A-Z0-9]{10}"
Note that AWS's <pattern> rendering is the anchor-stripped form of the modeled trait; that stripping is the requirement, not the one ARN string in this recording.

Emulator: the status (400), the error code (`ValidationException`), the violation count, the rejected value and the member path are all correct, and the surrounding positive controls pass (a shape-valid ARN is accepted, the bot is listed then deleted, no bot is materialized by the rejected call, and DescribeAppInstance still answers). Only the <pattern> substitution diverges: the emulator emits the modeled trait unmodified, so the message ends "... regular expression pattern: ^arn:aws:lex:[a-z]{2}-[a-z]+-\\d{1}:\\d{12}:bot-alias/[A-Z0-9]{10}/[A-Z0-9]{10}$" — two extra characters, the `^` prefix and the `$` suffix.

Why: the pattern clause is the machine-readable part of this message, and clients consume it. A client that asserts on, logs-and-diffs, or re-compiles the advertised pattern to pre-validate input before retrying sees a different regex source string than AWS publishes; equality assertions against AWS's message fail, and a client that compiles the echoed string and then anchors it itself ends up with a doubly-anchored expression. Because this is the shared validator's template rather than anything specific to this operation, every @pattern-constrained member of every operation in the service reports its constraint with the same two spurious characters.

Fix: the shared request-shape validation layer — the centralized "deserialize -> validate" walk invoked by `smithy_server_runtime::Operation` (the pipeline documented at services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/create_app_instance_bot.rs:10, which reaches `handle` only after validation passes). The pattern-constraint branch of that message formatter must strip a leading `^` and a trailing `$` from the modeled @pattern before interpolating it into the "Member must satisfy regular expression pattern:" clause, while leaving the compiled matching semantics unchanged. No change belongs in create_app_instance_bot.rs's `handle` body, which is never entered for this request.

Traces: chime-sdk-identity.create-app-instance-bot-malformed-lex-arn-rejected (mismatch; assertion at check line 79 comparing the full message).

Related open issue, which does NOT cover this: 20261029T182312Z-chime-sdk-identity-3-net-new-root-causes.md bundles CreateAppInstance token-reuse acceptance, the duplicate-id ConflictException message and the DeleteAppInstance user cascade — all handler-level state/business-rule causes on other operations, none of them the shared @pattern message template.

## Evidence
Check: chime-sdk-identity.create-app-instance-bot-malformed-lex-arn-rejected [mismatch]

Request (after a successful CreateAppInstance and a successful shape-valid positive-control bot that was then deleted):
  CreateAppInstanceBot(AppInstanceArn=<instance arn>, ClientRequestToken=<uuid4 hex>,
    Configuration={"Lex": {"LexBotAliasArn": "not-a-valid-lex-bot-alias-arn",
                            "LocaleId": "en_US", "RespondsTo": "STANDARD_MESSAGES"}})

Agreeing: HTTP 400, Error.Code == "ValidationException".

Diverging — Error.Message:
  EXPECTED (AWS):
    1 validation error detected: Value 'not-a-valid-lex-bot-alias-arn' at 'configuration.lex.lexBotAliasArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws:lex:[a-z]{2}-[a-z]+-\d{1}:\d{12}:bot-alias/[A-Z0-9]{10}/[A-Z0-9]{10}
  ACTUAL (emulator):
    1 validation error detected: Value 'not-a-valid-lex-bot-alias-arn' at 'configuration.lex.lexBotAliasArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws:lex:[a-z]{2}-[a-z]+-\d{1}:\d{12}:bot-alias/[A-Z0-9]{10}/[A-Z0-9]{10}$

Delta: a leading '^' and a trailing '$' that AWS does not emit. Everything else in the message is byte-identical.

Message anatomy (what is template vs. what comes from the request/model):
  FIXED TEXT: " validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: "
  FROM REQUEST: the violation count (1) and the rejected value ('not-a-valid-lex-bot-alias-arn')
  FROM INPUT SHAPE: the member path ('configuration.lex.lexBotAliasArn')
  FROM MODEL, ANCHOR-STRIPPED: the @pattern of com.amazonaws.chimesdkidentity#LexBotAliasArn

Smithy evidence (smithy_query shape=LexBotAliasArn,LexConfiguration on
services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/smithy-model/com-amazonaws-chimesdkidentity-chimeidentityservice.json):
  LexBotAliasArn: type string, length {min 15, max 2048},
    pattern "^arn:aws:lex:[a-z]{2}-[a-z]+-\\d{1}:\\d{12}:bot-alias/[A-Z0-9]{10}/[A-Z0-9]{10}$"
  LexConfiguration.LexBotAliasArn: required true, same length/pattern.
The model carries the anchors; AWS's validation message does not. The emulator is passing the trait through unmodified.

Scope: not operation-specific. ValidationException is not in CreateAppInstanceBot's modeled error list (BadRequestException, ConflictException, ForbiddenException, ResourceLimitExceededException, ServiceFailureException, ServiceUnavailableException, ThrottledClientException, UnauthorizedClientException), confirming the message originates in the shared pre-handler validator, which serves every @pattern-constrained member in the service.

Side effects correctly absent: ListAppInstanceBots returned [] both before and after the rejected call, and DescribeAppInstance still returned the AppInstance — so this is purely a message-rendering divergence.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator.
2. Build a chime-sdk-identity client pinned to region us-east-1 (this control plane is region-pinned).
3. instance_arn = CreateAppInstance(Name=<unique>, Metadata="", ClientRequestToken=<uuid4 hex>)["AppInstanceArn"].
4. Call CreateAppInstanceBot with a Configuration.Lex.LexBotAliasArn that violates the modeled @pattern, e.g.:
     CreateAppInstanceBot(AppInstanceArn=instance_arn, ClientRequestToken=<uuid4 hex>,
       Configuration={"Lex": {"LexBotAliasArn": "not-a-valid-lex-bot-alias-arn",
                              "LocaleId": "en_US", "RespondsTo": "STANDARD_MESSAGES"}})
5. Catch the ClientError and read e.response["Error"]["Code"] and ["Message"].
   Code is "ValidationException" (correct). The Message's trailing pattern clause is the modeled
   regex WITH '^' and '$'; AWS emits it WITHOUT them.
6. Generalize: repeat against any other @pattern-constrained member of this service (e.g. pass a
   ClientRequestToken violating ^[-_a-zA-Z0-9]*$, or a Name violating ResourceName's pattern) and
   observe the same spurious anchors in the "Member must satisfy regular expression pattern:" clause,
   confirming the shared validator's formatter is the cause.
7. Replay: run_behavioral_suite() and re-run chime-sdk-identity.create-app-instance-bot-malformed-lex-arn-rejected.
