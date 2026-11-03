Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-lsrno-traj-vpc-move-refused-then-env-fix
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-lsrno-traj-vpc-move-refused-then-env-fix lambda-lvau-traj-reserved-env-key-rejected lambda-lvau-traj-env-key-charset-rejected lambda-lvau-traj-wrong-function-identifier lambda-lvau-traj-env-over-4kb-rejected lambda-lvau-traj-qualified-update-then-republish
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->2xx|$.Type
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->2xx|$.message
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->2xx|status
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->4xx|$.message
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->4xx|status
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.LastUpdateStatus
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.LastUpdateStatusReason
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.LastUpdateStatusReasonCode
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.VpcConfig
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Architectures
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.CodeSha256
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.CodeSize
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Description
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Environment
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.EphemeralStorage
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.FunctionArn
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.FunctionName
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Handler
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.LastModified
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.LastUpdateStatus
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.LastUpdateStatusReason
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.LastUpdateStatusReasonCode
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.LoggingConfig
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.MemorySize
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.PackageType
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.RevisionId
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Role
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Runtime
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.RuntimeVersionConfig
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.SnapStart
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.State
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Timeout
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.TracingConfig
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Version
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|status
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.Environment.Variables.ANALYTICS_S3_REGION
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.Environment.Variables.NEW_TABLE_NAME
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.Environment.Variables.OLD_TABLE_NAME
Issue-Related: 20261102T111455Z-lambda-runtimeversionarn-carries-a-short-non-aws-identifier.md
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# Lambda UpdateFunctionConfiguration skips VPC and environment validation

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionConfiguration

## Description
UpdateFunctionConfiguration does not validate VpcConfig or Environment the way AWS does, and the function-update state machine never settles: the function stays in a Creating/InProgress update state, which makes the handler emit a spurious ResourceConflictException on the next legitimate update and makes it drop the environment the caller supplied. 10 causes, all in the UpdateFunctionConfiguration handler's validation and the function update state machine.

Cause 1 of 10 - a malformed security-group id is accepted instead of refused.
AWS: Smithy: UpdateFunctionConfiguration input VpcConfig; the operation declares InvalidParameterValueException (400). AWS resolves the VPC references through EC2 and surfaces EC2's refusal verbatim: HTTP 400, Type 'User', message 'Error occurred while DescribeSecurityGroups. EC2 Error Code: InvalidGroupId.Malformed. EC2 Error Message: Invalid id: "<security-group-id>"' - fixed text with the offending id from the request quoted at the end.
Emulator: answers HTTP 200 and applies the VpcConfig; no Type, no message, no x-amzn-errortype.
Why: a client that mistypes a security-group id gets a function it believes is attached to a VPC it is not attached to, and never learns the reference is invalid.
Fix: the UpdateFunctionConfiguration handler's VpcConfig validation.
Traces: lambda-lsrno-traj-vpc-move-refused-then-env-fix#task#4 (2 steps affected).

Cause 2 of 10 - a non-existent subnet is answered with a ResourceConflictException instead of the EC2-sourced 400.
AWS: same InvalidParameterValueException (400); for an unknown subnet the message is "Error occurred while DescribeSubnets. EC2 Error Code: InvalidSubnetID.NotFound. EC2 Error Message: The subnet ID '<subnet-id>' does not exist" - fixed text with the subnet id from the request quoted.
Emulator: answers HTTP 409 ResourceConflictException with 'The operation cannot be performed at this time. An update is in progress for resource: <function-arn>' - a wholly different error class whose cause is the stuck update state of cause 4.
Why: a client sees a retryable conflict for what is a permanent input error, so it retries forever instead of fixing the subnet id.
Fix: the UpdateFunctionConfiguration handler's VpcConfig validation, ahead of the update-in-progress guard.
Traces: lambda-lsrno-traj-vpc-move-refused-then-env-fix#task#5.

Cause 3 of 10 - the rejected VpcConfig is nevertheless persisted.
AWS: because the update in cause 1 is refused, the subsequent GetFunctionConfiguration reports no VpcConfig at all.
Emulator: reports VpcConfig with the invalid subnet and security-group ids, i.e. the refused update was committed.
Why: a client reading back the configuration concludes the invalid VPC move succeeded.
Fix: the UpdateFunctionConfiguration handler must validate before writing state.
Traces: lambda-lsrno-traj-vpc-move-refused-then-env-fix#task#6.

Cause 4 of 10 - the function never leaves the Creating update state, so LastUpdateStatus stays InProgress.
AWS: Smithy: FunctionConfiguration.LastUpdateStatus / LastUpdateStatusReason / LastUpdateStatusReasonCode. Once the create or update settles, AWS reports LastUpdateStatus='Successful' and leaves LastUpdateStatusReason and LastUpdateStatusReasonCode unset.
Emulator: reports LastUpdateStatus='InProgress' with LastUpdateStatusReason='The function is being created.' and LastUpdateStatusReasonCode='Creating' indefinitely.
Why: a client that waits for LastUpdateStatus=Successful before invoking or updating (the documented readiness gate, and what the function_updated waiter polls) never proceeds.
Fix: the function update state machine - the transition out of Creating/InProgress.
Traces: lambda-lsrno-traj-vpc-move-refused-then-env-fix#task#6, lambda-lvau-traj-env-key-charset-rejected#task#5.

Cause 5 of 10 - a legitimate update is refused 409 because of that stuck state.
AWS: the update succeeds with HTTP 200 and returns the full FunctionConfiguration (FunctionName, FunctionArn, Runtime, Role, Handler, Environment with the new variables, LoggingConfig, EphemeralStorage, SnapStart, Architectures and so on).
Emulator: answers HTTP 409 ResourceConflictException 'The operation cannot be performed at this time. An update is in progress for resource: <function-arn>' with an empty body, so none of the modelled output members are returned.
Why: every configuration change after the first is permanently rejected; the function is effectively frozen.
Fix: the update-in-progress guard in the UpdateFunctionConfiguration handler, together with cause 4's state machine.
Traces: lambda-lsrno-traj-vpc-move-refused-then-env-fix#task#7 (4 steps affected).

Cause 6 of 10 - the environment change requested in that update is lost.
AWS: the read-back reports the Environment.Variables the update supplied (and no VpcConfig, per cause 3).
Emulator: reports the pre-update value for the changed variable (and the VpcConfig from cause 3).
Why: a client that updates an environment variable and reads it back sees the old value with no error anywhere in the sequence.
Fix: as cause 5 - once the 409 stops being emitted the write must land; the environment merge in the UpdateFunctionConfiguration handler.
Traces: lambda-lsrno-traj-vpc-move-refused-then-env-fix#task#8.

Cause 7 of 10 - an environment variable name that violates the modelled key pattern is accepted.
AWS: Smithy: EnvironmentVariables is a map whose key EnvironmentVariableName carries @pattern ^[a-zA-Z]([a-zA-Z0-9_])+$. A key that does not match is refused with HTTP 400, x-amzn-errortype ValidationException and the message "1 validation error detected: Value at 'environment.variables' failed to satisfy constraint: Map keys must satisfy constraint: [Member must satisfy regular expression pattern: [a-zA-Z]([a-zA-Z0-9_])+]" - fixed text with the member path and the pattern as its variable parts.
Emulator: answers HTTP 200 with no message and no x-amzn-errortype.
Why: a client gets a function carrying an environment key the runtime cannot export, and no diagnostic pointing at the key.
Fix: the UpdateFunctionConfiguration handler's Environment validation.
Traces: lambda-lvau-traj-env-key-charset-rejected#task#4.

Cause 8 of 10 - a later environment write is silently dropped.
AWS: the read-back reports the variable the preceding successful update set.
Emulator: omits that variable from Environment.Variables entirely.
Why: the caller's configuration change vanishes with a 200 on the mutation and a 200 on the read.
Fix: the Environment merge in the UpdateFunctionConfiguration handler.
Traces: lambda-lvau-traj-env-key-charset-rejected#task#7.

Cause 9 of 10 - the environment-size refusal uses emulator wording and a different measurement.
AWS: InvalidParameterValueException (400) with the message 'Lambda was unable to configure your environment variables because the environment variables you have provided exceeded the 4KB limit. Measured size: <n> bytes' - fixed text; the limit phrase is AWS's 4KB and the measured size is computed from the request's own variables (the recording's number is this probe's payload, not part of the template).
Emulator: refuses with the correct 400 but the message "Environment.Variables must be smaller than 4096 bytes for all environment variables combined, and this request's are <n> bytes", and measures a different byte count for the same payload, so its accounting of the combined size differs from AWS's.
Why: a client that parses the measured size to trim variables to fit is given both the wrong sentence and the wrong number.
Fix: the UpdateFunctionConfiguration validation's environment-size check and its message.
Traces: lambda-lvau-traj-env-over-4kb-rejected#task#4 (3 steps affected).

Cause 10 of 10 - a reserved-key refusal path leaves the wrong environment value behind.
AWS: after the reserved-key update is refused, the read-back still reports the previously configured value for the variable and LastUpdateStatus='Successful' with no LastUpdateStatusReason/ReasonCode.
Emulator: reports the newer value for that variable (the refused update partially landed) plus the stuck InProgress/Creating state of cause 4.
Why: a client that relies on a refusal being atomic finds the function's environment mutated by a call that failed.
Fix: the UpdateFunctionConfiguration handler must validate reserved environment keys before writing state.
Traces: lambda-lvau-traj-reserved-env-key-rejected#task#5.

## Evidence
Cause 1: lambda-lsrno-traj-vpc-move-refused-then-env-fix#task#4 - expected 400 + $.Type='User' + $.message='Error occurred while DescribeSecurityGroups. EC2 Error Code: InvalidGroupId.Malformed. EC2 Error Message: Invalid id: "sg-..."'; got 200.
Cause 2: #task#5 - expected 400 + "Error occurred while DescribeSubnets. EC2 Error Code: InvalidSubnetID.NotFound. EC2 Error Message: The subnet ID 'subnet-...' does not exist"; got 409 ResourceConflictException 'The operation cannot be performed at this time. An update is in progress for resource: <arn>'.
Cause 3/4: #task#6 - $.VpcConfig expected absent, got {SubnetIds, SecurityGroupIds}; $.LastUpdateStatus expected 'Successful', got 'InProgress'; $.LastUpdateStatusReason expected absent, got 'The function is being created.'; $.LastUpdateStatusReasonCode expected absent, got 'Creating'.
Cause 5: #task#7 - expected 200 with the full FunctionConfiguration, got 409 with every modelled member missing (4 steps affected).
Cause 6: #task#8 - $.Environment.Variables.<key> expected the updated value, got the pre-update value; $.VpcConfig still present.
Cause 7: lambda-lvau-traj-env-key-charset-rejected#task#4 - expected 400 + x-amzn-errortype ValidationException + "1 validation error detected: Value at 'environment.variables' failed to satisfy constraint: Map keys must satisfy constraint: [Member must satisfy regular expression pattern: [a-zA-Z]([a-zA-Z0-9_])+]"; got 200.
Cause 8: #task#7 - $.Environment.Variables.<key> expected present, got absent.
Cause 9: lambda-lvau-traj-env-over-4kb-rejected#task#4 - 400 vs 400, $.message expected 'Lambda was unable to configure your environment variables because the environment variables you have provided exceeded the 4KB limit. Measured size: <n> bytes', got "Environment.Variables must be smaller than 4096 bytes ... and this request's are <n> bytes" with a different measured size (3 steps affected).
Cause 10: lambda-lvau-traj-reserved-env-key-rejected#task#5 - $.Environment.Variables.<key> expected the earlier value, got the newer one, plus the InProgress/Creating state.

## Steps to Reproduce
1. Replay lambda-lsrno-traj-vpc-move-refused-then-env-fix and walk task#4 through task#8: the malformed-security-group update, the unknown-subnet update, the read-back, the follow-up environment update and its read-back.
2. Replay lambda-lvau-traj-env-key-charset-rejected task#4, task#5 and task#7.
3. Replay lambda-lvau-traj-env-over-4kb-rejected task#4 and compare the refusal message and measured size.
4. Replay lambda-lvau-traj-reserved-env-key-rejected task#5 and compare the read-back environment.
