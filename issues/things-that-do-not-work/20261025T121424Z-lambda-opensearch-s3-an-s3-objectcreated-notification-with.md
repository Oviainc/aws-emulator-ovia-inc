Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.s3-notification-filter-suffix-positive lambda__opensearch__s3.s3-notification-lambda-opensearch-write-failure-negative
Issue-Key: lambda__opensearch__s3.s3-notification-filter-suffix-positive|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: lambda__opensearch__s3.s3-notification-lambda-opensearch-write-failure-negative|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: e6eec4415380d0861b3cd74514fe77af8743d21c

# lambda__opensearch__s3: an S3 ObjectCreated notification with a Lambda target never invokes the function, so no document is ever indexed and neither the suffix filter nor the denied-write path can be observed

> _Note: this issue was identified against the code base at commit `e6eec4415380d0861b3cd74514fe77af8743d21c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
One root cause (Cause 1 of 1), grounded in two failing checks that no open issue carries.

Cause 1 of 1: a bucket notification configuration naming a Lambda function is accepted but is inert — a PutObject that matches the configuration produces no invocation, so every downstream effect of the S3 -> Lambda -> OpenSearch chain is absent.
AWS: Smithy s3#PutBucketNotificationConfiguration (PUT /{Bucket}?notification) stores a LambdaFunctionConfiguration whose Events include 's3:ObjectCreated:*' and whose optional Filter.Key.FilterRules scope delivery by prefix/suffix; AWS resolves and authorizes the destination at configuration time and then, for every matching object write, invokes the named function with the S3 event record. The two checks pin the observable consequences: (a) with a suffix filter '.json', writing payloads/data.json causes the function to index a document that a signed GET of /<index>/_doc/<url-encoded key> answers 200 with _source.key equal to the key, while payloads/data.txt is never delivered (that document stays 404) and /<index>/_count reports exactly 1; (b) with no filter but an OpenSearch domain access policy that omits the function's role, the invoked function receives 403 from the domain, writes its error marker object errors/<key>.err with body b'403' and indexes nothing — and after UpdateDomainConfig adds the role to the access policy the identical write is indexed (200).
Emulator: nothing is delivered. Both checks wait on the first observable effect of the invocation — the indexed document (bounded 120 s poll) and the error-marker object (bounded 90 s poll) — and neither ever appears, so both are cut off at the harness cap ('Error: Check timed out after 60.0s'). No invocation, no document, no marker.
Why: an event-driven ingest pipeline wired exactly as AWS documents produces silence, which is indistinguishable from a slow run: a developer cannot tell a broken filter from a broken trigger, cannot test the negative path (a permission error at the destination must leave NO document), and any delivery-filter regression in their own code is invisible here.
Fix: the S3 event emitter on the object write paths and the Lambda-target delivery path it feeds (destination resolution/authorization in the PutBucketNotificationConfiguration handler, and the invocation of the configured function). Related open issues: 20261025T022656Z-dynamodb-lambda-s3-an-s3-putobject-never-invokes.md, 20261024T190039Z-events-lambda-s3-sfn-s3-event-notifications-never-fire-a-putobject-publishes.md and Cause 7 of 20261025T032458Z-s3-request-auth-and-eventing-plane-10-root.md describe the same family in other service groups, and 20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md carries the third check in this group; none of them carries either check id below, and these two additionally pin the suffix-filter scoping and the destination-denied negative path.
Traces: lambda__opensearch__s3.s3-notification-filter-suffix-positive, lambda__opensearch__s3.s3-notification-lambda-opensearch-write-failure-negative

## Evidence
Both failing checks report `Error: Check timed out after 60.0s` with no response from the chain.
lambda__opensearch__s3.s3-notification-filter-suffix-positive: after PutBucketNotificationConfiguration with Events=['s3:ObjectCreated:*'] and FilterRules=[{'Name':'suffix','Value':'.json'}] and AddPermission for the S3 principal, PutObject of payloads/data.json never produces the indexed document the check polls for (`_wait_for_effect(... 'matching .json key indexed', timeout_s=120)`); required: a signed GET of /objects/_doc/payloads%2Fdata.json answers 200 with _source.key='payloads/data.json', /objects/_doc/payloads%2Fdata.txt answers 404, and /objects/_count reports 1.
lambda__opensearch__s3.s3-notification-lambda-opensearch-write-failure-negative: after the same wiring with a domain access policy that omits the Lambda role, the error marker the function writes on a 403 never appears (`_wait_for_effect(... 'error marker errors/denied-doc.txt.err written after OpenSearch AccessDenied', timeout_s=90)`); required: that object exists with body b'403', the denied document is 404, and after the access policy is updated the next write is indexed 200.

## Steps to Reproduce
1) CreateBucket; create a Lambda execution role and an OpenSearch domain; CreateFunction with an indexing handler. 2) lambda.add_permission for principal s3.amazonaws.com on the function. 3) s3.put_bucket_notification_configuration with a LambdaFunctionConfiguration for 's3:ObjectCreated:*' (once with Filter.Key.FilterRules suffix='.json', once with no filter). 4) PutObject a matching key and a non-matching key. 5) Observe that the function is never invoked: no document is indexed in the domain and, in the negative variant, no errors/<key>.err marker object is written — both polls run to their bound. Replay checks lambda__opensearch__s3.s3-notification-filter-suffix-positive and lambda__opensearch__s3.s3-notification-lambda-opensearch-write-failure-negative to confirm the fix.
