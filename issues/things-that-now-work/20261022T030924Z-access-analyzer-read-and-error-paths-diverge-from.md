Issue-Source: trace-sweep-divergence
Issue-Trace: accessanalyzer-analyzer-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: accessanalyzer-analyzer-lifecycle accessanalyzer-archive-rule-workflow accessanalyzer-policy-generation-workflow accessanalyzer-unused-access-analyzer-stats
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: access-analyzer|GetAnalyzer|2xx->2xx|$.analyzer.createdAt
Issue-Key: access-analyzer|ListAnalyzers|2xx->2xx|$.analyzers[arn="arn
Issue-Key: access-analyzer|GetArchiveRule|2xx->2xx|$.archiveRule.createdAt
Issue-Key: access-analyzer|GetArchiveRule|2xx->2xx|$.archiveRule.updatedAt
Issue-Key: access-analyzer|ListArchiveRules|2xx->2xx|$.archiveRules[*]
Issue-Key: access-analyzer|GetArchiveRule|4xx->2xx|$.message
Issue-Key: access-analyzer|GetArchiveRule|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: access-analyzer|GetArchiveRule|4xx->2xx|status
Issue-Key: access-analyzer|ListPolicyGenerations|4xx->4xx|$.message
Issue-Key: access-analyzer|ListPolicyGenerations|4xx->4xx|$.reason
Issue-Key: access-analyzer|GetAnalyzer|2xx->2xx|$.analyzer.tags
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# access-analyzer read and error paths diverge from modeled wire shape

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAnalyzer

## Description
Replay of the access-analyzer wire fixtures surfaced 7 distinct causes in the service's read and error paths. They share one family of fault: the restJson1 response layer emits shapes the Smithy model does not describe (timestamps serialized in the wrong form, required error members dropped, a deleted resource still served) instead of the modeled wire shape. All 7 causes must be closed before this issue is submitted.

Cause 1 of 7: GetAnalyzer serializes AnalyzerSummary.createdAt as a bare epoch number.
AWS: Smithy: GetAnalyzer -> output member analyzer: AnalyzerSummary (required); AnalyzerSummary.createdAt: Timestamp, @required, @timestampFormat("date-time"). On the wire that member is therefore an RFC3339/ISO-8601 string (the recording shows '2026-09-06T06:50:51Z'). HTTP 200.
Emulator: answers 200 but renders $.analyzer.createdAt as a JSON number of epoch seconds with a fractional part (recorded example: 1792638455.2929208).
Why: every AWS SDK deserializes this member per @timestampFormat("date-time") and parses a string; a bare number either fails to deserialize or is read as a nonsense instant, so a client cannot read an analyzer's creation time at all.
Fix: the access-analyzer response serializer for timestamp members (date-time format must be applied when encoding Timestamp, not the protocol's default epoch-seconds encoding).
Traces: accessanalyzer-analyzer-lifecycle#task#2.

Cause 2 of 7: ListAnalyzers serializes the same createdAt member as an epoch number.
AWS: Smithy: ListAnalyzers -> output member analyzers: AnalyzersList (required) of AnalyzerSummary, whose createdAt is @required with @timestampFormat("date-time") -> ISO-8601 string. HTTP 200.
Emulator: 200, but each list element's createdAt is an epoch number (recorded example: expected '2026-09-06T06:50:51Z', got 1792638455.2929208), so the list projection diverges the same way the single-resource read does.
Why: a client listing analyzers cannot sort or display creation times, and strict SDK deserializers reject the whole page rather than one member.
Fix: same serializer path as Cause 1, exercised through the list projection of ListAnalyzers.
Traces: accessanalyzer-analyzer-lifecycle#task#3.

Cause 3 of 7: GetArchiveRule serializes createdAt and updatedAt as epoch numbers.
AWS: Smithy: GetArchiveRule -> output member archiveRule: ArchiveRuleSummary (required); ArchiveRuleSummary.createdAt and .updatedAt are both Timestamp, @required, @timestampFormat("date-time") -> ISO-8601 strings. HTTP 200.
Emulator: 200 with both $.archiveRule.createdAt and $.archiveRule.updatedAt as epoch numbers (recorded example: expected '2026-09-06T06:53:39Z', got 1792638456.186541).
Why: archive-rule audit flows that compare createdAt/updatedAt to detect updates get unparseable values.
Fix: same timestamp encoding in the archive-rule response serializer.
Traces: accessanalyzer-archive-rule-workflow#task#4 (cluster also covers accessanalyzer-archive-rule-workflow#task#7).

Cause 4 of 7: ListArchiveRules emits the same mis-typed timestamps in its list elements.
AWS: Smithy: ListArchiveRules -> output member archiveRules: ArchiveRulesList (required) of ArchiveRuleSummary; createdAt/updatedAt are @required date-time timestamps. HTTP 200.
Emulator: 200, and the element is reported both missing and extra because it does not match: ruleName and filter agree while createdAt/updatedAt are epoch numbers instead of ISO-8601 strings.
Why: the whole list element fails to match the modeled shape, so a client paging archive rules cannot deserialize them.
Fix: same serializer path as Cause 3, through the ListArchiveRules projection.
Traces: accessanalyzer-archive-rule-workflow#task#5.

Cause 5 of 7: GetArchiveRule still answers 200 for an archive rule that was deleted.
AWS: Smithy: GetArchiveRule models ResourceNotFoundException (errorKind client, httpError 404) among its errors; restJson1 signals it with the x-amzn-errortype header. The recording shows HTTP 404 with x-amzn-errortype: ResourceNotFoundException and a message ('Archive rule not found' is the recorded example text).
Emulator: answers HTTP 200 for the deleted rule, with no x-amzn-errortype header and no message - i.e. it serves a rule that no longer exists.
Why: this is the most damaging cause in the bundle. Delete-then-read is how clients confirm a deletion, and a 200 here reports the rule as still present; SDK waiters and idempotent cleanup loops never terminate, and tests that assert ResourceNotFoundException pass against AWS and fail only here.
Fix: the GetArchiveRule handler plus archive-rule state - the delete must remove the rule from state, and a lookup miss must raise the modeled ResourceNotFoundException (404) rather than falling through to a success response.
Traces: accessanalyzer-archive-rule-workflow#task#10.

Cause 6 of 7: ListPolicyGenerations' ValidationException omits the required reason member.
AWS: Smithy: ListPolicyGenerations.principalArn is @httpQuery("principalArn") with pattern ^arn:[^:]*:iam::[^:]*:(role|user)/.{1,576}$, and the operation models ValidationException (errorKind client, httpError 400). ValidationException has BOTH message (@required) and reason: ValidationExceptionReason (@required). The recording shows 400 carrying reason='FIELD_VALIDATION_FAILED'.
Emulator: returns the correct 400, but $.reason is absent entirely; it emits only a message. (The message text also differs - recorded 'Invalid principalArn: not-an-arn' vs the emulator's generic constraint sentence - but the recorded wording is only an example, so the defect filed here is the missing required reason member, not the wording.)
Why: reason is the machine-readable discriminator clients branch on to tell a field-validation failure from cannotParse/notSupported/other; a modeled-required member that is never serialized makes the error undecodable by strict deserializers and unbranchable by everyone else.
Fix: the ValidationException serializer/constructor on the access-analyzer validation path - populate reason from the ValidationExceptionReason enum whenever a validation rejection is raised.
Traces: accessanalyzer-policy-generation-workflow#task#6.

Cause 7 of 7: GetAnalyzer drops the empty tags map instead of serializing it.
AWS: Smithy: AnalyzerSummary.tags: TagsMap (map, optional). The recording shows the member present as an empty object ({}) for an untagged unused-access analyzer, alongside the same date-time createdAt as Cause 1.
Emulator: omits $.analyzer.tags from the response (got None) - and the same step also shows createdAt as an epoch number (expected '2026-09-06T06:48:16Z', got 1792638462.2029133), confirming Cause 1 on the unused-access analyzer configuration too.
Why: a client that reads analyzer.tags directly to diff or merge tags sees an absent member rather than an empty map and must special-case the emulator; tag-reconciliation flows that iterate the map break on the null.
Fix: the GetAnalyzer response serializer - emit tags as a (possibly empty) map rather than suppressing it when the analyzer carries no tags.
Traces: accessanalyzer-unused-access-analyzer-stats#task#2.

Related but NOT covering these: the open access-analyzer issues are all unimplemented-stub reports (CheckAccessNotGranted, CheckNoNewAccess, CheckNoPublicAccess, CreateAccessPreview, TagResource, UpdateAnalyzer, ValidatePolicy) plus '20261022T015008Z-accessanalyzer-three-negative-paths-answer-notimplemented-instead-of'. None of them touches GetAnalyzer, ListAnalyzers, GetArchiveRule, ListArchiveRules or ListPolicyGenerations, and none carries a replay key for the steps above. Replay reported state_reset_failures=0, so none of these causes is contaminated by a prior fixture's state.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, state_reset_failures=0, 66 steps: 20 passed / 43 mismatched / 3 blocked, score 30.3%). All 7 clusters below are service=access-analyzer, stub_path empty, owned_by empty.

Cause 1 - GetAnalyzer, accessanalyzer-analyzer-lifecycle#task#2, status 200 -> 200:
  $.analyzer.createdAt: value_mismatch (expected '2026-09-06T06:50:51Z', got 1792638455.2929208)
  Expected type: ISO-8601 string per @timestampFormat("date-time"); actual type: JSON number.

Cause 2 - ListAnalyzers, accessanalyzer-analyzer-lifecycle#task#3, status 200 -> 200:
  $.analyzers[arn="arn:aws:access-analyzer:us-west-2:000000000000:analyzer/sim2real-probe-accessanalyzer-aa-lifecycle-e9141965"].createdAt: value_mismatch (expected '2026-09-06T06:50:51Z', got 1792638455.2929208)

Cause 3 - GetArchiveRule, accessanalyzer-archive-rule-workflow#task#4, status 200 -> 200 (cluster size 2; also task#7):
  $.archiveRule.createdAt: value_mismatch (expected '2026-09-06T06:53:39Z', got 1792638456.186541)
  $.archiveRule.updatedAt: value_mismatch (expected '2026-09-06T06:53:39Z', got 1792638456.186541)

Cause 4 - ListArchiveRules, accessanalyzer-archive-rule-workflow#task#5, status 200 -> 200:
  $.archiveRules[0]: missing/extra - expected {'ruleName': ..., 'filter': {'resourceType': {'eq': ['AWS::S3::Bucket']}}, 'createdAt': '2026-09-06T06:53:39Z', 'updatedAt': '2026-09-06T06:53:39Z'}, got the same ruleName/filter with 'createdAt': 1792638456.186541, 'updatedAt': 1792638456.186541.

Cause 5 - GetArchiveRule, accessanalyzer-archive-rule-workflow#task#10, status 404 -> 200:
  status: expected 404, got 200
  $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException', got None)
  $.message: missing (expected 'Archive rule not found' - recorded example text - got None)

Cause 6 - ListPolicyGenerations, accessanalyzer-policy-generation-workflow#task#6, status 400 -> 400:
  $.reason: missing (expected 'FIELD_VALIDATION_FAILED', got None)  <- required member, the actual defect
  $.message: value_mismatch (expected 'Invalid principalArn: not-an-arn', got "1 validation error detected: Value 'not-an-arn' at 'principalArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[^:]*:iam::[^:]*:(role|user)/.{1,576}$") - wording treated as an example, not a requirement.

Cause 7 - GetAnalyzer, accessanalyzer-unused-access-analyzer-stats#task#2, status 200 -> 200:
  $.analyzer.createdAt: value_mismatch (expected '2026-09-06T06:48:16Z', got 1792638462.2029133)
  $.analyzer.tags: missing (expected {}, got None)

Model citations (services/service-com-amazonaws-accessanalyzer-accessanalyzer/smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json): AnalyzerSummary.createdAt @required @timestampFormat("date-time"); AnalyzerSummary.tags TagsMap; ArchiveRuleSummary.createdAt/.updatedAt @required @timestampFormat("date-time"); GetArchiveRule errors include ResourceNotFoundException (client, 404); ValidationException members message @required AND reason: ValidationExceptionReason @required; ListPolicyGenerations.principalArn pattern ^arn:[^:]*:iam::[^:]*:(role|user)/.{1,576}$.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout (replay_wire_corpus), then re-run these steps per cause:

1. Cause 1: fixture accessanalyzer-analyzer-lifecycle, step task#2 - create an analyzer, then GET /analyzer/{analyzerName}. Inspect $.analyzer.createdAt: it is a JSON number, expected an ISO-8601 date-time string.
2. Cause 2: fixture accessanalyzer-analyzer-lifecycle, step task#3 - GET /analyzer and inspect $.analyzers[*].createdAt; same number-instead-of-string.
3. Cause 3: fixture accessanalyzer-archive-rule-workflow, step task#4 (and task#7) - create an archive rule, then GET /analyzer/{analyzerName}/archive-rule/{ruleName}; inspect $.archiveRule.createdAt and .updatedAt.
4. Cause 4: fixture accessanalyzer-archive-rule-workflow, step task#5 - GET /analyzer/{analyzerName}/archive-rule; inspect $.archiveRules[0].createdAt/.updatedAt.
5. Cause 5: fixture accessanalyzer-archive-rule-workflow, step task#10 - DeleteArchiveRule, then GET the same archive rule. Observe HTTP 200 with the rule body; expected HTTP 404 with header x-amzn-errortype: ResourceNotFoundException and a message member.
6. Cause 6: fixture accessanalyzer-policy-generation-workflow, step task#6 - GET /policy/generation?principalArn=not-an-arn. Observe HTTP 400 whose body has no $.reason member; expected the required reason member (recorded value 'FIELD_VALIDATION_FAILED') alongside message.
7. Cause 7: fixture accessanalyzer-unused-access-analyzer-stats, step task#2 - create an unused-access analyzer with no tags, then GET /analyzer/{analyzerName}. Observe $.analyzer.tags absent (expected {}) and $.analyzer.createdAt as an epoch number.

All seven must be re-replayed green before closing; replay reported state_reset_failures=0, so no step above ran against a previous fixture's state.
