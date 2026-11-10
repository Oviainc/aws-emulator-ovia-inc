Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm__sts.parameter-last-modified-user-is-the-live-sts-session-identity
Issue-Key: ssm__sts|DescribeParameters|NotImplemented
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# SSM DescribeParameters is an unimplemented scaffold stub, so parameter metadata (LastModifiedUser) can never be read back

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeParameters

## Description
ONE root cause (cause 1 of 1).

AWS: DescribeParameters is a served SSM operation; it returns a Parameters list of ParameterMetadata, each carrying Name, Type, LastModifiedDate, LastModifiedUser ("the Amazon Resource Name (ARN) of the Amazon Web Services user who last changed the parameter") and the rest of the metadata, HTTP 200.
Emulator: every call is refused with error code NotImplemented, message "operation DescribeParameters is scaffolded but not implemented in this emulator" — so no parameter metadata is observable and nothing about the recorded writer identity can be read back.
Why: a client listing or filtering parameters, or auditing who last wrote one, gets a 501-class refusal for an operation AWS serves; the check that would settle what LastModifiedUser contains cannot run at all.
Fix: implement the DescribeParameters handler in the SSM service crate (populating ParameterMetadata, including LastModifiedUser, from the parameter state the Put path already records).
Traces: ssm__sts.parameter-last-modified-user-is-the-live-sts-session-identity. Note: the replay report did not mark this failure as already filed, and no open issue names SSM DescribeParameters.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1, check ssm__sts.parameter-last-modified-user-is-the-live-sts-session-identity:

"botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeParameters operation: operation DescribeParameters is scaffolded but not implemented in this emulator"

Expected: HTTP 200 with a Parameters list of ParameterMetadata for the parameters the check just wrote.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay ssm__sts.parameter-last-modified-user-is-the-live-sts-session-identity.
3. Manual repro: ssm.put_parameter(Name='/sim2real/probe', Value='v', Type='String'), then ssm.describe_parameters() — the emulator answers NotImplemented.
