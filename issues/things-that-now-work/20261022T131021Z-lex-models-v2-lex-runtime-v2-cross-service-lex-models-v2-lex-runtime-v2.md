Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-models__lexv2-runtime.recognize-text-elicits-then-resolves-slot-synonym lexv2-models__lexv2-runtime.recognize-text-fills-slot-from-utterance lexv2-models__lexv2-runtime.recognize-text-rejects-out-of-vocabulary-slot-value lexv2-models__lexv2-runtime.unbuilt-bot-locale-recognize-text-is-rejected
Issue-Key: lexv2-models__lexv2-runtime|CreateBot|AmbiguousService
Issue-Commit: 6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1

# Cross-service Lex Models V2 + Lex Runtime V2 checks never run: router refuses every 'lex'-signed request with AmbiguousService

> _Note: this issue was identified against the code base at commit `6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBot

## Description
Cause 1 of 1: the four cross-service (Lex Models V2 + Lex Runtime V2) behavioral checks die on their very first call, lexmodelsv2 CreateBot, because the emulator's request router resolves the target service from the sigv4 signing name / endpoint prefix alone. Four emulated services legitimately share the signing name 'lex', so the router refuses the request rather than dispatching it. Nothing after that first call is exercised, so the model-build-then-talk behavior these checks exist to verify (building a DRAFT locale with a custom slot type and then resolving an utterance against it) is never reached. Failing check ids: lex-models-v2__lex-runtime-v2.recognize-text-elicits-then-resolves-slot-synonym, lex-models-v2__lex-runtime-v2.recognize-text-fills-slot-from-utterance, lex-models-v2__lex-runtime-v2.recognize-text-rejects-out-of-vocabulary-slot-value, lex-models-v2__lex-runtime-v2.unbuilt-bot-locale-recognize-text-is-rejected.

AWS: a shared sigv4 signing name across the Lex family is normal AWS and is NOT an error condition — the services are distinguished by endpoint/host, not by signing name. Smithy services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/smithy-model/com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2.json declares service LexModelBuildingServiceV2, version 2020-08-07, protocol restJson1, sdkId 'Lex Models V2', awsService.endpointPrefix 'models-v2-lex', arnNamespace 'lex', sigv4Name 'lex'. services/service-com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0/... declares sdkId 'Lex Runtime V2', endpointPrefix 'runtime-v2-lex', the SAME sigv4Name 'lex'; services/service-com-amazonaws-lexmodelbuildingservice-awsdeepsensemodelbuildingservice/... declares endpointPrefix 'models.lex' and again sigv4Name 'lex'; the v1 runtime service is the fourth claimant. Smithy also pins the HTTP bindings that disambiguate these restJson1 APIs on the wire: CreateBot PUT /bots -> 202 with botId/botStatus (errors ValidationException 400, ServiceQuotaExceededException 402, ConflictException 409, PreconditionFailedException 412, ThrottlingException 429, InternalServerException 500); RecognizeText POST /bots/{botId}/botAliases/{botAliasId}/botLocales/{localeId}/sessions/{sessionId}/text -> 200 with interpretations/sessionState, errors including ResourceNotFoundException 404 and ValidationException 400. 'AmbiguousService' is not a modeled error of either Lex service and does not exist in AWS.

Emulator: every request signed for 'lex' is rejected with error code AmbiguousService; the recorded message (example only) is "4 emulated services all claim requests for 'lex': lex, lex, lex, lex; the request is rejected rather than served by an arbitrary one". botocore retried to its ceiling ('reached max retries: 3'), i.e. the response is classified as retryable/server-side rather than as a modeled client error.

Why: a client cannot build a Lex V2 bot and then converse with it against this emulator at all. The caller sees a non-AWS error code that the SDK retries and then surfaces as an unknown fault instead of a 202 CreateBot response or a modeled 4xx. Specifically unverified: a custom slot type's synonym resolving to its canonical sample value, a slot filled directly from the utterance, an out-of-vocabulary slot value being re-elicited rather than resolved, and RecognizeText being rejected while the bot locale has not been built.

Fix: the shared request router / service-resolution layer. For restJson1 services sharing a signing name it must dispatch on the awsService endpointPrefix taken from the request Host (models-v2-lex vs runtime-v2-lex vs models.lex vs runtime.lex) and/or on the operation's modeled HTTP method+URI, and must never answer AmbiguousService for a signing name AWS legitimately shares across services and API versions. Note: open issues 20261022T124043Z-glue-kinesis-analytics-v2-cross-service-glue-kinesis-analytics-v2-studio-notebook.md and 20261022T071206Z-kinesis-analytics-v2-is-entirely-unreachable-router-refuses.md report the same router defect for 'kinesisanalytics'; they carry no lex replay key, so the lex collision is filed here. The companion single-service keys are filed under the lex-runtime-v2 issue of this same sweep; the four cross-service keys below are not covered by it.

Traces: the four lex-models-v2__lex-runtime-v2 check ids listed above; all four report verdict 'error' at CreateBot.

## Evidence
Expected (AWS, per the Smithy models cited above): CreateBot returns HTTP 202 with botId/botName/botStatus (the check then polls DescribeBot to 'Available', creates the locale/slot type/intent, builds the locale, and calls RecognizeText for a 200 with interpretations, or a modeled 400/404 when the locale is unbuilt). Sharing sigv4Name 'lex' across the four Lex services is expected and harmless because they declare distinct endpointPrefixes.

Actual (emulator replay, all four cross-service checks, identical): botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateBot operation (reached max retries: 3): 4 emulated services all claim requests for 'lex': lex, lex, lex, lex; the request is rejected rather than served by an arbitrary one.

Suite report: Total 0/14 passed (0.0%); per-service pass rate lex-models-v2__lex-runtime-v2 0/4 (0%), 4 failed, every verdict 'error'. No CreateBotLocale/CreateSlotType/CreateIntent/BuildBotLocale/RecognizeText request was ever served, so neither the models-side nor the runtime-side assertion was evaluated. No failing lex check is marked ALREADY FILED or OWNED by an existing issue.

## Steps to Reproduce
1. Boot the emulator from mainline.
2. Create a role lexv2.amazonaws.com can assume via iam.create_role + iam.put_role_policy.
3. Point a botocore client at the emulator: lex = boto3.client('lexv2-models', endpoint_url=<emulator>).
4. Call lex.create_bot(botName=<name>, roleArn=<role arn>, dataPrivacy={'childDirected': False}, idleSessionTTLInSeconds=300, botType='Bot', botTags={...}).
5. Observe error code AmbiguousService naming a 4-way 'lex' collision, retried to the botocore ceiling, instead of HTTP 202 with botId/botStatus. Every other Lex call (create_bot_locale, create_slot_type, create_intent, build_bot_locale, and lexv2-runtime recognize_text) reproduces it identically because all four Lex services sign as 'lex'.
6. Replay the four lex-models-v2__lex-runtime-v2 check ids in trace_ids to confirm the fix; each must then get past CreateBot and exercise the build-then-recognize assertions.
