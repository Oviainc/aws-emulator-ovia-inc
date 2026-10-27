Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.layer-content-does-not-echo-its-s3-source lambda__s3.layer-content-survives-deletion-of-its-source-object lambda__s3.invalid-notification-filter-rule-name-rejected
Issue-Key: lambda__s3.layer-content-does-not-echo-its-s3-source|run#10|AssertionError|assert keys == ["CodeSha256", "CodeSize", "Location"], (
Issue-Key: lambda__s3.layer-content-survives-deletion-of-its-source-object|_fetch#15|HTTPError|with urllib.request.urlopen(urllib.request.Request(_url, headers=_route), timeout=60) as resp: # noqa: S310 - URL from the API's answer
Issue-Key: lambda__s3.invalid-notification-filter-rule-name-rejected|run#9|AssertionError|assert {r["Name"] for r in stored[0]["Filter"]["Key"]["FilterRules"]} == {"Prefix", "Suffix"}, (
Issue-Commit: fbdf9d3baa643423c5a7eac120dec57cfd2abcbc

# lambda__s3: an S3-sourced layer version omits CodeSha256 and its Content.Location 404s after the source object is deleted, and S3 notification filter rule names are echoed in the submitted casing instead of Prefix/Suffix

> _Note: this issue was identified against the code base at commit `fbdf9d3baa643423c5a7eac120dec57cfd2abcbc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLayerVersion

## Description
Three distinct net-new root causes, numbered below, each with its own failing check ids; all three must be closed before this issue is submitted. The replay report did not mark any of these three as ALREADY FILED or OWNED. The open issue `20261025T022656Z-lambda-s3-layer-content-from-s3-loses-codesha256.md` is adjacent — it bundles the layer causes together with a CloudFormation Ref cause and does not carry these replay keys — and its cause 4 misdiagnoses the notification failure as 'a configuration was dropped'; cause 3 below gives the actual diagnosis (both configurations are stored, the filter rule NAMES come back lowercase).

Cause 1 of 3 — GetLayerVersion omits CodeSha256 for a layer published from S3.
AWS: Smithy (lambda) LayerVersionContentOutput carries Location, CodeSha256 and CodeSize for every published layer version, whatever Content the publish used (ZipFile, or S3Bucket/S3Key/S3ObjectVersion); the digest is how a client verifies the bytes it published. The response must also not echo the S3 source location — Content carries exactly Location, CodeSha256 and CodeSize. Emulator: for the S3-sourced layer the Content block carries only CodeSize and Location; CodeSha256 is absent (the inline-published twin is unaffected). Why: a client cannot verify or compare the layer's content digest, cannot detect that two publishes were the same archive, and any code reading CodeSha256 unconditionally breaks for S3-sourced layers while working for inline ones. Fix: the layer content handling for the S3 source — compute and persist the digest at PublishLayerVersion and emit it from GetLayerVersion/PublishLayerVersion in the lambda service. Traces: lambda__s3.layer-content-does-not-echo-its-s3-source.

Cause 2 of 3 — the layer version's Content.Location stops serving once the source S3 object is deleted.
AWS: PublishLayerVersion copies the archive into Lambda's own content store at publish time, so the layer version's Content.Location keeps downloading the original bytes (same CodeSha256) after every version of the source S3 object has been deleted. Recorded behavior only — Location is a presigned download URL, not a modelled operation. Emulator: fetching Content.Location after the source object is deleted answers `HTTP Error 404: Not Found`, while the API still reports the layer version as present. Why: a client that cleans up its build bucket silently loses the content of every layer it published; functions referencing the layer can no longer be rebuilt and nothing in the control plane says the layer is hollow. Fix: copy-on-publish into the lambda layer content store, plus the data-plane route that serves Content.Location. Traces: lambda__s3.layer-content-survives-deletion-of-its-source-object.

Cause 3 of 3 — GetBucketNotificationConfiguration returns filter rule names in the submitted casing instead of S3's canonical Prefix/Suffix.
AWS: PutBucketNotificationConfiguration accepts the key filter rule names case-insensitively, and GetBucketNotificationConfiguration returns them canonicalised as `Prefix` and `Suffix` (the only two names S3 accepts; any other name is refused with InvalidArgument rather than stored). Recorded behavior only for the canonicalisation. Emulator: the stored configuration reads back `{'Filter': {'Key': {'FilterRules': [{'Name': 'prefix', 'Value': 'raw/'}, {'Name': 'suffix', 'Value': '.csv'}]}}}` — both rules ARE present, but their Name values are echoed verbatim in the lowercase casing they were submitted with. Why: SDKs and clients match filter rules on the canonical `Prefix`/`Suffix` names, so a round-trip of a configuration an emulator accepted produces rules the client cannot find — notification filters silently appear unset, and re-submitting the read-back value can accumulate duplicate rules. Fix: the filter-rule name normalisation in the PutBucketNotificationConfiguration storage/serialisation path in the s3 service (and the InvalidArgument refusal for names other than prefix/suffix). Traces: lambda__s3.invalid-notification-filter-rule-name-rejected.

## Evidence
Cause 1: AssertionError: the from-s3 layer's Content must carry exactly CodeSha256, CodeSize, Location; it carries ['CodeSize', 'Location'].
Cause 2: urllib.error.HTTPError: HTTP Error 404: Not Found — raised fetching the layer version's Content.Location after every version of the source S3 object was deleted.
Cause 3: AssertionError: the control configuration must store both accepted rule names; it reads [{'Id': '...', 'LambdaFunctionArn': 'arn:aws:lambda:<region>:<account>:function:<name>', 'Events': ['s3:ObjectCreated:*'], 'Filter': {'Key': {'FilterRules': [{'Name': 'prefix', 'Value': 'raw/'}, {'Name': 'suffix', 'Value': '.csv'}]}}}] — expected the rule names {'Prefix', 'Suffix'}.

## Steps to Reproduce
1. (Cause 1) Put a layer zip in S3; PublishLayerVersion once with Content={'ZipFile': ...} and once with Content={'S3Bucket','S3Key'}; GetLayerVersion both and compare the Content keys.
2. (Cause 2) For the S3-sourced layer, delete every version of the source object, then fetch the layer version's Content.Location over HTTP.
3. (Cause 3) PutBucketNotificationConfiguration with a LambdaFunctionConfiguration whose Filter.Key.FilterRules use the names 'prefix' and 'suffix', then GetBucketNotificationConfiguration and inspect the returned rule names.
