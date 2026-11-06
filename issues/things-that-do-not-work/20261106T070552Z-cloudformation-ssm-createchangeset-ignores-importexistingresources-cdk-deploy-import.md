Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ssm.cdk-deploy-import-existing-resources-adopts-a-named-parameter
Issue-Key: cloudformation__ssm.cdk-deploy-import-existing-resources-adopts-a-named-parameter|run#23|AssertionError|assert rc == 0, f"the --import-existing-resources deploy exited {rc}; tail {out[-800:]!r}"
Issue-Commit: 0de1c688e6ce96ac9f39fea8e8ed5bb261aa89c9

# cloudformation__ssm: CreateChangeSet ignores ImportExistingResources — `cdk deploy --import-existing-resources` fails early validation on a parameter AWS adopts

> _Note: this issue was identified against the code base at commit `0de1c688e6ce96ac9f39fea8e8ed5bb261aa89c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Cause 1 of 1: ImportExistingResources is dropped, so the AWS::EarlyValidation::ResourceExistenceCheck name-conflict gate fires on exactly the resource the caller asked to adopt.

AWS: Smithy: CreateChangeSet.ImportExistingResources is a boolean input member. With it set (the wire form of `cdk deploy --import-existing-resources`), a CREATE change set whose template declares an AWS::SSM::Parameter whose name already exists out of band clears early validation, settles CREATE_COMPLETE/AVAILABLE with that resource's change recorded as Action `Import`, and executing it brings the existing parameter under the new stack without rewriting its value. Documented at https://docs.aws.amazon.com/AWSCloudFormation/latest/UserGuide/import-resources-automatically.html. Without the flag the same deploy is correctly refused on the name.
Emulator: the change set fails early validation regardless of the flag. `cdk deploy --import-existing-resources` exits 1 with `Early validation failed for change set cdk-deploy-change-set:` and the per-resource line `<stack>/Param  (AWS::SSM::Parameter Param)` / `Resource of type 'AWS::SSM::Parameter' with identifier '<parameter name>' already exists. (at )`, so no stack is created and nothing is adopted.
Why: automatic resource import is unusable from the CLI and from CDK: an existing parameter can never be brought under stack management, and the flag that is supposed to turn the name conflict into an adoption changes nothing.
Fix: CreateChangeSet's handler — when ImportExistingResources is true the resource-existence early validation must not fail the matched resources; their changes must be recorded as Action Import and executed as adoptions.
Traces: cloudformation__ssm.cdk-deploy-import-existing-resources-adopts-a-named-parameter. The same cause under the `cloudformation` heading is filed separately in this sweep (check cloudformation.import-existing-resources-adopts-named-parameter), where the raw CreateChangeSet response is quoted.

## Evidence
`cdk deploy <stack> --import-existing-resources` exited 1, expected 0. Output tail:
`<stack>: creating CloudFormation changeset...`
`Early validation failed for change set cdk-deploy-change-set:`
`sim2real-cdk-impx-adopt-<id>/Param  (AWS::SSM::Parameter Param)`
`  Resource of type 'AWS::SSM::Parameter' with identifier '/sim2real-cdk-impx-<id>' already exists. (at )`
Expected: rc 0, the stack created with the pre-existing parameter adopted (change Action 'Import') and its out-of-band value untouched. (Stack and parameter names are per-run.)

## Steps to Reproduce
PutParameter a String parameter out of band. Then `cdk deploy <stack> --import-existing-resources --require-approval never` for an app declaring an AWS::SSM::Parameter with that exact name — equivalently CreateChangeSet(StackName=<new>, ChangeSetType='CREATE', ImportExistingResources=True, TemplateBody=<that template>). The change set fails early validation instead of recording an Import.
Replay: cloudformation__ssm.cdk-deploy-import-existing-resources-adopts-a-named-parameter.
