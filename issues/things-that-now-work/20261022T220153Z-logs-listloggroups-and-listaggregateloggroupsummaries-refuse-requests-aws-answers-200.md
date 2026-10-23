Issue-Source: trace-sweep-divergence
Issue-Trace: logs-aggregate-log-group-summaries-by-datasource
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-aggregate-log-group-summaries-by-datasource logs-aggregate-log-group-summaries-with-name-pattern logs-bearer-token-auth-and-aggregate-summaries logs-list-aggregate-log-group-summaries-basic logs-log-group-lifecycle-and-tags logs-b2-policy-suite-with-data-protection
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|ListAggregateLogGroupSummaries|2xx->4xx|$
Issue-Key: logs|ListAggregateLogGroupSummaries|2xx->4xx|$.aggregateLogGroupSummaries
Issue-Key: logs|ListAggregateLogGroupSummaries|2xx->4xx|$.aggregateLogGroupSummaries[*].logGroupCount
Issue-Key: logs|ListAggregateLogGroupSummaries|2xx->4xx|$.nextToken
Issue-Key: logs|ListAggregateLogGroupSummaries|2xx->4xx|status
Issue-Key: logs|ListLogGroups|2xx->4xx|$.logGroups
Issue-Key: logs|ListLogGroups|2xx->4xx|status
Issue-Key: logs|ListLogGroups|2xx->4xx|$.logGroups[*].logGroupClass
Issue-Key: logs|ListLogGroups|2xx->4xx|$.nextToken
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# ListLogGroups and ListAggregateLogGroupSummaries refuse requests AWS answers 200

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAggregateLogGroupSummaries

## Description
The two name-pattern-based list operations in CloudWatch Logs -- ListAggregateLogGroupSummaries and ListLogGroups -- refuse requests the recorded service answers with 200. Both take `logGroupNamePattern` targeting the shared LogGroupNameRegexPattern shape, whose modeled @pattern decomposes the value into up to five `|`-separated segments of 3..24 characters. Real log group names routinely exceed 24 characters in a single segment, and the recordings show the service accepting such a value and returning the matching summaries; the emulator enforces the segment regex literally and rejects. The result is that a successful list call becomes a 400, so nothing downstream of the list can run. This bundle has 4 causes; all 4 must be closed.

Cause 1 of 4 -- ListAggregateLogGroupSummaries rejects a logGroupNamePattern the service accepts.
AWS: Smithy -- ListAggregateLogGroupSummaries.logGroupNamePattern targets LogGroupNameRegexPattern, @length 3..129, @pattern ^(\^?[\.\-_\/#A-Za-z0-9]{3,24})(\|\^?[\.\-_\/#A-Za-z0-9]{3,24}){0,4}$; output members aggregateLogGroupSummaries + nextToken; errors InvalidParameterException / ValidationException / ServiceUnavailableException. Recorded behavior: the service answers 200 with a one-element aggregateLogGroupSummaries list (groupingIdentifiers dataSource.Name / dataSource.Type, logGroupCount 1) plus a nextToken, for a single-segment pattern longer than 24 characters. Hypothesis: the service treats logGroupNamePattern as a search expression and does not apply the modeled segment-length regex to it at the wire.
Emulator: 400 ValidationException reporting the member failed the segment regex; aggregateLogGroupSummaries and nextToken absent. The fixture then polls the summary for 60s and never converges.
Why: a client cannot enumerate aggregate log-group summaries for any log group whose name is longer than 24 characters -- which is most of them. Every scenario that lists first and acts second is dead at the first call.
Fix: the ListAggregateLogGroupSummaries handler's input validation -- stop applying the LogGroupNameRegexPattern segment regex as a hard reject on logGroupNamePattern; treat it as a match expression as the recordings do.
Traces: logs-aggregate-log-group-summaries-by-datasource#task#4 (also logs-aggregate-log-group-summaries-with-name-pattern#task#4).

Cause 2 of 4 -- ListAggregateLogGroupSummaries 400s where the service returns an empty summary list.
AWS: as cause 1. Recorded: 200 with aggregateLogGroupSummaries = [] (an empty result is a success, not a refusal).
Emulator: 400; aggregateLogGroupSummaries absent entirely.
Why: an empty aggregate result is indistinguishable from a bad request, so a client cannot tell "no log groups matched" from "your request was invalid" and will not retry or proceed. This cluster spans six fixture steps, so it is the widest single failure in the logs corpus.
Fix: ListAggregateLogGroupSummaries handler -- return 200 with an empty aggregateLogGroupSummaries list for a request that matches nothing.
Traces: logs-aggregate-log-group-summaries-by-datasource#task#5 (also logs-aggregate-log-group-summaries-with-name-pattern#task#5, logs-bearer-token-auth-and-aggregate-summaries#task#6, logs-list-aggregate-log-group-summaries-basic#task#2 and #task#3, logs-log-group-lifecycle-and-tags#task#4).

Cause 3 of 4 -- ListLogGroups 400s where the service returns an empty logGroups list.
AWS: Smithy -- ListLogGroups.logGroupNamePattern targets the same LogGroupNameRegexPattern shape; output logGroups (LogGroupSummaries) + nextToken; the operation's ONLY modeled errors are InvalidParameterException and ServiceUnavailableException. Recorded: 200 with logGroups = [].
Emulator: 400 with logGroups absent.
Why: as cause 2; additionally the emulator's refusal carries an error code outside this operation's modeled error set, so a generated client has no branch for it.
Fix: ListLogGroups handler / input validation.
Traces: logs-b2-policy-suite-with-data-protection#task#15.

Cause 4 of 4 -- ListLogGroups rejects a logGroupNamePattern the service accepts and matches.
AWS: as cause 3. Recorded: 200 with one LogGroupSummary carrying logGroupName, logGroupArn and logGroupClass STANDARD, plus a nextToken, for a single-segment pattern longer than 24 characters.
Emulator: 400; logGroups and nextToken absent, and the fixture's poll on logGroups[0].logGroupClass never converges (13 attempts).
Why: as cause 1 -- log groups cannot be discovered by name pattern at all.
Fix: ListLogGroups handler's input validation -- same LogGroupNameRegexPattern over-enforcement as cause 1.
Traces: logs-log-group-lifecycle-and-tags#task#3.

## Evidence
Cause 1 (ListAggregateLogGroupSummaries, logs-aggregate-log-group-summaries-by-datasource#task#4): status expected 200, got 400. Expected body {'aggregateLogGroupSummaries': [{'groupingIdentifiers': [{'key':'dataSource.Name',...},{'key':'dataSource.Type',...}], 'logGroupCount': 1}], 'nextToken': <shaped>}; actual {'__type':'ValidationException','message':"1 validation error detected: Value '<log group name>' at 'logGroupNamePattern' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(\^?[\.\-_\/#A-Za-z0-9]{3,24})(\|\^?[\.\-_\/#A-Za-z0-9]{3,24}){0,4}$"}. $.aggregateLogGroupSummaries missing; poll capped at 60s.
Cause 2 (same fixture task#5, 6 affected steps): status expected 200, got 400; $.aggregateLogGroupSummaries expected [], got None.
Cause 3 (ListLogGroups, logs-b2-policy-suite-with-data-protection#task#15): status expected 200, got 400; $.logGroups expected [], got None.
Cause 4 (ListLogGroups, logs-log-group-lifecycle-and-tags#task#3): status expected 200, got 400; $.logGroups expected [{'logGroupArn': ..., 'logGroupClass':'STANDARD','logGroupName': ...}], got None; $.nextToken missing; poll on $.logGroups[0].logGroupClass not in ['STANDARD'] after 13 attempts.
Smithy citations: LogGroupNameRegexPattern @length 3..129 with the 3..24-per-segment @pattern, shared by ListLogGroups.logGroupNamePattern and ListAggregateLogGroupSummaries.logGroupNamePattern; ListLogGroups errors [InvalidParameterException, ServiceUnavailableException] (no ValidationException).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay logs-aggregate-log-group-summaries-by-datasource step task#4: ListAggregateLogGroupSummaries with a logGroupNamePattern whose single segment exceeds 24 characters. The emulator answers 400 ValidationException; the recording is 200 with one aggregate summary.
3. Replay the same fixture's task#5 -- the service answers 200 with aggregateLogGroupSummaries=[]; the emulator answers 400.
4. Replay logs-b2-policy-suite-with-data-protection#task#15 (ListLogGroups, recorded 200 with logGroups=[]) -- the emulator answers 400.
5. Replay logs-log-group-lifecycle-and-tags#task#3 (ListLogGroups by name pattern, recorded 200 with one STANDARD summary) -- the emulator answers 400 and the fixture's logGroupClass poll never converges.
