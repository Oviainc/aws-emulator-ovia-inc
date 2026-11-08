Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-alarm-mute-rule-onetime-window
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-alarm-mute-rule-onetime-window cloudwatch-alarm-mute-rule-workflow cloudwatch-alarm-mute-rules cloudwatch-mute-rule-targeted-alarms-and-listing monitoring-alarm-mute-rule-lifecycle cwp-list-alarm-mute-rules-pagination
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|GetAlarmMuteRule|2xx->2xx|$.CreationId
Issue-Key: monitoring|GetAlarmMuteRule|2xx->2xx|$.MuteType
Issue-Key: monitoring|ListAlarmMuteRules|2xx->2xx|$.AlarmMuteRuleSummaries[*]
Issue-Key: monitoring|ListAlarmMuteRules|2xx->2xx|$.AlarmMuteRuleSummaries[*].MuteType
Issue-Key: monitoring|GetAlarmMuteRule|4xx->2xx|$.__type
Issue-Key: monitoring|GetAlarmMuteRule|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: monitoring|GetAlarmMuteRule|4xx->2xx|status
Issue-Key: monitoring|ListAlarmMuteRules|2xx->4xx|$.AlarmMuteRuleSummaries
Issue-Key: monitoring|ListAlarmMuteRules|2xx->4xx|status
Issue-Commit: 544316519d780db35004270636d68199c524c41e

# Alarm mute rule reads diverge: MuteType wire casing, missing CreationId, deleted rules still readable, and a valid listing refused (7 causes)

> _Note: this issue was identified against the code base at commit `544316519d780db35004270636d68199c524c41e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAlarmMuteRule

## Description
Seven net-new wire divergences on the CloudWatch alarm-mute-rule read path (GetAlarmMuteRule / ListAlarmMuteRules). They share one surface: the emulator's mute-rule projection and its not-found / pagination guards. No open issue carries any of these replay keys (the nearest open monitoring issues — 'CloudWatch alarm writes and pagination skip validation or reword refusals' and 'CloudWatch alarm history data and CreationId are emulator-authored or absent' — cover alarms and alarm history, not mute rules, and the replay report attributes none of these clusters to them).

Cause 1 of 7: GetAlarmMuteRule serializes MuteType in the wrong case and omits CreationId.
AWS: Smithy: GetAlarmMuteRule output member MuteType targets com.amazonaws.cloudwatch#MuteType, which is a plain `string` with NO enum constraint, so the wire value is whatever the service emits; the member's doc prose claims 'ONE_TIME or RECURRING', but the recorded response is authoritative for the wire and carries the lower-case hyphenated token for a one-time rule. The recording also carries a CreationId member that the model does not declare on GetAlarmMuteRuleOutput — recorded behavior only: AWS returns CreationId on a mute-rule read.
Emulator: returns MuteType as the upper-case underscored constant and omits CreationId entirely.
Why: a client that switches on the documented wire token (or on the value it got from a previous AWS call) sees an unknown mute type and treats a one-time rule as unclassified; code that threads CreationId for idempotency/correlation gets a null and cannot correlate the rule it just created.
Fix: the GetAlarmMuteRule output serializer / the mute-rule state projection — emit MuteType as the service's lower-case hyphenated token and populate CreationId.
Traces: cloudwatch-alarm-mute-rule-onetime-window#task#2 (also cloudwatch-alarm-mute-rule-workflow#task#3, cloudwatch-alarm-mute-rules#task#3, cloudwatch-mute-rule-targeted-alarms-and-listing#task#5, #task#6).

Cause 2 of 7: ListAlarmMuteRules carries the same MuteType casing defect in its summaries.
AWS: Smithy: AlarmMuteRuleSummary.MuteType targets MuteType (unconstrained string). The recorded ListAlarmMuteRules summary for a recurring rule carries the lower-case token 'recurring'.
Emulator: emits the upper-case underscored constant 'RECURRING' in AlarmMuteRuleSummaries[].MuteType.
Why: the same read value differs between Get and List in a client's own vocabulary, so filtering or grouping a listing by mute type silently matches nothing.
Fix: the ListAlarmMuteRules summary serializer, same MuteType mapping as cause 1. (The LastUpdatedTimestamp difference in this cluster is a per-run clock value and is NOT part of the finding.)
Traces: cloudwatch-alarm-mute-rule-workflow#task#4 (also cloudwatch-alarm-mute-rule-workflow#task#7, cloudwatch-alarm-mute-rules#task#4, #task#6, cloudwatch-mute-rule-targeted-alarms-and-listing#task#7, #task#8, #task#11, #task#14, monitoring-alarm-mute-rule-lifecycle#task#2).

Cause 3 of 7: the MuteType casing defect also appears on the paginated listing path.
AWS: as cause 2 — the summary's MuteType is the lower-case token.
Emulator: upper-case underscored constant on the first page of a paginated ListAlarmMuteRules.
Why: a paging client accumulates summaries whose mute type it cannot interpret.
Fix: same serializer as cause 2; this cluster shows the defect is not confined to the unpaginated path.
Traces: cwp-list-alarm-mute-rules-pagination#task#4 (also #task#5).

Cause 4 of 7: the MuteType casing defect applies to every element of a multi-entry page, not just the first.
AWS: as cause 2, for each summary in AlarmMuteRuleSummaries.
Emulator: every summary in the page carries the upper-case underscored constant.
Why: no element of a listing is usable by mute type.
Fix: same serializer as cause 2. (The LastUpdatedTimestamp values in this cluster are per-run and are NOT part of the finding.)
Traces: cwp-list-alarm-mute-rules-pagination#task#6.

Cause 5 of 7: GetAlarmMuteRule on a deleted/absent mute rule answers 200 with a body instead of 404 ResourceNotFoundException.
AWS: Smithy: GetAlarmMuteRule declares exactly one error, ResourceNotFoundException, with @httpError 404 and awsQueryError code ResourceNotFoundException/404. The recorded refusal is HTTP 404 with $.__type = 'com.amazonaws.cloudwatch.v2010_08_01#ResourceNotFoundException' and header x-amzn-query-error = 'ResourceNotFoundException;Sender'. Fixed text of that contract: the __type shape id and the query-error header 'ResourceNotFoundException;Sender'; the variable part is the rule name taken from the request.
Emulator: answers HTTP 200 and serializes a mute-rule body, with no __type and no x-amzn-query-error header — i.e. a deleted rule is still readable.
Why: a client that deletes a mute rule and polls for its disappearance never observes it; error handling keyed on ResourceNotFoundException never fires, and delete-then-verify and 'does this rule exist' flows report the rule as live forever.
Fix: the GetAlarmMuteRule handler's existence guard and its error serializer (404 + ResourceNotFoundException + the query-error header).
Traces: cloudwatch-alarm-mute-rule-onetime-window#task#4 (also cloudwatch-alarm-mute-rule-workflow#task#6, cloudwatch-mute-rule-targeted-alarms-and-listing#task#10, #task#13).

Cause 6 of 7: the same missing not-found guard on the mute-rule teardown path.
AWS: as cause 5 — 404 with $.__type 'com.amazonaws.cloudwatch.v2010_08_01#ResourceNotFoundException'.
Emulator: 200 with no __type after the rule has been deleted in teardown.
Why: same as cause 5; recorded here on a separate fixture to show the guard is absent regardless of how the rule was removed.
Fix: same guard as cause 5.
Traces: monitoring-alarm-mute-rule-lifecycle#teardown#4.

Cause 7 of 7: a ListAlarmMuteRules request AWS answers 200 with an empty list is refused 400.
AWS: Smithy: ListAlarmMuteRules is @paginated (inputToken/outputToken NextToken, items AlarmMuteRuleSummaries, pageSize MaxRecords) and models InvalidNextToken (400) and ResourceNotFoundException (404). The recorded response for this request is HTTP 200 with AlarmMuteRuleSummaries = [] — an empty result set is a successful empty page, not a client error.
Emulator: answers HTTP 400 and omits AlarmMuteRuleSummaries entirely.
Why: a paging client that walks to the end of the mute-rule listing (or lists with a filter that matches nothing) gets a hard failure instead of an empty page, so the loop aborts rather than terminating.
Fix: the ListAlarmMuteRules handler's pagination/filter path — an exhausted or empty result must serialize 200 with an empty AlarmMuteRuleSummaries list.
Traces: cwp-list-alarm-mute-rules-pagination#task#9.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (trace_set_id simparity-wire-aws-v1; all seven clusters report stub_path='' and owned_by=[], i.e. net-new).

Cause 1 — cloudwatch-alarm-mute-rule-onetime-window#task#2, GetAlarmMuteRule, 200 vs 200:
  $.MuteType: expected 'one-time', got 'ONE_TIME'
  $.CreationId: missing (expected present/shaped, got None)

Cause 2 — cloudwatch-alarm-mute-rule-workflow#task#4, ListAlarmMuteRules, 200 vs 200:
  $.AlarmMuteRuleSummaries[0]: expected {...'MuteType': 'recurring', 'Status': 'SCHEDULED'...}, got {...'MuteType': 'RECURRING', 'Status': 'SCHEDULED'...}

Cause 3 — cwp-list-alarm-mute-rules-pagination#task#4, ListAlarmMuteRules, 200 vs 200:
  $.AlarmMuteRuleSummaries[0].MuteType: expected 'recurring', got 'RECURRING'

Cause 4 — cwp-list-alarm-mute-rules-pagination#task#6, ListAlarmMuteRules, 200 vs 200:
  $.AlarmMuteRuleSummaries[0].MuteType and $.AlarmMuteRuleSummaries[1].MuteType: expected 'recurring', got 'RECURRING' for both entries

Cause 5 — cloudwatch-alarm-mute-rule-onetime-window#task#4, GetAlarmMuteRule:
  status: expected 404, got 200
  $.__type: missing (expected 'com.amazonaws.cloudwatch.v2010_08_01#ResourceNotFoundException', got None)
  $headers.x-amzn-query-error: missing (expected 'ResourceNotFoundException;Sender', got None)

Cause 6 — monitoring-alarm-mute-rule-lifecycle#teardown#4, GetAlarmMuteRule:
  status: expected 404, got 200
  $.__type: missing (expected 'com.amazonaws.cloudwatch.v2010_08_01#ResourceNotFoundException', got None)

Cause 7 — cwp-list-alarm-mute-rules-pagination#task#9, ListAlarmMuteRules:
  status: expected 200, got 400
  $.AlarmMuteRuleSummaries: missing (expected [], got None)

Model citations: smithy_query operation=GetAlarmMuteRule,ListAlarmMuteRules on services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json; smithy_query shape=MuteType,AlarmMuteRuleSummary,GetAlarmMuteRuleOutput (MuteType is an unconstrained string; GetAlarmMuteRuleOutput declares no CreationId — that member is recorded behavior only).

Per-run values (LastUpdatedTimestamp, request ids, generated rule-name suffixes) are excluded from every cause above.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Causes 1 and 5: fixture cloudwatch-alarm-mute-rule-onetime-window — step task#2 (GetAlarmMuteRule on a one-time rule: compare $.MuteType and the presence of $.CreationId) and step task#4 (GetAlarmMuteRule after the rule is gone: expect HTTP 404 with $.__type 'com.amazonaws.cloudwatch.v2010_08_01#ResourceNotFoundException' and header x-amzn-query-error 'ResourceNotFoundException;Sender').
4. Cause 2: fixture cloudwatch-alarm-mute-rule-workflow step task#4 — ListAlarmMuteRules, inspect AlarmMuteRuleSummaries[].MuteType.
5. Causes 3, 4 and 7: fixture cwp-list-alarm-mute-rules-pagination steps task#4, task#6 and task#9 — the last must answer HTTP 200 with AlarmMuteRuleSummaries = [].
6. Cause 6: fixture monitoring-alarm-mute-rule-lifecycle step teardown#4 — GetAlarmMuteRule on the deleted rule must answer 404.
7. Re-replay all seven fixture#step bindings after the fix; every one must pass.
