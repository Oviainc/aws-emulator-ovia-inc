Issue-Source: trace-sweep-divergence
Issue-Trace: logs-import-task-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-import-task-error-paths logs-import-task-lifecycle logs-insights-query-workflow logs-log-group-query-discovery logs-query-log-groups-for-query logs-logs-insights-query-workflow logs-osdocstat-traj-insights-query-audit-group
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|CreateImportTask|4xx->4xx|$.__type
Issue-Key: logs|CreateImportTask|4xx->4xx|$.message
Issue-Key: logs|ListLogGroupsForQuery|2xx->2xx|$.logGroupIdentifiers[*]
Issue-Key: logs|GetQueryResults|4xx->4xx|$.__type
Issue-Key: logs|GetQueryResults|4xx->4xx|$.message
Issue-Key: logs|GetQueryResults|2xx->2xx|$.results[*][*].value
Issue-Commit: ff1b8d9cb347801d3f8cfa6abe35a387dbb8463c

# logs: 4 net-new causes — Insights query-result identifiers and @ptr encoding, plus two InvalidParameterException refusals answered as ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `ff1b8d9cb347801d3f8cfa6abe35a387dbb8463c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryResults

## Description
Four net-new wire divergences in the CloudWatch Logs (logs, logs-2014-03-28) emulator, all in the same family: the service answers 400-class refusals with the wrong modeled error shape and emulator-authored wording, and it renders Insights query-result identifiers/pointers in its own internal form instead of AWS's. ALL FOUR must be closed before this issue is submitted.

Cause 1 of 4: CreateImportTask does not reject a cross-account importRoleArn, and answers the source-lookup refusal instead.
AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328, logs-2014-03-28) models CreateImportTask with required input members importSourceArn (target Arn) and importRoleArn (target RoleArn, @length min 1 — documented as "The ARN of the IAM role that grants CloudWatch Logs permission to import from the CloudTrail Lake Event Data Store"), and declares AccessDeniedException, ConflictException, InvalidOperationException, InvalidParameterException, ResourceNotFoundException, ThrottlingException and ValidationException, all client errors with httpError 400. Recorded behavior only for the precedence and wording: when the importRoleArn names an account other than the caller's, AWS refuses the call with HTTP 400, code InvalidParameterException and the message `Cross-account role passing is not allowed.` This message is entirely fixed text with NO variable part — no ARN, account id or member name is interpolated. The check fires on the role ARN's account before the importSourceArn is resolved against any event data store, so a request that is both cross-account AND names a nonexistent source still gets this refusal.
Emulator: the same request is answered HTTP 400 with code ResourceNotFoundException and the message `The specified import source does not exist.` — a different modeled error shape and a sentence that is not AWS's. The cross-account role check is absent entirely, so the request falls through to the import-source lookup.
Why: a client branching on the refusal class gets the wrong branch — InvalidParameterException is a permanent request defect the caller must fix (correct the role's account), while ResourceNotFoundException is routinely treated as a resource that may appear later and is retried, waited on, or swallowed as "already gone". The operator is also pointed at the wrong input: the message blames the import source when the defect is the role ARN's account.
Fix: the logs CreateImportTask handler's input validation — add the importRoleArn account check and order it BEFORE the importSourceArn state lookup, raising InvalidParameterException with AWS's fixed sentence. NOTE: open issue 20261109T035408Z-logs-createimporttask-a-nonexistent-import-source-is-refused.md covers the DIFFERENT wrong refusal on this same operation (a nonexistent importSourceArn must be InvalidParameterException `Invalid Event Data Store ARN: <importSourceArn>`, not ResourceNotFoundException). It does not cover the cross-account role path and does not carry this fixture step, so this cause is net-new; fixing only that issue leaves this one failing.
Traces: logs-import-task-error-paths#task#2 (cluster also covers logs-import-task-lifecycle#task#3).

Cause 2 of 4: ListLogGroupsForQuery returns full log-group ARNs where AWS returns the identifier form the query was submitted with (the bare log-group name).
AWS: Smithy models ListLogGroupsForQuery with required queryId and output member logGroupIdentifiers (target LogGroupIdentifiers, list), documented as "An array of the names and ARNs of the log groups that were processed in the query." Recorded behavior only for the choice between the two: AWS echoes each log group in the SAME identifier form the query's own log-group selector used — a query submitted with bare log-group names lists bare names, not ARNs. The variable part is the log-group identifier taken verbatim from the StartQuery request; the recorded name value is one instance filling that, not part of the requirement.
Emulator: for a query started with bare log-group names, logGroupIdentifiers[i] is normalized to the full ARN form `arn:aws:logs:<region>:<account>:log-group:<name>` instead of the submitted name.
Why: callers correlate this list against the names they passed to StartQuery (set membership, dict lookups, display). An unrequested ARN rewrite makes every such comparison miss, so the query appears to have processed log groups the caller never named and none of the ones it did.
Fix: the logs ListLogGroupsForQuery handler/serializer must emit the stored per-query log-group identifiers as submitted rather than re-rendering them from state as ARNs.
Traces: logs-insights-query-workflow#task#5 (cluster also covers logs-log-group-query-discovery#task#5 and logs-query-log-groups-for-query#task#5).

Cause 3 of 4: GetQueryResults for an unknown queryId uses the wrong error class and misspaces AWS's message.
AWS: Smithy models GetQueryResults with required queryId (@length 1..256) and declares exactly InvalidParameterException, ResourceNotFoundException (client, httpError 400) and ServiceUnavailableException (server, 500). Recorded behavior for this path: an all-zero / unknown queryId is refused HTTP 400 with code InvalidParameterException and the message `The provided queryId = 00000000-0000-0000-0000-000000000000 is invalid ` — the fixed text is `The provided queryId = `, a space-delimited `=` on both sides, followed by the request's queryId value, then the fixed suffix ` is invalid ` with a trailing space and NO terminating period. The only variable part is the queryId copied verbatim from the request; the all-zero UUID in the recording is one instance filling that slot.
Emulator: answers HTTP 400 with code ResourceNotFoundException and the message `The provided queryId=00000000-0000-0000-0000-000000000000 is invalid.` — wrong modeled error shape, `=` with no surrounding spaces, and a period instead of AWS's trailing space.
Why: a client branching on the class treats an unrecognised query id as a resource that may materialize (poll/retry loop) rather than as a permanent bad-parameter failure, so Insights polling loops hang on a typo'd id instead of failing fast; assertions on AWS's exact refusal text also fail.
Fix: the logs GetQueryResults handler's queryId validation — classify an unresolvable queryId as InvalidParameterException and render AWS's exact template, spaces and all.
Traces: logs-logs-insights-query-workflow#task#21.

Cause 4 of 4: the Insights @ptr result field is emitted in the emulator's own hex-encoded-JSON form instead of AWS's pointer encoding.
AWS: Smithy models GetQueryResults.results as QueryResults, a list of rows of ResultField ("Contains one field from one log event returned by a CloudWatch Logs Insights query, along with the value of that field"); the discovered-field @ptr is the opaque record pointer later handed to GetLogRecord/GetLogObject. Recorded behavior only: AWS's @ptr value is a single opaque token of the form `hotstore:` followed by a base64 payload. The requirement here is the ENCODING — the `hotstore:` scheme prefix and base64 body — not the recorded token's contents: the decoded payload embeds the account id, log-group name and ingest timestamps of one particular run, which are per-run values and are not part of the template.
Emulator: the same field's value is a bare lowercase hex string with no scheme prefix (hex of a JSON object `{"kind":"LogRecordPointer","logGroupName":...,"logStreamName":...,"timestamp":...,"seq":...}`) — the emulator's internal pointer struct leaked onto the wire.
Why: @ptr is contractually opaque but its encoding is observable: clients and tooling that detect, store, length-bound, or round-trip pointers (and anything asserting the `hotstore:`/base64 shape before calling GetLogRecord) see a value AWS would never return, and the leaked JSON exposes emulator internals as API output.
Fix: the logs Insights query-results serializer's @ptr rendering — encode the pointer struct as `hotstore:` + base64 instead of hex-encoded JSON. Related open issues 20261108T145640Z-logs-getlogrecord-stub.md and 20261109T143515Z-logs-5-net-new-causes-getlogobject-pointer-refusal.md concern the pointer CONSUMERS; neither carries this fixture step nor the producer-side encoding, so this cause is net-new.
Traces: logs-osdocstat-traj-insights-query-audit-group#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (emulator_up=true, bootstrap_signal=false, 1208 steps, 1039 passed, score 86.0%). The four clusters below are the only divergences with neither a stub_path nor an owning open issue.

Cause 1 — fixture logs-import-task-error-paths, step task#2 (service=logs, CreateImportTask, expected 400 / actual 400):
  $.__type: value_mismatch (expected 'InvalidParameterException', got 'ResourceNotFoundException')
  $.message: value_mismatch (expected 'Cross-account role passing is not allowed.', got 'The specified import source does not exist.')
  also affected: logs-import-task-lifecycle#task#3

Cause 2 — fixture logs-insights-query-workflow, step task#5 (service=logs, ListLogGroupsForQuery, expected 200 / actual 200):
  $.logGroupIdentifiers[0]: missing (expected the bare log-group name as submitted, got 'arn:aws:logs:<region>:<account>:log-group:<that same name>')
  also affected: logs-log-group-query-discovery#task#5, logs-query-log-groups-for-query#task#5

Cause 3 — fixture logs-logs-insights-query-workflow, step task#21 (service=logs, GetQueryResults, expected 400 / actual 400):
  $.__type: value_mismatch (expected 'InvalidParameterException', got 'ResourceNotFoundException')
  $.message: value_mismatch (expected 'The provided queryId = <queryId> is invalid ' with spaced '=' and trailing space, got 'The provided queryId=<queryId> is invalid.')

Cause 4 — fixture logs-osdocstat-traj-insights-query-audit-group, step task#9 (service=logs, GetQueryResults, expected 200 / actual 200):
  $.results[0][3].value: value_mismatch — expected a `hotstore:`-prefixed base64 token; got a bare hex string decoding to {"kind":"LogRecordPointer","logGroupName":"/aws/opensearch/domains/<domain>/audit-logs","logStreamName":"<stream>","timestamp":<ms>,"seq":0}. Only the encoding (scheme prefix + base64 vs bare hex JSON) is asserted here; the decoded account id, generated domain name and timestamps are per-run values.

Model evidence (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json): CreateImportTask errors include both InvalidParameterException and ResourceNotFoundException (both 400), so the emulator is picking the wrong one rather than lacking the shape; GetQueryResults declares only InvalidParameterException, ResourceNotFoundException and ServiceUnavailableException; ListLogGroupsForQueryResponse.logGroupIdentifiers is documented as "An array of the names and ARNs of the log groups that were processed in the query."

## Steps to Reproduce
1. Clone the repo and build/boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1) and re-replay these fixture steps: logs-import-task-error-paths#task#2, logs-insights-query-workflow#task#5, logs-logs-insights-query-workflow#task#21, logs-osdocstat-traj-insights-query-audit-group#task#9.
3. Manual repro, cause 1: call logs CreateImportTask with importRoleArn whose account differs from the caller's account and any importSourceArn. Observed 400 ResourceNotFoundException 'The specified import source does not exist.'; expected 400 InvalidParameterException 'Cross-account role passing is not allowed.'
4. Manual repro, cause 2: StartQuery naming a log group by bare NAME, then ListLogGroupsForQuery with the returned queryId. Observed logGroupIdentifiers containing the full log-group ARN; expected the bare name as submitted.
5. Manual repro, cause 3: call logs GetQueryResults with queryId='00000000-0000-0000-0000-000000000000'. Observed 400 ResourceNotFoundException 'The provided queryId=<id> is invalid.'; expected 400 InvalidParameterException 'The provided queryId = <id> is invalid ' (spaces around '=', trailing space, no period).
6. Manual repro, cause 4: run any Insights query that returns records (fields @timestamp, @message, @ptr) and inspect the @ptr field of a result row. Observed a bare hex string that decodes to the emulator's LogRecordPointer JSON; expected a 'hotstore:'-prefixed base64 token.
