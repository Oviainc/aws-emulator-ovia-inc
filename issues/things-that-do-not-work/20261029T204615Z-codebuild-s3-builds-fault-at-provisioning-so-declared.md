Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__s3.codebuild-artifact-upload-denied-without-s3-putobject-permission codebuild__s3.codebuild-missing-declared-artifact-rejects-upload-and-no-object-lands codebuild__s3.codebuild-secondary-artifacts-land-at-their-own-declared-locations
Issue-Key: codebuild__s3.codebuild-artifact-upload-denied-without-s3-putobject-permission|run#20|AssertionError|assert ok_build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild__s3.codebuild-missing-declared-artifact-rejects-upload-and-no-object-lands|run#13|AssertionError|assert bad["buildStatus"] == "FAILED", (
Issue-Key: codebuild__s3.codebuild-secondary-artifacts-land-at-their-own-declared-locations|run#16|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# codebuild__s3: builds FAULT at PROVISIONING, so declared and secondary artifacts never land and a missing-artifact build never FAILS (1 cause)

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
ONE root cause, observed on three further CodeBuild/S3 artifact checks: the emulator never provisions a build container, so every build terminates FAULT in PROVISIONING and no buildspec, and therefore no artifact upload, ever runs.

Cause 1 of 1: StartBuild's build never leaves PROVISIONING, so the S3 artifact surface is unobservable.
AWS: Smithy StartBuild(projectName @required, artifactsOverride, secondaryArtifactsOverride @length 0-12, ...) -> {build}; Build.phases runs SUBMITTED, QUEUED, PROVISIONING, DOWNLOAD_SOURCE, ... UPLOAD_ARTIFACTS, COMPLETED, and Build.buildStatus/Build.artifacts/Build.secondaryArtifacts report the upload outcome. Recorded behavior: (a) with s3:PutObject granted the build reaches buildStatus SUCCEEDED and its declared artifact lands in the project's bucket; (b) a build whose buildspec declares a file that was never produced ends buildStatus FAILED (not FAULT) and NO object lands; (c) a project whose secondaryArtifacts artifactIdentifiers match the buildspec's artifacts/secondary-artifacts keys uploads each declared file set to its OWN S3 location and reports a BuildArtifacts entry per artifactIdentifier with its location and md5sum.
Emulator: every one of those builds ends buildStatus 'FAULT' with phases [('SUBMITTED','SUCCEEDED'), ('QUEUED','SUCCEEDED'), ('PROVISIONING','FAULT'), ('COMPLETED', None)] and the PROVISIONING phase context message 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'. No object is written to the bucket and no artifacts/secondaryArtifacts members are populated.
Why: FAULT is the wrong terminal status for all three scenarios — it is a service fault, so a client retries where AWS reports SUCCEEDED (a) or a deterministic client-side FAILED (b). The permission boundary in (a) is also unobservable: the permitted control build fails the same way as the denied one, so an emulator user cannot tell a missing s3:PutObject grant from an unrunnable build. And no artifact ever appears in S3, so artifact layout, secondary-artifact identifiers and md5sums cannot be verified at all.
Fix: build execution in the StartBuild/build-lifecycle component of the codebuild service crate — the build state machine that advances PROVISIONING and then runs the buildspec and the UPLOAD_ARTIFACTS phase against S3. Open issue 20261025T014438Z-codebuild-s3-builds-fault-at-provisioning-so-no.md and 20261029T072224Z-codebuild-s3-concurrently-started-builds-all-fault-in.md describe the same execution gap and own OTHER codebuild__s3 checks in this suite; neither carries a replay key for the three checks below, so they must be replayed here.
Traces: codebuild__s3.codebuild-artifact-upload-denied-without-s3-putobject-permission, codebuild__s3.codebuild-missing-declared-artifact-rejects-upload-and-no-object-lands, codebuild__s3.codebuild-secondary-artifacts-land-at-their-own-declared-locations.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout: codebuild__s3 0/8 passed (8 failed); the three below are the ones not already owned by an open issue.

1. codebuild__s3.codebuild-artifact-upload-denied-without-s3-putobject-permission: "the permitted control build was 'FAULT'; phases=[('SUBMITTED', 'SUCCEEDED'), ('QUEUED', 'SUCCEEDED'), ('PROVISIONING', 'FAULT'), ('COMPLETED', None)]" — expected SUCCEEDED for the role that DOES hold s3:PutObject.
2. codebuild__s3.codebuild-missing-declared-artifact-rejects-upload-and-no-object-lands: "expected FAILED for a missing declared artifact, got 'FAULT'; phases=[('SUBMITTED', 'SUCCEEDED'), ('QUEUED', 'SUCCEEDED'), ('PROVISIONING', 'FAULT'), ('COMPLETED', None)]".
3. codebuild__s3.codebuild-secondary-artifacts-land-at-their-own-declared-locations: "the build must succeed for its artifacts to be evidence; got 'FAULT'; phases=[('SUBMITTED', 'SUCCEEDED'), ('QUEUED', 'SUCCEEDED'), ('PROVISIONING', 'FAULT'), ('COMPLETED', None)]; contexts=[None, None, [{'message': 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'}], None]".

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — StartBuild (artifactsOverride ProjectArtifacts, secondaryArtifactsOverride ProjectArtifactsList @length 0-12) -> StartBuildOutput.build, errors AccountLimitExceededException / InvalidInputException / ResourceNotFoundException (client, httpError 400).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Create an S3 bucket and an IAM role CodeBuild can assume that grants logs:* plus s3:PutObject on that bucket.
3. CreateProject with source NO_SOURCE whose buildspec writes a file and declares it under artifacts/files, and artifacts of type S3 pointing at the bucket.
4. StartBuild and poll BatchGetBuilds to a terminal state — expect buildStatus SUCCEEDED and the declared file present in the bucket; observe buildStatus FAULT with PROVISIONING/FAULT and an empty bucket.
5. Repeat with a buildspec that declares a file it never creates — expect buildStatus FAILED and no object; observe FAULT.
6. Repeat with project secondaryArtifacts whose artifactIdentifiers match the buildspec's artifacts/secondary-artifacts keys — expect one BuildArtifacts entry per identifier with its own location and md5sum; observe FAULT and no secondaryArtifacts.
