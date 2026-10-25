Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.layer-content-does-not-echo-its-s3-source lambda__s3.layer-content-survives-deletion-of-its-source-object lambda__s3.e2e-check-lambda-layers-s-code-lifecycle lambda__s3.invalid-notification-filter-rule-name-rejected
Issue-Key: lambda__s3.layer-content-does-not-echo-its-s3-source|run#9|AssertionError|assert keys == ["CodeSha256", "CodeSize", "Location"], (
Issue-Key: lambda__s3.layer-content-survives-deletion-of-its-source-object|_fetch#14|HTTPError|with urllib.request.urlopen(urllib.request.Request(_url, headers=_route), timeout=60) as resp: # noqa: S310 - URL from the API's answer
Issue-Key: lambda__s3.e2e-check-lambda-layers-s-code-lifecycle|phase#7|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: lambda__s3.invalid-notification-filter-rule-name-rejected|run#8|AssertionError|assert {r["Name"] for r in stored[0]["Filter"]["Key"]["FilterRules"]} == {"Prefix", "Suffix"}, (
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# lambda__s3: layer content from S3 loses CodeSha256 and its Content.Location 404s, a stack exports a layer name instead of its ARN, and a notification read-back drops a configuration (4 causes)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLayerVersion

## Description
Cause 1 of 4 — GetLayerVersion for an S3-sourced layer omits CodeSha256.
AWS: Smithy (lambda) LayerVersionContentOutput carries Location, CodeSha256 and CodeSize for every layer version, however the content was supplied (ZipFile or S3Bucket/S3Key); the digest is what lets a client verify the published bytes. Emulator: the Content block carries only ['CodeSize','Location']. Why: a client cannot verify or compare the layer's content digest, and code that reads CodeSha256 unconditionally breaks for S3-sourced layers while working for inline ones. Fix: the PublishLayerVersion/GetLayerVersion content handling for the S3 source in the lambda service (compute and store the digest). Traces: lambda__s3.layer-content-does-not-echo-its-s3-source.

Cause 2 of 4 — the layer's Content.Location stops serving once the source S3 object is deleted.
AWS: PublishLayerVersion copies the content into Lambda's own code store, so the layer version's Content.Location keeps serving the bytes after the source object is deleted. Emulator: fetching Content.Location answers HTTP 404. Why: a client that cleans up its build bucket silently loses every layer it published, and functions referencing the layer can no longer be rebuilt; the API still reports the layer version as present. Fix: content copy-on-publish in the lambda layer store and the data-plane route that serves Content.Location. Traces: lambda__s3.layer-content-survives-deletion-of-its-source-object.

Cause 3 of 4 — a stack exports a layer's logical resource name instead of its version ARN.
AWS: Ref on AWS::Lambda::LayerVersion is the layer version ARN (…:layer:<name>:<version>), which is what a stack output must carry so consumers can reference the layer. Emulator: the export is 'sim2real-scn-e2elyr-stack-<id>-S3Layer-171B2B528656' — a generated resource name, not an ARN. Why: anything consuming the output (a function's Layers list, a cross-stack import) gets a value Lambda will reject. Fix: the AWS::Lambda::LayerVersion provisioner's Ref/physical-id mapping in the CloudFormation resource handlers. Traces: lambda__s3.e2e-check-lambda-layers-s-code-lifecycle.

Cause 4 of 4 — the bucket-notification read-back does not reflect both accepted configurations.
AWS: GetBucketNotificationConfiguration returns every configuration PutBucketNotificationConfiguration accepted, each with its filter rules exactly as submitted; a filter rule whose Name is not 'prefix' or 'suffix' is refused with an InvalidArgument error instead of being silently dropped. Recorded behavior only for the exact refusal body. Emulator: the read-back carries a single LambdaFunctionConfiguration, so one of the accepted configurations is not stored or not serialised. Why: a client cannot trust the notification state it just wrote; events it configured never fire and nothing in the API says why. Fix: the PutBucketNotificationConfiguration storage/serialisation path in the s3 service (and its filter-rule-name validation). Traces: lambda__s3.invalid-notification-filter-rule-name-rejected.

## Evidence
Cause 1: "the from-s3 layer's Content must carry exactly CodeSha256, CodeSize, Location; it carries ['CodeSize','Location']".
Cause 2: urllib.error.HTTPError: HTTP Error 404: Not Found (fetching Content.Location after the source object was deleted).
Cause 3: "the stack must export the version ARN of layer 'sim2real-scn-e2elyr-...', version 1; it exported 'sim2real-scn-e2elyr-stack-c3e88b8067c5-S3Layer-171B2B528656'".
Cause 4: "the control configuration must store both accepted rule names; it reads [{... 'Filter': {'Key': {'FilterRules': [{'Name':'prefix',...},{'Name':'suffix',...}]}}}]" — only one configuration present.

## Steps to Reproduce
1. Upload a layer zip to S3 and PublishLayerVersion with Content={'S3Bucket','S3Key'}; GetLayerVersion and inspect Content.
2. Delete the S3 object, then fetch the layer version's Content.Location.
3. CreateStack with AWS::Lambda::LayerVersion (S3 content) and an Output that Refs it; read the output value.
4. PutBucketNotificationConfiguration with two Lambda configurations (and one with an invalid filter rule name), then GetBucketNotificationConfiguration.
