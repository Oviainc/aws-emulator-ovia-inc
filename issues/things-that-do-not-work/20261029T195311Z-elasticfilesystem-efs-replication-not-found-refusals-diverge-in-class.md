Issue-Source: trace-sweep-divergence
Issue-Trace: efs-mount-target-replication-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: efs-mount-target-replication-error-paths efs-replication-configuration-workflow
Issue-Shape-Id: com.amazonaws.efs#MagnolioAPIService_v20150201
Issue-Key: elasticfilesystem|DescribeReplicationConfigurations|4xx->4xx|$.ErrorCode
Issue-Key: elasticfilesystem|DescribeReplicationConfigurations|4xx->4xx|$.Message
Issue-Key: elasticfilesystem|DescribeReplicationConfigurations|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: elasticfilesystem|CreateReplicationConfiguration|4xx->4xx|$.ErrorCode
Issue-Key: elasticfilesystem|DeleteReplicationConfiguration|4xx->4xx|$.ErrorCode
Issue-Key: elasticfilesystem|DeleteReplicationConfiguration|4xx->4xx|$.Message
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# EFS replication not-found refusals diverge in class, message and ErrorCode

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeReplicationConfigurations

## Description
Every not-found refusal on the EFS replication surface is composed by the emulator rather than reproduced: the wrong error class is chosen, the messages are emulator-authored prose, and an ErrorCode member is added to bodies the recording serves without one. Four causes, all on the replication-configuration operations.

Cause 1 of 4: DescribeReplicationConfigurations answers FileSystemNotFound where AWS answers ReplicationNotFound (fixture efs-mount-target-replication-error-paths, step task#4).
AWS: Smithy: DescribeReplicationConfigurations, GET /2015-02-01/file-systems/replication-configurations with an optional FileSystemId httpQuery, errors BadRequest(400), FileSystemNotFound(404), ReplicationNotFound(404), ValidationException(400), InternalServerError(500); ReplicationNotFound is errorKind client, httpError 404, members ErrorCode (optional) + Message, documented "Returned if the specified file system does not have a replication configuration." The recording shows that filtering on a FileSystemId for which no replication exists - including an id that resolves to no file system at all - is answered 404 with ErrorCode "ReplicationNotFound" and Message "No replications found.", whose text is entirely fixed: there is no variable part interpolated from the request.
Emulator: answers 404 but with ErrorCode "FileSystemNotFound", Message "File system '<FileSystemId>' does not exist." and x-amzn-errortype: FileSystemNotFound.
Why: a client's `except ReplicationNotFound` branch - the normal way to probe whether a file system is replicated - never fires, and the client is instead told the file system does not exist, which is a different and misleading condition; code that treats FileSystemNotFound as fatal aborts where AWS lets it proceed.
Fix: the DescribeReplicationConfigurations handler in the efs service crate - this read never raises FileSystemNotFound; a FileSystemId filter that matches no replication is ReplicationNotFound with the fixed sentence.
Traces: efs-mount-target-replication-error-paths#task#4.

Cause 2 of 4: the ReplicationNotFound message on DescribeReplicationConfigurations is emulator-authored (fixture efs-replication-configuration-workflow, step task#4).
AWS: as above, the ReplicationNotFound message for this operation is the fixed sentence "No replications found." - all fixed text, nothing taken from the request or from state.
Emulator: picks the right error class here but words it "File system '<FileSystemId>' does not have a replication configuration.", interpolating the requested file-system id into a sentence AWS does not use.
Why: a client that matches on the refusal text (log assertions, user-facing error surfacing) sees a sentence that never appears in AWS, and the message leaks a request value where AWS's is value-free.
Fix: the DescribeReplicationConfigurations error construction in the efs service crate - emit the fixed sentence verbatim.
Traces: efs-replication-configuration-workflow#task#4.

Cause 3 of 4: CreateReplicationConfiguration's not-found body carries an ErrorCode member the recording does not (fixture efs-mount-target-replication-error-paths, step task#3).
AWS: recorded behavior only. Smithy models CreateReplicationConfiguration with SourceFileSystemId as an httpLabel and errors including FileSystemNotFound(404) and ReplicationNotFound(404); the model marks FileSystemNotFound.ErrorCode @required, but the recorded 404 for a source file system that does not exist has NO ErrorCode member in the body at all - only Message (which the emulator matches) - with the error shape carried by the x-amzn-errortype header. The status (404) and the shape named in the header are the requirement; the body member is absent.
Emulator: adds "ErrorCode": "FileSystemNotFound" to that body.
Why: a client that round-trips or asserts on the error body of this operation sees a member AWS omits; SDK deserialization of a required member that AWS leaves out is exactly the asymmetry a fixture records, and code branching on body.ErrorCode behaves differently against the emulator than against AWS.
Fix: the error serializer for the replication operations in the efs service crate - do not synthesize an ErrorCode member for these bodies.
Traces: efs-mount-target-replication-error-paths#task#3.

Cause 4 of 4: DeleteReplicationConfiguration adds an ErrorCode member and rewords the ReplicationNotFound message (fixture efs-mount-target-replication-error-paths, step task#5).
AWS: Smithy: DeleteReplicationConfiguration, DELETE /2015-02-01/file-systems/{SourceFileSystemId}/replication-configuration, success 204, errors BadRequest(400), FileSystemNotFound(404), ReplicationNotFound(404), InternalServerError(500). The recorded refusal for a file system that exists but has no replication configuration is 404 with NO ErrorCode member in the body and Message "Cannot find a replication configuration for file system <SourceFileSystemId>." - the fixed text is "Cannot find a replication configuration for file system " plus the SourceFileSystemId taken from the request path, then a period.
Emulator: answers 404 with "ErrorCode": "ReplicationNotFound" present and Message "File system '<id>' does not have a replication configuration." - a sentence of its own, with the id quoted, which AWS does not use.
Why: the delete-if-missing path is the one clients retry or ignore; both the extra body member and the reworded sentence break text-based handling, and the message no longer matches the operation AWS reports it for.
Fix: the DeleteReplicationConfiguration error construction and serializer in the efs service crate - emit AWS's sentence with the request's SourceFileSystemId interpolated, and no ErrorCode member.
Traces: efs-mount-target-replication-error-paths#task#5.

No open issue covers these: 20261022T151224Z-elasticfilesystem-efs-not-found-and-already-exists-error-messages is resolved and concerned file-system/access-point not-found texts, not the replication surface; 20261022T154034Z-efs-control-plane-backup-policy-file-system-policy covers backup-policy/file-system-policy 501s.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (8 divergence clusters, all elasticfilesystem):

Cause 1 - efs-mount-target-replication-error-paths#task#4, DescribeReplicationConfigurations, status 404 both sides: $.ErrorCode value_mismatch (expected 'ReplicationNotFound', got 'FileSystemNotFound'); $.Message value_mismatch (expected 'No replications found.', got "File system 'fs-00000000' does not exist."); $headers.x-amzn-errortype shape_mismatch (got 'FileSystemNotFound').

Cause 2 - efs-replication-configuration-workflow#task#4, DescribeReplicationConfigurations, status 404 both sides: $.Message value_mismatch (expected 'No replications found.', got "File system 'fs-e220a8397b1dcdaf' does not have a replication configuration."); the error class matches here.

Cause 3 - efs-mount-target-replication-error-paths#task#3, CreateReplicationConfiguration, status 404 both sides: $.ErrorCode type_mismatch - expected absent, got 'FileSystemNotFound'.

Cause 4 - efs-mount-target-replication-error-paths#task#5, DeleteReplicationConfiguration, status 404 both sides: $.ErrorCode type_mismatch - expected absent, got 'ReplicationNotFound'; $.Message value_mismatch (expected 'Cannot find a replication configuration for file system fs-e220a8397b1dcdaf.', got "File system 'fs-e220a8397b1dcdaf' does not have a replication configuration.").

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture efs-mount-target-replication-error-paths step task#4 - GET /2015-02-01/file-systems/replication-configurations?FileSystemId=<id that matches no replication>; expect 404 ErrorCode 'ReplicationNotFound' / Message 'No replications found.' and x-amzn-errortype naming ReplicationNotFound, observe FileSystemNotFound with a "does not exist" sentence.
4. Cause 2: replay fixture efs-replication-configuration-workflow step task#4 - same query for an existing file system that has no replication; expect Message 'No replications found.', observe the emulator's own sentence.
5. Cause 3: replay fixture efs-mount-target-replication-error-paths step task#3 - POST /2015-02-01/file-systems/<nonexistent fs>/replication-configuration; expect a 404 body with Message only and no ErrorCode member, observe an added ErrorCode.
6. Cause 4: replay the same fixture step task#5 - DELETE /2015-02-01/file-systems/<fs with no replication>/replication-configuration; expect 404 with no ErrorCode member and Message 'Cannot find a replication configuration for file system <FileSystemId>.', observe an ErrorCode member and a reworded message.
