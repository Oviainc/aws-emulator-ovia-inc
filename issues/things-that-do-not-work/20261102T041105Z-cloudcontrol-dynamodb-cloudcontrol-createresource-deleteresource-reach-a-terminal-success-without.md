Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__dynamodb.cloudcontrol-success-means-the-native-table-is-already-active-and-writable
Issue-Key: cloudcontrol__dynamodb|DescribeTable|ResourceNotFoundException
Issue-Commit: 24815f14484dd960d79e1098c6c5cde2b90f5d2a

# cloudcontrol CreateResource/DeleteResource reach a terminal SUCCESS without provisioning the native resource — an AWS::DynamoDB::Table that Cloud Control reports SUCCESS for does not exist in DynamoDB

> _Note: this issue was identified against the code base at commit `24815f14484dd960d79e1098c6c5cde2b90f5d2a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResource

## Description
One net-new root cause.

Cause 1 of 1: Cloud Control API's resource-operation state machine runs entirely inside the cloudcontrol crate's own store and never provisions (or deletes) the resource in the service that owns the resource type. A `CreateResource` for `TypeName="AWS::DynamoDB::Table"` is accepted non-terminal and later reports `OperationStatus="SUCCESS"`, but no DynamoDB table with that name ever comes into existence.
Failing check: cloudcontrol__dynamodb.cloudcontrol-success-means-the-native-table-is-already-active-and-writable

AWS: Smithy (services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json): `CreateResource` takes `TypeName` (required, pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}$) and `DesiredState` (required) and answers a `ProgressEvent`; `GetResourceRequestStatus` answers the `ProgressEvent` for a `RequestToken`. `ProgressEvent.OperationStatus` is modeled as the enum PENDING | IN_PROGRESS | SUCCESS | FAILED | CANCEL_IN_PROGRESS | CANCEL_COMPLETE, and the model documents SUCCESS as "The resource operation has successfully completed." (PENDING: "hasn't yet started", IN_PROGRESS: "currently in progress"). Cloud Control is a facade over the resource type's handler: the type `AWS::DynamoDB::Table` is owned by DynamoDB, so a request that terminates SUCCESS means the DynamoDB table exists, has finished stabilizing (DescribeTable reports TableStatus ACTIVE) and accepts data-plane writes; `ProgressEvent.Identifier` is the resource's primary identifier (the TableName). Symmetrically, a `DeleteResource` that terminates SUCCESS means the table is already gone, so DynamoDB answers ResourceNotFoundException for it. The model's NotStabilizedException (HTTP 400, "the downstream resource failed to complete all of its ready-state checks") and HandlerFailureException (HTTP 502) exist precisely so a facade that cannot stand the resource up reports FAILED/refuses rather than reporting SUCCESS.

Emulator: `CreateResource` is accepted with a non-terminal status and the request record later materialises to SUCCESS, but the effect is applied only to the cloudcontrol crate's private store. The handler parses `DesiredState`, derives the identifier and records a `crate::state::ResourceEffect::Create { .. }` against its own `store` (via `store.materialise_due(now)` / `store.record_request(..)`); nothing in that path reaches the DynamoDB service state. Replaying the check, the Cloud Control request reports a non-terminal status first and then terminal `SUCCESS`, and the very next DynamoDB call fails:
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTable operation: Requested resource not found: Table: <table-name> not found
That refusal is DynamoDB behaving correctly for a table that was never created; the defect is that Cloud Control claimed the create had successfully completed. The same split store means the delete half of the contract is unverifiable too: a `DeleteResource` SUCCESS removes only the cloudcontrol-side record.

Why: Cloud Control API is the generic control plane clients use to create resources without knowing the owning service's API. A client that creates a table through `CreateResource`, polls `GetResourceRequestStatus` to SUCCESS (the documented stabilization barrier) and then issues DynamoDB PutItem/GetItem/DescribeTable against it gets ResourceNotFoundException from a resource the control plane said was ready. Every Cloud Control-driven provisioning flow (and anything built on it, e.g. generic IaC/resource-management tooling) silently produces nothing while reporting success, and a client cannot distinguish "created" from "not created" from the API. Error handling built around NotStabilizedException/FAILED never fires.

Fix: the cloudcontrol crate's resource-effect application — the code that materialises a `RequestRecord`'s `ResourceEffect::Create`/`Delete` when it falls due (`crate::state` in the cloudcontrol service crate, driven from the CreateResource/DeleteResource handlers) must apply the effect to the state of the service that owns `TypeName`, not only to cloudcontrol's own resource map: for `AWS::DynamoDB::Table` it must create/delete the table in the DynamoDB service state (so DescribeTable reports it ACTIVE and the data plane serves it) before the request record is allowed to reach SUCCESS, and must terminate the request FAILED (or refuse with the modeled NotStabilizedException/HandlerFailureException) when it cannot.

Traces: cloudcontrol__dynamodb.cloudcontrol-success-means-the-native-table-is-already-active-and-writable. Handler/state citations: services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/create_resource.rs lines 237-398 (parse DesiredState, derive identifier, record ResourceEffect::Create in the crate-local store, answer ProgressEvent) and the paired services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/delete_resource.rs.

No open issue covers this: the only other cloudcontrol issue in the repo is 20261021T215758Z-bootstrap-service-com-amazonaws-cloudcontrol-cloudapiservice.md, which is resolved (the service is bootstrapped and does answer).

## Evidence
Check: cloudcontrol__dynamodb.cloudcontrol-success-means-the-native-table-is-already-active-and-writable (verdict: error).

Expected (AWS):
1. CreateResource(TypeName="AWS::DynamoDB::Table", DesiredState={TableName, KeySchema[pk HASH], AttributeDefinitions[pk S], BillingMode PAY_PER_REQUEST, Tags}) -> 200, ProgressEvent.OperationStatus in {PENDING, IN_PROGRESS} (non-terminal).
2. GetResourceRequestStatus(RequestToken) polled -> ProgressEvent.OperationStatus == "SUCCESS", ProgressEvent.Identifier == the requested TableName.
3. Immediately after that SUCCESS, with no further waiting: DynamoDB DescribeTable(TableName) -> 200 with Table.TableStatus == "ACTIVE"; PutItem + ConsistentRead GetItem round-trip the item.
4. After DeleteResource(..) reaches SUCCESS: DescribeTable -> ResourceNotFoundException with message "Requested resource not found: Table: <table-name> not found" (fixed text "Requested resource not found: Table: " + the TableName from the request + " not found"); PutItem -> ResourceNotFoundException with message "Requested resource not found"; cloudcontrol GetResource -> ResourceNotFoundException.

Actual (emulator):
Steps 1 and 2 pass — the request is accepted non-terminal and the poll reaches terminal SUCCESS. Step 3 fails on the first DynamoDB call:

  File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTable operation: Requested resource not found: Table: sim2real-behav-ccstab-0c09be01bea9 not found

(The table name is a per-run generated value; the requirement is that DescribeTable of the table Cloud Control reported SUCCESS for answers 200/ACTIVE, not that any particular name appears.) So Cloud Control reported `OperationStatus="SUCCESS"` — modeled as "The resource operation has successfully completed." — for a resource that does not exist in the owning service at all. Steps 3 and 4 are therefore never reached.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cc = client("cloudcontrol"); ddb = client("dynamodb").
3. desired = json.dumps({"TableName": "<unique-name>", "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}], "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}], "BillingMode": "PAY_PER_REQUEST", "Tags": [{"Key": "sim2real", "Value": "x"}]})
4. ev = cc.create_resource(TypeName="AWS::DynamoDB::Table", DesiredState=desired)["ProgressEvent"] — note ev["OperationStatus"] is non-terminal and keep ev["RequestToken"].
5. Poll cc.get_resource_request_status(RequestToken=ev["RequestToken"])["ProgressEvent"] until OperationStatus is terminal; observe it becomes "SUCCESS".
6. Without any further waiting call ddb.describe_table(TableName="<unique-name>").
   Observed: ResourceNotFoundException "Requested resource not found: Table: <unique-name> not found".
   Expected: 200 with Table.TableStatus == "ACTIVE", followed by a successful ddb.put_item / ConsistentRead ddb.get_item round-trip.
7. (Blocked by step 6, same cause) cc.delete_resource(TypeName="AWS::DynamoDB::Table", Identifier="<unique-name>"), poll to SUCCESS, and check DynamoDB now refuses DescribeTable/PutItem with ResourceNotFoundException — the delete side of the SUCCESS-means-stabilized contract is equally unprovisioned because the effect never leaves the cloudcontrol-local store.
