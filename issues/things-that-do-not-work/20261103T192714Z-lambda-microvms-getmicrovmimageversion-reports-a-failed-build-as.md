Issue-Source: trace-sweep-divergence
Issue-Trace: lambdamicrovms-customer-image-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdamicrovms-customer-image-lifecycle lambdamicrovms-image-version-status-and-update
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|GetMicrovmImageVersion|2xx->2xx|$.baseImageVersion
Issue-Key: lambda-microvms|GetMicrovmImageVersion|2xx->2xx|$.state
Issue-Key: lambda-microvms|GetMicrovmImageVersion|2xx->2xx|$.stateReason
Issue-Key: lambda-microvms|GetMicrovmImageVersion|2xx->2xx|$.status
Issue-Key: lambda-microvms|GetMicrovmImageVersion|2xx->2xx|$.versionStateTimeBucket
Issue-Commit: 2b5605b2de23550e6c1f97402ef3aec1933f6882

# lambda-microvms GetMicrovmImageVersion reports a failed build as a healthy active version

> _Note: this issue was identified against the code base at commit `2b5605b2de23550e6c1f97402ef3aec1933f6882`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMicrovmImageVersion

## Description
This issue bundles 2 distinct root causes in the lambda-microvms emulator, both observed on replay of the wire corpus and both on the single-version READ path (GetMicrovmImageVersion), on two different fixtures: the CREATE path and the UPDATE path. In both, a version whose code artifact cannot be fetched is reported as a successfully built, launchable version. Neither cluster carries a stub_path or an owning issue. A fixer must close both (Cause 1 of 2 .. Cause 2 of 2) before submitting.

Related open issues, NEITHER of which carries either replay key below: '20261030T024426Z-lambda-microvms-image-build-failure-is-never-modeled.md' (same build-failure gap, keyed on GetMicrovmImage/ListMicrovmImages/ListMicrovmImageVersions/ListMicrovmImageBuilds/GetMicrovmImageBuild/RunMicrovm, never on GetMicrovmImageVersion) and '20261102T072953Z-lambda-microvms-image-version-records-are-written-from.md' (the baseImageVersion default and the update-path list view). '20261025T210105Z-lambda-microvms-image-builds-never-fail-image-store.md' is the behavioral-suite view of the same state machine. A fixer should expect all of these to converge on one state-machine change, but the two causes here must be re-replayed on the steps named below, which none of those issues exercises.

=== Cause 1 of 2: on the CREATE path, GetMicrovmImageVersion reports state SUCCESSFUL / status ACTIVE with no stateReason for a version whose code artifact does not exist, and echoes a constant baseImageVersion ===
Fixture lambdamicrovms-customer-image-lifecycle, step task#6 (GetMicrovmImageVersion, HTTP 200 both sides).
AWS: Smithy GetMicrovmImageVersion output has state @required over MicrovmImageVersionState (PENDING|IN_PROGRESS|SUCCESSFUL|FAILED|DELETING|DELETED|DELETE_FAILED), status @required over MicrovmImageVersionStatus (ACTIVE|INACTIVE), stateReason optional String ('the reason for the current state — for example, one or more builds failed'), and baseImageVersion optional Version (@length 1..2048, pattern ^[^\s]+$) identifying the base image version this version was actually built from. The build is asynchronous and TERMINAL: a version created from a codeArtifact.uri whose S3 bucket does not exist settles state=FAILED, status=INACTIVE, with stateReason set. For this failure class the recorded stateReason is the complete fixed sentence 'The S3 bucket does not exist.' — it has no variable parts. The settled version additionally carries versionStateTimeBucket, a member the model does not declare (recorded behavior only); its value is per-run, so only its PRESENCE on a settled version is the requirement. baseImageVersion is the version resolved for the request's baseImageArn (the managed al2023-1 base image resolves to '2.0' in the recording — that literal is a value, not the requirement).
Emulator: state 'SUCCESSFUL', status 'ACTIVE', stateReason absent, versionStateTimeBucket absent, baseImageVersion '1.0' (the built-in default, regardless of the base image named in the request).
Why: the emulator advertises a version that cannot possibly run as built and launchable. A client that polls this read until the version leaves the in-flight states gets SUCCESSFUL/ACTIVE and proceeds to RunMicrovm against an image with no usable code; the failure branch keyed on state=FAILED and the operator-facing stateReason are dead code against the emulator; and a reproducible-build or re-base gate that reads baseImageVersion back is told every image sits on base version 1.0.
Fix: the deferred build transition in the service state must resolve the version's codeArtifact S3 location against the emulator's own object store and settle the version on the FAILED branch (status INACTIVE) with that fixed stateReason, and GetMicrovmImageVersion's serializer must emit stateReason and versionStateTimeBucket when set; the version record written by CreateMicrovmImage must carry the request's baseImageVersion when supplied and otherwise the version resolved for the named baseImageArn, instead of defaulting to '1.0'.

=== Cause 2 of 2: on the UPDATE path, GetMicrovmImageVersion likewise reports state SUCCESSFUL / status ACTIVE with no stateReason for a version the update created from an unfetchable artifact ===
Fixture lambdamicrovms-image-version-status-and-update, step task#6 (GetMicrovmImageVersion, HTTP 200 both sides).
AWS: as above — the version UpdateMicrovmImage creates from a codeArtifact.uri in a nonexistent bucket settles state=FAILED, status=INACTIVE, stateReason 'The S3 bucket does not exist.' (fixed text, no variable parts), versionStateTimeBucket present.
Emulator: state 'SUCCESSFUL', status 'ACTIVE', stateReason absent, versionStateTimeBucket absent. This is a separate cause from Cause 1 because the version here is produced by the UPDATE transition rather than the CREATE one, in a different fixture, and the update-path transition can be fixed independently of the create-path one.
Why: a client that updates an image and then polls the new version by GetMicrovmImageVersion to decide whether to promote it or roll back is told the broken version is ACTIVE and promotes it; the stateReason that is the only machine-readable explanation of the rejected artifact is never delivered.
Fix: the same deferred-build transition must resolve the artifact and settle the FAILED branch on the UPDATE path as well, with the serializer emitting stateReason and versionStateTimeBucket.

Traces: lambdamicrovms-customer-image-lifecycle#task#6 (Cause 1); lambdamicrovms-image-version-status-and-update#task#6 (Cause 2).

## Evidence
Wire replay of trace set simparity-wire-aws-v1 (emulator up; 81 steps, 52 passed, score 64.2%, 18 divergence clusters). Expected = recorded AWS response, Actual = emulator.

Cause 1 — lambdamicrovms-customer-image-lifecycle#task#6, GetMicrovmImageVersion, status 200 vs 200:
  $.state: expected 'FAILED', got 'SUCCESSFUL'
  $.status: expected 'INACTIVE', got 'ACTIVE'
  $.stateReason: expected 'The S3 bucket does not exist.' (fixed text for this failure class), got (absent)
  $.versionStateTimeBucket: expected present, got (absent)
  $.baseImageVersion: expected the version resolved for the request's baseImageArn ('2.0' in the recording), got '1.0'
  divergence_key: service=lambda-microvms|GetMicrovmImageVersion|status=200->200|keys=$.baseImageVersion,$.state,$.stateReason,$.status,$.versionStateTimeBucket

Cause 2 — lambdamicrovms-image-version-status-and-update#task#6, GetMicrovmImageVersion, status 200 vs 200:
  $.state: expected 'FAILED', got 'SUCCESSFUL'
  $.status: expected 'INACTIVE', got 'ACTIVE'
  $.stateReason: expected 'The S3 bucket does not exist.', got (absent)
  $.versionStateTimeBucket: expected present, got (absent)
  divergence_key: service=lambda-microvms|GetMicrovmImageVersion|status=200->200|keys=$.state,$.stateReason,$.status,$.versionStateTimeBucket

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json): GetMicrovmImageVersion is @readonly, GET /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}, 200; output members state @required -> MicrovmImageVersionState = PENDING|IN_PROGRESS|SUCCESSFUL|FAILED|DELETING|DELETED|DELETE_FAILED, status @required -> MicrovmImageVersionStatus, stateReason optional String, baseImageVersion optional Version (@length 1..2048, pattern ^[^\s]+$), codeArtifact @required, imageArn/imageVersion/baseImageArn/buildRoleArn/createdAt @required. versionStateTimeBucket is NOT declared in the model (recorded behavior only).

## Steps to Reproduce
Cause 1 (create path):
1. mvm = client('lambda-microvms'); base = ListManagedMicrovmImages()['items'][0]['imageArn'] (the managed al2023-1 base image).
2. CreateMicrovmImage(name=<unique>, baseImageArn=base, buildRoleArn=<an IAM role arn>, codeArtifact={'uri': 's3://<unique>-absent/code.zip'}) — a bucket that does not exist.
3. Poll GetMicrovmImageVersion(imageIdentifier=<imageArn>, imageVersion='1.0').
   AWS: settles state='FAILED', status='INACTIVE', stateReason='The S3 bucket does not exist.', versionStateTimeBucket present, baseImageVersion = the version resolved for `base`.
   Emulator: state='SUCCESSFUL', status='ACTIVE', no stateReason, no versionStateTimeBucket, baseImageVersion='1.0'.

Cause 2 (update path):
4. UpdateMicrovmImage(imageIdentifier=<imageArn>, baseImageArn=base, buildRoleArn=<same role>, codeArtifact={'uri': 's3://<unique>-absent/code2.zip'}).
5. Poll GetMicrovmImageVersion(imageIdentifier=<imageArn>, imageVersion=<the version the update created, '2.0' in the recording>).
   AWS: settles state='FAILED', status='INACTIVE', stateReason='The S3 bucket does not exist.', versionStateTimeBucket present.
   Emulator: state='SUCCESSFUL', status='ACTIVE', no stateReason, no versionStateTimeBucket.
