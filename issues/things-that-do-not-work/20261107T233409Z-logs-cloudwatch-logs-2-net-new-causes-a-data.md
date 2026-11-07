Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.data-protection-policy-masks-sensitive-data-server-side logs.delete-data-protection-policy-unmasks-subsequent-events logs.e2e-export-task-produces-s3-object-with-log-content logs.export-task-honors-time-window-boundaries logs.export-task-server-managed-completion
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.data-protection-policy-masks-sensitive-data-server-side|run#15|AssertionError|assert masked_message is not None, (
Issue-Key: logs.delete-data-protection-policy-unmasks-subsequent-events|_poll_until_masked#9|AssertionError|raise AssertionError(
Issue-Key: logs.e2e-export-task-produces-s3-object-with-log-content|_download_prefix_text#16|AssertionError|assert keys, f"no objects found under s3://{bucket}/{prefix}"
Issue-Key: logs.export-task-honors-time-window-boundaries|run#24|AssertionError|raise AssertionError(
Issue-Key: logs.export-task-server-managed-completion|_poll_until#19|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last result={result!r}")
Issue-Commit: 8f24345936df6262f616e6a72be898a454a52720

# CloudWatch Logs: 2 net-new causes — a data protection policy never masks events on ingest, and an export task reports COMPLETED while writing no object to the destination bucket

> _Note: this issue was identified against the code base at commit `8f24345936df6262f616e6a72be898a454a52720`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExportTask

## Description
TWO net-new root causes in the CloudWatch Logs crate (Cause 1 of 2 and Cause 2 of 2), grounded in five failing checks. None of the five is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 2 — a log group's data protection policy is stored but never applied to ingested events, so no value is ever masked (and consequently deleting the policy cannot be shown to unmask).
AWS: Smithy: PutDataProtectionPolicy(logGroupIdentifier, policyDocument) returns 200 with the logGroupIdentifier, the policyDocument and lastUpdatedTime; it models InvalidParameterException / LimitExceededException / OperationAbortedException / ResourceNotFoundException for refusals, so an accepted call means the policy is in force. Documented behavior: events ingested AFTER the policy is in place are audited and the matched data identifier's value (e.g. EmailAddress) is MASKED in every plain read — GetLogEvents/FilterLogEvents without unmask=true return the message with the matched substring replaced by asterisk characters ('*'), the surrounding text unchanged; reading the raw value requires unmask=true plus the logs:Unmask permission. DeleteDataProtectionPolicy removes the policy, and events ingested after the delete are returned verbatim.
Emulator: PutDataProtectionPolicy is accepted, but subsequently ingested events come back from GetLogEvents byte-for-byte as they were put: the raw value is present and no mask marker ('*') ever appears, within the checks' 120 s budgets. The second check cannot even reach its "delete then unmask" assertion because the masked phase never happens.
Why: a client that relies on CloudWatch Logs masking to keep PII out of plain reads gets the sensitive value served to every unprivileged reader while the control plane reports the protection as configured — a silent data-protection failure, and the unmask permission boundary is unobservable.
Fix: the log-event ingest path must apply the group's (and the account's) data protection policy when writing events and mask matched spans in the non-unmasked read path (PutLogEvents ingest + GetLogEvents/FilterLogEvents projection), and DeleteDataProtectionPolicy must stop that masking for later events. Related OPEN issue 20261029T182816Z-logs-transformers-are-not-applied-on-ingest-data.md carries the same symptom for two OTHER checks but not these, so it does not cover this cause.
Traces: logs.data-protection-policy-masks-sensitive-data-server-side, logs.delete-data-protection-policy-unmasks-subsequent-events.

Cause 2 of 2 — CreateExportTask drives the task to COMPLETED but writes no object into the destination bucket.
AWS: Smithy: CreateExportTask(logGroupName, from, to, destination, destinationPrefix) returns a taskId; DescribeExportTasks reports the task's ExportTaskStatusCode. Documented behavior: the managed service copies every event whose timestamp is in [from, to) out of the group and writes gzip objects under s3://<destination>/<destinationPrefix>/... ; the task only reaches COMPLETED once that copy has been made, so a client may treat COMPLETED as "the objects exist". Events outside the window are not written; a window containing confirmed-visible events yields at least one object.
Emulator: the task reaches COMPLETED (DescribeExportTasks status code COMPLETED) but an independent read of the destination — ListObjectsV2 under the destinationPrefix — returns no Contents at all, including for hours-old events the check first confirms are readable through GetLogEvents. So COMPLETED is a self-report with nothing behind it, and the time-window contract cannot be exercised.
Why: a client that archives log data to S3 and then trusts COMPLETED loses the data silently — the bucket is empty while the control plane says the export succeeded; downstream consumers of the exported objects (Athena, Glue, retention jobs) read nothing and cannot distinguish an empty window from a failed copy.
Fix: the CreateExportTask executor in the logs crate's export component — perform the copy (gzip objects under destinationPrefix, honouring [from, to)) and only transition the task to COMPLETED once objects are written; a copy that cannot be made must end FAILED with its reason. Related OPEN issue 20261026T102156Z-logs-cloudwatch-logs-export-tasks-complete-but-write-no.md carries the same symptom for the check logs.export-task-time-range-window-respected only, so it does not cover these three.
Traces: logs.e2e-export-task-produces-s3-object-with-log-content, logs.export-task-honors-time-window-boundaries, logs.export-task-server-managed-completion.

## Evidence
Cause 1:
- logs.data-protection-policy-masks-sensitive-data-server-side — AssertionError: "sensitive email was never masked (raw value absent AND a mask marker '*' present) in plain GetLogEvents within the timeout". Expected: the plain GetLogEvents message contains asterisks in place of the email and not the raw address.
- logs.delete-data-protection-policy-unmasks-subsequent-events — AssertionError: "data protection policy never masked any event within 120.0s (last seen: 'contact email is <raw address>, order=<marker>')" — the ingested message is returned verbatim, so the post-delete unmask phase is never reached.

Cause 2:
- logs.e2e-export-task-produces-s3-object-with-log-content — AssertionError: "no objects found under s3://<bucket>/<prefix>" after the task reported COMPLETED.
- logs.export-task-honors-time-window-boundaries — AssertionError: "export task completed with no exported objects even for hours-old, confirmed-visible source events".
- logs.export-task-server-managed-completion — AssertionError: "condition not met within 60.0s; last result=[]" — DescribeExportTasks said COMPLETED, then ListObjectsV2(Bucket=<bucket>, Prefix=<destinationPrefix>) returned no Contents for the whole 60 s poll.

## Steps to Reproduce
Cause 1:
1. logs.create_log_group(<group>) / create_log_stream(<stream>).
2. logs.put_data_protection_policy(logGroupIdentifier=<group>, policyDocument=<audit+deidentify policy with the EmailAddress data identifier>) — accepted.
3. logs.put_log_events of a message containing an email address.
4. logs.get_log_events(<group>, <stream>) without unmask — the raw address is returned and no '*' appears.
5. logs.delete_data_protection_policy(<group>), put another such event, read again — the "masked then unmasked" transition is unobservable because step 4 never masked.

Cause 2:
1. Create an S3 bucket and put the logs-export bucket policy for logs.<region>.amazonaws.com.
2. logs.create_log_group / create_log_stream / put_log_events of a probe event.
3. logs.create_export_task(logGroupName=<group>, destination=<bucket>, destinationPrefix=<prefix>, from=<now-5m>, to=<now+1m>).
4. Poll logs.describe_export_tasks until the status code is terminal — it is COMPLETED.
5. s3.list_objects_v2(Bucket=<bucket>, Prefix=<prefix>) — no Contents, ever.
