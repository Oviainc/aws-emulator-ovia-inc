Issue-Source: trace-sweep-divergence
Issue-Trace: logs-agentcore-traj-vended-groups-empty-reference-path
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-agentcore-traj-vended-groups-empty-reference-path logs-osdocstat-traj-find-domain-log-groups logs-osdocstat-traj-audit-log-json-filter-patterns logs-agentcore-traj-rewire-delivery-for-deleted-runtime
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.logGroups[*].logGroupArn
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.logGroups[*].logGroupClass
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.logGroups[*].metricFilterCount
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.nextToken
Issue-Key: logs|GetLogEvents|2xx->2xx|$.events[*].eventId
Issue-Key: logs|GetLogEvents|2xx->2xx|$.nextBackwardToken
Issue-Key: logs|GetLogEvents|2xx->2xx|$.nextForwardToken
Issue-Key: logs|DescribeLogStreams|2xx->2xx|$
Issue-Key: logs|DescribeLogStreams|2xx->2xx|$.logStreams
Issue-Key: logs|DescribeLogStreams|2xx->2xx|$.logStreams[*]
Issue-Key: logs|DescribeLogGroups|2xx->4xx|$.logGroups
Issue-Key: logs|DescribeLogGroups|2xx->4xx|status
Issue-Commit: 92fa21155a5f185bd35dca5c5e524f786e59267d

# logs log-group, log-stream and log-event read paths diverge from AWS (6 causes)

> _Note: this issue was identified against the code base at commit `92fa21155a5f185bd35dca5c5e524f786e59267d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLogGroups

## Description
Six net-new wire divergences on the CloudWatch Logs read paths (DescribeLogGroups, DescribeLogStreams, GetLogEvents). The shared fault is that these readers do not project what AWS projects: modeled response members are omitted, pagination tokens are emitter-internal rather than opaque, a stream AWS creates is never visible, and one well-formed list query is refused outright. SIX causes must be closed before this issue is submitted.

Cause 1 of 6 - DescribeLogGroups entries omit logGroupArn, logGroupClass and metricFilterCount, and the response omits nextToken.
AWS: Smithy LogGroup models logGroupName, creationTime, retentionInDays, metricFilterCount (FilterCount, integer - "The number of metric filters"), arn (ARN *with* a trailing ":*"), storedBytes, kmsKeyId, dataProtectionStatus, inheritedProperties, logGroupClass (enum) and logGroupArn (per its own doc, the ARN that does NOT include the trailing ":*", and the form to pass back "in the logGroupIdentifier input field in many CloudWatch Logs APIs"). DescribeLogGroupsResponse models logGroups and nextToken, and the op is @paginated(outputToken: nextToken, items: logGroups). AWS populates all four on every listed group: metricFilterCount is 0 when the group has no metric filters, logGroupClass is "STANDARD" for an ordinary group, logGroupArn is arn:aws:logs:<region>:<account>:log-group:<logGroupName> (no trailing ":*"), and nextToken is returned as an opaque continuation value.
Emulator: each entry carries neither logGroupArn, logGroupClass nor metricFilterCount, and the response carries no nextToken at all.
Why: a client that feeds logGroupArn back as logGroupIdentifier (the documented use) gets null and cannot address the group by ARN; a client that filters or displays logGroupClass or metricFilterCount gets null instead of STANDARD/0; and a paginator that keys on nextToken treats the first page as the complete result.
Fix: the DescribeLogGroups output serializer / log-group state projection in the logs crate - populate the non-":*" logGroupArn alongside arn, default logGroupClass to STANDARD, derive metricFilterCount from the group's metric filters, and emit the response nextToken.
Traces: logs-agentcore-traj-vended-groups-empty-reference-path#task#11 (three bedrock-agentcore vended groups).

Cause 2 of 6 - the same DescribeLogGroups projection gap on a different group set, confirming it is not fixture-specific.
AWS: as cause 1 - every listed group carries logGroupArn, logGroupClass "STANDARD" and metricFilterCount 0, and the response carries nextToken.
Emulator: all three members absent on both listed groups and nextToken absent.
Why: as cause 1; this is the opensearch domain-log-group discovery path.
Fix: as cause 1 - closing cause 1 should close this, but it is a distinct replay cluster and must be re-replayed.
Traces: logs-osdocstat-traj-find-domain-log-groups#task#4.

Cause 3 of 6 - GetLogEvents omits per-event eventId and returns non-canonical nextForwardToken/nextBackwardToken.
AWS: Smithy OutputLogEvent models only timestamp, message and ingestionTime, so for eventId this is recorded behavior only: the recorded AWS response carries an additional `eventId` on every element of `events` (an opaque per-event identifier). GetLogEventsResponse.nextForwardToken/nextBackwardToken target NextToken (@length min 1); the recorded AWS values are short opaque tokens of the form "f/<fixed-width opaque body>" and "b/<fixed-width opaque body>" - the direction prefix is the only part a client may read.
Emulator: no eventId on any event, and the two tokens are "f/"/"b/" followed by a hex encoding of the emulator's own cursor JSON - hundreds of characters long and carrying the internal cursor fields (the log group name, the log stream name, a timestamp, a sequence number and null start/end bounds) in recoverable clear text.
Why: a reader that de-duplicates or keys events by eventId gets null for every event; and a token that is unbounded in length and decodes to internal state is not interchangeable with an AWS token - it breaks clients that persist or size-bound continuation tokens and it exposes emulator internals to the caller.
Fix: the GetLogEvents output serializer (project eventId per event) and its pagination-token encoder (emit a fixed-width opaque body after the f//b/ prefix rather than a hex-encoded cursor document).
Traces: logs-agentcore-traj-vended-groups-empty-reference-path#task#10.

Cause 4 of 6 - the same GetLogEvents gap on a different log group, with two events.
AWS: as cause 3 - eventId present on events[0] and events[1], tokens in AWS's short opaque form.
Emulator: eventId absent on both events; both tokens are the long hex-encoded cursor.
Why: as cause 3; this is the opensearch audit-log read path.
Fix: as cause 3 - distinct replay cluster, must be re-replayed.
Traces: logs-osdocstat-traj-audit-log-json-filter-patterns#task#12.

Cause 5 of 6 - the delivery-validation log stream AWS creates in a vended-log destination group never exists, so DescribeLogStreams stays empty.
AWS: when a vended-log delivery is wired to a CloudWatch Logs destination, AWS creates a log stream named "log_stream_created_by_aws_to_validate_log_delivery_subscriptions" in the delivery's destination log group; DescribeLogStreams then lists it as a LogStream with logStreamName equal to that fixed name, a creationTime, and arn arn:aws:logs:<region>:<account>:log-group:<logGroupName>:log-stream:<logStreamName> (Smithy: DescribeLogStreams -> DescribeLogStreamsResponse.logStreams of LogStream{logStreamName, creationTime, arn, ...}). The stream name is fixed text; the region, account and log-group name come from the delivery's destination.
Emulator: DescribeLogStreams answers 200 with logStreams: [] forever - the step polled 13 times and the wait was capped at 60 s without the stream ever appearing.
Why: any workflow that waits for a vended delivery to be validated (the standard way to confirm a delivery is live) never settles, and a caller cannot distinguish "delivery not validated" from "log group empty".
Fix: the CreateDelivery / delivery state wiring must create the validation stream in the destination log group when the delivery is created, so the existing DescribeLogStreams reader lists it. Open issue "logs vended-delivery pipeline: delivery records and destination policies lose modeled fields and validation" (20261029T222444Z-logs-vended-delivery-pipeline-...) owns other steps of this same fixture but not this replay key, so it does not cover this cause.
Traces: logs-agentcore-traj-rewire-delivery-for-deleted-runtime#task#8.

Cause 6 of 6 - DescribeLogGroups answers 400 to a list query AWS answers 200 with an empty page.
AWS: DescribeLogGroups declares only InvalidParameterException (400) and ServiceUnavailableException (500); a well-formed query that matches no log group is a success - status 200 with logGroups present as an EMPTY LIST (the member is serialized as [], not omitted) and no error body. Note the model permits logGroupNamePattern at @length min 0, i.e. a zero-length pattern is a legal value.
Emulator: the same request is refused with status 400 and the response carries no logGroups member at all.
Why: a caller doing log-group discovery gets a hard client error where AWS reports "nothing matched", so the discovery branch that handles an empty result is never reached and the workflow aborts.
Fix: DescribeLogGroups input validation - stop refusing the accepted parameter form used here, and make the success path always emit logGroups (as [] when nothing matches). Hypothesis: the emulator is rejecting a selector combination AWS accepts - this step follows the prefix+pattern mutual-exclusion refusal earlier in the same fixture, so the single-selector retry (plausibly a zero-length or non-matching logGroupNamePattern, which the model allows) is what is being refused; the replay records only the status and the missing member, not the request, so this is a hypothesis and not a stated requirement.
Traces: logs-osdocstat-traj-find-domain-log-groups#task#7.

Related open issue: "logs read paths omit modeled response members and serve non-canonical values (9 causes)" (20261029T222444Z-logs-read-paths-omit-modeled-response-members-and.md) reports the same family on other operations and does NOT carry any of the six replay keys above, so it does not cover them.

## Evidence
Cause 1 (logs-agentcore-traj-vended-groups-empty-reference-path#task#11, DescribeLogGroups, 200 -> 200): $.logGroups[0..2].logGroupArn missing (expected e.g. 'arn:aws:logs:us-west-2:000000000000:log-group:/aws/bedrock-agentcore/runtimes/<runtime>-DEFAULT', got None); $.logGroups[0..2].logGroupClass missing (expected 'STANDARD', got None); $.logGroups[0..2].metricFilterCount missing (expected 0, got None); $.nextToken missing (expected a token, got None).
Cause 2 (logs-osdocstat-traj-find-domain-log-groups#task#4): identical detail for two groups (/aws/opensearch/domains/<domain>/audit-logs and <domain>-app-logs).
Cause 3 (logs-agentcore-traj-vended-groups-empty-reference-path#task#10, GetLogEvents, 200 -> 200): $.events[0].eventId missing (expected a value, got None); $.nextBackwardToken / $.nextForwardToken value_mismatch - got 'b/7b226b696e64223a224765744c6f674576656e7473...' / 'f/7b22...', a ~600-character hex blob that decodes to {"kind":"GetLogEvents","logGroupName":...,"logStreamName":...,"timestamp":...,"seq":0,"startTime":null,"endTime":null}.
Cause 4 (logs-osdocstat-traj-audit-log-json-filter-patterns#task#12): $.events[0].eventId and $.events[1].eventId missing; both tokens the same hex-encoded cursor form.
Cause 5 (logs-agentcore-traj-rewire-delivery-for-deleted-runtime#task#8, DescribeLogStreams, 200 -> 200): $.logStreams poll: 0 item(s), not 1 after 13 attempt(s), poll capped at 60s - expected one stream {logStreamName 'log_stream_created_by_aws_to_validate_log_delivery_subscriptions', arn 'arn:aws:logs:us-west-2:000000000000:log-group:/aws/vendedlogs/bedrock-agentcore/runtime/APPLICATION_LOGS/<runtime>:log-stream:log_stream_created_by_aws_to_validate_log_delivery_subscriptions', creationTime shaped}, got {'logStreams': []}.
Cause 6 (logs-osdocstat-traj-find-domain-log-groups#task#7, DescribeLogGroups): status expected 200, got 400; $.logGroups missing (expected [], got None).
Model evidence: smithy_query on services/service-com-amazonaws-cloudwatchlogs-logs-20140328 - LogGroup members include metricFilterCount, logGroupClass and logGroupArn (doc: ARN without trailing ':*', to be used as logGroupIdentifier); DescribeLogGroupsResponse has logGroups + nextToken and is @paginated; OutputLogEvent models only timestamp/message/ingestionTime (so eventId is recorded behavior only); DescribeLogGroups errors are InvalidParameterException (400) and ServiceUnavailableException (500) only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: fixture logs-agentcore-traj-vended-groups-empty-reference-path, step task#11 - DescribeLogGroups over the three vended agentcore groups; inspect each entry for logGroupArn/logGroupClass/metricFilterCount and the response for nextToken.
3. Cause 2: fixture logs-osdocstat-traj-find-domain-log-groups, step task#4 - same assertion over two opensearch domain groups.
4. Cause 3: fixture logs-agentcore-traj-vended-groups-empty-reference-path, step task#10 - GetLogEvents on a stream with one event; inspect events[].eventId and the two continuation tokens.
5. Cause 4: fixture logs-osdocstat-traj-audit-log-json-filter-patterns, step task#12 - GetLogEvents with two events.
6. Cause 5: fixture logs-agentcore-traj-rewire-delivery-for-deleted-runtime, step task#8 - after the vended delivery is created, poll DescribeLogStreams on the APPLICATION_LOGS vended group for log_stream_created_by_aws_to_validate_log_delivery_subscriptions; it never appears within the 60 s cap.
7. Cause 6: fixture logs-osdocstat-traj-find-domain-log-groups, step task#7 - re-issue the DescribeLogGroups discovery query; it answers 400 instead of 200 with logGroups: [].
