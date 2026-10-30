Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-microvms-image-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-microvms-image-lifecycle lambda-microvms-managed-images-catalog
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda|CreateMicrovmImage|2xx->2xx|$.id
Issue-Key: lambda|GetMicrovmImage|2xx->2xx|$.id
Issue-Key: lambda|ListMicrovmImages|2xx->2xx|$.items[name="sim2real-probe-lambda-microvms-microvmimg-9a120e37"].id
Issue-Key: lambda|ListManagedMicrovmImageVersions|2xx->2xx|$.items[*]
Issue-Commit: 3cbee5fc05633571f18b42c799709a3e75b70289

# lambda MicroVM image responses omit the recorded id member and the managed version catalog is synthesized

> _Note: this issue was identified against the code base at commit `3cbee5fc05633571f18b42c799709a3e75b70289`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMicrovmImage

## Description
This issue bundles 4 distinct root causes observed on replay of the wire corpus against the lambda service token, all in the SUCCESSFUL-response content of the MicroVM image API. A fixer must close all four (Cause 1 of 4 .. Cause 4 of 4) before submitting.

=== Cause 1 of 4: CreateMicrovmImage's 201 body omits the image's id ===
Fixture lambda-microvms-image-lifecycle, step task#2.
AWS: recorded behavior only - the Smithy CreateMicrovmImageResponse declares no 'id' member, yet every recorded create response carries one: a lowercase 8-4-4-4-12 UUID that is the image's own identity. The requirement is the member's PRESENCE and its UUID shape; the particular UUID a recording shows is a per-run value and is not a requirement.
Emulator: the 201 body has no 'id' member at all.
Why: the id is the handle a client stores for the image it just created; a client that keys its own records on it stores null and cannot correlate the create with any later read.
Fix: CreateMicrovmImage's response projection must emit the stored image's id.

=== Cause 2 of 4: GetMicrovmImage omits the same id ===
Fixture lambda-microvms-image-lifecycle, step task#3.
AWS: the read answers the SAME id the create response returned for that image - byte-identical across the image's lifetime, which is what makes it usable as an identity.
Emulator: absent.
Why: a client that fetched the image to confirm the id it stored gets nothing to compare, so the identity cannot be validated at all.
Fix: the shared image-to-response projection the read handler uses must include id (one change for Causes 1-3, not three).

=== Cause 3 of 4: ListMicrovmImages rows omit the id ===
Fixture lambda-microvms-image-lifecycle, step task#4.
AWS: each listing row for the image carries the same id as the create and the get, so a client can join a listing back to resources it created.
Emulator: the row for the image (selected by name) has no id member.
Why: a reconciliation sweep that enumerates images and matches them to its own records by id matches nothing.
Fix: as Cause 2 - the shared projection, applied to the list row.

=== Cause 4 of 4: ListManagedMicrovmImageVersions publishes one invented version instead of the catalog's version set, and omits supportedCpuConfigurations ===
Fixture lambda-microvms-managed-images-catalog, step task#2.
AWS: ListManagedMicrovmImageVersions returns ManagedMicrovmImageVersion items - imageArn (@required), imageVersion (@required), status (AVAILABLE|DEPRECATED), createdAt (@required), updatedAt. Recorded behavior for an AWS-published base image: MULTIPLE versions are published and a managed base-image version id is a plain integer generation counter ('1', '0'), not the '<major>.<minor>' form the service mints for customer images; each item also carries supportedCpuConfigurations, a list of {architecture} entries (recorded-only member, not in the model) telling the caller which architectures the base image supports.
Emulator: one item only, imageVersion='1.0' in the customer-image format, and no supportedCpuConfigurations; the older published generation is missing.
Why: a client pinning a base-image generation for reproducible builds pins an id that does not exist in AWS, and cannot select a base image by architecture at all.
Fix: the managed-image catalog seed in the service state must publish each managed image's full version set with integer generation ids and per-version supportedCpuConfigurations, and the serializer must emit that member. The same divergence reproduces under the lambda-microvms service token and is filed there as well, so the fix belongs in the one shared catalog table rather than per routing token.

Traces: lambda-microvms-image-lifecycle#task#2, #task#3, #task#4; lambda-microvms-managed-images-catalog#task#2.

## Evidence
Wire replay of trace set simparity-wire-aws-v1. Expected = recorded AWS response, Actual = emulator.

Cause 1 (lambda-microvms-image-lifecycle#task#2, CreateMicrovmImage, 201 vs 201):
  $.id: expected a UUID, got (absent)

Cause 2 (lambda-microvms-image-lifecycle#task#3, GetMicrovmImage, 200 vs 200):
  $.id: expected the SAME UUID the create returned, got (absent)

Cause 3 (lambda-microvms-image-lifecycle#task#4, ListMicrovmImages, 200 vs 200):
  $.items[name="<the created image name>"].id: expected the same UUID, got (absent)
  (the three steps of this fixture assert one and the same id value, which is the stability requirement; the literal value is a per-run artifact)

Cause 4 (lambda-microvms-managed-images-catalog#task#2, ListManagedMicrovmImageVersions, 200 vs 200):
  $.items[0]: expected {imageArn: <aws-owned managed arn>, imageVersion: '1', status: 'AVAILABLE', supportedCpuConfigurations: [{architecture: 'ARM_64'}], createdAt: <shaped>, updatedAt: <shaped>}
              got      {imageArn: <same arn>, imageVersion: '1.0', status: 'AVAILABLE', createdAt: <request time>, updatedAt: <request time>} - no supportedCpuConfigurations
  $.items[1]: expected the entry at imageVersion '0', got (absent)

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json): CreateMicrovmImageResponse / GetMicrovmImageOutput / MicrovmImageSummary declare imageArn, name, state, latestActiveImageVersion, latestFailedImageVersion, createdAt (and updatedAt / config members) - no 'id', so id is recorded behavior only; ManagedMicrovmImageVersion{imageArn @required, imageVersion @required, status, createdAt @required, updatedAt} with no supportedCpuConfigurations member.

## Steps to Reproduce
1. Create an image: CreateMicrovmImage(name=<unique>, baseImageArn=<a managed base image arn>, buildRoleArn=<role arn>, codeArtifact={'uri': 's3://<bucket>/code.zip'}).
   AWS: 201 body carries 'id' = a UUID. Emulator: no 'id'. (Cause 1)
2. GetMicrovmImage(imageIdentifier=<imageArn from step 1>).
   AWS: 'id' equals the UUID from step 1. Emulator: no 'id'. (Cause 2)
3. ListMicrovmImages() and select the row whose name is the name from step 1.
   AWS: the row's 'id' equals the same UUID. Emulator: no 'id'. (Cause 3)
4. arn = ListManagedMicrovmImages()['items'][0]['imageArn']; ListManagedMicrovmImageVersions(imageIdentifier=arn).
   AWS: several items with integer generation ids ('1', '0'), each with supportedCpuConfigurations=[{'architecture': 'ARM_64'}]. Emulator: one item, imageVersion='1.0', no supportedCpuConfigurations. (Cause 4)
