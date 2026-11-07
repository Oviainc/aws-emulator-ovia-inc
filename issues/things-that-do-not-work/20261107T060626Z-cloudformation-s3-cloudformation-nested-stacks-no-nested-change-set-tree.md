Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3.change-set-include-nested-stacks-links-the-change-set-tree cloudformation__s3.stack-nested-child-update-failure-reason-chain
Issue-Key: cloudformation__s3.change-set-include-nested-stacks-links-the-change-set-tree|run#18|KeyError|child_cs = _stack_change(root, "Child")["ChangeSetId"]
Issue-Key: cloudformation__s3.stack-nested-child-update-failure-reason-chain|run#12|AssertionError|assert _settle(cfn, parent, timeout_s=_cap(ctx, _DEADLINE_S)) == "UPDATE_ROLLBACK_COMPLETE", f"parent update ended {_stack(cfn, parent)!r}"
Issue-Commit: 547f35aa751bdfccaa69888f930e73773cf7bea8

# cloudformation nested stacks: no nested change-set tree is built (ResourceChange.ChangeSetId/ParentChangeSetId/RootChangeSetId absent), and a child's failed update leaves the parent UPDATE_ROLLBACK_FAILED instead of UPDATE_ROLLBACK_COMPLETE (2 causes)

> _Note: this issue was identified against the code base at commit `547f35aa751bdfccaa69888f930e73773cf7bea8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Two distinct net-new root causes in this service, both in the nested-stack path. Each must be closed before this issue is submitted.

Cause 1 of 2: IncludeNestedStacks=True does not build a change-set tree — the parent's nested-stack ResourceChange has no ChangeSetId, so no child or grandchild change set exists and none of the tree linkage is reported.
AWS: Smithy: CreateChangeSet.IncludeNestedStacks (boolean); DescribeChangeSet output carries IncludeNestedStacks, ParentChangeSetId and RootChangeSetId; ResourceChange.ChangeSetId is documented as "The change set ID of the nested change set.", and ChangeSetSummary (ListChangeSets) carries ParentChangeSetId and RootChangeSetId. With a parent -> child -> grandchild stack tree and IncludeNestedStacks=True, CloudFormation creates one change set per affected nested stack: the root names no ParentChangeSetId and no RootChangeSetId; each nested change set reaches Status=CREATE_COMPLETE with StackId equal to the nested stack it belongs to, ParentChangeSetId equal to its immediate parent's change-set id and RootChangeSetId equal to the root's; ListChangeSets on the nested stack repeats that linkage; the grandchild's Changes name its own resource change ([('G2','Add')]); and DeleteChangeSet on the root removes the whole tree, after which the nested stacks list it no more and DescribeChangeSet answers ChangeSetNotFound with message "ChangeSet [<change-set-arn>] does not exist" (fixed text "ChangeSet [" ... "] does not exist" around the requested change-set id).
Emulator: the root change set reaches CREATE_COMPLETE and reports IncludeNestedStacks=True with no parent/root ids (correct), and its single ResourceChange is ('Child','AWS::CloudFormation::Stack','Modify') — but that ResourceChange has no ChangeSetId member, so reading it raises KeyError: 'ChangeSetId'. No nested change set is created at any depth, so the child/grandchild linkage, the nested ListChangeSets rows and the cascading delete are all unreachable.
Why: a client cannot review what a nested-stack deploy will change inside its children, and cannot walk or clean up the change-set tree. This is the same missing recursion reported for cloudformation__cloudwatch__s3__sns__sqs; it is filed per service because an issue carries one service token.
Fix: the CloudFormation CreateChangeSet handler's change-set computation (recursion into changed AWS::CloudFormation::Stack resources) plus the DescribeChangeSet/ListChangeSets serializers that must emit ResourceChange.ChangeSetId, ParentChangeSetId and RootChangeSetId, and the DeleteChangeSet handler which must delete a root's descendants.
Traces: cloudformation__s3.change-set-include-nested-stacks-links-the-change-set-tree.

Cause 2 of 2: when a nested child's update fails, the parent's rollback itself fails — the parent ends UPDATE_ROLLBACK_FAILED instead of UPDATE_ROLLBACK_COMPLETE.
AWS: recorded behavior only (stack status values are modelled, the rollback semantics are documented in the nested-stacks user guide). An UpdateStack that points the child at a template whose new resource cannot create (a WaitCondition that times out) makes the child fail, the parent records a 'Child' UPDATE_FAILED event whose reason is "Embedded stack <child-stack-arn> was not successfully updated. Currently in UPDATE_ROLLBACK_IN_PROGRESS with reason: The following resource(s) failed to create: [Fast]. " (fixed text "Embedded stack " / " was not successfully updated. Currently in " / " with reason: ", with the child's stack id, its current status and its own reason as the variable parts), and BOTH stacks then settle in UPDATE_ROLLBACK_COMPLETE, the child keeping only its pre-update resources and the parent reading the rolled-back child's outputs.
Emulator: the parent settles in StackStatus 'UPDATE_ROLLBACK_FAILED' with StackStatusReason 'The following resource(s) failed to update: [Child]. ', so the rollback never completes and the stack is left in a state that needs ContinueUpdateRollback.
Why: a nested-stack update that fails is the normal failure path of every parent/child deploy. A stack stuck in UPDATE_ROLLBACK_FAILED cannot be updated again, so a client's deploy pipeline is wedged on a failure AWS recovers from automatically, and the automatic "roll back to the previous child template" guarantee cannot be tested at all.
Fix: the CloudFormation stack provisioner's update-rollback state machine for AWS::CloudFormation::Stack resources — rolling a child back to its previous template must succeed and drive the parent to UPDATE_ROLLBACK_COMPLETE.
Traces: cloudformation__s3.stack-nested-child-update-failure-reason-chain.

## Evidence
Cause 1 — cloudformation__s3.change-set-include-nested-stacks-links-the-change-set-tree [error].
Expected: root DescribeChangeSet Changes == [('Child','AWS::CloudFormation::Stack','Modify')] with a ChangeSetId; DescribeChangeSet(child) == CREATE_COMPLETE with (StackId, ParentChangeSetId, RootChangeSetId) == (child stack id, root cs id, root cs id); DescribeChangeSet(grand) likewise with (grand stack id, child cs id, root cs id) and Changes == [('G2','Add')]; ListChangeSets(grand stack) repeats (child cs id, root cs id); after DeleteChangeSet(root) both nested sets are unlisted and DescribeChangeSet answers ('ChangeSetNotFound', 'ChangeSet [<cs-id>] does not exist').
Actual: KeyError: 'ChangeSetId' raised at `child_cs = _stack_change(root, "Child")["ChangeSetId"]` — the root's nested-stack ResourceChange has no ChangeSetId member (the preceding assertions on Status, IncludeNestedStacks=True and the absent root ParentChangeSetId/RootChangeSetId all passed).

Cause 2 — cloudformation__s3.stack-nested-child-update-failure-reason-chain [mismatch].
Expected: after UpdateStack repoints the child at the failing template, DescribeStacks(parent).StackStatus == 'UPDATE_ROLLBACK_COMPLETE' (and the child likewise), with the parent event ('Child','UPDATE_FAILED','Embedded stack <child-arn> was not successfully updated. Currently in UPDATE_ROLLBACK_IN_PROGRESS with reason: The following resource(s) failed to create: [Fast]. ').
Actual: AssertionError "parent update ended {... 'StackStatus': 'UPDATE_ROLLBACK_FAILED', 'StackStatusReason': 'The following resource(s) failed to update: [Child]. ' ...}" — the parent stops in UPDATE_ROLLBACK_FAILED, so neither stack reaches UPDATE_ROLLBACK_COMPLETE and the event/output assertions are never reached.

## Steps to Reproduce
Cause 1: 1) CreateBucket; PutObject child.json (one AWS::CloudFormation::Stack 'Grand' taking GrandUrl), grand-v1.json (handle G1) and grand-v2.json (handles G1+G2). 2) CreateStack the parent (resource 'Child' of type AWS::CloudFormation::Stack with TemplateURL={Ref ChildUrl}, Parameters={GrandUrl:{Ref GrandUrl}}) with GrandUrl=grand-v1; wait CREATE_COMPLETE. 3) CreateChangeSet(StackName=root, ChangeSetType='UPDATE', IncludeNestedStacks=True, UsePreviousTemplate=True, Parameters=[GrandUrl=grand-v2]). 4) Poll DescribeChangeSet to CREATE_COMPLETE and read Changes[0]['ResourceChange']['ChangeSetId'] — the key is absent.

Cause 2: 1) CreateBucket; PutObject child-v1.json (a WaitConditionHandle only) and child-v2.json (the handle plus an AWS::CloudFormation::WaitCondition that times out). 2) CreateStack a parent whose 'Child' AWS::CloudFormation::Stack points at child-v1.json; wait CREATE_COMPLETE. 3) UpdateStack the parent with the same template pointing at child-v2.json. 4) Poll DescribeStacks(parent) to a terminal status: it is UPDATE_ROLLBACK_FAILED with reason 'The following resource(s) failed to update: [Child]. '.
