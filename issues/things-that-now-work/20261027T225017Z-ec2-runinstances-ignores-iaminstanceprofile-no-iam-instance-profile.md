Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-describe-iam-instance-profile-associations
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-describe-iam-instance-profile-associations|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Commit: d439c22423981a98136da88f5cba5181ca1e215e

# EC2 RunInstances ignores IamInstanceProfile: no IAM instance profile association is ever minted, so DescribeIamInstanceProfileAssociations stays empty; its unknown-filter message also diverges (2 causes)

> _Note: this issue was identified against the code base at commit `d439c22423981a98136da88f5cba5181ca1e215e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Two root causes, both reached by the single failing check ec2.op-contract-describe-iam-instance-profile-associations. Cause 1 is what makes the check hang; Cause 2 is the assertion that follows it in the same check and will be hit as soon as Cause 1 is fixed. Both must be closed before this check can pass.

Cause 1 of 2: RunInstances accepts IamInstanceProfile and silently drops it, so no IamInstanceProfileAssociation is ever created.

AWS: RunInstances models `IamInstanceProfile` (IamInstanceProfileSpecification, members `Arn` and `Name` — Smithy: com.amazonaws.ec2#IamInstanceProfileSpecification). When an instance is launched with it, AWS creates an association record and `Instance$IamInstanceProfile` is populated (Smithy doc: "The IAM instance profile associated with the instance, if applicable."). DescribeIamInstanceProfileAssociations then reports that record as an `IamInstanceProfileAssociation` (Smithy: com.amazonaws.ec2#IamInstanceProfileAssociation with `AssociationId`, `InstanceId`, `IamInstanceProfile`, `State`, `Timestamp`); its `State` (IamInstanceProfileAssociationState) begins at `associating` and converges to `associated` shortly after launch, and the association is also addressable by `AssociationIds` and by the documented `instance-id` / `state` filters (Smithy: DescribeIamInstanceProfileAssociationsRequest$Filters — "instance-id - The ID of the instance. state - The state of the association (associating | associated | disassociating)").

Emulator: RunInstances never reads the `IamInstanceProfile` member at all — services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs contains no reference to an IAM instance profile anywhere in `handle`, and writes nothing into the `iam_instance_profile_associations` store that DescribeIamInstanceProfileAssociations reads. DescribeIamInstanceProfileAssociations itself is implemented and answers 200, but with an empty `IamInstanceProfileAssociations` set forever. The replayed check launches an instance with `IamInstanceProfile={"Name": <profile name>}` and then polls `describe_iam_instance_profile_associations(Filters=[{"Name":"instance-id","Values":[<instance id>]}])` for an entry whose `State` is `associated`; the condition never becomes true and the check is killed by the harness cap — verdict mismatch, `Error: Check timed out after 60.0s`. Nothing is refused, so the divergence is a permanently empty read, not an error code.

Why: a client that launches an instance with an instance profile and then confirms the binding — the standard way to verify that an instance can assume its role, and what the console, CDK/CloudFormation drift checks and ReplaceIamInstanceProfileAssociation callers all do — sees no association at all. Any wait-for-`associated` loop never terminates, and code that reads `Instance$IamInstanceProfile` to discover which profile an instance carries gets nothing. The resource is also unaddressable afterwards: there is no `AssociationId` to pass to ReplaceIamInstanceProfileAssociation or DisassociateIamInstanceProfile.

Fix: the RunInstances handler and the instance state it writes — RunInstances must resolve the `IamInstanceProfile` specification (by `Name` or `Arn`) when one is given, mint an `IamInstanceProfileAssociation` into the service state's `iam_instance_profile_associations` with a fresh `AssociationId`, the launched `InstanceId`, the resolved profile, `State` `associating` and a `Timestamp`, record the clock-owed move to `associated` the way the handler already records the instance's own `pending`→`running` transition (so `materialize_due_transitions` converges it), and populate the instance's own `IamInstanceProfile` output member. This is a sibling of, but not the same as, the two open issues 20261022T235108Z-ec2-associateiaminstanceprofile-stub.md (AssociateIamInstanceProfile is a different operation and a different entry path) and 20261023T102725Z-ec2-replaceiaminstanceprofileassociation-stub.md; note also that 20261022T235108Z-ec2-describeiaminstanceprofileassociations-stub.md reads stale — that read path is now implemented, it just has nothing to report.

Traces: ec2.op-contract-describe-iam-instance-profile-associations.

Cause 2 of 2: DescribeIamInstanceProfileAssociations rejects an undocumented filter name with the wrong message.

AWS: an unknown filter name on this operation is refused with error code `InvalidParameterValue` and the message template `An unknown filter '<filter-name>' was given`. Fixed text: `An unknown filter '` … `' was given`; the only variable part is `<filter-name>`, taken verbatim from the request's `Filters[].Name`.

Emulator: services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_iam_instance_profile_associations.rs refuses the same request with code `InvalidParameterValue` but the message `The filter '<filter-name>' is invalid` — right code, wrong fixed text. The replayed check asserts the AWS template exactly (`assert message == expected`), so it fails on the message even though the status and code match; it does not reach that assertion today only because Cause 1 kills the check first.

Why: a client that branches on, logs or surfaces the rejection reason gets text AWS never emits, so message-matching diagnostics and tests against this operation misreport why the call was refused.

Fix: the filter-name validation in this operation's handler (`filter_name_is_documented` / the `invalid_parameter_value` construction it feeds), which must emit AWS's template rather than this crate's generic "The filter '…' is invalid" phrasing.

Traces: ec2.op-contract-describe-iam-instance-profile-associations.

## Evidence
Failing check: [mismatch] ec2.op-contract-describe-iam-instance-profile-associations
Replay detail: `Error: Check timed out after 60.0s`

Cause 1 — expected vs actual:
  Expected (AWS): after RunInstances(ImageId=<al2023 ami>, InstanceType=t3.micro, MinCount=1, MaxCount=1, IamInstanceProfile={"Name": <profile>}), DescribeIamInstanceProfileAssociations(Filters=[{Name:"instance-id",Values:[<instance-id>]}]) returns 200 with exactly one IamInstanceProfileAssociation whose InstanceId is the launched instance, whose IamInstanceProfile.Arn is the created profile's ARN, and whose State converges from `associating` to `associated`; DescribeIamInstanceProfileAssociations(AssociationIds=[<that AssociationId>]) then returns exactly that one association.
  Actual (emulator): 200 with `IamInstanceProfileAssociations` empty, permanently. The check's poll for State==`associated` (its own budget 90 s) never satisfies, so the harness cap fires at 60 s. Source confirmation: services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs never references the input's IamInstanceProfile member nor the `iam_instance_profile_associations` store; services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_iam_instance_profile_associations.rs reads only that store (lines 233-278), so it can only ever return an empty set.

Cause 2 — expected vs actual:
  Expected (AWS): DescribeIamInstanceProfileAssociations(Filters=[{Name:"<unknown-name>",Values:["x"]}]) -> HTTP 400, Code `InvalidParameterValue`, Message `An unknown filter '<unknown-name>' was given`.
  Actual (emulator): HTTP 400, Code `InvalidParameterValue`, Message `The filter '<unknown-name>' is invalid` (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_iam_instance_profile_associations.rs, the `filter_name_is_documented` rejection at lines 190-196).

## Steps to Reproduce
1. Create an IAM role with an ec2.amazonaws.com trust policy, create an instance profile, and add the role to it.
2. ec2 DescribeImages(Owners=["amazon"], Filters=[name=al2023-ami-*-x86_64, state=available]) and take the newest ImageId.
3. ec2 RunInstances(ImageId=<that ami>, InstanceType=t3.micro, MinCount=1, MaxCount=1, IamInstanceProfile={"Name": <profile name>}); wait for the instance to be running.
4. Poll ec2 DescribeIamInstanceProfileAssociations(Filters=[{"Name":"instance-id","Values":[<instance id>]}]).
   Observed: the association list is empty on every poll, so no AssociationId is ever available and no State ever reaches `associated` (Cause 1). AWS returns one association that converges to `associated`.
5. Call ec2 DescribeIamInstanceProfileAssociations(Filters=[{"Name":"sim2real-bogus-filter","Values":["x"]}]).
   Observed: InvalidParameterValue with message `The filter 'sim2real-bogus-filter' is invalid` instead of AWS's `An unknown filter 'sim2real-bogus-filter' was given` (Cause 2).

Replay: run the behavioral suite check ec2.op-contract-describe-iam-instance-profile-associations.
