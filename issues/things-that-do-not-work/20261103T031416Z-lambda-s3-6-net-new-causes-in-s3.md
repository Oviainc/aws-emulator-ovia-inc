Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.layer-publish-rejected-for-a-bucket-in-another-region lambda__s3.layer-publish-rejected-for-a-missing-s3-key lambda__s3.layer-publish-rejected-for-an-s3-object-that-is-not-a-zip lambda__s3.layer-publish-rejected-for-an-unknown-s3-object-version lambda__s3.layer-published-from-s3-object-carries-its-sha256-and-size lambda__s3.layer-s3-object-version-pins-the-older-bytes lambda__s3.layer-from-s3-is-mounted-under-opt-for-a-function lambda__s3.layer-cli-latest-version-location-query-downloads-the-newest-archive lambda__s3.layer-publish-rejected-when-zipfile-and-s3-location-are-both-given lambda__s3.e2e-check-lambda-layers-s-code-lifecycle
Issue-Key: lambda__s3.layer-publish-rejected-for-a-bucket-in-another-region|_err#14|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda__s3.layer-publish-rejected-for-a-missing-s3-key|_error#9|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda__s3.layer-publish-rejected-for-an-s3-object-that-is-not-a-zip|_err#10|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda__s3.layer-publish-rejected-for-an-unknown-s3-object-version|_err#11|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda__s3.layer-published-from-s3-object-carries-its-sha256-and-size|run#9|KeyError|assert pub["Content"]["CodeSha256"] == want_sha, (
Issue-Key: lambda__s3.layer-s3-object-version-pins-the-older-bytes|run#11|KeyError|assert pinned["Content"]["CodeSha256"] == _b64_sha256(first), (
Issue-Key: lambda__s3.layer-from-s3-is-mounted-under-opt-for-a-function|run#9|AssertionError|assert layers == [{"Arn": pub["LayerVersionArn"], "CodeSize": len(archive)}], (
Issue-Key: lambda__s3.layer-cli-latest-version-location-query-downloads-the-newest-archive|_fetch#12|HTTPError|with urllib.request.urlopen(req, timeout=60, context=_ssl_context(ctx.http_verify)) as resp: # noqa: S310 - URL from the API's answer
Issue-Key: lambda__s3.layer-publish-rejected-when-zipfile-and-s3-location-are-both-given|run#13|AssertionError|assert error["Message"] == expected_message, (
Issue-Key: lambda__s3.e2e-check-lambda-layers-s-code-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# lambda__s3: 6 net-new causes in S3-sourced Lambda layers — PublishLayerVersion never reads the S3 object (4 refusals missing), Content omits CodeSha256, CodeSize reads 0, Content.Location 404s, a both-sources message diverges, and a CFN stack exports a layer name instead of its ARN

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishLayerVersion

## Description
Six distinct root causes under the suite heading `lambda__s3`, all in lambda's layer surface when the layer content comes from S3. A fixer must close all six before submitting. (The S3 bucket-notification causes in the same service are filed separately.)

Cause 1 of 6: PublishLayerVersion does not read the S3 object at publish time, so four distinct invalid S3 references are all accepted and mint a version.
AWS: Smithy: lambda `PublishLayerVersion` takes `Content: LayerVersionContentInput` and models `InvalidParameterValueException` (400); Lambda fetches and unpacks the object synchronously, so each bad reference is refused at publish and `ListLayerVersions` still shows only the control version. The four refusals, all `InvalidParameterValueException`, with these messages:
  - bucket in another Region: `Error occurred while GetObject. S3 Error Code: PermanentRedirect. S3 Error Message: The bucket is in this region: <bucket-region>. Please use this region to retry the request` — fixed text around `<bucket-region>`, which comes from the bucket's own location.
  - key absent from an existing bucket: `Error occurred while GetObject. S3 Error Code: NoSuchKey. S3 Error Message: The specified key does not exist.` (fixed text).
  - object that exists but is not a zip: `Could not unzip uploaded file. Please check your file, then try to upload again.` (fixed text).
  - `S3ObjectVersion` the object does not have: `Error occurred while GetObjectVersion. S3 Error Code: NoSuchVersion. S3 Error Message: The specified version does not exist.` (fixed text).
Emulator: all four publishes SUCCEED (`AssertionError: the call succeeded; real AWS rejects it, so there is no error code`), minting versions AWS would not create.
Why: the S3 reference is stored as an opaque string instead of being resolved, so a client's typo'd key, wrong Region, non-zip object or stale version id produces a layer version that exists but has no usable content — and the failure never surfaces at the call AWS refuses.
Fix: the lambda PublishLayerVersion handler's content resolution — fetch (and unzip) the S3 object, with the per-case InvalidParameterValueException messages above, minting no version on refusal.
Traces: lambda__s3.layer-publish-rejected-for-a-bucket-in-another-region, lambda__s3.layer-publish-rejected-for-a-missing-s3-key, lambda__s3.layer-publish-rejected-for-an-s3-object-that-is-not-a-zip, lambda__s3.layer-publish-rejected-for-an-unknown-s3-object-version

Cause 2 of 6: an S3-sourced layer version's `Content` omits `CodeSha256`.
AWS: `PublishLayerVersion`/`GetLayerVersion` return `Content` (LayerVersionContentOutput) carrying `CodeSha256` — the base64 SHA-256 of the archive bytes actually stored — and `CodeSize`. When `S3ObjectVersion` pins an older object version, CodeSha256 is the hash of THOSE bytes.
Emulator: `Content` has no `CodeSha256` key at all — reading it raises `KeyError: 'CodeSha256'` (two independent checks).
Why: CodeSha256 is how a client verifies which bytes a layer version carries and whether an object-version pin took effect; with the field absent that verification is impossible and typed consumers break.
Fix: the lambda layer state/serializer — compute and store CodeSha256 from the resolved content and emit it in `Content` on publish and read. Related open issues `20261025T022656Z-lambda-s3-layer-content-from-s3-loses-codesha256.md` and `20261027T052624Z-lambda-s3-an-s3-sourced-layer-version-omits.md` describe this field loss but do not carry these two replay keys.
Traces: lambda__s3.layer-published-from-s3-object-carries-its-sha256-and-size, lambda__s3.layer-s3-object-version-pins-the-older-bytes

Cause 3 of 6: a layer attached to a function reports `CodeSize: 0`.
AWS: `GetFunctionConfiguration().Layers[]` reports each layer version's `Arn` and its `CodeSize` — the size in bytes of the layer archive.
Emulator: the function lists the right ARN with `'CodeSize': 0` where the archive is non-empty.
Why: a client sizing its deployment against the 250 MB unzipped/50 MB packaging limits computes 0 for every layer, so quota and drift checks are wrong.
Fix: the lambda layer state (record the resolved archive size) and the GetFunctionConfiguration/Layers projection that reports it.
Traces: lambda__s3.layer-from-s3-is-mounted-under-opt-for-a-function

Cause 4 of 6: `GetLayerVersion().Content.Location` is not served — fetching it answers 404.
AWS: Smithy: GetLayerVersion returns `Content` with a `Location` presigned URL; an HTTP GET of it returns 200 and the layer archive bytes (the newest version's archive when the latest version is queried).
Emulator: the GET answers `urllib.error.HTTPError: HTTP Error 404: Not Found`.
Why: a client downloading a layer archive — the documented way to recover layer content — gets a 404 from a URL the API just handed it.
Fix: the lambda layer-content download route that `Content.Location` points at.
Traces: lambda__s3.layer-cli-latest-version-location-query-downloads-the-newest-archive

Cause 5 of 6: the "ZipFile and S3 location both given" refusal is worded differently from AWS.
AWS: `Content` takes EITHER an inline `ZipFile` OR an S3 location; supplying both is refused with `InvalidParameterValueException`, message `Please do not provide other FunctionCode parameters when providing a ZipFile.` (fixed text), minting no version.
Emulator: correct code, wrong message — `Content must name either an Amazon S3 location or an inline ZipFile, not both`.
Why: the message is a requirement; a client matching AWS's wording does not recognise the refusal.
Fix: the PublishLayerVersion input-validation message for the mutually exclusive content members.
Traces: lambda__s3.layer-publish-rejected-when-zipfile-and-s3-location-are-both-given

Cause 6 of 6: a CloudFormation stack exports the layer's physical/logical resource name where the LayerVersion ARN is required.
AWS: `Ref` on an `AWS::Lambda::LayerVersion` resolves to the layer VERSION ARN (`arn:aws:lambda:<region>:<account>:layer:<name>:<version>`), so a stack Output that references it exports that ARN.
Emulator: the Output's value is `<stack-name>-S3Layer-<suffix>` — a resource name, not an ARN. Replay detail: `the stack must export the version ARN of layer '<layer>', version 1; it exported '<stack>-S3Layer-<suffix>'`.
Why: a consumer that passes the exported value to `Lambda.UpdateFunctionConfiguration(Layers=[...])` or `GetLayerVersionByArn` fails, because what it receives is not an ARN at all.
Fix: cloudformation's `AWS::Lambda::LayerVersion` provisioner — set the resource's physical id / Ref value to the LayerVersionArn Lambda returned.
Traces: lambda__s3.e2e-check-lambda-layers-s-code-lifecycle

## Evidence
Cause 1 — expected ('InvalidParameterValueException', <the per-case message above>) with `ListLayerVersions` still reporting only [1]; actual: each publish returned success and minted a version (`AssertionError: the call succeeded; real AWS rejects it, so there is no error code`).
Cause 2 — expected `Content['CodeSha256']` to equal the base64 SHA-256 of the stored archive; actual `KeyError: 'CodeSha256'` — the key is absent.
Cause 3 — expected `Layers == [{'Arn': <layer-version-arn>, 'CodeSize': <archive length>}]`; actual `[{'Arn': '<layer-version-arn>', 'CodeSize': 0}]`.
Cause 4 — expected HTTP 200 and the archive bytes from `GetLayerVersion().Content.Location`; actual HTTP 404 Not Found.
Cause 5 — expected message `Please do not provide other FunctionCode parameters when providing a ZipFile.`; actual `Content must name either an Amazon S3 location or an inline ZipFile, not both`.
Cause 6 — expected the stack Output to carry `arn:aws:lambda:<region>:<account>:layer:<layer-name>:1`; actual `'<stack-name>-S3Layer-<suffix>'`.

## Steps to Reproduce
1. Replay the behavioral suite and re-run every check id in trace_ids.
2. Cause 1: publish a control layer from a valid S3 zip, then PublishLayerVersion with (a) a bucket in another Region, (b) a key that does not exist, (c) an object that is plain text, (d) an `S3ObjectVersion` that was permanently deleted; check the code/message and `ListLayerVersions`.
3. Cause 2: PublishLayerVersion from an S3 zip and read `Content['CodeSha256']`; repeat with `S3ObjectVersion` pinning an older object version.
4. Cause 3: attach that layer to a function and read `GetFunctionConfiguration().Layers`.
5. Cause 4: GetLayerVersion for the newest version and HTTP GET `Content.Location`.
6. Cause 5: PublishLayerVersion with `Content={'ZipFile': <bytes>, 'S3Bucket': b, 'S3Key': k}`.
7. Cause 6: deploy a stack declaring `AWS::Lambda::LayerVersion` with an Output that `Ref`s it, and read the Output value.
