Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.cfn-foreach-language-extensions-expands-to-24-working-queues-and-rejects-missing-capability cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse
Issue-Key: cloudformation__sns__sqs.cfn-foreach-language-extensions-expands-to-24-working-queues-and-rejects-missing-capability|run#20|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Commit: 415bc37b6ebe55c6c7e638f2212d4379947a3e7c

# cloudformation: 2 net-new behavioral root causes — an Fn::ForEach-expanded stack Output is returned with no OutputValue, and a diamond stack's two independent WaitCondition branches are never in flight at once

> _Note: this issue was identified against the code base at commit `415bc37b6ebe55c6c7e638f2212d4379947a3e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Two distinct net-new CloudFormation root causes, both grounded in failing behavioral checks of the cloudformation__sns__sqs group. They are independent defects and each must be closed before this issue is submitted.

=== Cause 1 of 2: DescribeStacks returns stack Outputs with no OutputValue member (operation DescribeStacks; template Outputs produced by an Fn::ForEach expansion under Transform: AWS::LanguageExtensions) ===

AWS: Smithy (com.amazonaws.cloudformation): DescribeStacks -> Stack.Outputs is a list of `Output`, whose members are `OutputKey` ("The key associated with the output"), `OutputValue` ("The value associated with the output"), `Description` and `ExportName`. For a stack in CREATE_COMPLETE, AWS returns, for every declared output, an entry carrying BOTH OutputKey and OutputValue; the value is the resolved Value expression (here `!Ref` on the expanded AWS::SQS::Queue, i.e. that queue's URL). An `Fn::ForEach` block inside Outputs expands to one such entry per collection item, with the loop's generated output key (Url<slot>) and the per-item resolved value. The fixed part of the contract is the member set; the keys and values themselves come from the submitted template and the created resources.

Emulator: the stack reaches CREATE_COMPLETE, the 24 Fn::ForEach-expanded queues exist, list_stack_resources reports exactly the expanded logical ids plus the sentinel topic, and the account holds exactly the 24 expanded queues — i.e. the expansion itself now works. But DescribeStacks returns Output entries that contain NO OutputValue key at all, so building `{o["OutputKey"]: o["OutputValue"] for o in stack["Outputs"]}` raises `KeyError: 'OutputValue'` (replay detail: `KeyError: 'OutputValue'`). The response is a 200; nothing is refused — the member is simply absent from the serialized Output.

Why: a client can never read back what a stack exports. Every consumer of stack outputs (`aws cloudformation describe-stacks --query 'Stacks[].Outputs'`, CDK/SAM wiring, nested-stack and cross-stack hand-offs, scripts that take a queue URL or ARN out of a stack) gets a missing/None value and cannot address the resources the stack just created, even though those resources exist.

Fix: the DescribeStacks handler's Output serialization plus the stack-output resolution step that evaluates each Outputs entry's Value (including the Fn::ForEach-expanded Outputs path) in the cloudformation service crate; OutputValue must be populated from the resolved value rather than dropped.

Related open issues that do NOT cover this check (neither carries its replay key): 20261024T065715Z-cloudformation-cognito-identity-sts-cloudformation-stack-outputs-are-returned-without-outputvalue-so.md (same symptom on a Cognito stack), 20261025T205422Z-cloudformation-an-fn-foreach-block-in-outputs-fails.md and 20261025T210446Z-cloudformation-2-net-new-behavioral-root-causes-fn.md (Fn::ForEach in Outputs previously failed the whole stack — the stack now succeeds, and the divergence has moved to the missing OutputValue), 20261025T015306Z-cloudformation-template-transforms-and-intrinsics-are-not-processed.md.

Traces (failing check ids): cloudformation__sns__sqs.cfn-foreach-language-extensions-expands-to-24-working-queues-and-rejects-missing-capability

=== Cause 2 of 2: independent branches of a dependency graph are never provisioned concurrently, and two unsignalled AWS::CloudFormation::WaitCondition resources are never both CREATE_IN_PROGRESS (operations CreateStack / DescribeStackResource / SignalResource / DescribeStackEvents) ===

AWS: recorded behavior only — the Smithy model describes the calls (SignalResource(StackName, LogicalResourceId, UniqueId, Status) with no output; DescribeStackResource -> StackResourceDetail.ResourceStatus; DescribeStackEvents -> paginated StackEvents) but says nothing about scheduling. AWS CloudFormation schedules every resource whose dependencies are already satisfied at the same time: a diamond template with two mutually independent AWS::CloudFormation::WaitCondition heads (each with CreationPolicy.ResourceSignal Timeout PT10M, Count 1) puts BOTH heads into CREATE_IN_PROGRESS, so a single pair of DescribeStackResource reads observes GateA and GateB both CREATE_IN_PROGRESS, and each gate stays in progress until its own SignalResource(Status=SUCCESS) arrives. Downstream, a DependsOn child is not scheduled before its parent's CREATE_COMPLETE, the join resource is scheduled only after ALL of its predecessors complete, and the delete phase runs in reverse topological order with per-resource DELETE_IN_PROGRESS/DELETE_COMPLETE events.

Emulator: the check never gets past its first gate. Polling DescribeStackResource for GateA == CREATE_IN_PROGRESS and GateB == CREATE_IN_PROGRESS in one poll never becomes true, and the check hits the harness cap: replay detail `Check timed out after 60.0s`. The two independent WaitConditions are therefore either provisioned serially or completed without ever waiting for a signal, so the concurrency window AWS always exposes never exists. Because the run dies there, nothing downstream (DependsOn gating, join-after-all-predecessors, reverse-order teardown, the SNS->SQS fan-out through the stack's JoinPolicy) is reachable either.

Why: a client cannot rely on CloudFormation's deployment semantics. Deploys that depend on ResourceSignal/CreationPolicy gating (the standard EC2/ASG bootstrap pattern) never behave as on AWS, provisioning order and parallelism are not observable from the event log, and tools that read DescribeStackEvents to reason about what the stack is doing see a different graph execution than AWS produces.

Fix: the stack provisioning scheduler in the cloudformation service crate — the dependency-graph ready-set execution (all currently-ready resources started together, children gated on their DependsOn/Ref parents' completion) — together with the AWS::CloudFormation::WaitCondition / CreationPolicy.ResourceSignal resource provider that must hold a gate CREATE_IN_PROGRESS until SignalResource arrives, and the per-resource CREATE_IN_PROGRESS / CREATE_COMPLETE / DELETE_IN_PROGRESS / DELETE_COMPLETE events those phases emit.

Related open issues that do NOT cover this check (neither carries its replay key): 20261026T012701Z-cloudformation-never-provisions-a-dependency-graph-concurrently-and.md (same family, different recorded unit) and 20261027T060315Z-cloudformation-sns-sqs-a-stack-s-delete-phase.md (the delete-phase events this check also needs).

Traces (failing check ids): cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse

## Evidence
Cause 1 — expected (AWS): DescribeStacks on the CREATE_COMPLETE Fn::ForEach stack returns Stacks[0].Outputs as 24 entries, each `{"OutputKey": "Url<slot>", "OutputValue": "<the expanded queue's URL>"}` (one per CommaDelimitedList item), so `{o["OutputKey"]: o["OutputValue"]}` yields the 24 Url<slot> keys.
Actual (emulator): the stack is CREATE_COMPLETE, list_stack_resources reports exactly {Qa..Qx, Sentinel} and all 24 queues are real and usable, but each Outputs entry omits the OutputValue member entirely; the check fails with `KeyError: 'OutputValue'` at `outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}`. HTTP status 200, no error code — the member is dropped from the serialized Output structure.

Cause 2 — expected (AWS): after CreateStack of the diamond (GateA/GateB independent WaitConditions -> QueueA/QueueB -> SubA/SubB -> JoinPolicy), a single poll of DescribeStackResource for GateA and GateB reports {"GateA": "CREATE_IN_PROGRESS", "GateB": "CREATE_IN_PROGRESS"}; both stay in progress (the check then holds 20 s) until SignalResource(Status=SUCCESS) is sent per gate; the create event log then shows a >= 5 s overlap between the two gates, QueueA scheduled only after GateA's CREATE_COMPLETE and before GateB's, JoinPolicy scheduled after all of SubA/SubB/QueueA/QueueB complete and completing last, and the delete phase showing JoinPolicy DELETE_COMPLETE before either branch's DELETE_IN_PROGRESS.
Actual (emulator): that first poll never reports both gates CREATE_IN_PROGRESS; the check is killed by the harness with `Check timed out after 60.0s`, so no overlap, no DependsOn ordering, no join ordering and no reverse-order teardown is ever observable.

## Steps to Reproduce
Cause 1:
1. CreateStack with `Transform: AWS::LanguageExtensions` and an `Fn::ForEach` loop in BOTH Resources (24 AWS::SQS::Queue entries named from the loop variable) and Outputs (`Fn::ForEach::Urls` producing `Url<slot>` whose Value is a Ref to the matching queue), passing Capabilities=["CAPABILITY_AUTO_EXPAND"].
2. Poll DescribeStacks until StackStatus == CREATE_COMPLETE.
3. Read Stacks[0].Outputs: each entry has OutputKey but no OutputValue. Expected: every entry carries the resolved OutputValue (the expanded queue's URL).

Cause 2:
1. CreateStack with a diamond template: GateA and GateB, both AWS::CloudFormation::WaitCondition with CreationPolicy.ResourceSignal {Timeout: PT10M, Count: 1} and no dependency on each other; QueueA DependsOn GateA; QueueB DependsOn GateB; an SNS topic Fanout; SubA/SubB subscribing each queue; JoinPolicy (AWS::SQS::QueuePolicy) DependsOn [SubA, SubB] and Ref-ing both queues.
2. Poll DescribeStackResource for GateA and GateB in the same pass, looking for both == CREATE_IN_PROGRESS. The emulator never reports that pair.
3. (On AWS) sleep 20 s, SignalResource(GateA, SUCCESS), wait for QueueA CREATE_COMPLETE while GateB is still CREATE_IN_PROGRESS, SignalResource(GateB, SUCCESS), then compare DescribeStackEvents timestamps for the create and the delete phase.
