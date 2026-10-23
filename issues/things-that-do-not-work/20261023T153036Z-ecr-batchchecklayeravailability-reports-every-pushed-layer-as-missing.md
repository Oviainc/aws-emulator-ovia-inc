Issue-Source: trace-sweep-divergence
Issue-Trace: api-image-push-via-layer-upload
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-image-push-via-layer-upload ecr-batch-check-layer-availability-mixed
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|BatchCheckLayerAvailability|2xx->2xx|$.failures[*]
Issue-Key: ecr|BatchCheckLayerAvailability|2xx->2xx|$.layers[*]
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# ECR BatchCheckLayerAvailability reports every pushed layer as missing

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchCheckLayerAvailability

## Description
This issue bundles 2 DISTINCT root causes on ecr#BatchCheckLayerAvailability. Both must be closed before submitting; each carries its own fixture and step.

Cause 1 of 2: a layer that was successfully uploaded and committed is reported as a MissingLayerDigest failure instead of an AVAILABLE layer.
AWS: Smithy ecr#BatchCheckLayerAvailability returns layers (list of ecr#Layer: layerDigest, layerAvailability enum AVAILABLE|UNAVAILABLE|ARCHIVED, layerSize long, mediaType) and failures (list of ecr#LayerFailure). After the documented upload sequence (InitiateLayerUpload, UploadLayerPart, CompleteLayerUpload) for a digest, checking that digest yields an entry in layers with layerAvailability=AVAILABLE and layerSize equal to the uploaded byte count, and an EMPTY failures list.
Emulator: returns layers empty and puts the digest in failures with failureCode MissingLayerDigest, i.e. the completed upload left nothing in the repository's layer store that this operation can see.
Why: BatchCheckLayerAvailability is how every push client (docker/containerd/skopeo and the SDKs) decides whether it must upload a blob. Being told a just-uploaded layer is missing makes the push loop re-upload forever, and it is also the signal that explains Cause 1 of the companion PutImage issue (manifests whose layers appear absent). Note the related open issue 20261023T113357Z-ecr-completelayerupload-is-an-unimplemented-scaffold-stub-so.md: whatever stands in for CompleteLayerUpload during this replay returns success without registering the layer, and that issue does not carry these fixture steps.
Fix: the layer-upload state (CompleteLayerUpload's commit path) must register the assembled blob under its digest with its byte size, and the BatchCheckLayerAvailability handler must read it, so a committed digest resolves to an AVAILABLE Layer.
Traces: api-image-push-via-layer-upload#task#5.

Cause 2 of 2: an unknown digest is reported as a failures entry instead of an UNAVAILABLE layer, so the mixed known/unknown batch is wrong in both halves.
AWS: for a batch mixing one uploaded digest and one digest that was never uploaded, AWS answers HTTP 200 with BOTH digests in layers — the uploaded one as layerAvailability=AVAILABLE with its layerSize, the unknown one as layerAvailability=UNAVAILABLE with no layerSize — and failures empty. The UNAVAILABLE value exists in the ecr#LayerAvailability enum precisely for this case.
Emulator: puts both digests in failures with failureCode MissingLayerDigest and returns layers empty, so the two cases are indistinguishable and the UNAVAILABLE branch is unreachable.
Why: clients branch on layerAvailability per digest to decide which blobs to upload; a failures entry is handled as an error for the whole digest rather than as "not present, please upload", which turns a normal cache-miss into a push failure.
Fix: in the BatchCheckLayerAvailability handler, map a digest that is well-formed but absent to a Layer with layerAvailability=UNAVAILABLE and reserve LayerFailure/MissingLayerDigest for digests that are not usable at all.
Traces: ecr-batch-check-layer-availability-mixed#task#5.

## Evidence
Cause 1 — fixture api-image-push-via-layer-upload, step task#5: expected $.layers[0] = {'layerAvailability': 'AVAILABLE', 'layerDigest': 'sha256:ebad0ed4...257c2', 'layerSize': 256} and no failures; got $.layers empty and $.failures[0] = {'layerDigest': 'sha256:ebad0ed4...257c2', 'failureCode': 'MissingLayerDigest', 'failureReason': "The layer with digest '...' does not exist within the repository with name '<repo>' in the registry with id '<account>'"}. Status 200 on both sides.
Cause 2 — fixture ecr-batch-check-layer-availability-mixed, step task#5: expected $.layers[0] = {'layerAvailability': 'AVAILABLE', 'layerDigest': 'sha256:90b95bae...4fa4', 'layerSize': 19} and $.layers[1] = {'layerAvailability': 'UNAVAILABLE', 'layerDigest': 'sha256:ffff...ffff'} with failures empty; got $.layers empty and two $.failures entries, both failureCode MissingLayerDigest. Status 200 on both sides.

## Steps to Reproduce
Replay the wire corpus and re-run api-image-push-via-layer-upload#task#5 and ecr-batch-check-layer-availability-mixed#task#5. Manually:
1. CreateRepository R. InitiateLayerUpload(R) -> UploadLayerPart with the blob bytes -> CompleteLayerUpload(R, layerDigests=[D]).
2. BatchCheckLayerAvailability(repositoryName=R, layerDigests=[D]) -> expect layers[0].layerAvailability == 'AVAILABLE' with layerSize == len(blob), failures == [].
3. BatchCheckLayerAvailability(repositoryName=R, layerDigests=[D, 'sha256:' + 'f'*64]) -> expect two layers entries, AVAILABLE and UNAVAILABLE, failures == [].
