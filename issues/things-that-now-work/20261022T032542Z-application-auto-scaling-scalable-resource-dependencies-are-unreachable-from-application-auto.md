Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.deregister-in-one-region-keeps-the-same-named-target-in-another application-autoscaling.scheduled-action-in-a-second-region-scales-only-that-regions-table application-autoscaling.deregister-index-target-leaves-the-table-policy-and-alarms application-autoscaling.gsi-scalable-target-moves-the-index-not-the-table application-autoscaling.register-scalable-target-capacity-clamped-on-max-decrease application-autoscaling.dynamodb-predefined-metric-on-a-keyspaces-dimension-rejected application-autoscaling.keyspaces-missing-table-in-an-existing-keyspace-rejected application-autoscaling.keyspaces-on-demand-table-register-rejected application-autoscaling.keyspaces-registration-floor-raises-the-table-write-capacity application-autoscaling.keyspaces-scheduled-action-moves-the-table-read-capacity application-autoscaling.keyspaces-table-auto-scaling-specification-registers-the-targets-and-policies application-autoscaling.keyspaces-target-tracking-alarms-watch-the-table-metrics application-autoscaling.ecs-registration-floor-sets-the-desired-count-with-no-capacity-to-run-it
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling|CreateTable|ResourceInUseException
Issue-Key: application-autoscaling|CreateTable|NotImplemented
Issue-Key: application-autoscaling|UpdateTable|NotImplemented
Issue-Key: keyspaces|CreateKeyspace|AmbiguousService
Issue-Key: application-autoscaling|CreateService|NotImplemented
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# Scalable-resource dependencies are unreachable from Application Auto Scaling checks: DynamoDB state is not region-partitioned, CreateTable+GSI and UpdateTable are stubs, two services both claim 'cassandra', and ECS CreateService is a stub

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterScalableTarget

## Description
Five distinct root causes, each one blocking Application Auto Scaling behavioral checks before the Auto Scaling assertion is ever reached. They are defects of the resources Application Auto Scaling scales (DynamoDB tables and indexes, Keyspaces tables, ECS services) and of the emulator's service routing; they are filed under the `application-auto-scaling` suite because that is the suite whose checks they break.

Cause 1 of 5: DynamoDB table state is not partitioned by region.
AWS: table names are per-region; CreateTable for the SAME name in a second region succeeds and yields an independent table (recorded behavior; this is the premise of the two region-isolation checks, which then assert that deregistering/scheduling in one region leaves the other region's target untouched).
Emulator: the second region's CreateTable is refused with HTTP 400 `ResourceInUseException` 'Table already exists: <name>' — the same table object is visible in both regions.
Why: a client that provisions the same stack in two regions gets a spurious conflict, and any per-region isolation claim (targets, scheduled actions, capacity) cannot hold because the underlying resource is global.
Fix: the DynamoDB emulator's table state keying — the store must be partitioned by region (and account) rather than by table name alone.
Traces: application-auto-scaling.deregister-in-one-region-keeps-the-same-named-target-in-another, application-auto-scaling.scheduled-action-in-a-second-region-scales-only-that-regions-table.

Cause 2 of 5: DynamoDB CreateTable with GlobalSecondaryIndexes is unimplemented.
AWS: CreateTable accepts GlobalSecondaryIndexes and the index becomes a scalable resource (`table/<t>/index/<i>` with dimension dynamodb:index:ReadCapacityUnits).
Emulator: HTTP 501-class answer, error code `NotImplemented`, message 'operation CreateTable with GlobalSecondaryIndexes is scaffolded but not implemented in this emulator'.
Why: the entire GSI scalable-dimension surface of Application Auto Scaling is untestable — no index target can be registered, scaled or deregistered.
Fix: the DynamoDB CreateTable handler's GlobalSecondaryIndexes path and the index state it must create.
Traces: application-auto-scaling.deregister-index-target-leaves-the-table-policy-and-alarms, application-auto-scaling.gsi-scalable-target-moves-the-index-not-the-table.

Cause 3 of 5: DynamoDB UpdateTable is unimplemented.
AWS: UpdateTable changes ProvisionedThroughput, which is how Application Auto Scaling moves DynamoDB capacity and how a client observes a clamped capacity after a MaxCapacity decrease.
Emulator: error code `NotImplemented`, 'operation UpdateTable is scaffolded but not implemented in this emulator'.
Why: no DynamoDB capacity can ever change, so every capacity-movement assertion against the emulator is unreachable.
Fix: implement the DynamoDB UpdateTable operation handler and the provisioned-throughput state it mutates.
Traces: application-auto-scaling.register-scalable-target-capacity-clamped-on-max-decrease.

Cause 4 of 5: two emulated services both claim the 'cassandra' signing name, so Keyspaces is unreachable.
AWS: Keyspaces (sigv4 name 'cassandra') answers CreateKeyspace/CreateTable and its tables are scalable resources for the `cassandra` namespace.
Emulator: every CreateKeyspace call is refused after 3 retries with error code `AmbiguousService`: '2 emulated services all claim requests for \'cassandra\': cassandra, cassandra; the request is rejected rather than served by an arbitrary one'. The emulator announced two services for one signing name.
Why: the whole Keyspaces namespace of Application Auto Scaling (7 checks) is unusable — no client can reach Keyspaces at all.
Fix: the service-routing/registration table that maps a sigv4 signing name to one emulated service — de-duplicate the two registrations claiming 'cassandra'.
Traces: application-auto-scaling.dynamodb-predefined-metric-on-a-keyspaces-dimension-rejected, application-auto-scaling.keyspaces-missing-table-in-an-existing-keyspace-rejected, application-auto-scaling.keyspaces-on-demand-table-register-rejected, application-auto-scaling.keyspaces-registration-floor-raises-the-table-write-capacity, application-auto-scaling.keyspaces-scheduled-action-moves-the-table-read-capacity, application-auto-scaling.keyspaces-table-auto-scaling-specification-registers-the-targets-and-policies, application-auto-scaling.keyspaces-target-tracking-alarms-watch-the-table-metrics.

Cause 5 of 5: ECS CreateService is unimplemented.
AWS: CreateService returns the service with its desiredCount; `service/<cluster>/<service>` with dimension ecs:service:DesiredCount is the canonical ECS scalable resource.
Emulator: error code `NotImplemented`, 'operation CreateService is scaffolded but not implemented in this emulator'.
Why: no ECS scalable target can be created, so the ECS half of Application Auto Scaling cannot be exercised.
Fix: implement the ECS CreateService operation handler (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs) and the service/desiredCount state it owns.
Traces: application-auto-scaling.ecs-registration-floor-sets-the-desired-count-with-no-capacity-to-run-it.

## Evidence
Verbatim replay errors (suite simparity-behavioral-aws-v1):
- `botocore.errorfactory.ResourceInUseException: An error occurred (ResourceInUseException) when calling the CreateTable operation: Table already exists: sim2real-behav-aas-rgn-21b7cfe26a21` — raised for a CreateTable in the SECOND region with the same name; AWS creates an independent table.
- `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateTable operation: operation CreateTable with GlobalSecondaryIndexes is scaffolded but not implemented in this emulator`.
- `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateTable operation: operation UpdateTable is scaffolded but not implemented in this emulator`.
- `botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateKeyspace operation (reached max retries: 3): 2 emulated services all claim requests for 'cassandra': cassandra, cassandra; the request is rejected rather than served by an arbitrary one`.
- `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateService operation: operation CreateService is scaffolded but not implemented in this emulator`.
Expected in each case: a successful CreateTable/UpdateTable/CreateKeyspace/CreateService response, after which the Application Auto Scaling assertions run.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Cause 1: CreateTable 'T' in us-west-2, then CreateTable 'T' in us-east-1 — observe ResourceInUseException instead of a second independent table.
3. Cause 2: CreateTable with a GlobalSecondaryIndexes member — observe NotImplemented.
4. Cause 3: UpdateTable with new ProvisionedThroughput — observe NotImplemented.
5. Cause 4: any Keyspaces call, e.g. CreateKeyspace — observe AmbiguousService for signing name 'cassandra'.
6. Cause 5: ECS CreateService — observe NotImplemented.
