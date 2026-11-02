Issue-Source: trace-sweep-divergence
Issue-Trace: lambdamicrovms-customer-image-lifecycle
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdamicrovms-customer-image-lifecycle lambdamicrovms-image-version-status-and-update
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|CreateMicrovmImage|2xx->2xx|$.baseImageVersion
Issue-Key: lambda-microvms|UpdateMicrovmImage|2xx->2xx|$.latestFailedImageVersion
Issue-Key: lambda-microvms|ListMicrovmImageVersions|2xx->2xx|$.items[*]
Issue-Commit: 88dedb85e694d97545027b37e695bddb9d3861f2

# lambda-microvms image version records are written from defaults: baseImageVersion is pinned to 1.0 and the update-path build failure never settles

> _Note: this issue was identified against the code base at commit `88dedb85e694d97545027b37e695bddb9d3861f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrovmImage

## Description
This issue bundles 3 distinct root causes in the lambda-microvms emulator, all observed on replay of the wire corpus, all rooted in the same defect class: the MicroVM image VERSION record the emulator writes is populated from built-in defaults rather than from the request and from the outcome of the build, so the version's base-image pedigree and its terminal state are both wrong. A fixer must close all three (Cause 1 of 3 .. Cause 3 of 3) before submitting.

Related open issues, neither of which carries any of the three replay keys below, so these causes are filed here: '20261030T024426Z-lambda-microvms-image-build-failure-is-never-modeled.md' (same build-failure gap, but keyed only on the lambdamicrovms-customer-image-lifecycle CREATE path and on RunMicrovm) and '20261025T210105Z-lambda-microvms-image-builds-never-fail-image-store.md' (the behavioral-suite view of the same state machine). Causes 2 and 3 here are on the UPDATE path and in a different fixture; a fixer should expect them to converge on the same state-machine change, but they must be re-replayed on the steps named below, which neither of those issues exercises.

=== Cause 1 of 3: CreateMicrovmImage answers a constant baseImageVersion instead of the base image version the new version was built from ===
Fixture lambdamicrovms-customer-image-lifecycle, step setup#2 (CreateMicrovmImage, HTTP 201 both sides).
AWS: Smithy CreateMicrovmImageRequest$baseImageVersion and CreateMicrovmImageResponse$baseImageVersion both target Version (optional, @length 1..2048, pattern ^[^\s]+$); the response member is documented on the summary shape as 'The specific version of the base MicroVM image.' The echoed value therefore identifies the base image version this new image version is actually built from: the version named in the request when the request names one, and otherwise the version the service resolved for the given baseImageArn. Recorded behavior for a create against the managed base image arn:aws:lambda:us-west-2:aws:microvm-image:al2023-1: the 201 response carries baseImageVersion '2.0'.
Emulator: baseImageVersion '1.0' — the same literal for every create, regardless of what the request asked for or what versions the named base image has. The same constant then propagates into the version listing for that image (ListMicrovmImageVersions on the same fixture reports items[0].baseImageVersion '1.0' where the recording has '2.0').
Why: baseImageVersion is how a client pins and audits the base layer of its image. The emulator reports every image as built on base version 1.0, so a client that asserts it got the base version it pinned (a reproducible-build gate, a CVE-remediation sweep that re-bases images onto a newer managed version) is told its request was silently downgraded, and a client that reads the value back to decide whether a re-base is needed never sees one.
Fix: CreateMicrovmImage's handler and the version record it writes in the service state must carry the request's baseImageVersion when supplied, and resolve the base image's current version from the managed/customer base-image record when it is omitted, instead of defaulting the stored field to '1.0'. The constant must not be re-introduced in the response serializer.

=== Cause 2 of 3: UpdateMicrovmImage omits latestFailedImageVersion after the version it created failed to build ===
Fixture lambdamicrovms-image-version-status-and-update, step task#5 (UpdateMicrovmImage, HTTP 200 both sides).
AWS: Smithy UpdateMicrovmImageResponse$latestFailedImageVersion (optional, NonBlankString) is documented 'The latest failed version of the MicroVM image, if any.' When an earlier version of the image has settled in the failed branch, the update response reports that version in this member. Recorded behavior: latestFailedImageVersion is present and names the image's failed version ('1.0' in the recording — the member's value is the identifier of whichever version failed, never a literal '1.0').
Emulator: the member is absent from the 200 response entirely, because no version in the emulator ever reaches the failed branch, so the image-level bookkeeping that would name one is never written.
Why: latestFailedImageVersion is the single field an update response gives a client to notice that a previous build of the image is broken. Against the emulator it is never populated, so a deploy pipeline that reads it after an update to decide whether to roll back or alert sees a clean image forever, and the branch of its code that handles a failed version is unreachable.
Fix: the image record in the service state must maintain latestFailedImageVersion whenever a version settles in the failed branch, and UpdateMicrovmImage's serializer must emit it when set. It depends on the build transition actually resolving the failure branch (same state-machine change as the related issues).

=== Cause 3 of 3: a version whose code artifact cannot be fetched is left IN_PROGRESS with no stateReason instead of settling FAILED ===
Fixture lambdamicrovms-image-version-status-and-update, step task#7 (ListMicrovmImageVersions, HTTP 200 both sides).
AWS: Smithy MicrovmImageVersionSummary$state is @required, targets MicrovmImageVersionState (PENDING|IN_PROGRESS|SUCCESSFUL|FAILED|DELETING|DELETED|DELETE_FAILED) and is documented 'The current state of the version.'; $stateReason is documented 'The reason for the current state. For example, one or more builds failed.' The build is asynchronous and terminal: a version whose codeArtifact.uri points at an S3 bucket that does not exist settles state=FAILED with status=INACTIVE and stateReason set. For this failure class the recorded stateReason is the complete fixed sentence 'The S3 bucket does not exist.' — it has no variable parts. The recorded settled version additionally carries versionStateTimeBucket, a member the model does not declare (recorded behavior only); its value is per-run, so only its PRESENCE on a settled version is the requirement.
Emulator: items[0].state 'IN_PROGRESS' where the recording has 'FAILED', stateReason absent where the recording has 'The S3 bucket does not exist.', versionStateTimeBucket absent. (status is already 'INACTIVE' on both sides, so the version is advertised as non-launchable but never explains why and never reaches a terminal state.)
Why: IN_PROGRESS is a non-terminal state, so a client polling the version it just created by UpdateMicrovmImage polls forever against the emulator instead of observing the failure, and it never receives the stateReason that is the only machine-readable explanation of why the artifact was rejected. Timeout/retry logic keyed on FAILED is dead code.
Fix: the deferred build transition in the service state must resolve the version's codeArtifact S3 location against the emulator's own object store and settle the version on the FAILED branch with that fixed stateReason on the UPDATE path as well as the create path, and the ListMicrovmImageVersions / GetMicrovmImageVersion serializers must emit stateReason and versionStateTimeBucket when set.

Traces: lambdamicrovms-customer-image-lifecycle#setup#2 (Cause 1); lambdamicrovms-image-version-status-and-update#task#5 (Cause 2); lambdamicrovms-image-version-status-and-update#task#7 (Cause 3).

## Evidence
Wire replay of trace set simparity-wire-aws-v1 (emulator up, 81 steps, 52 passed, score 64.2%, 15 divergence clusters). Expected = recorded AWS response, Actual = emulator. These three clusters carry no stub_path and no owning issue.

Cause 1 — lambdamicrovms-customer-image-lifecycle#setup#2, CreateMicrovmImage, status 201 vs 201:
  $.baseImageVersion: expected '2.0', got '1.0'
  divergence_key: service=lambda-microvms|CreateMicrovmImage|status=201->201|keys=$.baseImageVersion
  (knock-on, already filed elsewhere on its own step: the same image's ListMicrovmImageVersions row reports baseImageVersion '1.0' against recorded '2.0')

Cause 2 — lambdamicrovms-image-version-status-and-update#task#5, UpdateMicrovmImage, status 200 vs 200:
  $.latestFailedImageVersion: expected the image's failed version ('1.0' in the recording), got (absent)
  divergence_key: service=lambda-microvms|UpdateMicrovmImage|status=200->200|keys=$.latestFailedImageVersion

Cause 3 — lambdamicrovms-image-version-status-and-update#task#7, ListMicrovmImageVersions, status 200 vs 200:
  $.items[*].state: expected 'FAILED', got 'IN_PROGRESS'
  $.items[*].stateReason: expected 'The S3 bucket does not exist.' (fixed text for this failure class), got (absent)
  $.items[*].versionStateTimeBucket: expected present, got (absent)
  matching members on both sides in that row: status 'INACTIVE', baseImageArn arn:aws:lambda:us-west-2:aws:microvm-image:al2023-1, imageVersion '2.0', codeArtifact.uri pointing into a bucket that does not exist
  divergence_key: service=lambda-microvms|ListMicrovmImageVersions|status=200->200|keys=$.items[0],$.items[1]

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json): CreateMicrovmImageRequest$baseImageVersion / CreateMicrovmImageResponse$baseImageVersion target Version, optional, @length 1..2048, pattern ^[^\s]+$; UpdateMicrovmImageResponse$latestFailedImageVersion optional, doc 'The latest failed version of the MicroVM image, if any.'; MicrovmImageVersionSummary$state @required over MicrovmImageVersionState = PENDING|IN_PROGRESS|SUCCESSFUL|FAILED|DELETING|DELETED|DELETE_FAILED; MicrovmImageVersionSummary$stateReason doc 'The reason for the current state. For example, one or more builds failed.'; MicrovmImageVersionSummary$status @required ACTIVE|INACTIVE. versionStateTimeBucket is not declared in the model (recorded behavior only).

## Steps to Reproduce
1. mvm = client('lambda-microvms'); base = ListManagedMicrovmImages()['items'][0]['imageArn']  (the managed al2023-1 base image).
2. CreateMicrovmImage(name=<unique>, baseImageArn=base, buildRoleArn=<an IAM role arn>, codeArtifact={'uri': 's3://<unique>-absent/code.zip'}).
   AWS: the 201 response's baseImageVersion is the base image version resolved for that baseImageArn (recorded: '2.0'). Emulator: '1.0' for every create. (Cause 1)
3. UpdateMicrovmImage(imageIdentifier=<imageArn>, baseImageArn=base, buildRoleArn=<same role>, codeArtifact={'uri': 's3://<unique>-absent/code2.zip'}) after the image's first version has failed to build.
   AWS: the 200 response carries latestFailedImageVersion naming that failed version. Emulator: the member is absent. (Cause 2)
4. Poll ListMicrovmImageVersions(imageIdentifier=<imageArn>) for the version the update created.
   AWS: it settles state='FAILED' with stateReason='The S3 bucket does not exist.' and a versionStateTimeBucket present (status='INACTIVE').
   Emulator: it stays state='IN_PROGRESS' with no stateReason and no versionStateTimeBucket, so the poll never terminates. (Cause 3)
