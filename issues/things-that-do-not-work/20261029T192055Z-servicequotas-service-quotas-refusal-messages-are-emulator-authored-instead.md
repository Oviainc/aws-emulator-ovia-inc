Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-error-paths service-quotas-quota-change-history-and-support
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: servicequotas|GetServiceQuota|4xx->4xx|$.Message
Issue-Key: servicequotas|ListServiceQuotas|4xx->4xx|$.Message
Issue-Key: servicequotas|RequestServiceQuotaIncrease|4xx->4xx|$.Message
Issue-Key: servicequotas|GetRequestedServiceQuotaChange|4xx->4xx|$.Message
Issue-Key: servicequotas|ListRequestedServiceQuotaChangeHistoryByQuota|4xx->4xx|$.Message
Issue-Key: servicequotas|ListRequestedServiceQuotaChangeHistory|4xx->4xx|$.Message
Issue-Key: servicequotas|CreateSupportCase|4xx->4xx|$.Message
Issue-Commit: 5bf04d9c01ed3990fbed03ee0a0bc59641f482e6

# Service Quotas refusal messages are emulator-authored instead of the service's fixed text

> _Note: this issue was identified against the code base at commit `5bf04d9c01ed3990fbed03ee0a0bc59641f482e6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetServiceQuota

## Description
Across seven refusal paths the emulator answers the right HTTP status (400) but composes its own diagnostic sentence instead of the service's message. Each of AWS's messages here is FIXED text for its error class - it interpolates nothing from the request - while the emulator's message is a lower-case description that quotes the request's ServiceCode, QuotaCode, RequestId or DesiredValue. In two of the seven the emulator also grades the failure as the wrong fact (quota-not-found where AWS reports service-not-available-in-Region).

Cause 1 of 7 - GetServiceQuota, unknown ServiceCode + QuotaCode.
AWS: HTTP 400, Message 'The request failed because the specified quota and service do not exist.' - entirely fixed text; no part of it comes from the request. (Smithy: GetServiceQuota errors NoSuchResourceException 'The specified resource does not exist.' and IllegalArgumentException 'Invalid input was provided.'; the recorded status for this path is 400.)
Emulator: HTTP 400 with Message "no quota with ServiceCode 'not-a-real-service-xyz' and QuotaCode 'L-00000000' exists" - its own sentence, echoing the two request members.
Why: a client that branches on, logs or presents the error message sees text AWS never sends; the request values leaking into the message also make the string non-comparable across calls.
Fix: the GetServiceQuota handler's refusal construction must take its message from the crate's single wording source for this error class rather than formatting the request members.
Traces: service-quotas-error-paths#task#1.

Cause 2 of 7 - ListServiceQuotas, unknown ServiceCode.
AWS: HTTP 400, Message 'This service is not available in the current Region. Choose a different Region or a different service. ' - fixed text including its trailing space; nothing is interpolated.
Emulator: HTTP 400 with Message "no service quotas are available for ServiceCode 'not-a-real-service-xyz'".
Why: AWS tells the caller the ServiceCode is not integrated in this Region, which is actionable (retry elsewhere); the emulator's wording reads as 'this service has no quotas', which points a client at the wrong remedy.
Fix: the ListServiceQuotas handler's unknown-ServiceCode refusal wording.
Traces: service-quotas-error-paths#task#3.

Cause 3 of 7 - RequestServiceQuotaIncrease, DesiredValue not above the default.
AWS: HTTP 400, Message 'You must provide a quota value greater than the default quota value of 5.0'. The fixed part is 'You must provide a quota value greater than the default quota value of '; the single variable part is the named quota's DEFAULT value, rendered as a decimal. (Smithy: RequestServiceQuotaIncrease.DesiredValue required, QuotaValue double @range 0..10000000000.)
Emulator: HTTP 400 with Message "DesiredValue 3 is not an increase on the current value 5 of QuotaCode 'L-F678F1CE'" - its own sentence, and it compares against the CURRENT value and names the QuotaCode, neither of which appears in AWS's message.
Why: the client is told a different rule from the one AWS enforces (greater than the default, not greater than the current value), and the number it is given to retry with is formatted differently.
Fix: the RequestServiceQuotaIncrease handler's below-default refusal: use the fixed sentence and fill it with the quota's default value only.
Traces: service-quotas-error-paths#task#5.

Cause 4 of 7 - GetRequestedServiceQuotaChange, unknown RequestId.
AWS: HTTP 400, Message 'The request failed because the specified requestId to increase the service quota does not exist.' - fixed text; the RequestId is NOT echoed.
Emulator: HTTP 400 with Message "no quota increase request with RequestId '00000000-0000-0000-0000-000000000000' exists", echoing the request id.
Why: same message-contract break, and echoing a caller-supplied id in an error message is behavior AWS does not have here.
Fix: the GetRequestedServiceQuotaChange handler's not-found refusal wording.
Traces: service-quotas-error-paths#task#6.

Cause 5 of 7 - ListRequestedServiceQuotaChangeHistoryByQuota, unknown ServiceCode + QuotaCode: the wrong fact AND the wrong wording.
AWS: HTTP 400, Message 'This service is not available in the current Region. Choose a different Region or a different service. ' - AWS grades an unrecognised ServiceCode on this operation as service-not-available-in-Region, not as a missing quota. (Smithy: ListRequestedServiceQuotaChangeHistoryByQuota.ServiceCode and .QuotaCode both required.)
Emulator: HTTP 400 with Message "no quota with ServiceCode 'not-a-real-service-xyz' and QuotaCode 'L-00000000' exists" - it reaches the quota lookup and reports a missing quota.
Why: the precedence is inverted - the ServiceCode must be rejected before the QuotaCode is considered - so a client cannot distinguish 'service not integrated here' from 'quota code wrong'.
Fix: the ListRequestedServiceQuotaChangeHistoryByQuota handler must apply the unknown-ServiceCode check first and state the service-not-available fact. The open issue 20261029T164513Z-service-quotas-listrequestedservicequotachangehistorybyquota-error-response.md observes that this operation does not use the crate's shared 'no such service' wording, but it carries no replay key for this step, so this divergence is not owned by it.
Traces: service-quotas-quota-change-history-and-support#task#1.

Cause 6 of 7 - ListRequestedServiceQuotaChangeHistory, unknown ServiceCode.
AWS: HTTP 400, Message 'This service is not available in the current Region. Choose a different Region or a different service. ' - fixed text.
Emulator: HTTP 400 with Message "no service quotas are available for ServiceCode 'not-a-real-service-xyz'".
Why: as cause 2 - the wrong remedy is suggested, and a request value is interpolated into a message that has no variable parts.
Fix: the ListRequestedServiceQuotaChangeHistory handler's unknown-ServiceCode refusal wording.
Traces: service-quotas-quota-change-history-and-support#task#2.

Cause 7 of 7 - CreateSupportCase, unknown RequestId.
AWS: HTTP 400, Message 'The request failed because the specified requestId to increase the service quota does not exist.' - fixed text; the RequestId is not echoed. (Smithy: CreateSupportCase.RequestId required @length 1..128 @pattern ^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$.)
Emulator: HTTP 400 with Message "no quota increase request with RequestId '00000000-0000-0000-0000-000000000001' exists".
Why: same message-contract break on the support-case path, which is the one a client falls back to when an increase cannot be auto-approved.
Fix: the CreateSupportCase handler's not-found refusal wording - it should be the same single wording source cause 4 uses.
Traces: service-quotas-quota-change-history-and-support#task#3.

Note on existing issues: 20261028T055821Z-service-quotas-listservicequotas-error-response.md (open) notes that ListServiceQuotas hand-rolls its own 'no such quota' message, and 20261029T164513Z (open) notes the same for ListRequestedServiceQuotaChangeHistoryByQuota; neither carries the replay key for these steps and the replay reports owned_by [] for all seven clusters, so all seven are net-new here. stub_path is empty on all seven.

## Evidence
All seven clusters are verdict=mismatch with expected_status 400 and actual_status 400 - the status class is right, the $.Message body is not. Copied from the replay detail:

1. `service=servicequotas|GetServiceQuota|status=400->400|keys=$.Message`, fixture service-quotas-error-paths, step task#1: `$.Message: value_mismatch (expected 'The request failed because the specified quota and service do not exist.', got "no quota with ServiceCode 'not-a-real-service-xyz' and QuotaCode 'L-00000000' exists")`
2. `service=servicequotas|ListServiceQuotas|status=400->400|keys=$.Message`, fixture service-quotas-error-paths, step task#3: `$.Message: value_mismatch (expected 'This service is not available in the current Region. Choose a different Region or a different service. ', got "no service quotas are available for ServiceCode 'not-a-real-service-xyz'")`
3. `service=servicequotas|RequestServiceQuotaIncrease|status=400->400|keys=$.Message`, fixture service-quotas-error-paths, step task#5: `$.Message: value_mismatch (expected 'You must provide a quota value greater than the default quota value of 5.0', got "DesiredValue 3 is not an increase on the current value 5 of QuotaCode 'L-F678F1CE'")`
4. `service=servicequotas|GetRequestedServiceQuotaChange|status=400->400|keys=$.Message`, fixture service-quotas-error-paths, step task#6: `$.Message: value_mismatch (expected 'The request failed because the specified requestId to increase the service quota does not exist.', got "no quota increase request with RequestId '00000000-0000-0000-0000-000000000000' exists")`
5. `service=servicequotas|ListRequestedServiceQuotaChangeHistoryByQuota|status=400->400|keys=$.Message`, fixture service-quotas-quota-change-history-and-support, step task#1: `$.Message: value_mismatch (expected 'This service is not available in the current Region. Choose a different Region or a different service. ', got "no quota with ServiceCode 'not-a-real-service-xyz' and QuotaCode 'L-00000000' exists")`
6. `service=servicequotas|ListRequestedServiceQuotaChangeHistory|status=400->400|keys=$.Message`, fixture service-quotas-quota-change-history-and-support, step task#2: `$.Message: value_mismatch (expected 'This service is not available in the current Region. Choose a different Region or a different service. ', got "no service quotas are available for ServiceCode 'not-a-real-service-xyz'")`
7. `service=servicequotas|CreateSupportCase|status=400->400|keys=$.Message`, fixture service-quotas-quota-change-history-and-support, step task#3: `$.Message: value_mismatch (expected 'The request failed because the specified requestId to increase the service quota does not exist.', got "no quota increase request with RequestId '00000000-0000-0000-0000-000000000001' exists")`

Fixed text vs variable parts: in clusters 1, 2, 4, 5, 6 and 7 the recorded AWS message is entirely fixed text for the error class - no ServiceCode, QuotaCode or RequestId appears in it, so the ids the emulator interpolates are additions, not substitutions. In cluster 3 the only variable part is the quota's DEFAULT value ('...default quota value of ' + value, rendered as a decimal); the recorded 5.0 is this run's value for that quota and is not part of the template.

Smithy evidence (services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json): GetServiceQuota / GetRequestedServiceQuotaChange / ListServiceQuotas / ListRequestedServiceQuotaChangeHistory / ListRequestedServiceQuotaChangeHistoryByQuota / RequestServiceQuotaIncrease / CreateSupportCase all declare IllegalArgumentException (client, httpError 400, doc 'Invalid input was provided.') and NoSuchResourceException (client, httpError 404, doc 'The specified resource does not exist.'), both carrying a single Message member of type ExceptionMessage. The recordings answer 400 on all seven paths, so only the Message text is at issue here - not the status.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay the simparity-wire-aws-v1 wire fixture corpus.
3. Fixture `service-quotas-error-paths`: step `task#1` calls GetServiceQuota with ServiceCode='not-a-real-service-xyz', QuotaCode='L-00000000'; step `task#3` calls ListServiceQuotas with the same ServiceCode; step `task#5` calls RequestServiceQuotaIncrease with a DesiredValue below the quota's default; step `task#6` calls GetRequestedServiceQuotaChange with an unknown RequestId. Each answers HTTP 400 with the emulator's own lower-case sentence in $.Message instead of the recorded text.
4. Fixture `service-quotas-quota-change-history-and-support`: step `task#1` calls ListRequestedServiceQuotaChangeHistoryByQuota with an unknown ServiceCode (and a QuotaCode), step `task#2` calls ListRequestedServiceQuotaChangeHistory with an unknown ServiceCode, step `task#3` calls CreateSupportCase with an unknown RequestId. Same shape: 400 with emulator-authored $.Message; on step `task#1` the emulator additionally reports a missing QUOTA where the recording reports the SERVICE is not available in the Region.
5. Diff $.Message against the recorded value for each step - the status matches in all seven, only the message text diverges.
