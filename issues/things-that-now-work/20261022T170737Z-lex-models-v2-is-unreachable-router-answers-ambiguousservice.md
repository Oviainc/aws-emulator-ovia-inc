Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-models.bot-locale-build-transitions-to-built lexv2-models.bot-version-snapshots-draft-intent lexv2-models.build-rejects-locale-with-no-utterances lexv2-models.build-rejects-required-slot-without-priority lexv2-models.create-slot-rejects-unknown-slot-type lexv2-models.create-slot-type-rejects-duplicate-name lexv2-models.delete-bot-locale-cascades-to-intents
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models|CreateBot|AmbiguousService
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Lex Models V2 is unreachable: router answers AmbiguousService for every 'lex'-signed request, so no build/version/validation check runs

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBot

## Description
Cause 1 of 1: all seven single-service Lex Models V2 behavioral checks die on their first call, CreateBot, because the emulator's request router resolves the target service from the sigv4 signing name / endpoint prefix alone. Four emulated services in the Lex family legitimately share the signing name 'lex', and instead of dispatching, the router refuses the request with a non-AWS error code 'AmbiguousService'. Nothing after that first call is served, so none of the model-building behavior these checks exist to verify is reached. Failing check ids: lex-models-v2.bot-locale-build-transitions-to-built, lex-models-v2.bot-version-snapshots-draft-intent, lex-models-v2.build-rejects-locale-with-no-utterances, lex-models-v2.build-rejects-required-slot-without-priority, lex-models-v2.create-slot-rejects-unknown-slot-type, lex-models-v2.create-slot-type-rejects-duplicate-name, lex-models-v2.delete-bot-locale-cascades-to-intents.

AWS: a shared sigv4 signing name across the Lex family is normal AWS and is NOT an error condition — the services are distinguished by endpoint/host (and, within a service, by the modeled HTTP method+URI), never by signing name. Smithy services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/smithy-model/com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2.json declares service LexModelBuildingServiceV2 (sdkId 'Lex Models V2', protocol restJson1, awsService.endpointPrefix 'models-v2-lex', arnNamespace 'lex', sigv4Name 'lex'); the Lex Runtime V2 model declares endpointPrefix 'runtime-v2-lex' with the same sigv4Name 'lex'; the v1 model-building and v1 runtime services declare 'models.lex' / 'runtime.lex' and again sigv4Name 'lex' — the four claimants the router reports. Smithy pins the bindings and responses the checks need: CreateBot PUT /bots -> 202 {botId (length 10, ^[0-9a-zA-Z]+$), botName, botStatus, creationDateTime}; CreateSlotType PUT /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/slottypes -> 200 {slotTypeId}; CreateSlot PUT /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/intents/{intentId}/slots -> 200 {slotId}; BuildBotLocale POST /bots/{botId}/botversions/{botVersion}/botlocales/{localeId} -> 202 {botLocaleStatus, lastBuildSubmittedDateTime}; CreateBotVersion PUT /bots/{botId}/botversions -> 202 {botVersion (^[0-9]+$), botStatus}; DeleteBotLocale DELETE /bots/{botId}/botversions/{botVersion}/botlocales/{localeId} -> 202 {botLocaleStatus}. Each of those operations models exactly the errors ValidationException 400, ServiceQuotaExceededException 402, ConflictException 409, PreconditionFailedException 412, ThrottlingException 429, InternalServerException 500 — 'AmbiguousService' is not a modeled error of this service and does not exist in AWS.

Emulator: every request signed for 'lex' is rejected with error code AmbiguousService; the recorded message (an example only) is "4 emulated services all claim requests for 'lex': lex, lex, lex, lex; the request is rejected rather than served by an arbitrary one". botocore retried to its ceiling ('reached max retries: 3'), i.e. the response is classified as retryable/server-side rather than as a modeled client error.

Why: a client cannot use Lex V2 model building against this emulator at all — the first CreateBot call fails with an error code no SDK knows, after three retries, instead of a 202 with botId/botStatus. Specifically unverified for this service: a DRAFT bot locale transitioning Creating->Building->Built after BuildBotLocale; CreateBotVersion snapshotting the DRAFT intent set into a numbered version; BuildBotLocale rejecting a locale whose intent has no sample utterances with a modeled 400 ValidationException; BuildBotLocale rejecting a required slot that has no slot priority; CreateSlot rejecting an unknown slotTypeId; CreateSlotType rejecting a duplicate slot type name in the same locale with a modeled 409 ConflictException; and DeleteBotLocale cascading to the locale's intents and slot types.

Fix: the shared request router / service-resolution layer (not any Lex handler). For restJson1 services that share a signing name it must dispatch on the awsService endpointPrefix taken from the request Host (models-v2-lex vs runtime-v2-lex vs models.lex vs runtime.lex) and/or on the operation's modeled HTTP method+URI, and must never answer AmbiguousService for a signing name AWS legitimately shares across services and API versions. Related open issues report the same router defect but carry different replay keys and so do not cover these seven: 20261022T131021Z-lex-models-v2-lex-runtime-v2-cross-service-lex-models-v2-lex-runtime-v2.md (the four cross-service lex keys, which the suite marks OWNED) and 20261022T131021Z-lex-runtime-v2-is-unreachable-router-answers-ambiguousservice.md (the runtime-only keys).

Traces: the seven lex-models-v2 check ids listed above; every one reports verdict 'error' at CreateBot with the same AmbiguousService response.

## Evidence
Expected (AWS, per the Smithy model cited above): CreateBot returns HTTP 202 with botId/botName/botStatus ('Creating', settling to 'Available'), after which the checks create a bot locale, a custom slot type, an intent and a slot, call BuildBotLocale for a 202 and poll DescribeBotLocale to 'Built', or receive the modeled 400 ValidationException / 409 ConflictException for the negative cases. Sharing sigv4Name 'lex' across the four Lex services is expected and harmless because they declare distinct endpointPrefixes.

Actual (emulator replay, all seven lex-models-v2 checks, identical): botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateBot operation (reached max retries: 3): 4 emulated services all claim requests for 'lex': lex, lex, lex, lex; the request is rejected rather than served by an arbitrary one.

Suite report: Total 0/11 passed (0.0%); per-service pass rate lex-models-v2 0/7 (0%), 7 failed, every verdict 'error'. No CreateBotLocale / CreateSlotType / CreateIntent / CreateSlot / BuildBotLocale / CreateBotVersion / DeleteBotLocale request was ever served, so no assertion in any of the seven checks was evaluated. None of these seven failures is marked ALREADY FILED or OWNED by an existing issue.

## Steps to Reproduce
1. Boot the emulator from mainline.
2. Create a role lexv2.amazonaws.com can assume via iam.create_role + iam.put_role_policy.
3. Point a botocore client at the emulator: lex = boto3.client('lexv2-models', endpoint_url=<emulator>).
4. Call lex.create_bot(botName=<name>, roleArn=<role arn>, dataPrivacy={'childDirected': False}, idleSessionTTLInSeconds=300, botType='Bot', botTags={...}).
5. Observe error code AmbiguousService naming a 4-way 'lex' collision, retried to the botocore ceiling, instead of HTTP 202 with botId/botStatus. Every other lexv2-models call (create_bot_locale, create_slot_type, create_intent, create_slot, build_bot_locale, create_bot_version, delete_bot_locale) reproduces it identically because all four Lex services sign as 'lex'.
6. Replay the seven lex-models-v2 check ids in trace_ids to confirm the fix; each must then get past CreateBot and exercise its build / version / validation / cascade assertions.
