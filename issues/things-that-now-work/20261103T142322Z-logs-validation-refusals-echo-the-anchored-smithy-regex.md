Issue-Source: trace-sweep-divergence
Issue-Trace: logs-agentcore-traj-vended-group-identifier-mistakes
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-agentcore-traj-vended-group-identifier-mistakes logs-batch-diag-empty-declared-log-group-reads logs-osdocstat-traj-log-group-arn-forms logs-osdocstat-traj-find-domain-log-groups logs-sftp-traj-metric-filter-reverse-lookup
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DescribeLogStreams|4xx->4xx|$.message
Issue-Key: logs|FilterLogEvents|4xx->4xx|$.message
Issue-Key: logs|GetLogEvents|4xx->4xx|$.message
Issue-Key: logs|DescribeLogGroups|4xx->4xx|$.message
Issue-Key: logs|DescribeMetricFilters|4xx->4xx|$.message
Issue-Commit: 92fa21155a5f185bd35dca5c5e524f786e59267d

# logs validation refusals echo the anchored Smithy regex and rewrite AWS refusal text (5 causes)

> _Note: this issue was identified against the code base at commit `92fa21155a5f185bd35dca5c5e524f786e59267d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLogStreams

## Description
Five net-new wire divergences in CloudWatch Logs input validation. In every one the HTTP status (400) and the error class match AWS; only the human-readable `message` diverges, so each is an error-text defect in the logs validation layer rather than a classification defect. The shared fault is that the emulator authors its own refusal wording instead of reproducing AWS's: three causes render the @pattern constraint with the Smithy anchors still attached, and two replace a fixed AWS sentence with a paraphrase. FIVE causes must be closed before this issue is submitted.

Cause 1 of 5 - DescribeLogStreams logGroupIdentifier pattern violation.
AWS: Smithy DescribeLogStreams.logGroupIdentifier targets LogGroupIdentifier with @length min 1 max 2048 and @pattern "^[\\w#+=/:,.@-]*$"; a violating value is refused 400 (errors: InvalidParameterException, ResourceNotFoundException). The recorded refusal message is "1 validation error detected: Value at 'logGroupIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w#+=/:,.@-]*". Its FIXED text is "<n> validation error(s) detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>"; its VARIABLE parts are the error count, the member name taken from the request, and the pattern - which AWS renders as the regex BODY, with no leading ^ and no trailing $.
Emulator: same status and class, but the message quotes the anchored model pattern: "... Member must satisfy regular expression pattern: ^[\\w#+=/:,.@-]*$".
Why: a client that surfaces or parses the quoted pattern (to retry with a conforming identifier, or to assert on AWS's refusal text) receives a regex AWS never emits; the anchors are an artifact of the internal model, not of the API contract.
Fix: the shared constraint-violation message formatter used by the logs crate's input validation - it must strip the leading ^ and trailing $ when rendering a @pattern into a validation message, for every member, not per operation.
Traces: logs-agentcore-traj-vended-group-identifier-mistakes#task#6 (also logs-batch-diag-empty-declared-log-group-reads#task#3).

Cause 2 of 5 - FilterLogEvents logGroupIdentifier pattern violation, same formatter.
AWS: Smithy FilterLogEvents.logGroupIdentifier, same LogGroupIdentifier target and @pattern; refusal 400 with the unanchored pattern in the message, exactly as in cause 1.
Emulator: 400 with "^[\\w#+=/:,.@-]*$".
Why: as cause 1, on the log-event filter read path.
Fix: same formatter; this cause exists only to pin the second operation that reaches it.
Traces: logs-osdocstat-traj-log-group-arn-forms#task#4.

Cause 3 of 5 - GetLogEvents logGroupIdentifier pattern violation, same formatter.
AWS: Smithy GetLogEvents.logGroupIdentifier, same target and @pattern; 400 with the unanchored pattern.
Emulator: 400 with "^[\\w#+=/:,.@-]*$".
Why: as cause 1, on the single-stream read path.
Fix: same formatter.
Traces: logs-osdocstat-traj-log-group-arn-forms#task#6.

Cause 4 of 5 - DescribeLogGroups refuses a prefix+pattern pair with paraphrased text.
AWS: Smithy DescribeLogGroups models both logGroupNamePrefix (LogGroupName, @length 1..512) and logGroupNamePattern (LogGroupNamePattern, @length 0..512) and declares InvalidParameterException (httpError 400). Supplying both is refused 400 with the message "LogGroup name prefix and LogGroup name pattern are mutually exclusive parameters." - this message is entirely FIXED text, with no variable part: no member name, value or limit is interpolated, and the trailing period is part of it.
Emulator: 400 with "logGroupNamePrefix and logGroupNamePattern are mutually exclusive; specify only one" - a different sentence, different capitalisation of the parameter names, and no trailing period.
Why: a caller that distinguishes this refusal from other 400s by its message (the error class is the generic InvalidParameterException, so the message is the only discriminator) does not recognise it.
Fix: the DescribeLogGroups handler's mutual-exclusion check must emit AWS's fixed sentence verbatim.
Traces: logs-agentcore-traj-vended-group-identifier-mistakes#task#2 (also logs-osdocstat-traj-find-domain-log-groups#task#5).

Cause 5 of 5 - DescribeMetricFilters refuses a half-specified metric selector with paraphrased text.
AWS: Smithy DescribeMetricFilters models metricName and metricNamespace (both MetricName/MetricNamespace, @length 0..255) and declares InvalidParameterException (httpError 400). Supplying one without the other is refused 400 with the message "Describe Metric Filters request must contain both MetricName and MetricNamespace" - again entirely FIXED text (note AWS's spaced operation name and the capitalised member names, and that there is no trailing period), with no value interpolated from the request.
Emulator: 400 with "metricName and metricNamespace must be specified together".
Why: as cause 4 - the message is the only discriminator for this refusal, so the reverse metric-filter lookup workflow cannot tell it apart from any other validation 400.
Fix: the DescribeMetricFilters handler's paired-parameter check must emit AWS's fixed sentence verbatim.
Traces: logs-sftp-traj-metric-filter-reverse-lookup#task#3.

Related open issues (neither carries these replay keys, so neither covers these causes): "logs: PutTransformer's vended-parser-position refusal message is rewritten, and CreateLogStream's validation message echoes the anchored Smithy regex" (20261031T171822Z-...) reports the same anchored-regex formatter on CreateLogStream only; "CloudWatch Logs metric-filter refusal texts are rewritten ..." (20261029T182452Z-cloudwatch-lambda-logs-...) covers metric-filter PUT refusals, not DescribeMetricFilters.

## Evidence
Cause 1 (logs-agentcore-traj-vended-group-identifier-mistakes#task#6, DescribeLogStreams, 400 -> 400): $.message value_mismatch - expected "1 validation error detected: Value at 'logGroupIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w#+=/:,.@-]*", got "... Member must satisfy regular expression pattern: ^[\\w#+=/:,.@-]*$". Same detail for cause 2 (logs-osdocstat-traj-log-group-arn-forms#task#4, FilterLogEvents) and cause 3 (logs-osdocstat-traj-log-group-arn-forms#task#6, GetLogEvents).
Cause 4 (logs-agentcore-traj-vended-group-identifier-mistakes#task#2, DescribeLogGroups, 400 -> 400): $.message value_mismatch - expected 'LogGroup name prefix and LogGroup name pattern are mutually exclusive parameters.', got 'logGroupNamePrefix and logGroupNamePattern are mutually exclusive; specify only one'.
Cause 5 (logs-sftp-traj-metric-filter-reverse-lookup#task#3, DescribeMetricFilters, 400 -> 400): $.message value_mismatch - expected 'Describe Metric Filters request must contain both MetricName and MetricNamespace', got 'metricName and metricNamespace must be specified together'.
Model evidence: smithy_query on services/service-com-amazonaws-cloudwatchlogs-logs-20140328 shows logGroupIdentifier @pattern "^[\\w#+=/:,.@-]*$" on DescribeLogStreams/GetLogEvents/FilterLogEvents, both logGroupNamePrefix and logGroupNamePattern on DescribeLogGroups, and metricName/metricNamespace on DescribeMetricFilters, each with InvalidParameterException httpError 400.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: fixture logs-agentcore-traj-vended-group-identifier-mistakes, step task#6 - DescribeLogStreams with a logGroupIdentifier containing a character outside [\\w#+=/:,.@-]; compare $.message against the recorded refusal.
3. Cause 2: fixture logs-osdocstat-traj-log-group-arn-forms, step task#4 (FilterLogEvents). Cause 3: same fixture, step task#6 (GetLogEvents).
4. Cause 4: fixture logs-agentcore-traj-vended-group-identifier-mistakes, step task#2 - DescribeLogGroups with logGroupNamePrefix AND logGroupNamePattern set.
5. Cause 5: fixture logs-sftp-traj-metric-filter-reverse-lookup, step task#3 - DescribeMetricFilters with metricName but no metricNamespace.
6. In each case the status is 400 as recorded and only $.message diverges.
