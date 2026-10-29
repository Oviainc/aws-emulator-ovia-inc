Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__s3.codebuild-four-concurrent-builds-write-four-distinct-artifacts-with-no-cross-talk
Issue-Key: codebuild__s3.codebuild-four-concurrent-builds-write-four-distinct-artifacts-with-no-cross-talk|run#21|AssertionError|assert statuses == {"SUCCEEDED"}, (
Issue-Commit: 8fe1b88606c77dfa59d771b1a110885b6dfaed56

# codebuild__s3: concurrently started builds all FAULT in PROVISIONING and upload no artifacts (1 cause)

> _Note: this issue was identified against the code base at commit `8fe1b88606c77dfa59d771b1a110885b6dfaed56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
ONE root cause. 1 CAUSE must be closed before this issue is submitted.

NOTE ON PRIOR ART: the open issue 20261025T014438Z-codebuild-s3-builds-fault-at-provisioning-so-no.md owns four sibling codebuild__s3 checks with the same symptom, but it does NOT carry this check's replay key, so this failure is currently un-gated: a fixer closing that issue would never re-replay the concurrent-builds check. Filed here so the concurrency arm is gated too; if the same code change fixes both, close them together.

Cause 1 of 1: every build started by StartBuild reaches the PROVISIONING phase and ends with phaseStatus FAULT, so the build never runs its buildspec and never uploads its declared S3 artifact; this holds for four builds of one project started concurrently. Failing check: codebuild__s3.codebuild-four-concurrent-builds-write-four-distinct-artifacts-with-no-cross-talk.

AWS: recorded behavior only for the phase progression (the Smithy model types phases as a list of BuildPhase{phaseType, phaseStatus, ...} but documents no required sequence). Recorded: a build of a well-formed project walks SUBMITTED -> QUEUED -> PROVISIONING -> DOWNLOAD_SOURCE -> INSTALL -> PRE_BUILD -> BUILD -> POST_BUILD -> UPLOAD_ARTIFACTS -> FINALIZING -> COMPLETED with phaseStatus SUCCEEDED on each non-terminal phase, BatchGetBuilds reports buildStatus SUCCEEDED, and each build's declared artifact object exists in the artifacts bucket under that build's own key. FAULT is reserved for an internal failure of the build service; a project whose source and artifacts are valid must not produce it. With four builds of one project in flight at once, each build's artifact must be written under its own key with no cross-talk — i.e. concurrency must not be what makes the build fault.

Emulator: all four concurrent builds report the phase list [(SUBMITTED, SUCCEEDED), (QUEUED, SUCCEEDED), (PROVISIONING, FAULT), (COMPLETED, None)] and the set of build statuses is {"FAULT"} instead of {"SUCCEEDED"}. The build is accepted by StartBuild, is queued, and then faults in PROVISIONING without ever entering DOWNLOAD_SOURCE, so no buildspec command runs and no artifact object is ever created.

Why: no CodeBuild build can be executed at all. Any client that starts a build and waits for SUCCEEDED gets a terminal FAULT, and FAULT specifically signals "the build service broke", which pushes callers down retry/escalation paths rather than reporting a bad project or a bad buildspec. Everything downstream of a build — artifact publication, caches, finally-blocks, retries pinned to a source version — is unreachable, and the concurrency contract (four builds, four distinct artifacts, no cross-talk) cannot be observed.

Fix: the build execution pipeline behind the StartBuild handler and the build-phase state it feeds BatchGetBuilds in the codebuild crate — the PROVISIONING phase must complete SUCCEEDED and hand off to DOWNLOAD_SOURCE/INSTALL/BUILD/UPLOAD_ARTIFACTS, with per-build state keyed by build id so concurrent builds of one project each write their own artifact; a build whose project and source are valid must never terminate in FAULT.

Traces: codebuild__s3.codebuild-four-concurrent-builds-write-four-distinct-artifacts-with-no-cross-talk (assertion at run() line 253).

## Evidence
Expected (AWS): the four concurrently started builds all reach buildStatus SUCCEEDED, with each phase before COMPLETED reporting phaseStatus SUCCEEDED, and each build's declared artifact present in S3 under that build's own key.

Actual (emulator):
  [mismatch] codebuild__s3.codebuild-four-concurrent-builds-write-four-distinct-artifacts-with-no-cross-talk
  File "check_codebuild__s3_codebuild_four_concurrent_builds_write_four_distinct_artifacts_with_no_cross_talk.py", line 253, in run
    assert statuses == {"SUCCEEDED"}
  AssertionError: concurrent build statuses were ['FAULT']; phases={<build-id>: [('SUBMITTED', 'SUCCEEDED'), ('QUEUED', 'SUCCEEDED'), ('PROVISIONING', 'FAULT'), ('COMPLETED', None)], ... x4}
The four build ids in the message are per-run values; the requirement is the phase progression and the terminal status, not those ids. No phase after PROVISIONING other than COMPLETED is ever reported, so no buildspec command runs and no artifact is uploaded.

## Steps to Reproduce
1. Boot the emulator from mainline.
2. Create an S3 source bucket holding a zipped source with a buildspec that writes a file, and an artifacts bucket; create a CodeBuild project with type S3 source and type S3 artifacts and a service role.
3. Call StartBuild four times concurrently for that project (distinct artifact keys/name overrides).
4. Poll BatchGetBuilds for all four build ids: each reports phases [(SUBMITTED, SUCCEEDED), (QUEUED, SUCCEEDED), (PROVISIONING, FAULT), (COMPLETED, None)] and buildStatus FAULT, instead of walking through DOWNLOAD_SOURCE/INSTALL/BUILD/UPLOAD_ARTIFACTS to SUCCEEDED.
5. ListObjectsV2 on the artifacts bucket: none of the four artifacts exists.
6. Replay: codebuild__s3.codebuild-four-concurrent-builds-write-four-distinct-artifacts-with-no-cross-talk.
