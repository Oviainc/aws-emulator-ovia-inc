Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify__cloudformation.deploy-amplify-app-and-branch-round-trip-and-delete amplify__cloudformation.deploy-amplify-branch-with-unknown-app-id-rejected-and-rolled-back
Issue-Key: amplify__cloudformation.deploy-amplify-app-and-branch-round-trip-and-delete|stack_outputs#10|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: amplify__cloudformation.deploy-amplify-branch-with-unknown-app-id-rejected-and-rolled-back|run#8|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# amplify__cloudformation: stack Outputs carry no OutputValue, and an AWS::Amplify::Branch naming an unknown app id is provisioned instead of rolling the stack back

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two independent causes behind the 2 failing amplify__cloudformation checks.

Cause 1 of 2: DescribeStacks returns Outputs entries with no OutputValue.
AWS: recorded behavior only — each entry of a CREATE_COMPLETE stack's Outputs carries OutputKey AND the resolved OutputValue; a client reads the app id and branch name the Amplify stack published straight out of it.
Emulator: the Outputs entry has no OutputValue key at all, so reading it raises KeyError: 'OutputValue'.
Why: every consumer of stack outputs (cdk, CLI `--query 'Stacks[0].Outputs'`, cross-stack wiring) has nothing to read and cannot resolve the value the stack published. Related open issues report the same shape for other stacks (20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md, 20261029T065950Z-cloudformation-dynamodb-s3-describestacks-returns-stack-outputs-entries.md); neither carries this check's replay key.
Fix: the Outputs serializer in the DescribeStacks handler and the output-resolution step that must populate OutputValue once the owning resources exist.
Traces: amplify__cloudformation.deploy-amplify-app-and-branch-round-trip-and-delete

Cause 2 of 2: an AWS::Amplify::Branch whose AppId does not exist is created instead of failing the stack.
AWS: recorded behavior only — a branch declared against an app id that does not exist fails to create and the stack settles ROLLBACK_COMPLETE with Amplify's own not-found refusal as the resource failure reason.
Emulator: the stack settles 'CREATE_COMPLETE'.
Why: a client gets a branch attached to no app — a resource AWS would never have created — and the dangling reference is never reported, so a template typo deploys silently.
Fix: the AWS::Amplify::Branch resource handler's create path (resolve AppId against amplify state and fail the resource when it is absent) and the rollback that failure must drive.
Traces: amplify__cloudformation.deploy-amplify-branch-with-unknown-app-id-rejected-and-rolled-back

## Evidence
amplify__cloudformation.deploy-amplify-app-and-branch-round-trip-and-delete — KeyError: 'OutputValue' raised while reading DescribeStacks()['Stacks'][0]['Outputs'][*]. Expected every entry to carry OutputKey and OutputValue.

amplify__cloudformation.deploy-amplify-branch-with-unknown-app-id-rejected-and-rolled-back — AssertionError: stack '<name>' ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE

## Steps to Reproduce
1. Cause 1: CreateStack with an Amplify app + branch template that declares Outputs; wait for CREATE_COMPLETE; call DescribeStacks and read Stacks[0].Outputs[*].OutputValue.
2. Cause 2: CreateStack with an AWS::Amplify::Branch whose AppId names no existing app; expect ROLLBACK_COMPLETE, observe CREATE_COMPLETE.
3. Re-replay both checks in trace_ids.
