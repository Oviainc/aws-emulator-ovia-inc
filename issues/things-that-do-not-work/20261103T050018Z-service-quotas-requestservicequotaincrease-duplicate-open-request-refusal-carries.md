Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: service-quotas.concurrent-duplicate-quota-increase-requests-dedup-race
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas.concurrent-duplicate-quota-increase-requests-dedup-race|run#10|AssertionError|assert all(err == _OPEN_REQUEST_ERROR for _, err in failures), failures
Issue-Commit: e331167544f8c61f25de403c8943054d68a37733

# service-quotas: RequestServiceQuotaIncrease duplicate-open-request refusal carries emulator-authored message with a per-run request id instead of AWS's fixed sentence (surfaced by the concurrent dedup race)

> _Note: this issue was identified against the code base at commit `e331167544f8c61f25de403c8943054d68a37733`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RequestServiceQuotaIncrease

## Description
One net-new behavioral root cause in the service-quotas (com.amazonaws.servicequotas#ServiceQuotasV20190624, awsJson1_1) emulator, surfaced by 1 failing behavioral check.

Note on existing issues: the open issue 20261030T035445Z-service-quotas-9-net-new-behavioral-root-causes.md describes the same emulator-authored-message habit for this refusal as its Cause 5, grounded only in service-quotas.duplicate-pending-quota-increase-request-rejected. It carries NO replay key for the check below (the suite reported neither OWNED-by nor ALREADY-FILED for it), so this cause is filed as net-new against this check id; a fixer closing that issue must also re-replay this check, because the concurrent path is a second call site of the same guard and the dedup race asserts the message on all four losing racers at once.

Cause 1 of 1 — RequestServiceQuotaIncrease: the one-open-request-per-quota guard answers the right error code but its own diagnostic sentence, into which it interpolates a generated request id and the QuotaCode.
AWS: HTTP 400 / ResourceAlreadyExistsException (Smithy: RequestServiceQuotaIncrease errors include ResourceAlreadyExistsException, errorKind client, httpError 400, a single Message member of ExceptionMessage, doc 'The specified resource already exists.'; input members ServiceCode/QuotaCode/DesiredValue all required). Recorded behavior: when a quota already has an open (PENDING/CASE_OPENED) increase request, every further RequestServiceQuotaIncrease on that same quota is refused with the FIXED Message 'Only one open service quota increase request is allowed per quota.' — the sentence has NO variable parts: no request id, no QuotaCode, no ServiceCode and no DesiredValue appears in it. The dedup semantics themselves are right in the emulator: of 5 identical racing requests exactly one opened a request and 4 were refused with ResourceAlreadyExistsException; only the message diverges.
Emulator: error code ResourceAlreadyExistsException (correct) with Message "request <generated request id> for QuotaCode '<QuotaCode>' is still awaiting a decision" — observed on all four losing racers as "request e02e112e618201a03a4a190b66502712 for QuotaCode 'L-1216C47A' is still awaiting a decision"; the 32-hex id is the winner's generated request id from this run and 'L-1216C47A' is the quota the check happened to pick — neither is part of any template, and neither belongs in AWS's message at all.
Why: the message is the only human/log-readable part of this refusal, and a client that matches AWS's fixed sentence to decide 'a request is already open on this quota, poll the history instead of retrying' never matches. Worse, because the emulator embeds a freshly generated request id, the refusal text differs on every run and between concurrent losers, so no client assertion, alarm pattern or log signature over this refusal is stable — exactly what the concurrency check exercises when it requires all losers of a race to report one identical refusal.
Fix: the RequestServiceQuotaIncrease handler's one-open-request-per-quota guard (the duplicate/dedup branch that resolves the existing open request before admitting a new one) — emit the service's fixed sentence from the crate's single wording source for this error class and stop formatting the stored request's id, the QuotaCode or the DesiredValue into it. The fix must cover the concurrent path: all losers of a simultaneous burst must get the identical fixed message, and the winner must still be readable by its Id through ListRequestedServiceQuotaChangeHistoryByQuota and GetRequestedServiceQuotaChange.
Traces: service-quotas.concurrent-duplicate-quota-increase-requests-dedup-race.

## Evidence
Suite: simparity-behavioral-aws-v1, 27/40 passed (67.5%), 13 mismatches, all under service heading 'service-quotas'. 12 of the 13 are marked OWNED by 20261030T035445Z-service-quotas-9-net-new-behavioral-root-causes.md and were skipped; this check is the only one carrying no owner and no ALREADY-FILED marker.

Replay detail, copied from the report for service-quotas.concurrent-duplicate-quota-increase-requests-dedup-race (check_service_quotas_concurrent_duplicate_quota_increase_requests_dedup_race.py line 79, `assert all(err == _OPEN_REQUEST_ERROR for _, err in failures)`):

AssertionError: [('error', ('ResourceAlreadyExistsException', "request e02e112e618201a03a4a190b66502712 for QuotaCode 'L-1216C47A' is still awaiting a decision")), ('error', ('ResourceAlreadyExistsException', "request e02e112e618201a03a4a190b66502712 for QuotaCode 'L-1216C47A' is still awaiting a decision")), ('error', ('ResourceAlreadyExistsException', "request e02e112e618201a03a4a190b66502712 for QuotaCode 'L-1216C47A' is still awaiting a decision")), ('error', ('ResourceAlreadyExistsException', "request e02e112e618201a03a4a190b66502712 for QuotaCode 'L-1216C47A' is still awaiting a decision"))]

Expected by the check (_OPEN_REQUEST_ERROR): ('ResourceAlreadyExistsException', 'Only one open service quota increase request is allowed per quota.').

So: status class and error code match (400 / ResourceAlreadyExistsException); the diverging part is strictly the Message. Fixed text required by AWS: the whole sentence 'Only one open service quota increase request is allowed per quota.' Variable parts required by AWS: none. Parts the emulator adds that AWS never sends: the generated request id of the already-open request (per-run value) and the QuotaCode taken from the request.

The check asserted `successes` was non-empty BEFORE this assertion and that assertion passed, so exactly one of the 5 racers was admitted — the dedup/serialization behaviour is correct and is NOT part of this finding. The subsequent history and GetRequestedServiceQuotaChange assertions were never reached.

Smithy evidence (services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json):
- RequestServiceQuotaIncrease errors: AccessDeniedException 403, DependencyAccessDeniedException 403, IllegalArgumentException 400, InvalidResourceStateException 405, NoSuchResourceException 404, QuotaExceededException 409, ResourceAlreadyExistsException 400, ServiceException 500, TooManyRequestsException 429.
- ResourceAlreadyExistsException: structure, errorKind client, httpError 400, single member Message -> ExceptionMessage, doc 'The specified resource already exists.' The model constrains the error class and status only; the sentence itself is recorded behavior.
- Input: ServiceCode required (@length 1..63, @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$), QuotaCode required (@length 1..128, @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$), DesiredValue required (QuotaValue double @range 0..10000000000).

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. Replay the simparity-behavioral-aws-v1 behavioral suite; service-quotas.concurrent-duplicate-quota-increase-requests-dedup-race mismatches.
3. Manually: page ListServiceQuotas(ServiceCode='ec2') and pick an adjustable On-Demand vCPU-family quota (e.g. a 'Running On-Demand ...' quota); note its Value.
4. Fire 5 identical RequestServiceQuotaIncrease(ServiceCode='ec2', QuotaCode=<that quota>, DesiredValue=Value+1000+jitter) calls concurrently from 5 threads.
5. Observe exactly one call succeed (correct) and the other 4 refused with HTTP 400 / ResourceAlreadyExistsException (correct) but with Message "request <winner's generated id> for QuotaCode '<QuotaCode>' is still awaiting a decision".
6. Diff $.Message of each refusal against AWS's fixed sentence 'Only one open service quota increase request is allowed per quota.' — it must be byte-identical for every loser, with no request id, QuotaCode, ServiceCode or DesiredValue interpolated.
7. The sequential form of the same guard is the same defect: open one increase request on an adjustable quota, then call RequestServiceQuotaIncrease again on that quota with a different DesiredValue.
8. After fixing, confirm the winner is still retrievable: ListRequestedServiceQuotaChangeHistoryByQuota(ServiceCode='ec2', QuotaCode=<quota>) contains exactly one entry for the raced DesiredValue, and GetRequestedServiceQuotaChange(RequestId=<winner Id>) returns it with that DesiredValue.
