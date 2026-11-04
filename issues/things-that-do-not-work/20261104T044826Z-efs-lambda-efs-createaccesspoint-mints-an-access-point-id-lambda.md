Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: efs__lambda.efs-access-point-confines-each-function-to-its-own-root-directory
Issue-Key: efs__lambda|CreateFunction|ValidationException
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# EFS CreateAccessPoint mints an access point id Lambda's own fileSystemConfigs pattern rejects

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessPoint

## Description
Cause 1 of 1: an access point created by this emulator cannot be attached to a Lambda, because the id it was given does not satisfy the access-point ARN pattern Lambda validates.
AWS: recorded behavior only for the id format — AWS mints an EFS AccessPointId of the form fsap- followed by 17 hexadecimal digits, which is exactly what Lambda's CreateFunction accepts: the emulator's own refusal quotes the modeled constraint on fileSystemConfigs[].arn, 'arn:aws[a-zA-Z-]*:elasticfilesystem:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:access-point/fsap-[a-f0-9]{17}$|...'. On AWS the CreateFunction call in this recording succeeds and the function mounts the access point.
Emulator: elasticfilesystem:CreateAccessPoint returns an AccessPointId with only 16 hex digits (recorded example: fsap-f88bb8a8724c81ec), so CreateFunction refuses the resulting ARN with ValidationException and message "1 validation error detected: Value '<arn>' at 'fileSystemConfigs.1.member.arn' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". The fixed text is AWS's standard validation-error wording; the variable parts are the ARN from the request, the member path, and the modeled pattern.
Why: EFS-backed Lambda is unusable end to end — every access point this emulator creates is permanently unattachable, and the error blames the caller's ARN rather than the id the emulator generated. The two services disagree about their own identifier format.
Fix: the EFS CreateAccessPoint handler's AccessPointId generator (fsap- plus 17 hex digits), so the ids it stores satisfy the access-point ARN pattern Lambda's fileSystemConfigs validation enforces.
Traces: efs__lambda.efs-access-point-confines-each-function-to-its-own-root-directory.

## Evidence
Expected (AWS): CreateAccessPoint returns AccessPointId fsap-<17 hex digits>, and CreateFunction with FileSystemConfigs naming that access point's ARN succeeds.
Actual (emulator): the access point id has 16 hex digits (e.g. fsap-f88bb8a8724c81ec) and CreateFunction fails with 'ValidationException: 1 validation error detected: Value \'arn:aws:elasticfilesystem:...:access-point/fsap-f88bb8a8724c81ec\' at \'fileSystemConfigs.1.member.arn\' failed to satisfy constraint: Member must satisfy regular expression pattern: ...fsap-[a-f0-9]{17}$...'.

## Steps to Reproduce
1. CreateFileSystem, then CreateAccessPoint with a RootDirectory. Note the returned AccessPointId length.
2. CreateFunction with FileSystemConfigs=[{'Arn': <that access point ARN>, 'LocalMountPath': '/mnt/data'}].
3. Observe ValidationException naming fileSystemConfigs.<n>.member.arn and the fsap-[a-f0-9]{17} pattern.
