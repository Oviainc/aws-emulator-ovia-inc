Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__iam__s3.s3-proxy-integration-renamed-path-parameters
Issue-Key: apigateway__iam__s3.s3-proxy-integration-renamed-path-parameters|_poll_get#9|Timeout|time.sleep(interval_s)
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# apigateway__iam__s3: an S3 AWS-service proxy integration with renamed path parameters never answers, so neither the object bytes nor S3's NoSuchKey 404 reaches the caller

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegration (AWS S3 proxy integration) / GET on the deployed stage

## Description
One net-new root cause in the apigateway__iam__s3 suite; this heading has no open issue carrying this check's replay key.

Cause 1 of 1: a GET /{folder}/{item} method whose AWS-service S3 integration remaps the method's path parameters onto the integration's bucket/object path parameters never answers the viewer, so neither the parameter mapping nor the relayed S3 error can be observed.
AWS: recorded behavior only for the end-to-end relay; the mapping surface is modelled. An AWS-service integration to S3 with requestParameters mapping method.request.path.folder -> integration.request.path.bucket and method.request.path.item -> integration.request.path.object, invoked through a role that grants s3:GetObject on the named buckets, returns HTTP 200 with the selected object's exact bytes — the folder segment selects the BUCKET and the item segment selects the OBJECT, independently. For a valid bucket and an item that was never put, API Gateway relays S3's own refusal: HTTP 404 with the body <Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message><Key>missing.txt</Key>...</Error>, where Code and Message are fixed text for that error class and Key echoes the requested key verbatim (RequestId/HostId are per-request values).
Emulator: the check is reported "Error: Check timed out after 60.0s" with no assertion reached — the poll of the deployed stage's GET never returns a matching response, for any of the four legs (two buckets, two items, and the missing-key negative), so no status, body or error document was ever compared.
Why: a client using API Gateway as an S3 proxy — a very common pattern for serving private objects behind an API — gets no response at all. And because the positive legs never answer, the parameter-renaming contract is unverifiable: an integration that ignored the renames and always hit one bucket/object would be indistinguishable.
Fix: the API Gateway AWS-service integration path for S3 — it must apply the integration requestParameters mapping from method.request.path.* to integration.request.path.*, sign and issue the S3 request under the integration credentials role, and relay S3's status, headers and body (including the XML Error document) to the caller unchanged.
Traces: apigateway__iam__s3.s3-proxy-integration-renamed-path-parameters.

## Evidence
Check apigateway__iam__s3.s3-proxy-integration-renamed-path-parameters, verdict [mismatch]:
  Error: Check timed out after 60.0s
with no assertion reached.

The assertions that were never reached, from the check source:
  assert status == 200 and body == b"bucket-one-alpha"   (folder=bucket1, item=alpha.txt)
  assert status == 200 and body == b"bucket-two-alpha"   (folder=bucket2, item=alpha.txt — proves folder selects the bucket)
  assert status == 200 and body == b"bucket-one-beta"    (folder=bucket1, item=beta.txt — proves item selects the object)
  assert status == 404 and _s3_error(body) == {"Code": "NoSuchKey", "Message": "The specified key does not exist.", "Key": "missing.txt"}
where _s3_error strips RequestId and HostId as per-request values.

Expected: those four responses from the deployed stage. Actual: the stage never answers within the run budget. Bucket names, api ids and stage names are per-run values; "NoSuchKey" / "The specified key does not exist." are the fixed text of the error class and <Key> is the key from the request.

## Steps to Reproduce
1. Create two buckets; put alpha.txt in both with different bodies and beta.txt in the first.
2. Create an IAM role API Gateway can assume with s3:GetObject on both buckets.
3. Build a REST API with resource /{folder}/{item}, a GET method with requestParameters method.request.path.folder/item required, and an AWS-service integration to S3 whose requestParameters map them to integration.request.path.bucket/object, using that role as Credentials; deploy a stage.
4. GET /<bucket1>/alpha.txt, /<bucket2>/alpha.txt and /<bucket1>/beta.txt (expect 200 with each distinct body) and GET /<bucket1>/missing.txt (expect 404 with S3's NoSuchKey Error document).
5. The emulator never answers. Re-replay apigateway__iam__s3.s3-proxy-integration-renamed-path-parameters to confirm the fix.
