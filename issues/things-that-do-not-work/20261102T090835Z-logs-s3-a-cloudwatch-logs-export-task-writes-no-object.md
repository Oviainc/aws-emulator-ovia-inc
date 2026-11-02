Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__s3.export-task-objects-carry-the-destination-buckets-own-encryption
Issue-Key: logs__s3.export-task-objects-carry-the-destination-buckets-own-encryption|_run_export_until_content#12|Timeout|time.sleep(2.0)
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# A CloudWatch Logs export task writes no object to the destination bucket, so its encryption and content cannot be read (1 cause)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExportTask

## Description
Cause 1 of 1 — the objects an export task should write to S3 never appear, so the exported data and the destination bucket's encryption applied to it are unobservable.
AWS: recorded behavior only. CreateExportTask over a log group holding events writes the exported log content as objects under the requested destination prefix in the destination bucket, and those objects carry the DESTINATION bucket's own default encryption (the bucket's SSE configuration decides their encryption, not the log group's CMK). A reader can list the prefix, fetch an object, and read both its content and its server-side-encryption headers.
Emulator: no object is ever written under the destination prefix, so the check's bounded poll for the exported object never succeeds and the harness ends it with `Check timed out after 60.0s`.
Why: the exported objects are the entire output of an export task. Without them a client that exports log data to S3 receives nothing, gets no refusal explaining why, and cannot verify either the content or the encryption the destination bucket imposes on it — the property compliance tooling checks.
Fix: the export-task worker in the logs crate — write the exported events to the destination bucket under the requested prefix (letting the S3 bucket's own default encryption apply) before the task is reported COMPLETED. Note: open issues 20261025T181311Z-logs-s3-cloudwatch-logs-export-tasks-never-run.md and 20261028T080827Z-logs-s3-an-export-task-reaches-completed-while.md record this family and carry none of this check's keys.
Traces: logs__s3.export-task-objects-carry-the-destination-buckets-own-encryption

One cause must be closed before submitting.

## Evidence
From the simparity-behavioral-aws-v1 replay against this checkout.

export-task-objects-carry-the-destination-buckets-own-encryption:
  Error: Check timed out after 60.0s

The check creates an export task into a bucket with its own default encryption and polls the destination prefix for the exported object (to read its content and its SSE headers); no object ever appears. A sibling check in the same group records the same absence as 'positive control: the accepted export wrote nothing under <prefix>/: []'.

## Steps to Reproduce
Boot the emulator from this checkout.

1. aws logs create-log-group --log-group-name g ; aws logs create-log-stream --log-group-name g --log-stream-name s ; aws logs put-log-events --log-group-name g --log-stream-name s --log-events timestamp=<now>,message=hello
2. aws s3api create-bucket --bucket b ; aws s3api put-bucket-encryption --bucket b --server-side-encryption-configuration '{"Rules":[{"ApplyServerSideEncryptionByDefault":{"SSEAlgorithm":"AES256"}}]}'
3. aws logs create-export-task --log-group-name g --from <t0> --to <t1> --destination b --destination-prefix exp
4. aws logs describe-export-tasks and then aws s3api list-objects-v2 --bucket b --prefix exp/
-> the prefix stays empty. AWS writes the exported object(s) there, and head-object reports the bucket's default encryption.

Then re-replay: logs__s3.export-task-objects-carry-the-destination-buckets-own-encryption
