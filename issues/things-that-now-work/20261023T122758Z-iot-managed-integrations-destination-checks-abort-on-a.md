Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-managed-integrations.destination-duplicate-name-conflict-rejected iot-managed-integrations.destination-lifecycle-and-update
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|CreateStream|NotImplemented
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# iot-managed-integrations Destination checks abort on a cross-service dependency: Kinesis CreateStream answers 501 NotImplemented, so CreateDestination is never exercised

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDestination

## Description
There is 1 cause in this issue.

Cause 1 of 1: the Kinesis `CreateStream` operation the iot-managed-integrations Destination surface depends on is an unimplemented scaffold stub, so both Destination checks abort before any iot-managed-integrations call is made.
AWS: `CreateDestination` requires a `DeliveryDestinationArn` naming a real delivery target of `DeliveryDestinationType="KINESIS"` in the SAME region as the service call (recorded behavior: a mismatched region is rejected with ValidationException, example message `Service region doesn't match region in DeliveryDestinationArn`). Setting that up requires Kinesis `CreateStream`, which in AWS returns an empty HTTP 200 and creates the stream. Recorded behavior for the iot-managed-integrations operations under test: a second CreateDestination reusing an existing `Name` is rejected with ConflictException and leaves the original destination untouched, while a distinct name still succeeds; and Create/Get/Update/DeleteDestination round-trip, persist field changes, and after delete the destination is both gone (independent GetDestination) and unusable (a mutation on the dead name also fails).
Emulator: the Kinesis `CreateStream` call answers HTTP 501 with error code `NotImplemented` and message `operation CreateStream is scaffolded but not implemented in this emulator`. Both checks raise on that call, so CreateDestination/GetDestination/UpdateDestination/DeleteDestination are never reached and their fidelity is entirely unverified.
Why: a client that builds an iot-managed-integrations Destination the way AWS requires — provision the Kinesis stream, then point CreateDestination at its ARN — cannot complete step one against this emulator, so the entire Destination/notification delivery surface is unreachable regardless of whether its own handlers are correct.
Fix: the Kinesis `CreateStream` handler (the kinesis service crate's ops module) must be implemented so the prerequisite resolves; once it does, the iot-managed-integrations Destination handlers (CreateDestination's duplicate-name conflict path and the Destination lifecycle/update state) need re-replay and likely implementation work of their own. Note: the open issue `Bootstrap the Kinesis emulator` (20261021T210901Z-bootstrap-service-com-amazonaws-kinesis-kinesis-20131202.md) covers standing Kinesis up and may subsume the prerequisite, but the replay report did not mark these two checks as owned by it and the iot-managed-integrations-side behavior above is not covered by any open issue.
Traces: iot-managed-integrations.destination-duplicate-name-conflict-rejected, iot-managed-integrations.destination-lifecycle-and-update.

## Evidence
Suite report (simparity-behavioral-aws-v1):

[error] iot-managed-integrations.destination-duplicate-name-conflict-rejected
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateStream operation: operation CreateStream is scaffolded but not implemented in this emulator

[error] iot-managed-integrations.destination-lifecycle-and-update
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateStream operation: operation CreateStream is scaffolded but not implemented in this emulator

Expected for the prerequisite: Kinesis CreateStream -> HTTP 200, empty body, stream created.
Actual: HTTP 501, error code NotImplemented.

Consequence (expected but unverified, because the checks never got there):
- destination-duplicate-name-conflict-rejected: second CreateDestination reusing an existing Name -> ConflictException; original destination untouched; distinct name still succeeds.
- destination-lifecycle-and-update: CreateDestination/GetDestination/UpdateDestination/DeleteDestination round-trip; GetDestination reflects DeliveryDestinationArn and updated Description; after delete the name is gone on an independent read and a mutation on it also fails.

Neither check reached a single iot-managed-integrations call.

## Steps to Reproduce
1. Boot the emulator.
2. With a kinesis client pointed at the emulator, call create_stream(StreamName="sim2real-stream-a-<suffix>", ShardCount=1). Expect HTTP 200 and a created stream; observe ClientError (NotImplemented), HTTP 501.
3. Because step 2 fails, the dependent flow cannot run. Once CreateStream answers, continue: create an IAM role trusted by iotmanagedintegrations.amazonaws.com, then with an iot-managed-integrations client in the same region call create_destination(DeliveryDestinationArn=<stream arn>, DeliveryDestinationType="KINESIS", Name=<name>, RoleArn=<role arn>, Description="initial description").
4. Re-call create_destination with the SAME Name and a different stream ARN; expect ConflictException and verify GetDestination still reports the original ARN.
5. Call update_destination (new Description), get_destination, delete_destination, then get_destination again (expect not-found) and a mutation on the deleted name (expect failure).
6. Replay the two checks named in trace_ids to confirm.
