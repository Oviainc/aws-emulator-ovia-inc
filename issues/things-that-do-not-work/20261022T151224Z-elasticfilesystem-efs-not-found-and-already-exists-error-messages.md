Issue-Source: trace-sweep-divergence
Issue-Trace: efs-filesystem-lifecycle
Issue-Step: task#3
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: efs-filesystem-lifecycle efs-not-found-error-paths
Issue-Shape-Id: com.amazonaws.efs#MagnolioAPIService_v20150201
Issue-Key: elasticfilesystem|CreateFileSystem|4xx->4xx|$.Message
Issue-Key: elasticfilesystem|DescribeFileSystems|4xx->4xx|$.Message
Issue-Key: elasticfilesystem|DeleteFileSystem|4xx->4xx|$.Message
Issue-Key: elasticfilesystem|DescribeMountTargets|4xx->4xx|$.Message
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# EFS not-found and already-exists error messages are not built from the request the way AWS builds them

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFileSystem

## Description
EFS builds its FileSystemAlreadyExists and FileSystemNotFound messages from the request plus the identity of the resource involved; the emulator returns the correct status and error class but a differently constructed message, and in the 409 case drops the conflicting file-system id from the text altogether. Four causes, all on service elasticfilesystem (Smithy sdkId EFS, sigv4Name elasticfilesystem, api 2015-02-01). THIS ISSUE HAS 4 CAUSES; all four must be fixed before it is closed.

Cause 1 of 4 - CreateFileSystem with an already-used CreationToken: the 409 message does not name the existing file system (fixture efs-filesystem-lifecycle, step task#3).
AWS: Smithy error FileSystemAlreadyExists, errorKind client, httpError 409, members ErrorCode (required), Message, FileSystemId (required) - doc "Returned if the file system you are trying to create already exists, with the creation token you provided". The message rule is request-derived and evidenced by the recording: it names the id of the pre-existing file system and the CreationToken from the request, each in single quotes - recorded example "File system 'fs-e220a8397b1dcdaf' already exists with creation token 'sim2real-probe-efs-fs-lifecycle-93866a91'".
Emulator: status 409 and the error class match, but Message is 'File system already exists with CreationToken sim2real-probe-efs-fs-lifecycle-93866a91' - the conflicting file-system id is absent and the creation token is unquoted.
Why: the one piece of actionable data in this error is which file system already holds the token; a client (or an operator reading a log line) that would otherwise adopt or describe that file system cannot, because the only copy of the id in the human-readable message is gone. The generated id itself is environment-specific - the requirement is that the message names the existing file system's id and the requested creation token, not the recorded characters.
Fix: the CreateFileSystem conflict path's error construction - format the FileSystemAlreadyExists message from the existing record's FileSystemId and the request's CreationToken.
Traces: efs-filesystem-lifecycle#task#3.

Cause 2 of 4 - DescribeFileSystems for a nonexistent FileSystemId: FileSystemNotFound message construction differs (fixture efs-not-found-error-paths, step task#1).
AWS: Smithy error FileSystemNotFound, errorKind client, httpError 404, members ErrorCode (required), Message - doc "Returned if the specified FileSystemId value doesn't exist in the requester's Amazon Web Services account". The message is built from the request's FileSystemId: the id appears in single quotes and the sentence is terminated with a period - recorded example "File system 'fs-00000000' does not exist.".
Emulator: status 404 and the error class match; Message is 'File system fs-00000000 does not exist' - id unquoted, no terminating period.
Why: callers and test suites that match EFS not-found text (quoted-id extraction, exact-sentence assertions) key off that construction, so emulator responses fail string matching that passes against AWS even though the status and code are right.
Fix: the shared EFS FileSystemNotFound error construction used by the file-system lookup helper - render the requested FileSystemId in single quotes and terminate the sentence.
Traces: efs-not-found-error-paths#task#1.

Cause 3 of 4 - DeleteFileSystem for a nonexistent FileSystemId: same message construction divergence (fixture efs-not-found-error-paths, step task#2).
AWS: DeleteFileSystem (DELETE /2015-02-01/file-systems/{FileSystemId}, 204) models FileSystemNotFound at 404 with the same request-derived message rule as Cause 2 - recorded example "File system 'fs-00000000' does not exist.".
Emulator: 404 with 'File system fs-00000000 does not exist'.
Why: the same construction defect is reached through a second entry point, which shows the message is built per-handler rather than once; fixing only the describe path would leave delete diverging.
Fix: route DeleteFileSystem's not-found refusal through the single corrected FileSystemNotFound constructor from Cause 2.
Traces: efs-not-found-error-paths#task#2.

Cause 4 of 4 - DescribeMountTargets for a nonexistent FileSystemId: same message construction divergence (fixture efs-not-found-error-paths, step task#3).
AWS: DescribeMountTargets (GET /2015-02-01/mount-targets, 200) models FileSystemNotFound at 404; when the queried FileSystemId does not exist the message is built from it the same way - recorded example "File system 'fs-00000000' does not exist.".
Emulator: 404 with 'File system fs-00000000 does not exist'.
Why: completes the set - every EFS operation that resolves a FileSystemId emits the non-AWS construction, so a client cannot rely on EFS not-found text anywhere against the emulator.
Fix: same single FileSystemNotFound constructor; DescribeMountTargets' file-system resolution step must use it.
Traces: efs-not-found-error-paths#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, state_reset_failures=0), 4 divergence clusters, all verdict=mismatch, service=elasticfilesystem, no stub_path and no owned_by. HTTP status and error class match in all four; only the message construction diverges:

1) efs-filesystem-lifecycle#task#3 CreateFileSystem expected 409 / actual 409:
   $.Message: value_mismatch (expected "File system 'fs-e220a8397b1dcdaf' already exists with creation token 'sim2real-probe-efs-fs-lifecycle-93866a91'", got 'File system already exists with CreationToken sim2real-probe-efs-fs-lifecycle-93866a91')

2) efs-not-found-error-paths#task#1 DescribeFileSystems expected 404 / actual 404:
   $.Message: value_mismatch (expected "File system 'fs-00000000' does not exist.", got 'File system fs-00000000 does not exist')

3) efs-not-found-error-paths#task#2 DeleteFileSystem expected 404 / actual 404:
   $.Message: value_mismatch (expected "File system 'fs-00000000' does not exist.", got 'File system fs-00000000 does not exist')

4) efs-not-found-error-paths#task#3 DescribeMountTargets expected 404 / actual 404:
   $.Message: value_mismatch (expected "File system 'fs-00000000' does not exist.", got 'File system fs-00000000 does not exist')

Model consulted: services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json - FileSystemAlreadyExists (client, httpError 409, required members ErrorCode and FileSystemId), FileSystemNotFound (client, httpError 404), CreateFileSystem / DescribeFileSystems / DeleteFileSystem / DescribeMountTargets error lists. The requirement is the construction (the requested FileSystemId or CreationToken quoted into the sentence, and the existing file-system id named on the 409); the particular ids above are recorded examples.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 against it (replay_wire_corpus).
3. Representative step: fixture efs-filesystem-lifecycle, step task#3 - re-issue CreateFileSystem with the CreationToken already used at setup#1. The emulator answers 409 FileSystemAlreadyExists but its Message omits the existing file system's id, where the recording's message names both that id and the creation token in single quotes.
4. Replay efs-not-found-error-paths#task#1 (DescribeFileSystems?FileSystemId=fs-00000000), #task#2 (DELETE /2015-02-01/file-systems/fs-00000000) and #task#3 (DescribeMountTargets?FileSystemId=fs-00000000): each answers 404 FileSystemNotFound with 'File system fs-00000000 does not exist' instead of the recorded "File system 'fs-00000000' does not exist." construction.
5. Statuses and error classes match on all four steps, so the only remaining difference is the message built from the request.
