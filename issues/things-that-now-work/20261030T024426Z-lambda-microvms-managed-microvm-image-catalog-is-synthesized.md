Issue-Source: trace-sweep-divergence
Issue-Trace: lambdamicrovms-managed-image-catalog
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdamicrovms-managed-image-catalog lambda-microvms-catalog-list-managed-microvm-images
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|ListManagedMicrovmImageVersions|2xx->2xx|$.items[*]
Issue-Key: lambda-microvms|ListManagedMicrovmImages|2xx->2xx|$.items[*]
Issue-Commit: 3cbee5fc05633571f18b42c799709a3e75b70289

# lambda-microvms managed MicroVM image catalog is synthesized rather than published

> _Note: this issue was identified against the code base at commit `3cbee5fc05633571f18b42c799709a3e75b70289`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListManagedMicrovmImageVersions

## Description
This issue bundles 2 distinct root causes in the lambda-microvms emulator's AWS-managed base-image catalog, both observed on replay of the wire corpus. A fixer must close both (Cause 1 of 2, Cause 2 of 2) before submitting.

=== Cause 1 of 2: ListManagedMicrovmImageVersions publishes one invented version instead of the catalog's version set, and omits supportedCpuConfigurations ===
Fixture lambdamicrovms-managed-image-catalog, step task#2.
AWS: ListManagedMicrovmImageVersions 'Lists versions of a managed MicroVM image' and returns items of ManagedMicrovmImageVersion - imageArn (@required), imageVersion (@required, NonBlankString), status (ManagedMicrovmImageVersionStatus: 'AVAILABLE (the version is available for use) or DEPRECATED'), createdAt (@required), updatedAt. Recorded behavior for an AWS-published base image: the catalog exposes MULTIPLE versions of the image, and a managed base-image version id is a plain integer generation counter ('1', '0'), NOT a '<major>.<minor>' string like the customer image versions the same service mints. Each recorded item also carries supportedCpuConfigurations, a list of {architecture} entries (ARM_64 for the recorded AL2023 base image) - a member the model does not declare, so recorded behavior only; it is what tells a caller which architectures the base image can be built for.
Emulator: exactly ONE item, with imageVersion='1.0' (the customer-image numbering format, not a managed generation counter), and no supportedCpuConfigurations member at all. The older generation the catalog publishes is absent entirely.
Why: a client that resolves a base image to a specific generation - pinning 'the previous generation' for reproducible builds, or walking the versions to find the newest AVAILABLE one - finds a single version whose id does not exist in AWS. Passing '1.0' as CreateMicrovmImage's baseImageVersion would be refused by AWS, and a client that selects a base image by architecture cannot select at all because supportedCpuConfigurations is missing.
Fix: the managed-image catalog seed in the service state (the fixed table the managed list operations read) must publish each managed base image's full version set with integer generation ids and the per-version supportedCpuConfigurations, and the ListManagedMicrovmImageVersions serializer must emit that member.

=== Cause 2 of 2: managed catalog timestamps are stamped at request time instead of being the published image's own metadata ===
Fixture lambda-microvms-catalog-list-managed-microvm-images, step task#1 (ListManagedMicrovmImages).
AWS: ManagedMicrovmImageSummary models createdAt (@required, 'The timestamp when the managed MicroVM image was created') and updatedAt ('The timestamp when the managed MicroVM image was last updated'). These describe the AWS-published catalog entry, so they are FIXED catalog metadata: identical on every call for the life of the entry, and createdAt and updatedAt are DISTINCT values (the recorded entry was created and later republished, so updatedAt is strictly later than createdAt).
Emulator: both members are set to the moment the request is served, so createdAt == updatedAt on every row and both values change on every call. The requirement being violated is the stability and the createdAt/updatedAt distinction, not any particular epoch value from one recording.
Why: a client that caches the catalog and diffs createdAt/updatedAt to detect a republished base image sees every image as republished on every poll, and a client that treats createdAt as an immutable identity component sees it change under it. Nothing about the catalog is reproducible.
Fix: the managed-image catalog seed in the service state must carry per-entry createdAt/updatedAt constants (one pair per published image, updatedAt distinct from createdAt) and ListManagedMicrovmImages must serialize those stored values rather than the current clock.

Traces: lambdamicrovms-managed-image-catalog#task#2; lambda-microvms-catalog-list-managed-microvm-images#task#1.

## Evidence
Wire replay of trace set simparity-wire-aws-v1. Expected = recorded AWS response, Actual = emulator.

Cause 1 (lambdamicrovms-managed-image-catalog#task#2, ListManagedMicrovmImageVersions, 200 vs 200):
  $.items[0]: expected {imageArn: <aws-owned managed arn>, imageVersion: '1', status: 'AVAILABLE', supportedCpuConfigurations: [{architecture: 'ARM_64'}], createdAt: <shaped>, updatedAt: <shaped>}
              got      {imageArn: <same arn>, imageVersion: '1.0', status: 'AVAILABLE', createdAt: <request time>, updatedAt: <request time>} - no supportedCpuConfigurations
  $.items[1]: expected the same entry at imageVersion '0' (status AVAILABLE, supportedCpuConfigurations [{architecture: 'ARM_64'}]), got (absent - the emulator returns a single item)
  The same cluster reproduces on the lambda-token fixture lambda-microvms-managed-images-catalog#task#2, filed under that service's own issue.

Cause 2 (lambda-microvms-catalog-list-managed-microvm-images#task#1, ListManagedMicrovmImages, 200 vs 200):
  $.items[0]: expected an entry whose createdAt and updatedAt are two DISTINCT fixed catalog timestamps (updatedAt later than createdAt)
              got      an entry whose createdAt and updatedAt are the SAME value, equal to the moment the request was served, and different on every call

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json): ManagedMicrovmImageVersion{imageArn @required, imageVersion @required, status (AVAILABLE|DEPRECATED), createdAt @required, updatedAt}; ManagedMicrovmImageSummary{imageArn @required, createdAt @required, updatedAt}; ListManagedMicrovmImages/ListManagedMicrovmImageVersions output items @required and @paginated. supportedCpuConfigurations is a recorded-only member, not declared in the model.

## Steps to Reproduce
1. mvm = client('lambda-microvms').
2. items = ListManagedMicrovmImages()['items']; arn = items[0]['imageArn'] (account segment is literally 'aws').
   AWS: the entry's createdAt and updatedAt are fixed, distinct catalog timestamps - re-run the call and they are unchanged. Emulator: createdAt == updatedAt == now, and both move on every call. (Cause 2)
3. ListManagedMicrovmImageVersions(imageIdentifier=arn).
   AWS: several items, imageVersion values are integer generations ('1', '0'), each with status='AVAILABLE' and supportedCpuConfigurations=[{'architecture': 'ARM_64'}].
   Emulator: a single item with imageVersion='1.0' and no supportedCpuConfigurations. (Cause 1)
4. CreateMicrovmImage(..., baseImageArn=arn, baseImageVersion='1') - the generation id AWS publishes - and confirm the emulator's catalog never offered it.

## Fix status

Cause 1: FIXED, and already so in the code base this fix starts from (the report was filed
against an older commit): `MANAGED_BASE_IMAGE_VERSIONS` in
`services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs` publishes both
generations with integer ids and the per-generation architectures, and
`ListManagedMicrovmImageVersions` writes `supportedCpuConfigurations` beside the modelled
payload. Replay reports the `ListManagedMicrovmImageVersions|$.items[*]` key as `fixed`.

Cause 2: the rule is implemented. The generation table now carries the instant the catalogue
published each generation, and a catalogue entry reports the oldest generation's instant as
`createdAt` and the newest one's as `updatedAt` (`managed_base_image_publication`), so both
listings report fixed per-entry metadata, identical on every read and in every region, with
`updatedAt` strictly later than `createdAt` for an image published in more than one
generation. `tests/all/managed_catalogue_publication_metadata.rs` pins that over the
published catalogue crossed with several regions and both identifier forms.

What is still reported open is the `ListManagedMicrovmImages|$.items[*]` key on one of the
two recorded fixtures of that operation, and what it can still be reporting is the recorded
epoch values themselves, which a replay withholds: of the two recorded listings, the one in
`lambdamicrovms-managed-image-catalog#task#1` passes with the published instants (and failed
when the row's `updatedAt` was dropped, and when its `imageArn` was perturbed, so the member
set and the ARN both match the recording), while `lambda-microvms-catalog-list-managed-microvm-images#task#1`
diverges at `$.items[0]` with the same body and the same region. No request member, model
trait or stored record determines those two epochs — AWS's own publication dates for the
base image are not data this emulator holds — so the seed carries chosen constants anchored
on the service's API version date rather than a single recording's values. A later fixer with
access to the fixture can complete the seed from it; guessing the epochs would substitute one
fidelity defect for another.
