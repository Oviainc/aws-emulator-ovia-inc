Issue-Source: trace-sweep-divergence
Issue-Trace: lambdamicrovms-customer-image-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdamicrovms-customer-image-lifecycle lambdamicrovms-image-tagging-and-run-refusals
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|GetMicrovmImage|2xx->2xx|$.id
Issue-Key: lambda-microvms|GetMicrovmImage|2xx->2xx|$.latestActiveImageVersion
Issue-Key: lambda-microvms|GetMicrovmImage|2xx->2xx|$.latestFailedImageVersion
Issue-Key: lambda-microvms|GetMicrovmImage|2xx->2xx|$.state
Issue-Key: lambda-microvms|ListMicrovmImages|2xx->2xx|$.items[*].id
Issue-Key: lambda-microvms|ListMicrovmImages|2xx->2xx|$.items[*].latestActiveImageVersion
Issue-Key: lambda-microvms|ListMicrovmImages|2xx->2xx|$.items[*].latestFailedImageVersion
Issue-Key: lambda-microvms|ListMicrovmImages|2xx->2xx|$.items[*].state
Issue-Key: lambda-microvms|ListMicrovmImageVersions|2xx->2xx|$.items[*].baseImageVersion
Issue-Key: lambda-microvms|ListMicrovmImageVersions|2xx->2xx|$.items[*].state
Issue-Key: lambda-microvms|ListMicrovmImageVersions|2xx->2xx|$.items[*].stateReason
Issue-Key: lambda-microvms|ListMicrovmImageVersions|2xx->2xx|$.items[*].status
Issue-Key: lambda-microvms|ListMicrovmImageVersions|2xx->2xx|$.items[*].versionStateTimeBucket
Issue-Key: lambda-microvms|ListMicrovmImageBuilds|2xx->2xx|$.items[*]
Issue-Key: lambda-microvms|GetMicrovmImageBuild|2xx->2xx|$.buildId
Issue-Key: lambda-microvms|GetMicrovmImageBuild|2xx->2xx|$.buildState
Issue-Key: lambda-microvms|GetMicrovmImageBuild|2xx->2xx|$.snapshotBuild
Issue-Key: lambda-microvms|GetMicrovmImageBuild|2xx->2xx|$.stateReason
Issue-Key: lambda-microvms|RunMicrovm|4xx->2xx|$.message
Issue-Key: lambda-microvms|RunMicrovm|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda-microvms|RunMicrovm|4xx->2xx|status
Issue-Commit: 3cbee5fc05633571f18b42c799709a3e75b70289

# lambda-microvms image build failure is never modeled: a broken code artifact still settles SUCCESSFUL

> _Note: this issue was identified against the code base at commit `3cbee5fc05633571f18b42c799709a3e75b70289`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMicrovmImage

## Description
This issue bundles 6 distinct root causes in the lambda-microvms emulator, all observed on replay of the wire corpus, all rooted in the same missing behavior: the asynchronous MicroVM image build always succeeds, so the whole FAILED/CREATE_FAILED branch of the image, version and build state machines is unreachable and the refusal it gates never happens. A fixer must close all six (Cause 1 of 6 .. Cause 6 of 6) before submitting.

Related open issue: '20261025T210105Z-lambda-microvms-image-builds-never-fail-image-store' (lambda-microvms: image builds never fail, image store is not region-scoped, and managed base images are 404 instead of 403) describes the same build-failure gap from the BEHAVIORAL suite. It does not carry any of the wire replay keys below, so these wire causes are filed here; a fixer should expect the two to converge on the same state-machine change.

=== Cause 1 of 6: GetMicrovmImage reports CREATED with an active version for an image whose build could not run ===
Fixture lambdamicrovms-customer-image-lifecycle, step task#3.
AWS: MicrovmImageState (Smithy: GetMicrovmImageOutput$state, @required, enum CREATING|CREATED|CREATE_FAILED|UPDATING|UPDATED|UPDATE_FAILED|DELETING|DELETE_FAILED|DELETED) settles on CREATE_FAILED when the build of the image's first version fails, and GetMicrovmImageOutput then carries latestFailedImageVersion = that version while latestActiveImageVersion is ABSENT (both members are optional in the model: 'The latest failed version of the MicroVM image, if any' / 'The latest active version'). CreateMicrovmImage's own doc states the rule: 'The build is asynchronous - the image transitions from CREATING to CREATED on success, or CREATE_FAILED on failure.' Recorded behavior for an image whose codeArtifact.uri points into an S3 bucket that does not exist: state=CREATE_FAILED, latestFailedImageVersion=<the image's first version>, no latestActiveImageVersion.
Emulator: state='CREATED', latestActiveImageVersion='1.0', latestFailedImageVersion absent.
Why: a client polls GetMicrovmImage precisely to tell a usable image from a broken one. It is told the build succeeded, hands the image to RunMicrovm, and never sees the failure. Retry/alerting logic keyed on CREATE_FAILED or latestFailedImageVersion is dead code against the emulator.
Fix: the image build state machine in the service state (the deferred build transition and the bookkeeping CreateMicrovmImage/UpdateMicrovmImage schedule it with) must resolve the codeArtifact S3 location against the emulator's own object store and settle the failure branch instead of unconditionally applying the success move.

=== Cause 2 of 6: ListMicrovmImages projects the same phantom success ===
Fixture lambdamicrovms-customer-image-lifecycle, step task#4.
AWS: MicrovmImageSummary carries state (@required), latestActiveImageVersion and latestFailedImageVersion with the same meanings as the Get output, so the failed image appears in the listing as state=CREATE_FAILED with latestFailedImageVersion set and latestActiveImageVersion absent.
Emulator: items[0].state='CREATED', items[0].latestActiveImageVersion='1.0', items[0].latestFailedImageVersion absent.
Why: a client that enumerates images to find the broken ones (a reconciliation sweep, a console list) sees a wholly healthy account.
Fix: the list projection reads the same stored image record as Cause 1 - it diverges only because that record never reaches the failed state; no separate serializer change is expected once Cause 1 is fixed.

=== Cause 3 of 6: the image VERSION never reaches FAILED/INACTIVE and never carries stateReason ===
Fixture lambdamicrovms-customer-image-lifecycle, step task#7 (ListMicrovmImageVersions).
AWS: MicrovmImageVersionSummary models state (@required, MicrovmImageVersionState = PENDING|IN_PROGRESS|SUCCESSFUL|FAILED|DELETING|DELETED|DELETE_FAILED), status (@required, 'ACTIVE (can be used by RunMicrovm) or INACTIVE (blocked from launching new MicroVMs)') and stateReason ('The reason for the current state. For example, one or more builds failed.'). Recorded behavior for the unfetchable-artifact failure class: state=FAILED, status=INACTIVE, stateReason='The S3 bucket does not exist.' - that sentence is entirely FIXED text for this failure class and has no variable parts. The recorded version also carries versionStateTimeBucket, a member the model does not declare (recorded behavior only); its value is a per-run bucket, so only its PRESENCE on a settled version is the requirement.
Emulator: items[0].state='SUCCESSFUL', items[0].status='ACTIVE', stateReason absent, versionStateTimeBucket absent.
Why: stateReason is the only machine-readable explanation of why the artifact was rejected, and status=ACTIVE tells the client the version may launch MicroVMs when it cannot.
Fix: the version record written by the build transition (service state) must settle FAILED/INACTIVE with the fixed stateReason, and the ListMicrovmImageVersions/GetMicrovmImageVersion serializers must emit stateReason and versionStateTimeBucket when set.

=== Cause 4 of 6: no build record is ever FAILED, and the per-generation build set is wrong ===
Fixture lambdamicrovms-customer-image-lifecycle, step task#8 (ListMicrovmImageBuilds).
AWS: MicrovmImageBuildSummary models buildState (@required, BuildState = PENDING|IN_PROGRESS|SUCCESSFUL|FAILED), chipsetGeneration (@required) and stateReason ('The reason for the build state, if applicable.'). Recorded behavior: EVERY per-chipset-generation build of a failed version settles buildState=FAILED carrying the SAME fixed stateReason as the version ('The S3 bucket does not exist.'), and the recorded items additionally carry terminationReasonCode (unmodeled, recorded behavior only) with no value for this class.
Emulator: both items report buildState='SUCCESSFUL', no stateReason, and the two build records differ only in chipsetGeneration ('2' and '3') where the recording shows the generation repeated per build row.
Why: a client that drills from a failed version into its builds to find which target failed is told every target built.
Fix: the build-record writer in the service state must propagate the failure verdict and reason to every build row it materializes for the version.

=== Cause 5 of 6: GetMicrovmImageBuild reports a successful build and a snapshotBuild for a build that failed ===
Fixture lambdamicrovms-customer-image-lifecycle, step task#9.
AWS: GetMicrovmImageBuildOutput models buildState (@required), stateReason and the optional snapshotBuild. Recorded behavior: a FAILED build answers buildState=FAILED with the fixed stateReason 'The S3 bucket does not exist.' and carries NO snapshotBuild member - there is no memory snapshot to describe because the build never ran.
Emulator: buildState='SUCCESSFUL', stateReason absent, and snapshotBuild is present with a fabricated memorySnapshotSizeInBytes.
Why: the snapshot size is read by clients sizing MicroVM memory; publishing one for a build that produced nothing is a value invented out of nothing.
Fix: GetMicrovmImageBuild's serializer must omit snapshotBuild unless the build actually produced a snapshot, and the build record must carry the failure verdict from Cause 4.

=== Cause 6 of 6: RunMicrovm launches from an image that has no active version instead of refusing 404 ===
Fixture lambdamicrovms-image-tagging-and-run-refusals, step task#7.
AWS: RunMicrovm models ResourceNotFoundException (errorKind client, httpError 404, message @required, 'The specified resource does not exist.'). Recorded behavior when the image has no ACTIVE version to launch: HTTP 404, x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/', message 'No active version found for MicroVM image <imageArn>' - 'No active version found for MicroVM image ' is fixed text and <imageArn> is the image identifier resolved from the request, never a value copied from one recording.
Emulator: HTTP 200 with a launched MicroVM; no message and no x-amzn-errortype at all.
Why: the caller gets a MicroVM id for a VM that can never have been built from a usable image, so the error that should have stopped the deployment is replaced by a phantom success.
Fix: RunMicrovm's handler must pre-check that the resolved image has an ACTIVE version before launching and raise the modeled ResourceNotFoundException with that message template. This refusal is only reachable once Causes 1-3 let a version settle inactive.

Traces: lambdamicrovms-customer-image-lifecycle#task#3, #task#4, #task#7, #task#8, #task#9; lambdamicrovms-image-tagging-and-run-refusals#task#7.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 (score 56.8%, 21 divergence clusters). Expected = recorded AWS response, Actual = emulator.

Cause 1 (lambdamicrovms-customer-image-lifecycle#task#3, GetMicrovmImage, 200 vs 200):
  $.state: expected 'CREATE_FAILED', got 'CREATED'
  $.latestFailedImageVersion: expected '1.0', got (absent)
  $.latestActiveImageVersion: expected (absent), got '1.0'

Cause 2 (lambdamicrovms-customer-image-lifecycle#task#4, ListMicrovmImages, 200 vs 200):
  $.items[0].state: expected 'CREATE_FAILED', got 'CREATED'
  $.items[0].latestFailedImageVersion: expected '1.0', got (absent)
  $.items[0].latestActiveImageVersion: expected (absent), got '1.0'

Cause 3 (lambdamicrovms-customer-image-lifecycle#task#7, ListMicrovmImageVersions, 200 vs 200):
  $.items[0].state: expected 'FAILED', got 'SUCCESSFUL'
  $.items[0].status: expected 'INACTIVE', got 'ACTIVE'
  $.items[0].stateReason: expected 'The S3 bucket does not exist.', got (absent)
  $.items[0].versionStateTimeBucket: expected present, got (absent)

Cause 4 (lambdamicrovms-customer-image-lifecycle#task#8, ListMicrovmImageBuilds, 200 vs 200):
  $.items[*].buildState: expected 'FAILED', got 'SUCCESSFUL'
  $.items[*].stateReason: expected 'The S3 bucket does not exist.', got (absent)
  recorded rows repeat the same generation per build; emulator rows differ only by chipsetGeneration '2'/'3'

Cause 5 (lambdamicrovms-customer-image-lifecycle#task#9, GetMicrovmImageBuild, 200 vs 200):
  $.buildState: expected 'FAILED', got 'SUCCESSFUL'
  $.stateReason: expected 'The S3 bucket does not exist.', got (absent)
  $.snapshotBuild: expected (absent), got {'memorySnapshotSizeInBytes': 2147483648}

Cause 6 (lambdamicrovms-image-tagging-and-run-refusals#task#7, RunMicrovm):
  status: expected 404, got 200
  $.message: expected 'No active version found for MicroVM image <imageArn>', got (absent)
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/', got (absent)

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json): MicrovmImageState enum includes CREATE_FAILED; MicrovmImageVersionState includes FAILED; BuildState includes FAILED; MicrovmImageVersionSummary.status @required ACTIVE|INACTIVE; MicrovmImageVersionSummary.stateReason and MicrovmImageBuildSummary.stateReason documented as the reason for the state; GetMicrovmImageBuildOutput.snapshotBuild optional; RunMicrovm errors include ResourceNotFoundException (404). versionStateTimeBucket and terminationReasonCode are recorded-only members, not declared in the model.

## Steps to Reproduce
1. mvm = client('lambda-microvms'); base = ListManagedMicrovmImages()['items'][0]['imageArn'].
2. CreateMicrovmImage(name=<unique>, baseImageArn=base, buildRoleArn=<an IAM role arn>, codeArtifact={'uri': 's3://<unique>-absent/code.zip'}) - the bucket does not exist. Observe state='CREATING'.
3. Poll GetMicrovmImage(imageIdentifier=<imageArn>) until state leaves CREATING.
   AWS: state='CREATE_FAILED', latestFailedImageVersion='1.0', no latestActiveImageVersion. Emulator: state='CREATED', latestActiveImageVersion='1.0'. (Cause 1)
4. ListMicrovmImages() and read the item for that image - same three fields diverge. (Cause 2)
5. ListMicrovmImageVersions(imageIdentifier=<imageArn>).
   AWS: items[0].state='FAILED', status='INACTIVE', stateReason='The S3 bucket does not exist.', versionStateTimeBucket present. Emulator: 'SUCCESSFUL'/'ACTIVE', no stateReason. (Cause 3)
6. ListMicrovmImageBuilds(imageIdentifier=<imageArn>, imageVersion='1.0').
   AWS: every row buildState='FAILED' with the same stateReason. Emulator: every row 'SUCCESSFUL', no stateReason. (Cause 4)
7. GetMicrovmImageBuild(imageIdentifier=<imageArn>, imageVersion='1.0', buildId=<a row's buildId>).
   AWS: buildState='FAILED', stateReason set, no snapshotBuild. Emulator: 'SUCCESSFUL', snapshotBuild present. (Cause 5)
8. RunMicrovm(imageIdentifier=<imageArn>) on an image with no ACTIVE version.
   AWS: 404 ResourceNotFoundException, message 'No active version found for MicroVM image <imageArn>'. Emulator: 200 with a launched MicroVM. (Cause 6)
