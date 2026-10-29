Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse
Issue-Key: cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse|run#15|AssertionError|assert held == {"GateA": "CREATE_IN_PROGRESS", "GateB": "CREATE_IN_PROGRESS"}, (
Issue-Commit: 6c829b3943454ada5df5dfe2630a7a4c6694ce1c

# cloudformation: an unsignalled AWS::CloudFormation::WaitCondition vanishes from the stack mid-create — DescribeStackResource answers ValidationError for a gate it had just reported CREATE_IN_PROGRESS

> _Note: this issue was identified against the code base at commit `6c829b3943454ada5df5dfe2630a7a4c6694ce1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackResource

## Description
REOPENS 20261025T180059Z-cloudformation-sns-sqs-stack-provisioning-has-no-timeline.md: its fix left the call refused. That issue's resolution introduced a planned per-resource provisioning timeline (src/state/timeline.rs) so that "a gate held open has an interval as long as the wait and nothing behind it is scheduled - or reported - until then". The diamond check now does get past the overlap observation it used to fail on (both gates ARE seen CREATE_IN_PROGRESS in one poll), but DescribeStackResource now REFUSES the two gate resources a few seconds later, while they are still unsignalled.

One root cause, grounded in one failing behavioral check.

Cause 1 of 1: a resource that is in flight inside a stack operation stops being addressable through DescribeStackResource - the call is refused with ValidationError instead of reporting the resource's current ResourceStatus.

AWS: Smithy (com.amazonaws.cloudformation): DescribeStackResource(StackName, LogicalResourceId both required) -> DescribeStackResourceOutput{StackResourceDetail}, whose ResourceStatus carries the resource's current lifecycle status; SignalResource(StackName, LogicalResourceId, UniqueId, Status) has no output. The model declares no operation-specific errors, so the only refusal AWS has for this call is the service-common ValidationError, and it is reserved for a stack or logical id that genuinely is not part of the stack. Recorded behavior: for a template declaring two mutually independent AWS::CloudFormation::WaitCondition resources, each with CreationPolicy.ResourceSignal {Timeout: PT10M, Count: 1}, AWS puts both heads into CREATE_IN_PROGRESS and KEEPS reporting StackResourceDetail.ResourceStatus == 'CREATE_IN_PROGRESS' for each of them, on every poll, continuously, until that gate's own SignalResource(Status=SUCCESS) arrives or its PT10M timeout elapses. The requirement is that an in-flight resource of a live stack operation remains describable for the whole interval it is in flight; the particular logical ids, stack name and poll cadence are the check's own.

Emulator: the first poll reports {'GateA': 'CREATE_IN_PROGRESS', 'GateB': 'CREATE_IN_PROGRESS'}, so the resources exist and the concurrency window is now produced. The check then re-reads the same two logical ids every 2 s for a 20 s hold without signalling anything, and on a subsequent read BOTH gates come back as a refusal: DescribeStackResource raises a ClientError with Error.Code 'ValidationError' (the check records only the code, as 'ABSENT(ValidationError)'). Replay detail: `AssertionError: an unsignalled gate left CREATE_IN_PROGRESS during the hold: {'GateA': 'ABSENT(ValidationError)', 'GateB': 'ABSENT(ValidationError)'}`. So an unsignalled WaitCondition is dropped from the stack's addressable resource set while the stack operation is still running - the status does not change to CREATE_COMPLETE or CREATE_FAILED, the resource simply stops existing for this call. Hypothesis (neither the model nor the recording states this): the timeline/settle projection only exposes resources whose planned event is already due, so a resource re-planned or not-yet-due reads as absent.

Why: a client cannot poll a deploy. Every tool that watches a stack - `aws cloudformation describe-stack-resource`, CDK/SAM progress reporting, the standard EC2/ASG cfn-signal bootstrap pattern that polls a gate until it is signalled, any script that waits on one logical id - sees a resource it just observed in flight turn into a hard ValidationError, which is indistinguishable from 'you named a resource that is not in this stack'. It converts a normal wait into a spurious fatal error, and it makes CreationPolicy.ResourceSignal gating unusable even though the gate itself now holds.

Fix: the cloudformation service's stack-resource projection - the resource lookup that DescribeStackResource's handler reads, together with the provisioning timeline/settle path that owns which resources are visible at a given instant (services/service-com-amazonaws-cloudformation-cloudformation/src/state/timeline.rs and the provisioning state it projects). Every resource declared by the template of a live stack operation must be describable for the whole time it is in flight, with its current ResourceStatus; ValidationError must be reserved for a logical id the stack does not declare.

Related open issue that does not carry this replay key: 20261027T101632Z-cloudformation-2-net-new-behavioral-root-causes-an.md Cause 2 describes the earlier form of this check's failure (the two gates were never both CREATE_IN_PROGRESS and the check timed out); that symptom is gone and the divergence has moved to this refusal. 20261026T012701Z-cloudformation-never-provisions-a-dependency-graph-concurrently-and.md is the same family on a different recorded unit.

Traces: cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse.

Service attribution: the check id carries the compound token 'cloudformation__sns__sqs' (its SERVICES are cloudformation, sns, sqs); the diverging operation is cloudformation's DescribeStackResource, so this is filed under service 'cloudformation'.

## Evidence
Expected (AWS): after CreateStack of the diamond template (GateA, GateB = AWS::CloudFormation::WaitCondition with CreationPolicy.ResourceSignal {Timeout: PT10M, Count: 1} and no dependency on each other; QueueA DependsOn GateA; QueueB DependsOn GateB; an SNS topic Fanout; SubA/SubB; JoinPolicy DependsOn [SubA, SubB]), every DescribeStackResource(StackName=<stack>, LogicalResourceId='GateA'|'GateB') returns HTTP 200 with StackResourceDetail.ResourceStatus == 'CREATE_IN_PROGRESS' for as long as that gate is unsignalled.

Actual (emulator): the first paired poll returns {'GateA': 'CREATE_IN_PROGRESS', 'GateB': 'CREATE_IN_PROGRESS'}; a later poll inside the same 20 s hold, with no SignalResource sent, returns a ClientError for BOTH logical ids with Error.Code 'ValidationError'.

[mismatch] cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse
  File "check_cloudformation__sns__sqs_cfn_independent_branches_provision_concurrently_and_tear_down_in_reverse.py", line 287, in run
    assert held == {"GateA": "CREATE_IN_PROGRESS", "GateB": "CREATE_IN_PROGRESS"}, (
  AssertionError: an unsignalled gate left CREATE_IN_PROGRESS during the hold: {'GateA': 'ABSENT(ValidationError)', 'GateB': 'ABSENT(ValidationError)'}

'ABSENT(<code>)' is the check's own rendering of `exc.response['Error']['Code']` from DescribeStackResource, so the recorded refusal code is ValidationError; the check does not capture the message. The logical ids and the generated stack name are examples - the requirement is that an in-flight resource stays describable.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite against the built emulator (region us-west-2).
2. CreateStack with a diamond template: GateA and GateB, both AWS::CloudFormation::WaitCondition with CreationPolicy.ResourceSignal {Timeout: PT10M, Count: 1} and mutually independent; QueueA (AWS::SQS::Queue) DependsOn GateA; QueueB DependsOn GateB; Fanout (AWS::SNS::Topic); SubA/SubB (AWS::SNS::Subscription, protocol sqs, RawMessageDelivery true); JoinPolicy (AWS::SQS::QueuePolicy) DependsOn [SubA, SubB].
3. Poll DescribeStackResource for GateA and GateB in one pass until both report CREATE_IN_PROGRESS (this now happens).
4. Send NO signal. Keep re-reading the same two logical ids every 2 s for 20 s.
5. Observe both calls begin to fail with Error.Code 'ValidationError' instead of continuing to report CREATE_IN_PROGRESS. Expected: ResourceStatus 'CREATE_IN_PROGRESS' on every poll until SignalResource(Status=SUCCESS) arrives or PT10M elapses.
6. Re-replay cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse to confirm the fix; note the check then goes on to assert DependsOn gating, join-after-all-predecessors and reverse-order teardown, which this refusal masks.
