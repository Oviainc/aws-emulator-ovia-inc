Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__s3.codebuild-missing-declared-artifact-rejects-upload-and-no-object-lands codebuild__s3.codebuild-buildspec-finally-block-runs-after-a-failing-command-and-its-output-is-uploaded
Issue-Key: codebuild__s3.codebuild-missing-declared-artifact-rejects-upload-and-no-object-lands|run#28|AssertionError|assert good["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild__s3.codebuild-buildspec-finally-block-runs-after-a-failing-command-and-its-output-is-uploaded|run#20|AssertionError|assert len(keys) == 1, (
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# codebuild__s3: buildspec commands run without a shell, so redirection and expansion exit 127 and no declared artifact is ever produced or uploaded (1 cause)

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
ONE net-new root cause behind both S3-artifact checks below. The open issues 20261025T014438Z-codebuild-s3-builds-fault-at-provisioning-so-no.md and 20261029T204615Z-codebuild-s3-builds-fault-at-provisioning-so-declared.md describe builds FAULTing at PROVISIONING; on this checkout these builds reach a BUILD verdict and the remaining defect is the command execution itself, for which neither issue carries a replay key.

Cause 1 of 1: buildspec commands are not executed through a shell, so every command that uses redirection, parameter expansion or command substitution fails with exit status 127 and the files the artifact declaration names are never created — the build FAILS and the artifact bucket stays empty.
AWS: recorded behavior only for the execution mechanism. AWS runs each entry of a phase's `commands` list through a POSIX shell inside the build container, so `echo marker >> out.txt` and `printf '%s' "$VAR" > file` create files, and `$VAR` resolves to the build's environment variable (including one supplied through environmentVariablesOverride). A phase whose command exits non-zero records a PhaseContext (Smithy BuildPhase.contexts -> PhaseContext{statusCode, message}) 'Error while executing command: <command as written>. Reason: exit status <N>' with N the status the container produced; `finally` commands still run after a failing command in the same phase; and UPLOAD_ARTIFACTS then packages the declared `artifacts/files` to the configured S3 location, so a build whose commands wrote the declared file leaves exactly one object there and reports its location (and md5sum) on the Build record.
Emulator: the command string is handed to the OS without a shell, so `printf '%s' "$SIM2REAL_MARKER" > never-written.txt` and `echo sim2real-pre-marker >> out.txt` fail immediately with 'Reason: exit status 127' (command not found — the literal string was looked up as an executable). The BUILD phase is FAILED, the declared file never exists, and the S3 artifact bucket holds no objects at all; a build that must SUCCEED and upload one object instead FAILS with an empty bucket.
Why: the whole S3 artifact contract is unreachable — a client that declares artifacts gets no object, no artifacts.location and no md5sum, and the phase context reports 127 rather than the status its command produced, so the failure looks like a missing executable rather than the real build outcome. A consumer polling the bucket for the build's output waits forever.
Fix: the build command runner in the codebuild build-execution component (services/service-com-amazonaws-codebuild-codebuild-20161006) — execute each buildspec command through a shell (sh -c) with the build's environment exported, record the real wait status in the PhaseContext message, and let UPLOAD_ARTIFACTS package the files the commands produced. The same root cause is filed for the plain-codebuild checks in the companion issue titled 'CodeBuild: buildspec commands are not run through a shell ... (4 causes)'.
Traces: codebuild__s3.codebuild-missing-declared-artifact-rejects-upload-and-no-object-lands, codebuild__s3.codebuild-buildspec-finally-block-runs-after-a-failing-command-and-its-output-is-uploaded.

## Evidence
Replay of the behavioral suite (simparity-behavioral-aws-v1) against the emulator built from this checkout; per-service line: codebuild__s3 0/8 (0%), 8 failed (the other six are owned by existing issues).

codebuild__s3.codebuild-missing-declared-artifact-rejects-upload-and-no-object-lands [mismatch]: AssertionError 'the control build must SUCCEED on the same project, got \'FAILED\'; phases=[(SUBMITTED, SUCCEEDED), (QUEUED, SUCCEEDED), (PROVISIONING, SUCCEEDED), (DOWNLOAD_SOURCE, SUCCEEDED), (BUILD, FAILED), (COMPLETED, None)]'. The control buildspec's only extra command is `printf '%s' "$SIM2REAL_MARKER" > never-written.txt`, with SIM2REAL_MARKER supplied via environmentVariablesOverride; expected SUCCEEDED and exactly one object under the build's namespace in the S3 artifact bucket.
codebuild__s3.codebuild-buildspec-finally-block-runs-after-a-failing-command-and-its-output-is-uploaded [mismatch]: AssertionError 'a FAILED build whose finally block wrote the declared artifact should still leave exactly one object, bucket holds []'. Its buildspec is `echo sim2real-pre-marker >> out.txt`, `exit 7`, finally `echo sim2real-finally-marker >> out.txt`, artifacts files [out.txt] to an S3 location; expected one object containing both markers in order, observed an empty bucket.
The shared mechanism is visible verbatim in a sibling codebuild__s3 check's phase context on this same run: 'COMMAND_EXECUTION_ERROR Error while executing command: export SIM2REAL_PAYLOAD="$(cat payload.txt)". Reason: exit status 127'.

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — BuildPhase{phaseType, phaseStatus, contexts}; PhaseContext{statusCode, message}; StatusType enum [SUCCEEDED, FAILED, FAULT, TIMED_OUT, IN_PROGRESS, STOPPED].

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout. Create an S3 bucket and a CodeBuild service role allowed s3:PutObject/GetObject/ListBucket on it.
2. CreateProject with source NO_SOURCE and artifacts {type: 'S3', location: <bucket>, path: 'out', name: 'build', packaging: 'NONE'}.
3. StartBuild with buildspecOverride whose build commands are `echo sim2real-build-phase-ran` and `printf '%s' "$SIM2REAL_MARKER" > never-written.txt`, artifacts files [never-written.txt], and environmentVariablesOverride SIM2REAL_MARKER=<marker>.
4. Expect buildStatus SUCCEEDED and exactly one object in the bucket whose bytes are <marker>; observe BUILD FAILED with a PhaseContext 'Reason: exit status 127' and an empty bucket.
5. Repeat with a buildspec using `echo marker >> out.txt`, `exit 7` and a `finally` block appending a second marker, artifacts files [out.txt]: expect a FAILED build that still uploads one object containing both markers; observe an empty bucket.
6. Re-replay the behavioral checks listed in trace_ids to confirm.
