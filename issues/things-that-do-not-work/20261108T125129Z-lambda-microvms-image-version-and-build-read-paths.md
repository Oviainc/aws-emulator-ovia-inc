Issue-Source: trace-sweep-divergence
Issue-Trace: lambdamicrovms-customer-image-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdamicrovms-customer-image-lifecycle lambdamicrovms-image-version-status-and-update
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|ListMicrovmImageVersions|2xx->2xx|$.items[*].baseImageVersion
Issue-Key: lambda-microvms|ListMicrovmImageVersions|2xx->2xx|$.items[*].versionStateTimeBucket
Issue-Key: lambda-microvms|ListMicrovmImageBuilds|2xx->2xx|$.items[*]
Issue-Key: lambda-microvms|UpdateMicrovmImage|2xx->2xx|$.latestActiveImageVersion
Issue-Commit: f1d70ff20b4a607718bee5570d5907dd84812972

# lambda-microvms image, version and build read paths report image-level bookkeeping from defaults and drop recorded members

> _Note: this issue was identified against the code base at commit `f1d70ff20b4a607718bee5570d5907dd84812972`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMicrovmImageVersions

## Description
This issue bundles 3 distinct root causes in the lambda-microvms emulator, all observed on replay of wire corpus simparity-wire-aws-v1, all of the same defect class: the records the emulator keeps for a MicroVM image, its versions and its per-target builds are written from built-in defaults and projected through member sets narrower than the ones AWS answers with, so a client reading an image back is told the wrong base-image pedigree, is never told which version is live, and never receives the recorded members AWS puts on a build row. A fixer must close all three (Cause 1 of 3 .. Cause 3 of 3) before submitting.

Related open issues, NONE of which carries any of the three replay keys below (the replay reports all three clusters with no owning issue and no stub): '20261102T072953Z-lambda-microvms-image-version-records-are-written-from.md' (the same baseImageVersion default, but keyed on CreateMicrovmImage#setup#2, on UpdateMicrovmImage$latestFailedImageVersion and on the UPDATE-path version listing — never on the CREATE-path listing row below, and never on latestActiveImageVersion); '20261103T192714Z-lambda-microvms-getmicrovmimageversion-reports-a-failed-build-as.md' (the single-version READ path only); '20261030T024426Z-lambda-microvms-managed-microvm-image-catalog-is-synthesized.md' and '20261025T210105Z-lambda-microvms-image-builds-never-fail-image-store.md' (the catalog and the build state machine). A fixer should expect Causes 1 and 3 to converge on the same version/image record change, but each cause must be re-replayed on the step named under its own Traces line, which none of those issues exercises.

=== Cause 1 of 3: on the CREATE path, a ListMicrovmImageVersions row reports the constant baseImageVersion '1.0' and a versionStateTimeBucket that does not match the recorded one ===
Fixture lambdamicrovms-customer-image-lifecycle, step task#7 (ListMicrovmImageVersions, HTTP 200 both sides).
AWS: Smithy MicrovmImageVersionSummary$baseImageVersion targets Version (optional, @length 1..2048, pattern ^[^\s]+$) and is documented 'The specific version of the base MicroVM image.' — it identifies the base image version the row's version was actually built from: the version the request named, and otherwise the version the service resolved for the given baseImageArn. Recorded behavior for a version created against the managed base image arn:aws:lambda:us-west-2:aws:microvm-image:al2023-1: the row reports baseImageVersion '2.0' (that literal is this run's resolved value, not the requirement). The same row carries versionStateTimeBucket, a member the model does NOT declare (recorded behavior only — `smithy_query shape=MicrovmImageVersionSummary` lists no such member); the corpus records its value as a shaped, non-asserted value, so the requirement is that a settled version carries a bucket IN THE FORM AWS records it, not any particular instant.
Emulator: items[0].baseImageVersion '1.0' — the built-in default every version record is written with, regardless of the base image the create named or the versions that base image has. items[0].versionStateTimeBucket '2026-11-08T12:00:00Z', which does not satisfy the recorded shape for that member: the emulator renders the bucket as a full ISO-8601 DateTime instant truncated to the hour (crate::state::state_time_bucket over VERSION_STATE_TIME_BUCKET = 1h), a form it chose itself rather than took from the recording.
Why: baseImageVersion is how a client pins and audits the base layer of an image. Every row the emulator serves claims base version 1.0, so a reproducible-build gate or a CVE re-base sweep that lists an image's versions and asserts the base version it pinned is told its request was silently downgraded, and never sees that a re-base is needed. A versionStateTimeBucket in a form AWS does not emit breaks any client that parses or groups rows by that bucket, and does so only for the listing path, where it is the row-level key a poller uses to tell two settlements apart.
Fix: the version record in the service state must carry the request's baseImageVersion when supplied and otherwise the version resolved from the named base image's own record, instead of defaulting the stored field to '1.0'; and the recorded-member projection for versionStateTimeBucket (crate::state::MicrovmImageVersion::reported_state_time_bucket / state_time_bucket, as written into the listing row by ListMicrovmImageVersions' summary_of) must emit the bucket in the recorded form rather than a self-chosen hour-truncated instant.
Traces: lambdamicrovms-customer-image-lifecycle#task#7.

=== Cause 2 of 3: ListMicrovmImageBuilds build rows omit the recorded member terminationReasonCode ===
Fixture lambdamicrovms-customer-image-lifecycle, step task#8 (ListMicrovmImageBuilds, HTTP 200 both sides).
AWS: recorded behavior only — MicrovmImageBuildSummary declares imageArn, imageVersion, buildId, buildState, architecture, chipset, chipsetGeneration, stateReason and createdAt, and declares NO terminationReasonCode (`smithy_query doc shape=MicrovmImageBuildSummary member=terminationReasonCode` -> member not found). The recording shows AWS answering every build row of a version with terminationReasonCode PRESENT, null-valued for a build that was not terminated — the member is part of the row's shape, so a client reading the row finds the key whether or not a code applies. Everything else about the recorded rows matches the emulator (buildState 'FAILED', architecture 'ARM_64', chipset 'GRAVITON', stateReason 'The S3 bucket does not exist.').
Emulator: the member is absent from every row. ListMicrovmImageBuilds projects MicrovmImageBuildSummary with exactly the modeled member set and writes no recorded members beside it, while the version-reporting paths of the same service do use that facility (crate::ops::types::RecordedMembers, as used for versionStateTimeBucket and for the image id).
Why: a client that reads terminationReasonCode off a build row to distinguish a build that was terminated by the platform from one that failed on its own gets a missing key against the emulator. In a typed SDK the member deserializes as absent rather than null, and in a hand-rolled client a direct index raises instead of yielding null, so the termination-handling branch is either dead code or a crash that never reproduces against AWS.
Fix: the build record in the service state must carry the termination reason code, and the build-row projection in ListMicrovmImageBuilds (and the matching single-build read, so the two cannot disagree) must write it beside the modeled payload as a recorded member, null when no code applies.
Traces: lambdamicrovms-customer-image-lifecycle#task#8.

=== Cause 3 of 3: UpdateMicrovmImage omits latestActiveImageVersion, so the update response never names the image's live version ===
Fixture lambdamicrovms-image-version-status-and-update, step task#5 (UpdateMicrovmImage, HTTP 200 both sides).
AWS: Smithy UpdateMicrovmImageResponse$latestActiveImageVersion (optional, NonBlankString, @length 1..2048, pattern ^[^\s]+$) is documented 'The latest active version of the MicroVM image.' When the image already holds a version that settled in the successful/ACTIVE branch, the 200 update response reports that version's identifier in this member. Recorded behavior: the member is present and names that version ('1.0' in the recording — the member's value is whichever version of the image is live, never the literal '1.0').
Emulator: the member is absent from the 200 response entirely. The image record does hold a latest_active_image_version field (crate::state::MicrovmImage), so the update path either never sets it when a version settles ACTIVE or does not project it into this response.
Why: latestActiveImageVersion is the single field the update response gives a client to learn which version is still serving traffic while the version the update just created builds. Against the emulator it is never there, so a deploy pipeline that updates an image and reads this member to decide what to roll back to, or to keep routing at, has nothing to pin, and its rollback path is unreachable.
Fix: the image record in the service state must maintain latestActiveImageVersion whenever a version settles in the successful branch, and UpdateMicrovmImage's handler/serializer must emit it when set. It shares the build-settlement transition with the related open issues above, but the image-level bookkeeping and the response projection are this cause.
Traces: lambdamicrovms-image-version-status-and-update#task#5.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200; 81 steps, 59 passed, 22 mismatched, score 72.8%, 15 divergence clusters). Expected = recorded AWS response, Actual = emulator. All three clusters below carry an EMPTY stub_path and an EMPTY owned_by.

Cause 1 — lambdamicrovms-customer-image-lifecycle#task#7, ListMicrovmImageVersions, status 200 vs 200:
  $.items[0].baseImageVersion: expected '2.0' (the version resolved for the request's baseImageArn), got '1.0' (constant default)
  $.items[0].versionStateTimeBucket: expected the recorded shape (value not asserted by the corpus), got '2026-11-08T12:00:00Z'
  divergence_key: service=lambda-microvms|ListMicrovmImageVersions|status=200->200|keys=$.items[0].baseImageVersion,$.items[0].versionStateTimeBucket

Cause 2 — lambdamicrovms-customer-image-lifecycle#task#8, ListMicrovmImageBuilds, status 200 vs 200:
  $.items[0] / $.items[1]: 'missing' — the expected row carries 'terminationReasonCode': null and the emulator's row has no such key. Every other member agrees on both sides: imageArn arn:aws:lambda:us-west-2:000000000000:microvm-image:sim2real-probe-mvmimg-8661b5c1, imageVersion '1.0', buildState 'FAILED', architecture 'ARM_64', chipset 'GRAVITON', stateReason 'The S3 bucket does not exist.'; buildId/chipsetGeneration/createdAt are per-run values the corpus does not assert.
  divergence_key: service=lambda-microvms|ListMicrovmImageBuilds|status=200->200|keys=$.items[0],$.items[1]

Cause 3 — lambdamicrovms-image-version-status-and-update#task#5, UpdateMicrovmImage, status 200 vs 200:
  $.latestActiveImageVersion: expected the image's live version ('1.0' in the recording), got (absent)
  divergence_key: service=lambda-microvms|UpdateMicrovmImage|status=200->200|keys=$.latestActiveImageVersion

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json): MicrovmImageVersionSummary$baseImageVersion optional Version, @length 1..2048, pattern ^[^\s]+$, doc 'The specific version of the base MicroVM image.'; MicrovmImageVersionSummary declares NO versionStateTimeBucket and MicrovmImageBuildSummary declares NO terminationReasonCode (both recorded behavior only); UpdateMicrovmImageResponse$latestActiveImageVersion optional NonBlankString, doc 'The latest active version of the MicroVM image.'; UpdateMicrovmImage is PUT /2025-09-09/microvm-images/{imageIdentifier}, 200.

Code sightings (for orientation only): services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/list_microvm_image_versions.rs:296-335 (summary_of/payload_of, the bucket projection); src/ops/list_microvm_image_builds.rs:261-277 (build-row projection with no recorded members); src/ops/get_microvm_image_build.rs:228-239 (same member set on the single-build read); src/state.rs:1054-1099 (reported_state_time_bucket / state_time_bucket, the self-chosen hour bucket) and src/state.rs:1101-1122 (MicrovmImage.latest_active_image_version).

## Steps to Reproduce
Cause 1 (create-path listing row):
1. mvm = client('lambda-microvms'); base = ListManagedMicrovmImages()['items'][0]['imageArn']  (the managed al2023-1 base image).
2. CreateMicrovmImage(name=<unique>, baseImageArn=base, buildRoleArn=<an IAM role arn>, codeArtifact={'uri': 's3://<unique>-absent/code.zip'}).
3. ListMicrovmImageVersions(imageIdentifier=<imageArn>) once the version has settled.
   AWS: items[0].baseImageVersion is the base version resolved for `base` (recorded: '2.0'), and items[0].versionStateTimeBucket is in the form AWS records.
   Emulator: baseImageVersion '1.0' for every create; versionStateTimeBucket '<hour-truncated ISO instant>' (e.g. '2026-11-08T12:00:00Z').

Cause 2 (build rows):
4. ListMicrovmImageBuilds(imageIdentifier=<imageArn>, imageVersion='1.0').
   AWS: every row carries terminationReasonCode, null for a build that was not terminated.
   Emulator: no row carries the key at all. (Same member set on GetMicrovmImageBuild.)

Cause 3 (update response):
5. Create an image whose first version builds successfully, so the image holds an ACTIVE version.
6. UpdateMicrovmImage(imageIdentifier=<imageArn>, baseImageArn=base, buildRoleArn=<same role>, codeArtifact={'uri': 's3://<unique>-absent/code2.zip'}).
   AWS: the 200 response carries latestActiveImageVersion naming the image's live version (recorded: '1.0').
   Emulator: the member is absent.
