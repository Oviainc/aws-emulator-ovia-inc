Issue-Source: trace-sweep-divergence
Issue-Trace: transcribe-call-analytics-category-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transcribe-call-analytics-category-lifecycle
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe|CreateCallAnalyticsCategory|2xx->2xx|$.CategoryProperties.Rules[*].TranscriptFilter.Negate
Issue-Key: transcribe|GetCallAnalyticsCategory|2xx->2xx|$.CategoryProperties.Rules[*].TranscriptFilter.Negate
Issue-Key: transcribe|UpdateCallAnalyticsCategory|2xx->2xx|$.CategoryProperties.Rules[*].TranscriptFilter.Negate
Issue-Key: transcribe|ListCallAnalyticsCategories|2xx->2xx|$.Categories[*]
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# Call Analytics category properties omit TranscriptFilter.Negate on every read and write path

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCallAnalyticsCategory

## Description
A Call Analytics category whose rule is a TranscriptFilter comes back from AWS with the TranscriptFilter.Negate member PRESENT and false when the request did not set it; the emulator stores and serves the rule with Negate absent entirely. Because the member is dropped by whatever renders CategoryProperties, every operation that returns a category — create, get, update and list — diverges the same way, so this is one serialization/state defect observed on four surfaces (4 causes below, all HTTP 200 on both sides).

Smithy: TranscriptFilter.Negate targets Boolean with NO @default and no @required, so the model does not state that it is echoed; the behavior is recorded behavior only — the recorded AWS response carries "Negate": false inside CategoryProperties.Rules[].TranscriptFilter. The absent-vs-false distinction is meaningful for this member, whose own doc says "Set to TRUE to flag the absence of the phrase ... Set to FALSE to flag the presence".

Cause 1 of 4: CreateCallAnalyticsCategory.
AWS: recorded behavior only. The 200 response's CategoryProperties.Rules[0].TranscriptFilter contains Negate = false alongside the TranscriptFilterType, ParticipantRole and Targets taken from the request.
Emulator: 200 with the same rule but no Negate member at all ($.CategoryProperties.Rules[0].TranscriptFilter.Negate: missing, expected False, got None).
Why: a client that reads rule.Negate to decide whether a rule flags presence or absence gets null/None instead of a boolean. Typed SDKs surface an unset optional, and round-tripping the returned CategoryProperties back into UpdateCallAnalyticsCategory silently loses the flag.
Fix: the serializer for CategoryProperties / TranscriptFilter in the transcribe crate (or the state record it is built from) must materialize Negate as false when the request omitted it, rather than leaving it unset.
Traces: transcribe-call-analytics-category-lifecycle#task#2

Cause 2 of 4: GetCallAnalyticsCategory.
AWS: recorded behavior only; the stored category read back carries Negate = false in the same position.
Emulator: 200 with Negate missing (same detail as cause 1). Observed twice in the fixture, before and after the update.
Why: same — the primary read path for a category never reports the flag.
Fix: same component; the read path shares the CategoryProperties rendering.
Traces: transcribe-call-analytics-category-lifecycle#task#4

Cause 3 of 4: UpdateCallAnalyticsCategory.
AWS: recorded behavior only; the 200 response echoes the updated CategoryProperties with Negate = false.
Emulator: 200 with Negate missing.
Why: same, on the write-echo path.
Fix: same component.
Traces: transcribe-call-analytics-category-lifecycle#task#5

Cause 4 of 4: ListCallAnalyticsCategories.
AWS: recorded behavior only; each element of Categories carries the full CategoryProperties including Rules[].TranscriptFilter.Negate = false.
Emulator: 200, Categories[0] compares unequal solely because its TranscriptFilter has no Negate member. The CreateTime / LastUpdateTime values also differ between the recording and the replay, but those are per-run clock values and are NOT part of this finding.
Why: a client enumerating categories cannot read the flag for any of them.
Fix: same component, reached through the list projection.
Traces: transcribe-call-analytics-category-lifecycle#task#7

## Evidence
All four steps returned HTTP 200 on both sides; the only structural divergence is the missing member.
Cause 1 (task#2, CreateCallAnalyticsCategory): $.CategoryProperties.Rules[0].TranscriptFilter.Negate: missing (expected False, got None).
Cause 2 (task#4, GetCallAnalyticsCategory, cluster also covers task#6): $.CategoryProperties.Rules[0].TranscriptFilter.Negate: missing (expected False, got None).
Cause 3 (task#5, UpdateCallAnalyticsCategory): $.CategoryProperties.Rules[0].TranscriptFilter.Negate: missing (expected False, got None).
Cause 4 (task#7, ListCallAnalyticsCategories): $.Categories[0] value_mismatch — expected Rules[0].TranscriptFilter = {Negate: False, ParticipantRole: CUSTOMER, Targets: [<target phrase>], TranscriptFilterType: EXACT}; actual the same structure with Negate omitted. The recorded CreateTime/LastUpdateTime and the generated category name differ per run and are excluded.

## Steps to Reproduce
1. CreateCallAnalyticsCategory(CategoryName=<new>, InputType='POST_CALL', Rules=[{TranscriptFilter:{TranscriptFilterType:'EXACT', ParticipantRole:'CUSTOMER', Targets:['goodbye']}}]) — i.e. do NOT send Negate. Inspect CategoryProperties.Rules[0].TranscriptFilter in the 200 response: AWS reports Negate=false, the emulator omits it.
2. GetCallAnalyticsCategory(CategoryName=<same>) and inspect the same path.
3. UpdateCallAnalyticsCategory(CategoryName=<same>, Rules=[...same rule...]) and inspect the echoed CategoryProperties.
4. ListCallAnalyticsCategories() and inspect Categories[0].Rules[0].TranscriptFilter.
Replay: transcribe-call-analytics-category-lifecycle steps task#2, task#4, task#6, task#5 and task#7.
