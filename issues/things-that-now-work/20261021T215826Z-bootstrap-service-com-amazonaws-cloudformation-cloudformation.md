Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the CloudFormation emulator

_Filed: 2026-10-21T21:58:26Z_
_Operation: service-com-amazonaws-cloudformation-cloudformation | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 90 operations in `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and the crate holds no CloudFormation state. No client workflow works at all.

Implement a FIRST, SMALL, STATEFUL slice: the stack lifecycle happy path, in this order. Wire protocol is `awsQuery` (SDK id `CloudFormation`, signing name `cloudformation`), so responses must be the Query/XML `<OperationName>Result` wrappers the generated serializer produces.

1. **CreateStack** — input `StackName` (required), `TemplateBody` | `TemplateURL`, `Parameters` (list<Parameter>: `ParameterKey`, `ParameterValue`), `Capabilities`, `Tags`, `NotificationARNs`, `TimeoutInMinutes`, `OnFailure`, `DisableRollback`, `EnableTerminationProtection`, `ClientRequestToken`. Must allocate and persist a new stack, returning the generated `StackId` as an ARN of the form `arn:aws:cloudformation:<region>:<account>:stack/<StackName>/<uuid>`. Store the template body verbatim, record `CreationTime` = now, and set `StackStatus` to `CREATE_COMPLETE` (synchronous completion is acceptable for the first slice; `CREATE_IN_PROGRESS` then immediate completion is also fine as long as later reads are consistent). Creating a stack whose name already exists must fail with `AlreadyExistsException` (HTTP 400), not succeed.
2. **DescribeStacks** — input `StackName` (optional), `NextToken`; output `Stacks` (list<Stack>), `NextToken`. With no `StackName`, return every live stack from the store; with `StackName` (name OR stack id), return exactly that one stack. The `Stack` members must be read back from what CreateStack persisted: `StackId`, `StackName` (required), `CreationTime` (required), `StackStatus` (required), plus `Description`, `Parameters`, `Tags`, `Capabilities`, `NotificationARNs`, `TimeoutInMinutes`, `RoleARN`, `DisableRollback`, `EnableTerminationProtection`, `LastUpdatedTime`, `Outputs`, `DriftInformation.StackDriftStatus = NOT_CHECKED`. An unknown `StackName` must produce the well-known `ValidationError` (HTTP 400, message `Stack with id <name> does not exist`) rather than an empty list or a 501.
3. **ListStacks** — input `NextToken`, `StackStatusFilter` (list<StackStatus>); output `StackSummaries` (list<StackSummary>), `NextToken`. Project each persisted stack into `StackSummary{StackId, StackName, TemplateDescription, CreationTime, LastUpdatedTime, DeletionTime, StackStatus, StackStatusReason}`. `StackStatusFilter` must actually filter; deleted stacks must still appear with `DELETE_COMPLETE` and a `DeletionTime`.
4. **GetTemplate** — input `StackName`, `TemplateStage`; output `TemplateBody`, `StagesAvailable`. Must return the exact `TemplateBody` persisted by CreateStack/UpdateStack (byte-identical round trip), with `StagesAvailable = [Original, Processed]`.
5. **UpdateStack** — input `StackName` (required), `TemplateBody`/`TemplateURL`/`UsePreviousTemplate`, `Parameters`, `Capabilities`, `Tags`; output `StackId`. Must mutate the SAME stored stack (same `StackId`), replace the template when a new one is supplied (or keep the previous one when `UsePreviousTemplate=true`), merge `Parameters` honouring `UsePreviousValue`, set `LastUpdatedTime` = now and `StackStatus = UPDATE_COMPLETE`. A subsequent DescribeStacks/GetTemplate must observe the update.
6. **DeleteStack** — input `StackName` (required), `RetainResources`, `DeletionMode`; empty output. Must transition the stored stack to `DELETE_COMPLETE` with `DeletionTime` = now and remove it from the live set, so DescribeStacks by name then fails with `ValidationError` while ListStacks still shows the `DELETE_COMPLETE` summary. A delete on a stack with `EnableTerminationProtection=true` must be refused (`ValidationError`, "...cannot be deleted while TerminationProtection is enabled").

Supporting read that makes the loop demonstrable (include if cheap, otherwise it stays stubbed): **DescribeStackEvents** (`StackName` required; output `StackEvents` list<StackEvent> with `StackId`, `EventId`, `StackName`, `LogicalResourceId`, `ResourceType`, `Timestamp`, `ResourceStatus`) emitting at least the stack-level CREATE/UPDATE/DELETE events recorded by the operations above, newest first.

NEVER STUBS: each of these six must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (see README "State"). No hardcoded `StackId`s, no echoing the request back, no generating an id without storing the stack. A create followed by a describe must return what was created.

All other 84 operations (change sets, stack sets, type registry, drift detection, resource scans, refactors, imports/exports) stay at 501 and will be filed as individual gap issues later.

## State Model

In `src/state.rs`, on the service state struct:

- `stacks: IndexMap<String /* StackName */, Stack>` — live stacks, insertion-ordered for deterministic DescribeStacks/ListStacks output.
- `stacks_by_id: HashMap<String /* StackId ARN */, String /* StackName */>` — so every operation accepting `StackName` can also be addressed by stack id, as real CloudFormation allows.
- `deleted_stacks: Vec<Stack>` — stacks in `DELETE_COMPLETE`, retained so ListStacks can report them with `DeletionTime` (and so DescribeStacks by stack *id* of a deleted stack still resolves).
- `Stack { stack_id: String, stack_name: String, description: Option<String>, template_body: String, parameters: Vec<(String /* key */, String /* value */)>, capabilities: Vec<String>, tags: Vec<(String, String)>, notification_arns: Vec<String>, role_arn: Option<String>, timeout_in_minutes: Option<i32>, disable_rollback: bool, enable_termination_protection: bool, on_failure: Option<String>, status: StackStatus, status_reason: Option<String>, creation_time: SystemTime, last_updated_time: Option<SystemTime>, deletion_time: Option<SystemTime>, outputs: Vec<StackOutput { output_key, output_value, description, export_name }>, client_request_token: Option<String>, events: Vec<StackEvent> }`
- `StackEvent { event_id: String /* uuid */, stack_id: String, stack_name: String, logical_resource_id: String, physical_resource_id: Option<String>, resource_type: Option<String>, timestamp: SystemTime, resource_status: String, resource_status_reason: Option<String> }` — appended (never overwritten) by CreateStack/UpdateStack/DeleteStack.
- `next_token_cursors: HashMap<String /* NextToken */, usize>` (or an offset encoded in the token) so the `@paginated(inputToken=NextToken, outputToken=NextToken)` contract on DescribeStacks and ListStacks can be honoured once page sizes matter.

## Evidence

`services/service-com-amazonaws-cloudformation-cloudformation/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-cloudformation-cloudformation.json`) lists all 90 operations and their contracts: CreateStack (line 69: `StackName` required, `TemplateBody`/`TemplateURL`/`Parameters`/`Tags`/`Capabilities`/`EnableTerminationProtection` in, `StackId` out, `AlreadyExistsException` HTTP 400), DescribeStacks (line 239: `@paginated(items=Stacks)`, `StackName`+`NextToken` in, `Stacks`+`NextToken` out), ListStacks (line 433: `StackStatusFilter`+`NextToken` in, `StackSummaries` out), GetTemplate (line 326: `TemplateBody`+`StagesAvailable` out), UpdateStack (line 572), DeleteStack (line 118: `StackName` required, empty output). `smithy_query shape=Stack,StackSummary` confirms `StackName`, `CreationTime` and `StackStatus` are `@required` on both, plus the optional `StackId`, `Parameters`, `Tags`, `Outputs`, `LastUpdatedTime`, `DeletionTime`, `DriftInformation` members the state model above must back. README.md documents the protocol as `awsQuery` and that every scaffold answers HTTP 501 until the `SCAFFOLDED, NOT IMPLEMENTED` marker is deleted; all 90 files under `src/ops/` still carry that marker, so no test asserts any stack behaviour today.

## Steps to Reproduce

1. Start the emulator and point the AWS CLI/SDK at it with `--endpoint-url` and the `cloudformation` service.
2. `aws cloudformation create-stack --stack-name demo --template-body '{"Resources":{"T":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}' --tags Key=env,Value=dev` — expect a `StackId` ARN containing `stack/demo/`; today: HTTP 501 NotImplemented.
3. `aws cloudformation describe-stacks --stack-name demo` — expect one `Stack` with `StackName=demo`, a `CreationTime`, `StackStatus=CREATE_COMPLETE`, the tag `env=dev`, and the same `StackId` returned by create.
4. `aws cloudformation get-template --stack-name demo` — expect `TemplateBody` byte-identical to what was submitted.
5. `aws cloudformation list-stacks` — expect a `StackSummary` for `demo`; `--stack-status-filter DELETE_COMPLETE` must exclude it.
6. `aws cloudformation create-stack --stack-name demo --template-body '{}'` again — expect `AlreadyExistsException` (HTTP 400).
7. `aws cloudformation update-stack --stack-name demo --template-body '{"Resources":{"T2":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}'`, then describe-stacks — expect the same `StackId`, `StackStatus=UPDATE_COMPLETE`, a populated `LastUpdatedTime`, and get-template returning the NEW body.
8. `aws cloudformation describe-stack-events --stack-name demo` — expect the create and update events, newest first.
9. `aws cloudformation delete-stack --stack-name demo`, then `describe-stacks --stack-name demo` — expect `ValidationError` (HTTP 400) `Stack with id demo does not exist`, while `list-stacks --stack-status-filter DELETE_COMPLETE` still shows `demo` with a `DeletionTime`.
10. `aws cloudformation describe-stacks --stack-name no-such-stack` — expect `ValidationError` (HTTP 400), not an empty list and not 501.
