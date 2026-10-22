Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__s3.codebuild-artifact-upload-denied-without-s3-putobject-permission codebuild__s3.codebuild-build-uploads-its-declared-artifact-and-the-bytes-match codebuild__s3.codebuild-buildspec-finally-block-runs-after-a-failing-command-and-its-output-is-uploaded codebuild__s3.codebuild-four-concurrent-builds-write-four-distinct-artifacts-with-no-cross-talk codebuild__s3.codebuild-missing-declared-artifact-rejects-upload-and-no-object-lands codebuild__s3.codebuild-s3-cache-carries-a-file-from-one-build-into-the-next codebuild__s3.codebuild-secondary-artifacts-land-at-their-own-declared-locations codebuild__s3.codebuild-retry-build-reuses-the-pinned-source-version-after-the-object-changes
Issue-Key: codebuild__s3.codebuild-artifact-upload-denied-without-s3-putobject-permission|run#20|AssertionError|assert ok_build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild__s3.codebuild-build-uploads-its-declared-artifact-and-the-bytes-match|run#15|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild__s3.codebuild-buildspec-finally-block-runs-after-a-failing-command-and-its-output-is-uploaded|run#13|AssertionError|assert build["buildStatus"] == "FAILED", (
Issue-Key: codebuild__s3.codebuild-four-concurrent-builds-write-four-distinct-artifacts-with-no-cross-talk|run#20|AssertionError|assert statuses == {"SUCCEEDED"}, (
Issue-Key: codebuild__s3.codebuild-missing-declared-artifact-rejects-upload-and-no-object-lands|run#13|AssertionError|assert bad["buildStatus"] == "FAILED", (
Issue-Key: codebuild__s3.codebuild-s3-cache-carries-a-file-from-one-build-into-the-next|run#13|AssertionError|assert first["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild__s3.codebuild-secondary-artifacts-land-at-their-own-declared-locations|run#16|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild__s3.codebuild-retry-build-reuses-the-pinned-source-version-after-the-object-changes|_upload_source_zip#12|AssertionError|assert version and version != "null", (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeBuild/S3: builds FAULT before any phase runs so no artifact or cache object is ever produced, and PutObject returns no VersionId on a versioning-enabled bucket

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
Two distinct root causes found in the `codebuild__s3` cross-service suite (all 8 checks failed; none marked ALREADY FILED or OWNED).

Cause 1 of 2: builds never execute, so artifacts, secondary artifacts and S3 cache objects are never written.
AWS: a build walks the Smithy BuildPhaseType sequence (…INSTALL, PRE_BUILD, BUILD, POST_BUILD, UPLOAD_ARTIFACTS, FINALIZING, COMPLETED) and ends SUCCEEDED on exit 0 or FAILED on a non-zero command (Smithy StatusType); during UPLOAD_ARTIFACTS the declared primary and secondary artifacts are uploaded to their declared S3 locations, an S3 cache is written and restored on the next build, a `finally` block runs after a failing command, a missing declared artifact fails the build, and concurrent builds write their own distinct objects.
Emulator: every build ends buildStatus FAULT with phases [SUBMITTED SUCCEEDED, QUEUED SUCCEEDED, PROVISIONING FAULT, COMPLETED] and the context message 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'. No UPLOAD_ARTIFACTS phase exists and no object lands in S3.
Why: a client cannot obtain build output at all, and cannot distinguish a build that failed from one whose artifact was missing — AWS's FAULT means an infrastructure fault, not either of those.
Fix: the CodeBuild build state machine behind StartBuild and its artifact/cache upload step (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_build.rs and the build phase state read back by batch_get_builds.rs). Same underlying defect as the codebuild-service issue filed in this sweep.
Traces: codebuild__s3.codebuild-artifact-upload-denied-without-s3-putobject-permission, codebuild__s3.codebuild-build-uploads-its-declared-artifact-and-the-bytes-match, codebuild__s3.codebuild-buildspec-finally-block-runs-after-a-failing-command-and-its-output-is-uploaded, codebuild__s3.codebuild-four-concurrent-builds-write-four-distinct-artifacts-with-no-cross-talk, codebuild__s3.codebuild-missing-declared-artifact-rejects-upload-and-no-object-lands, codebuild__s3.codebuild-s3-cache-carries-a-file-from-one-build-into-the-next, codebuild__s3.codebuild-secondary-artifacts-land-at-their-own-declared-locations.

Cause 2 of 2: S3 PutObject returns no VersionId after PutBucketVersioning Status=Enabled.
AWS: once a bucket's versioning status is Enabled, every PutObject response carries an x-amz-version-id (PutObject VersionId), and the object's versions are addressable for a pinned source version.
Emulator: PutObject returns VersionId None for an object written to a bucket whose versioning was just set to Enabled, so the check cannot pin a source version at all.
Why: CodeBuild S3 sources (and any versioned-bucket workflow) pin a sourceVersion by object version id; with no version id the caller cannot pin, and a retry silently reads whatever bytes are current.
Fix: object-version assignment in the S3 service state (PutBucketVersioning must flip the bucket to versioning-enabled and PutObject must mint and return a version id).
Traces: codebuild__s3.codebuild-retry-build-reuses-the-pinned-source-version-after-the-object-changes.

## Evidence
Cause 1: "build did not succeed: 'FAULT' phases=[('SUBMITTED','SUCCEEDED'),('QUEUED','SUCCEEDED'),('PROVISIONING','FAULT'),('COMPLETED',None)] contexts=[None, None, [{'message': 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'}], None]"; "expected FAILED after a command exiting 7, got 'FAULT'"; "concurrent build statuses were ['FAULT']".
Cause 2: AssertionError: the source bucket must have versioning ENABLED before the first upload - PutObject returned VersionId None for s3://sim2real-behav-cb-src-<id>/source.zip.

## Steps to Reproduce
Boot the emulator from this checkout and replay the codebuild__s3 suite. Individually: (1) create a project with artifacts type S3 and a buildspec that writes a declared artifact file, StartBuild and poll BatchGetBuilds to terminal, then HeadObject the declared location; (2) CreateBucket, PutBucketVersioning Status=Enabled, PutObject and read the response's VersionId.
