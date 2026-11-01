Issue-Source: trace-sweep-divergence
Issue-Trace: lambdamicrovms-customer-image-lifecycle
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdamicrovms-customer-image-lifecycle lambdamicrovms-image-version-status-and-update
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|CreateMicrovmImage|2xx->2xx|$.baseImageVersion
Issue-Key: lambda-microvms|CreateMicrovmImage|2xx->2xx|$.id
Issue-Key: lambda-microvms|UpdateMicrovmImage|2xx->2xx|$.id
Issue-Key: lambda-microvms|UpdateMicrovmImage|2xx->2xx|$.imageVersion
Issue-Key: lambda-microvms|UpdateMicrovmImage|2xx->2xx|$.latestFailedImageVersion
Issue-Key: lambda-microvms|ListMicrovmImageVersions|2xx->2xx|$.items[*]
Issue-Commit: 3cbee5fc05633571f18b42c799709a3e75b70289

# lambda-microvms image responses omit the recorded id member and UpdateMicrovmImage mis-numbers the new version

> _Note: this issue was identified against the code base at commit `3cbee5fc05633571f18b42c799709a3e75b70289`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrovmImage

## Description
This issue bundles 4 distinct root causes in the lambda-microvms emulator around image IDENTITY and VERSION NUMBERING on the create/update path, all observed on replay of the wire corpus. A fixer must close all four (Cause 1 of 4 .. Cause 4 of 4) before submitting.

=== Cause 1 of 4: CreateMicrovmImage omits the id member and ignores the requested baseImageVersion ===
Fixture lambdamicrovms-customer-image-lifecycle, step setup#2.
AWS (id): recorded behavior only - CreateMicrovmImageResponse in the Smithy model does not declare an 'id' member, yet every recorded create/read response carries one: a lowercase 8-4-4-4-12 UUID that is the image's own identity and is byte-identical in the Create response, in GetMicrovmImage and in every ListMicrovmImages row for that image, and stable across UpdateMicrovmImage. The requirement is the member's PRESENCE, its UUID shape and its stability for the life of the image; the specific UUID in any one recording is a per-run value and is not a requirement.
AWS (baseImageVersion): Smithy CreateMicrovmImageRequest$baseImageVersion (target Version, @length 1..2048, @pattern ^[^\s]+$) is the caller-chosen version of the base image, and CreateMicrovmImageResponse$baseImageVersion echoes back the value the request carried. A request that selects a non-default base version is recorded as echoing that selection.
Emulator: no 'id' member at all in the 201 body, and baseImageVersion comes back as the base image's default version instead of the version the request selected (request asked for one version, the response reports a different one).
Why: a client that stores the id as its handle on the image stores null and can never correlate the create response with later reads; a client that pins a base-image version and verifies the echo concludes its pin was applied when the emulator silently substituted another base version, so the image it built is not the image it asked for.
Fix: CreateMicrovmImage's response serializer must emit the image's id, and its handler must carry the request's baseImageVersion into the stored image/version record rather than defaulting it.

=== Cause 2 of 4: the omitted id is systemic, not specific to one fixture ===
Fixture lambdamicrovms-image-version-status-and-update, step setup#2 (CreateMicrovmImage).
AWS: as Cause 1 - the 201 body carries the image's UUID id.
Emulator: the member is absent here too, on a different fixture and a different request shape, which shows the defect is in the shared image-response projection and not in one code path.
Why: same as Cause 1 - the client's primary handle on the resource is missing.
Fix: the single image-to-response projection used by the create/read/update handlers (not a per-operation patch) must include id.

=== Cause 3 of 4: UpdateMicrovmImage mints a point release instead of the next major version, omits id, and does not report the failed version ===
Fixture lambdamicrovms-image-version-status-and-update, step task#5.
AWS: UpdateMicrovmImage is documented as 'Updates the configuration of a MicroVM image and triggers a new version build'; UpdateMicrovmImageResponse$imageVersion is @required. Recorded behavior: the new version an update triggers is the NEXT MAJOR version of the image in '<n>.0' form - an image whose first version is '1.0' gets '2.0' on its first update, not a '1.1' point release. The response also carries the image's id (recorded-only member, see Cause 1) and latestFailedImageVersion when a previous version failed.
Emulator: imageVersion is a point release of the current version instead of the next major version, id is absent, and latestFailedImageVersion is absent.
Why: the version string is the key every downstream call uses - GetMicrovmImageVersion, ListMicrovmImageBuilds, RunMicrovm's version selector. A client that reads the recorded numbering (or computes the expected next version itself) addresses a version the emulator does not have, and every version-scoped call then 404s. latestFailedImageVersion being absent is the same gap as Cause 1 of the companion issue 'lambda-microvms image build failure is never modeled' filed alongside this one.
Fix: the version-minting rule belongs in the service state's version allocator used by UpdateMicrovmImage (next major '<n>.0', not a point bump), plus the shared id projection from Cause 2.

=== Cause 4 of 4: ListMicrovmImageVersions does not show the version the update created ===
Fixture lambdamicrovms-image-version-status-and-update, step task#7.
AWS: ListMicrovmImageVersions returns one MicrovmImageVersionSummary per version of the image, newest first, so after one update the listing's first row is the update's new version carrying the UPDATED configuration from the update request (its description, its codeArtifact.uri) and the previous version follows.
Emulator: items[0] is still the original version - the original description and the original codeArtifact.uri, and an imageVersion that is the pre-update number - so the update's new version is either absent from the listing or ordered behind the original. (The state/status/stateReason values that also diverge on this step are the build-failure cause of the companion issue filed alongside this one; what is filed here is the version IDENTITY and ORDERING of the rows.)
Why: a client that updates an image and then lists its versions to pick the newest one picks the stale one, and deploys the configuration it just replaced.
Fix: UpdateMicrovmImage must append the new version record to the image's version collection (service state) and ListMicrovmImageVersions must order its rows newest-first so the row an update produced is the first one.

Traces: lambdamicrovms-customer-image-lifecycle#setup#2; lambdamicrovms-image-version-status-and-update#setup#2, #task#5, #task#7.

## Evidence
Wire replay of trace set simparity-wire-aws-v1. Expected = recorded AWS response, Actual = emulator.

Cause 1 (lambdamicrovms-customer-image-lifecycle#setup#2, CreateMicrovmImage, 201 vs 201):
  $.baseImageVersion: expected '2.0', got '1.0'
  $.id: expected a shaped UUID, got (absent)

Cause 2 (lambdamicrovms-image-version-status-and-update#setup#2, CreateMicrovmImage, 201 vs 201):
  $.id: expected a shaped UUID, got (absent)

Cause 3 (lambdamicrovms-image-version-status-and-update#task#5, UpdateMicrovmImage, 200 vs 200):
  $.imageVersion: expected '2.0', got '1.1'
  $.id: expected a shaped UUID, got (absent)
  $.latestFailedImageVersion: expected '1.0', got (absent)

Cause 4 (lambdamicrovms-image-version-status-and-update#task#7, ListMicrovmImageVersions, 200 vs 200):
  $.items[0].imageVersion: expected '2.0', got '1.0'
  $.items[0].description: expected the description the UPDATE request carried, got the description the CREATE request carried
  $.items[0].codeArtifact.uri: expected the uri the UPDATE request carried, got the uri the CREATE request carried
  (the recorded response has two rows, newest first; the emulator's first row is the pre-update version)

That the same $.id omission appears on three different operations across two fixtures (and on GetMicrovmImage/ListMicrovmImages in the lambda-token fixtures) shows a shared projection defect rather than per-operation serializer bugs.

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json): CreateMicrovmImageRequest$baseImageVersion and CreateMicrovmImageResponse$baseImageVersion (target Version); UpdateMicrovmImageResponse$imageVersion @required; UpdateMicrovmImage doc 'Updates the configuration of a MicroVM image and triggers a new version build'; MicrovmImageVersionSummary carries description and codeArtifact (@required). No 'id' member is declared on any image output shape - its presence is recorded behavior only.

## Steps to Reproduce
1. mvm = client('lambda-microvms'); base = ListManagedMicrovmImages()['items'][0]['imageArn'].
2. CreateMicrovmImage(name=<unique>, baseImageArn=base, baseImageVersion=<a non-default managed base version>, buildRoleArn=<role arn>, codeArtifact={'uri': 's3://<bucket>/code.zip'}, description='rev 1').
   AWS: 201 with an 'id' UUID and baseImageVersion equal to the value sent. Emulator: no 'id'; baseImageVersion is the base image's default version. (Cause 1)
3. Repeat the create for a second image with a different request shape - 'id' is absent again. (Cause 2)
4. UpdateMicrovmImage(imageIdentifier=<imageArn>, baseImageArn=base, buildRoleArn=<role arn>, codeArtifact={'uri': 's3://<bucket>/code2.zip'}, description='rev 2').
   AWS: 200 with imageVersion='2.0' (next major), an 'id' UUID identical to the create response's, and latestFailedImageVersion when an earlier version failed. Emulator: imageVersion='1.1', no 'id', no latestFailedImageVersion. (Cause 3)
5. ListMicrovmImageVersions(imageIdentifier=<imageArn>).
   AWS: items[0] is the new version - imageVersion='2.0', description='rev 2', codeArtifact.uri ending code2.zip - with the earlier version after it. Emulator: items[0] is the pre-update version with description='rev 1' and code.zip. (Cause 4)
