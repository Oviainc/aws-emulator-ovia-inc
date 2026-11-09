Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iot.cfn-iot-job-template-duplicate-id-rejected
Issue-Key: cloudformation__iot|CreateJobTemplate|NotImplemented
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__iot: IoT CreateJobTemplate is an unimplemented scaffold stub, so the duplicate job-template-id refusal is unreachable

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJobTemplate

## Description
Cause 1 of 1: IoT CreateJobTemplate answers NotImplemented.
AWS: CreateJobTemplate returns HTTP 201 with jobTemplateArn and jobTemplateId; calling it twice with the same jobTemplateId is refused with ConflictException (the second call must fail, not the first).
Emulator: CreateJobTemplate answers the error code `NotImplemented` with `operation CreateJobTemplate is scaffolded but not implemented in this emulator` — a message AWS never sends for this operation.
Why: no IoT job template can be created at all, so the AWS::IoT::JobTemplate resource type cannot be provisioned and the duplicate-id conflict can never be observed.
Fix: the IoT CreateJobTemplate operation handler and its job-template state.
Traces: the failing check below.

## Evidence
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateJobTemplate operation: operation CreateJobTemplate is scaffolded but not implemented in this emulator. Expected HTTP 201 with jobTemplateId/jobTemplateArn on the first call and a ConflictException on the duplicate.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Call IoT CreateJobTemplate with a fresh jobTemplateId.
3. Observe the NotImplemented error.
