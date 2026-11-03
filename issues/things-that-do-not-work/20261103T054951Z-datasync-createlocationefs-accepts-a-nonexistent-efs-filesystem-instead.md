Issue-Source: trace-sweep-divergence
Issue-Trace: datasync-efs-create-location-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datasync-efs-create-location-validation
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: datasync|CreateLocationEfs|4xx->2xx|$.__type
Issue-Key: datasync|CreateLocationEfs|4xx->2xx|$.datasyncErrorCode
Issue-Key: datasync|CreateLocationEfs|4xx->2xx|$.errorCode
Issue-Key: datasync|CreateLocationEfs|4xx->2xx|$.message
Issue-Key: datasync|CreateLocationEfs|4xx->2xx|status
Issue-Commit: 39390cfa3000e408bc0d1db242197466d1e66569

# DataSync CreateLocationEfs accepts a nonexistent EFS filesystem instead of refusing with EfsFilesystemInvalid

> _Note: this issue was identified against the code base at commit `39390cfa3000e408bc0d1db242197466d1e66569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLocationEfs

## Description
1 cause.

Cause 1 of 1: CreateLocationEfs does not validate that the referenced Amazon EFS file system actually exists; it creates a location and returns 200 where AWS refuses the request.

AWS: CreateLocationEfs takes a required EfsFilesystemArn (Smithy: CreateLocationEfs.EfsFilesystemArn @required, @length max 128, @pattern ^arn:(aws|aws-cn|aws-us-gov|aws-eusc|aws-iso|aws-iso-b):elasticfilesystem:[a-z\-0-9]+:[0-9]{12}:file-system/fs-[0-9a-f]{8,40}$) and models InvalidRequestException (errorKind client, httpError 400, members message/errorCode/datasyncErrorCode). The recorded AWS response for a syntactically valid ARN naming a file system that does not exist in the caller's account/region is HTTP 400 with __type 'InvalidRequestException', errorCode 'EfsFilesystemInvalid', datasyncErrorCode 'EfsFilesystemInvalid' and message "Invalid EFS filesystem, ARN: <EfsFilesystemArn>, reason: EFS filesystem doesn't exist". In that message the fixed text is "Invalid EFS filesystem, ARN: ", ", reason: " and "EFS filesystem doesn't exist"; the only variable part is the EfsFilesystemArn echoed verbatim from the request (the recording shows arn:aws:elasticfilesystem:us-west-2:000000000000:file-system/fs-0b52a0f59862d10c7 — that value is an example filling the template, not part of it). Existence of the referenced resource is recorded behavior only: the model constrains the ARN's syntax, not its referent.

Emulator: returns HTTP 200 with a freshly minted LocationArn. All four error fields are absent (__type, errorCode, datasyncErrorCode, message all missing), i.e. the pattern check passes and no existence check runs at all.

Why: a client that probes for a bad or deleted file system gets a success and a location ARN that points at nothing. Error-path tests that assert on EfsFilesystemInvalid never fire, and downstream CreateTask/StartTaskExecution against that location proceed on state AWS would never have created, so a misconfiguration surfaces much later (or not at all) instead of at location-creation time.

Fix: in the CreateLocationEfs handler's request validation, after the ARN pattern check, resolve EfsFilesystemArn against the emulator's EFS file-system state (and, when supplied, AccessPointArn) and reject an unresolvable reference with InvalidRequestException/400 carrying errorCode = datasyncErrorCode = 'EfsFilesystemInvalid' and the message template above; the error serializer must populate both errorCode and datasyncErrorCode, not just message.

Traces: datasync-efs-create-location-validation#task#1.

Related open issues that do NOT cover this cause: '20261029T180920Z-datasync-accepts-three-requests-aws-refuses-unassumable-bucket.md' covers CreateLocationS3 role assumption, absent-location describes and self-referential CreateTask; '20261029T225203Z-datasync-azure-blob-location-operations-accept-requests-aws.md' covers the Azure Blob location operations; '20261029T180920Z-datasync-refusals-omit-errorcode-datasyncerrorcode-and-use-emulator.md' covers refusals that already return 400 but with wrong fields. None of them carries the CreateLocationEfs replay key, and this step returns 200 rather than a malformed 400.

## Evidence
Fixture datasync-efs-create-location-validation, step task#1, operation CreateLocationEfs.
Expected (recorded AWS): HTTP 400, body $.__type = 'InvalidRequestException', $.errorCode = 'EfsFilesystemInvalid', $.datasyncErrorCode = 'EfsFilesystemInvalid', $.message = "Invalid EFS filesystem, ARN: arn:aws:elasticfilesystem:us-west-2:000000000000:file-system/fs-0b52a0f59862d10c7, reason: EFS filesystem doesn't exist".
Actual (emulator): HTTP 200; $.__type missing, $.errorCode missing, $.datasyncErrorCode missing, $.message missing. Replay detail: "status: expected 400, got 200" plus all four body keys reported missing.
Smithy: CreateLocationEfs errors = [InternalException(500), InvalidRequestException(400)]; InvalidRequestException members = message, errorCode, datasyncErrorCode.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture datasync-efs-create-location-validation.
2. At step task#1 the recorded request calls DataSync CreateLocationEfs with a syntactically valid EfsFilesystemArn (elasticfilesystem file-system ARN) naming a file system that does not exist, plus the required Ec2Config.
3. Observe the emulator answers HTTP 200 with a LocationArn instead of HTTP 400 InvalidRequestException with errorCode/datasyncErrorCode 'EfsFilesystemInvalid' and the "Invalid EFS filesystem, ARN: <arn>, reason: EFS filesystem doesn't exist" message.
