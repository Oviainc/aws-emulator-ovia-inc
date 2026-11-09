Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.layer-location-downloads-the-published-archive lambda__s3.layer-cli-latest-version-location-query-downloads-the-newest-archive lambda__s3.layer-content-survives-deletion-of-its-source-object lambda__s3.nodejs22-global-fetch-reads-presigned-object lambda__s3.nodejs22-global-fetch-tampered-presigned-url-rejected
Issue-Key: lambda__s3.layer-location-downloads-the-published-archive|_fetch#9|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: lambda__s3.layer-cli-latest-version-location-query-downloads-the-newest-archive|_fetch#12|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: lambda__s3.layer-content-survives-deletion-of-its-source-object|_fetch#15|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: lambda__s3.nodejs22-global-fetch-reads-presigned-object|run#11|AssertionError|assert got.get("fetchType") == "function", (
Issue-Key: lambda__s3.nodejs22-global-fetch-tampered-presigned-url-rejected|run#12|AssertionError|assert got.get("fetchType") == "function", (
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# lambda__s3: 2 net-new causes — GetLayerVersion publishes a Content.Location on awslambda-<region>-layers.s3.amazonaws.com that the emulator does not serve, and the nodejs22.x runtime has no global fetch

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLayerVersion

## Description
Two causes.

Cause 1 of 2: the layer-archive download URL is minted on an AWS-owned host instead of the emulator's own host.
AWS: GetLayerVersion/GetLayerVersionByArn return Content.Location, a pre-signed URL the caller downloads the layer archive from; fetching it returns the exact bytes that were published (and the latest version's Location returns the newest archive). Smithy: lambda GetLayerVersion output Content is LayerVersionContentOutput{Location, CodeSha256, CodeSize}. The contract is that Location is fetchable by the caller.
Emulator: Location is published as 'https://awslambda-us-west-2-layers.s3.us-west-2.amazonaws.com/snapshots/<account>/<layer>-<version>' — an address the emulator does not answer on: 'its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener'. The AWS-owned host and the /snapshots/<account>/<layer>-<version> shape are the fixed part; account, layer name, version and port vary per run.
Why: no caller can download a layer — not the CLI, not a build pipeline that re-packages a layer, not the check that the content survives deletion of its S3 source object. The control plane reports a Location that is unusable.
Fix: the Lambda layer-version serializer/state must publish a Content.Location served by the emulator (its own host or a per-resource listener) and serve the stored archive bytes from it.
Traces: lambda__s3.layer-location-downloads-the-published-archive, lambda__s3.layer-cli-latest-version-location-query-downloads-the-newest-archive, lambda__s3.layer-content-survives-deletion-of-its-source-object.

Cause 2 of 2: the nodejs22.x managed runtime does not provide the global fetch function.
AWS: Node.js 18+ managed runtimes expose the WHATWG fetch API as a global, so in a nodejs22.x handler `typeof fetch === 'function'` and the handler can fetch a presigned S3 URL (and must see the tampered URL refused by S3, not by a missing API). Recorded behavior: the check reads typeof fetch inside the handler.
Emulator: the handler reports typeof fetch === 'undefined'.
Why: any nodejs18/20/22 handler written against the documented runtime — the now-standard way to make HTTP calls without bundling a client — throws ReferenceError in the emulator, and the S3-side verdicts (presigned GET serves the object; a tampered signature is refused) can never be reached.
Fix: the Lambda nodejs22.x runtime's global environment must expose fetch (and the Request/Response/Headers globals that come with it).
Traces: lambda__s3.nodejs22-global-fetch-reads-presigned-object, lambda__s3.nodejs22-global-fetch-tampered-presigned-url-rejected.

## Evidence
Cause 1 (all three checks): simparity.published.PublishedUrlRejected: endpoint_url was given https://awslambda-us-west-2-layers.s3.us-west-2.amazonaws.com/snapshots/000000000000/sim2real-scn-lyr-<id>-<n> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener.
Cause 2 (both checks): AssertionError: typeof fetch in the nodejs22.x handler is 'undefined', expected 'function'.

## Steps to Reproduce
Cause 1: publish_layer_version(Content={'S3Bucket':b,'S3Key':k}); get_layer_version(LayerName=l, VersionNumber=n); fetch Content.Location -> the host is not one the emulator serves.
Cause 2: create_function(Runtime='nodejs22.x', Handler=...) whose handler returns {fetchType: typeof fetch}; invoke it -> 'undefined'.
