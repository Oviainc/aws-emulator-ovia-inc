Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.deploy-lambda-opensearch-s3-stack-positive lambda__opensearch__s3.deploy-s3-notification-lambda-opensearch-pipeline-positive
Issue-Key: lambda__opensearch__s3.deploy-lambda-opensearch-s3-stack-positive|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: lambda__opensearch__s3.deploy-s3-notification-lambda-opensearch-pipeline-positive|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: 6ff397d47b7f23b010f8590f84afedb6b986e178

# lambda__opensearch__s3: a CloudFormation-deployed S3→Lambda→OpenSearch pipeline never indexes the uploaded document

> _Note: this issue was identified against the code base at commit `6ff397d47b7f23b010f8590f84afedb6b986e178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Cause 1 of 1: the deployed stack's S3 ObjectCreated notification never reaches the indexing Lambda, so no document is ever written to the OpenSearch domain.
AWS: recorded behavior only for the end-to-end effect. Once the stack (bucket + OpenSearch domain + indexing function + bucket notification + domain access policy) is CREATE_COMPLETE, a PutObject of `items/widget-1.json` triggers the function, which indexes the document under id `items_widget-1.json`. A SigV4-signed `GET https://<domain endpoint>/documents/_doc/items_widget-1.json` then answers HTTP 200 with `_source` equal to the uploaded JSON body.
Emulator: that GET never answers 200 for the whole poll — the document is never indexed — and both checks are cut off by the harness after 60.0 s with no verdict. Nothing in the control plane reports a problem: the stack deploys and DescribeDomain hands back an endpoint.
Why: the whole search-indexing pattern this service covers is unobservable. A client gets a green deploy and an addressable domain endpoint, but documents uploaded to the bucket never become searchable, and there is no API-visible signal of the break.
Fix: the S3 object-write notification dispatch that must invoke the bucket's Lambda destination (and the Lambda execution path that runs the indexing handler against the OpenSearch data plane); both deploy shapes in this service exercise the same wiring.
The open issue 20261103T020000Z-lambda-opensearch-s3-invoke-execution-and-cloudformation-stack-causes.md covers related invoke/stack causes in this service but does not carry these two checks' replay keys.
Traces: lambda__opensearch__s3.deploy-lambda-opensearch-s3-stack-positive and lambda__opensearch__s3.deploy-s3-notification-lambda-opensearch-pipeline-positive.

## Evidence
Expected: after `_deploy_cfn(...)` and PutObject(Bucket=<stack bucket>, Key="items/widget-1.json", Body=json of {"id":"widget-1","title":"sim2real widget","qty":7}), a signed GET /documents/_doc/items_widget-1.json on DescribeDomain(...).DomainStatus.Endpoint answers status 200 with body["_source"] equal to that document.
Actual: the polled GET never returns 200 (the document is absent from the index) and each check ends with "Error: Check timed out after 60.0s". Both checks fail identically; only the generated bucket/domain/function names differ.

## Steps to Reproduce
Replay lambda__opensearch__s3.deploy-lambda-opensearch-s3-stack-positive (and deploy-s3-notification-lambda-opensearch-pipeline-positive): deploy the stack built by _build_template(..., with_notification=True), PutObject items/widget-1.json into the stack's bucket, read DescribeDomain's Endpoint, then issue a SigV4-signed (service "es") GET of /documents/_doc/items_widget-1.json.
