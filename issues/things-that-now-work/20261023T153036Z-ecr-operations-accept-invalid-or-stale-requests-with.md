Issue-Source: trace-sweep-divergence
Issue-Trace: api-image-push-via-layer-upload
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-image-push-via-layer-upload ecr-put-image-missing-layers-error ecr-put-image-missing-layers-error-then-recover api-immutable-tag-conflict-and-image-deletion api-pull-through-cache-rule-lifecycle api-replication-configuration-and-registry-state
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|PutImage|4xx->2xx|$.__type
Issue-Key: ecr|PutImage|4xx->2xx|$.message
Issue-Key: ecr|PutImage|4xx->2xx|status
Issue-Key: ecr|DescribeImages|4xx->2xx|$.__type
Issue-Key: ecr|DescribeImages|4xx->2xx|$.message
Issue-Key: ecr|DescribeImages|4xx->2xx|status
Issue-Key: ecr|UpdatePullThroughCacheRule|4xx->2xx|$.__type
Issue-Key: ecr|UpdatePullThroughCacheRule|4xx->2xx|$.message
Issue-Key: ecr|UpdatePullThroughCacheRule|4xx->2xx|status
Issue-Key: ecr|PutReplicationConfiguration|4xx->2xx|$.__type
Issue-Key: ecr|PutReplicationConfiguration|4xx->2xx|$.message
Issue-Key: ecr|PutReplicationConfiguration|4xx->2xx|status
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# ECR operations accept invalid or stale requests with 200 instead of the modeled 400

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutImage

## Description
This issue bundles 4 DISTINCT root causes in the ecr wire replay, all of the same shape: a request AWS rejects with a modeled 400 client error is answered HTTP 200 by the emulator, so the rejection is unreachable and the emulator's state moves on as if the call were legal. All four must be closed before submitting; each carries its own fixture and step.

Cause 1 of 4: PutImage accepts a manifest whose referenced layers were never uploaded.
AWS: Smithy ecr#PutImage models LayersNotFoundException (client error, HTTP 400, httpError defaulted) among its errors. Putting a manifest that references a layer digest which is not present in the repository is refused with status 400 and error code LayersNotFoundException; the recorded message is built from the request and names the missing digests, the repository name and the registry id (example: "Layers with digests '[sha256:4b9f...ae5]' required for pushing image into repository with name '<repo>' in the registry with id '<account>' do not exist").
Emulator: returns HTTP 200 with an image payload and no __type/message at all, i.e. the manifest is stored and tagged even though its layers do not exist.
Why: a client that pushes a manifest before (or instead of) uploading its layers is told the push succeeded. The repository then advertises an image that cannot be pulled, and the documented push ordering contract (upload layers, then PutImage) is never enforced, so broken images accumulate silently.
Fix: the PutImage handler must resolve every layer digest referenced by imageManifest against the repository's layer state before committing, and raise LayersNotFoundException (400) listing the unresolved digests.
Traces: api-image-push-via-layer-upload#task#12 (representative); also ecr-put-image-missing-layers-error#task#2 and ecr-put-image-missing-layers-error-then-recover#task#2 in the same cluster.

Cause 2 of 4: DescribeImages answers 200 for an image identifier that no longer exists.
AWS: Smithy ecr#DescribeImages models ImageNotFoundException (client, HTTP 400). After the image has been deleted, DescribeImages naming that imageDigest in imageIds is refused with 400 / ImageNotFoundException; the recorded message is built from the request and echoes the imageId, the repository name and the registry id (example: "The image with imageId {imageDigest:'sha256:257e...ccb', imageTag:'null'} does not exist within the repository with name '<repo>' in the registry with id '<account>'").
Emulator: returns HTTP 200 with no __type and no message for the same request after the delete, so a deleted image is indistinguishable from a present one on this call.
Why: callers use an explicit-imageIds DescribeImages as an existence probe (delete-verification, garbage collection, drift detection). A 200 tells them the image is still there, or leaves them parsing an empty body with no error to branch on, so deletions can never be confirmed.
Fix: the DescribeImages handler must reject imageIds that resolve to nothing in repository image state with ImageNotFoundException (400) rather than returning a success envelope.
Traces: api-immutable-tag-conflict-and-image-deletion#task#13.

Cause 3 of 4: UpdatePullThroughCacheRule accepts a credential update against a rule that has no authentication.
AWS: Smithy ecr#UpdatePullThroughCacheRule models InvalidParameterException (client, HTTP 400) and takes credentialArn/customRoleArn as optional members. Updating credentials on a rule that was created unauthenticated is refused with 400 / InvalidParameterException; the recorded message is the fixed-template rejection "Invalid input: Please create an authenticated pull through cache rule to update the credentials".
Emulator: returns HTTP 200 with no __type/message and stores the credential on the rule.
Why: the authenticated and unauthenticated rule kinds are distinct in AWS, and a client that believes it attached a secret to an unauthenticated rule has a configuration AWS would never have accepted; the misconfiguration surfaces only much later at pull time, if at all.
Fix: the UpdatePullThroughCacheRule handler's validation must check the stored rule's authentication kind before applying credentialArn/customRoleArn and raise InvalidParameterException. Related but not covering: open issue 20261022T234922Z-ecr-completelayerupload-is-a-501-stub-cfn-provisioned.md, Cause 5, which is about credentialArn/customRoleArn format and secret existence on this operation, not about the unauthenticated-rule precondition and does not carry this fixture step.
Traces: api-pull-through-cache-rule-lifecycle#task#5.

Cause 4 of 4: PutReplicationConfiguration accepts a destination region that does not exist.
AWS: Smithy ecr#PutReplicationConfiguration models InvalidParameterException (client, HTTP 400) and ValidationException; the replicationConfiguration member is required. A configuration whose destination region is not a real region is refused with 400 / InvalidParameterException, the message following AWS's parameter-and-constraint template and naming the parameter and the offending region (example: "Invalid parameter at 'replicationConfiguration' failed to satisfy constraint: 'Invalid region 'not-a-real-region-xyz''").
Emulator: returns HTTP 200 with no __type/message and installs the configuration.
Why: replication destinations are validated at write time by AWS precisely because a bad destination silently breaks replication afterwards. A client gets a registry whose replication configuration can never work, and the validation error it tests for is unreachable.
Fix: validate each destination region (and registry id) in the PutReplicationConfiguration handler's validation before storing, raising InvalidParameterException with the parameter-and-constraint message.
Traces: api-replication-configuration-and-registry-state#task#5.

## Evidence
Cause 1 — PutImage: expected status 400 with $.__type = 'LayersNotFoundException' and a message naming the missing layer digests; got status 200 with $.__type and $.message absent (fixture api-image-push-via-layer-upload, step task#12; same cluster: ecr-put-image-missing-layers-error#task#2, ecr-put-image-missing-layers-error-then-recover#task#2).
Cause 2 — DescribeImages: expected status 400 with $.__type = 'ImageNotFoundException' and a message echoing the imageId; got status 200, $.__type and $.message missing (fixture api-immutable-tag-conflict-and-image-deletion, step task#13).
Cause 3 — UpdatePullThroughCacheRule: expected status 400 with $.__type = 'InvalidParameterException' and message 'Invalid input: Please create an authenticated pull through cache rule to update the credentials'; got status 200, $.__type and $.message missing (fixture api-pull-through-cache-rule-lifecycle, step task#5).
Cause 4 — PutReplicationConfiguration: expected status 400 with $.__type = 'InvalidParameterException' and a message of the form "Invalid parameter at 'replicationConfiguration' failed to satisfy constraint: 'Invalid region '<region>''"; got status 200, $.__type and $.message missing (fixture api-replication-configuration-and-registry-state, step task#5).
In all four the recorded expectation is a 400 with a modeled error code and the emulator produced a 200 success envelope.

## Steps to Reproduce
Replay the wire corpus (replay_wire_corpus) and re-run the fixture steps listed per cause. Manually:
1. CreateRepository R; PutImage(repositoryName=R, imageManifest=<manifest referencing a layer digest never uploaded>, imageTag='v1') -> inspect status and __type.
2. CreateRepository R, push image I, BatchDeleteImage it, then DescribeImages(repositoryName=R, imageIds=[{imageDigest: I}]) -> inspect status and __type.
3. CreatePullThroughCacheRule(ecrRepositoryPrefix=P, upstreamRegistryUrl='public.ecr.aws') with no credentialArn, then UpdatePullThroughCacheRule(ecrRepositoryPrefix=P, credentialArn=<an ecr-pullthroughcache secret ARN>) -> inspect status and __type.
4. PutReplicationConfiguration(replicationConfiguration={rules:[{destinations:[{region:'not-a-real-region-xyz', registryId:'000000000000'}]}]}) -> inspect status and __type.
