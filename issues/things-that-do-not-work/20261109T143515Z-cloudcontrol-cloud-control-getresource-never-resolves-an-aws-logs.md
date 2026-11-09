Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__logs.native-log-group-delete-leaves-cloudcontrol-not-found
Issue-Key: cloudcontrol__logs.native-log-group-delete-leaves-cloudcontrol-not-found|_poll_until#8|AssertionError|raise AssertionError(
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# Cloud Control GetResource never resolves an AWS::Logs::LogGroup created through the native CloudWatch Logs API

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
Cause 1 of 1: the Cloud Control API only sees resources it created itself.
AWS: Cloud Control is a registry front end over the live resource, not a private inventory: GetResource(TypeName='AWS::Logs::LogGroup', Identifier=<name>) describes a log group created with logs:CreateLogGroup, and after the group is deleted natively the same call fails with ResourceNotFoundException (Smithy cloudcontrol GetResource errors include ResourceNotFoundException). The recorded run resolves the natively created group first and only then reports not-found.
Emulator: GetResource never resolves a log group it did not create — 'waited 30.0s for Cloud Control GetResource to resolve <group>, a log group it never created and it never happened; last observation None' — so the not-found-after-native-delete transition can never be observed.
Why: a client using Cloud Control as the uniform read path sees resources that plainly exist as absent, so drift detection and generic resource tooling built on it are wrong for every resource not created through Cloud Control.
Fix: the GetResource handler's resource resolution in the cloudcontrol crate (it must read the owning service's state rather than a Cloud-Control-only registry).
Traces: cloudcontrol__logs.native-log-group-delete-leaves-cloudcontrol-not-found.

## Evidence
Expected: GetResource describes the natively created log group, then answers ResourceNotFoundException after it is deleted. Actual: 'AssertionError: waited 30.0s for Cloud Control GetResource to resolve 'sim2real-behav-ccnat-<id>', a log group it never created and it never happened; last observation None'.

## Steps to Reproduce
Replay cloudcontrol__logs.native-log-group-delete-leaves-cloudcontrol-not-found: create a log group with logs:CreateLogGroup, poll cloudcontrol GetResource for AWS::Logs::LogGroup with that name, then delete the group natively.
