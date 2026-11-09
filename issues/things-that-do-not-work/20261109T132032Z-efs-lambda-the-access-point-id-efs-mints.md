Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: efs__lambda.filesystemconfigs-mount-path-outside-mnt-and-missing-vpc-rejected
Issue-Key: efs__lambda.filesystemconfigs-mount-path-outside-mnt-and-missing-vpc-rejected|run#20|AssertionError|assert code[1] == (
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# efs__lambda: the access point id EFS mints has 16 hex digits, so CreateFunction adds a spurious fileSystemConfigs.N.member.arn validation error to its LocalMountPath refusal

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessPoint

## Description
Cause 1 of 1: an access point created by this emulator is unattachable to a Lambda function, and the extra refusal it provokes corrupts the message of an unrelated, legitimate FileSystemConfigs rejection.
AWS: AWS mints an EFS AccessPointId of 'fsap-' plus 17 hexadecimal digits; Smithy (efs) CreateAccessPoint.AccessPointId allows ^(arn:...|fsap-[0-9a-f]{8,40})$, while Lambda's CreateFunction validates FileSystemConfigs[].Arn against the narrower pattern the emulator itself quotes, 'arn:aws[a-zA-Z-]*:elasticfilesystem:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:access-point/fsap-[a-f0-9]{17}$|^arn:aws[-a-z]*:s3files:...'. So on AWS a CreateFunction whose only defect is a LocalMountPath outside /mnt/ is refused with exactly one error: ValidationException, message '1 validation error detected: Value '<path>' at 'fileSystemConfigs.1.member.localMountPath' failed to satisfy constraint: Member must satisfy regular expression pattern: /mnt/[a-zA-Z0-9-_.]+' (fixed text is AWS's standard validation wording; the variable parts are the path from the request, the member path index and the modeled pattern), and nothing is created.
Emulator: elasticfilesystem:CreateAccessPoint returns an AccessPointId with only 16 hex digits (recorded shape: fsap-f88bb8a8724c81ec — the digits are per-run, the 16-digit length is the defect), so the resulting ARN also violates Lambda's own modeled arn pattern. CreateFunction then answers ValidationException with '2 validation errors detected: Value '<access-point arn>' at 'fileSystemConfigs.1.member.arn' failed to satisfy constraint: Member must satisfy regular expression pattern: ...fsap-[a-f0-9]{17}$...; Value '<path>' at 'fileSystemConfigs.1.member.localMountPath' failed to satisfy constraint: Member must satisfy regular expression pattern: /mnt/[a-zA-Z0-9-_.]+'.
Why: EFS-backed Lambda is unusable end to end — every access point this emulator mints is permanently unattachable — and a client testing its own validation gets a refusal that blames an ARN the emulator generated, with a count and a message that do not match AWS's for that error class, so message/count assertions fail and the real defect in the request is obscured. The two services disagree about their own identifier format.
Fix: the EFS CreateAccessPoint handler's AccessPointId generator ('fsap-' plus 17 hex digits), so stored ids satisfy the access-point ARN pattern Lambda's FileSystemConfigs validation enforces.
Traces: efs__lambda.filesystemconfigs-mount-path-outside-mnt-and-missing-vpc-rejected. Related open issue 20261104T044826Z-efs-lambda-efs-createaccesspoint-mints-an-access-point-id-lambda.md reports the same id-format root cause from a different check and does not carry this check's replay key.

## Evidence
Expected (AWS): CreateFunction with FileSystemConfigs=[{'Arn': <valid fsap-<17 hex> access point arn>, 'LocalMountPath': '/data/sim2real'}] is refused with ValidationException and message "1 validation error detected: Value '/data/sim2real' at 'fileSystemConfigs.1.member.localMountPath' failed to satisfy constraint: Member must satisfy regular expression pattern: /mnt/[a-zA-Z0-9-_.]+".
Actual (emulator): AssertionError: unexpected message for a LocalMountPath outside /mnt/: "2 validation errors detected: Value 'arn:aws:elasticfilesystem:us-west-2:000000000000:access-point/fsap-f88bb8a8724c81ec' at 'fileSystemConfigs.1.member.arn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-zA-Z-]*:elasticfilesystem:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:access-point/fsap-[a-f0-9]{17}$|^arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}/access-point/fsap-[0-9a-f]{17,40}; Value '/data/sim2real' at 'fileSystemConfigs.1.member.localMountPath' failed to satisfy constraint: Member must satisfy regular expression pattern: /mnt/[a-zA-Z0-9-_.]+" — the access point id has 16 hex digits.

## Steps to Reproduce
1. CreateFileSystem, wait for available, CreateMountTarget in a subnet, then CreateAccessPoint with RootDirectory '/sim2real'. Note the returned AccessPointId: 'fsap-' plus 16 hex digits.
2. CreateFunction with VpcConfig set, FileSystemConfigs=[{'Arn': <that access point ARN>, 'LocalMountPath': '/data/sim2real'}].
3. Observe ValidationException reporting '2 validation errors detected' — one for fileSystemConfigs.1.member.arn against fsap-[a-f0-9]{17}, one for localMountPath — where AWS reports only the localMountPath error.
