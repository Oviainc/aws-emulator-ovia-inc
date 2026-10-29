Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.e2e-sampling-rule-drives-target-allocation
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.e2e-sampling-rule-drives-target-allocation|run#37|AssertionError|assert fresh_unprocessed == [], (
Issue-Commit: b507a4381467ffed831d4190b6b2d951be29d4b4

# xray: GetSamplingTargets refuses an unseen ClientID reporting for a deleted sampling rule — UnprocessedStatistics 400 "Sampling rule does not exist" instead of a computed zero-quota target

> _Note: this issue was identified against the code base at commit `b507a4381467ffed831d4190b6b2d951be29d4b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSamplingTargets

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: GetSamplingTargets consults the sampling-rule store per statistics document and, for a rule that has been deleted, refuses a statistics document whose ClientID it has not seen before — it answers that document under UnprocessedStatistics instead of computing a target for it. Failing check: xray.e2e-sampling-rule-drives-target-allocation.

AWS: Smithy (services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json) models GetSamplingTargets as POST /SamplingTargets -> 200 with output members SamplingTargetDocuments (SamplingTargetDocument: RuleName, FixedRate, ReservoirQuota, ReservoirQuotaTTL, Interval), LastRuleModification, UnprocessedStatistics (UnprocessedStatistics: RuleName, ErrorCode, Message) and UnprocessedBoostStatistics; the only modeled errors are InvalidRequestException (400) and ThrottledException (429). The model does not state the deleted-rule behavior, so for that part: recorded behavior only — the recording this check was built from (and which its own comments state was re-confirmed twice against real AWS) shows that the data-plane target computation is keyed by rule NAME and is a separate path from the rule store: after DeleteSamplingRule, GetSamplingRules no longer returns the rule, yet GetSamplingTargets keeps answering statistics documents that name it, for the already-known ClientID AND for a brand-new ClientID, with UnprocessedStatistics (for that RuleName) staying EMPTY in both cases. The newcomer still receives a SamplingTargetDocument for the rule, with the rule's FixedRate and a freshly computed ReservoirQuotaTTL relative to the call time; because the rule's reservoir is zero-sum and the earlier client already claimed all of it, the newcomer's ReservoirQuota is exactly 0 — a served target with no reservoir, not a refusal.

Emulator: the already-known client is still served (the preceding assertions pass), but a statistics document for the same deleted rule carrying a ClientID the emulator has not seen is turned away. The response is HTTP 200 with SamplingTargetDocuments: [] and UnprocessedStatistics: [{"RuleName": <the rule name from the request>, "ErrorCode": "400", "Message": "Sampling rule does not exist"}]. In that message, "Sampling rule does not exist" is fixed text the emulator emits and RuleName is the value echoed from the request's statistics document ('sim2real-alloc-<suffix>' in the replay — a per-run generated name, not part of the requirement). AWS emits no UnprocessedStatistics entry for this case at all, so there is no AWS wording for it to match: the entry itself is the divergence.

Why: an X-Ray SDK's sampling poller reports statistics for every rule it currently caches. Rules are deleted while pollers still hold them, and a freshly started process (new ClientID) polls with its cached/initial rule set. Against AWS such a poller gets a target document and simply ends up with ReservoirQuota 0, so it falls back to the rule's fixed rate and keeps sampling correctly. Against the emulator the same poller sees the document rejected with ErrorCode 400, so it finds no target for the rule, cannot read FixedRate/ReservoirQuota/ReservoirQuotaTTL, and either retries the failing document forever or drops sampling for that rule — and a client that treats a non-empty UnprocessedStatistics as an error surfaces a spurious failure. It also makes the zero-sum reservoir invariant (full quota to the first claimant, 0 to a newcomer) unobservable.

Fix: the GetSamplingTargets handler and its sampling-target/reservoir state in the X-Ray service crate (services/service-com-amazonaws-xray-awsxray) — the per-document target computation must not gate on the rule still being present in the rule store, nor on the ClientID having been seen before. For a statistics document naming a rule the target computation knows, emit a SamplingTargetDocument (rule FixedRate, ReservoirQuota = the unclaimed remainder of the rule's reservoir, which is 0 once fully allocated, and a ReservoirQuotaTTL computed from the call time) and leave UnprocessedStatistics empty for that RuleName.

Traces: xray.e2e-sampling-rule-drives-target-allocation — fails at the `fresh_unprocessed == []` assertion with the UnprocessedStatistics entry quoted above; the earlier assertions (known client still served after delete, full reservoir retained) pass.

## Evidence
Check xray.e2e-sampling-rule-drives-target-allocation [mismatch].

Expected (AWS, recorded): after DeleteSamplingRule, GetSamplingTargets with a statistics document {RuleName: <rule>, ClientID: <brand-new 24-hex id>, Timestamp, RequestCount: 1, SampledCount: 1, BorrowCount: 0} returns HTTP 200 with UnprocessedStatistics empty for <rule> and one SamplingTargetDocument for <rule> whose ReservoirQuota == 0 (the reservoir having been fully claimed by the earlier client) and whose ReservoirQuotaTTL is computed from the call time.

Actual (emulator): HTTP 200 with
  'SamplingTargetDocuments': [],
  'UnprocessedStatistics': [{'RuleName': 'sim2real-alloc-d39ed7210e00', 'ErrorCode': '400', 'Message': 'Sampling rule does not exist'}],
  'UnprocessedBoostStatistics': []
AssertionError: "expected the deleted rule's target computation to still answer a brand-new client rather than mark it unprocessed".
('sim2real-alloc-d39ed7210e00' is the per-run generated rule name echoed from the request; the fixed text is 'Sampling rule does not exist'.)

The same call for the ClientID that had already reported to the rule is served correctly by the emulator, which localises the defect to the unseen-ClientID path of the deleted-rule target computation.

## Steps to Reproduce
1. CreateSamplingRule with RuleName=<R>, ResourceARN='*', Priority=<p>, FixedRate=0.42, ReservoirSize=15, ServiceName/ServiceType/Host/HTTPMethod/URLPath='*', Version=1.
2. GetSamplingTargets with SamplingStatisticsDocuments=[{RuleName:<R>, ClientID:<C1>, Timestamp:now, RequestCount:500, SampledCount:50, BorrowCount:0}] and repeat until the returned target for <R> carries ReservoirQuota == 15 (the whole reservoir is now claimed by <C1>).
3. DeleteSamplingRule(RuleName=<R>); confirm GetSamplingRules no longer lists <R>.
4. GetSamplingTargets again for ClientID=<C1>, RuleName=<R> — the emulator still serves a target (matches AWS).
5. GetSamplingTargets with a BRAND-NEW ClientID=<C2> and the same RuleName=<R>.
   Expected: UnprocessedStatistics empty for <R>, plus a SamplingTargetDocument for <R> with ReservoirQuota == 0.
   Observed: SamplingTargetDocuments empty and UnprocessedStatistics [{RuleName:<R>, ErrorCode:'400', Message:'Sampling rule does not exist'}].
