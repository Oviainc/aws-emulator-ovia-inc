Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__servicecatalog-appregistry.appregistry-sync-unassociated-cfn-stack-is-no-action
Issue-Key: cloudformation__servicecatalog-appregistry.appregistry-sync-unassociated-cfn-stack-is-no-action|run#8|KeyError|topic_arn = _outputs(deployed)["TopicArn"]
Issue-Commit: 2d8e5ac28e0b90b67e06be860948cea48d2b4817

# CloudFormation stack Outputs are never produced: a CREATE_COMPLETE stack declaring Outputs for an AWS::SNS::Topic returns a Stack with no Outputs, blocking the AppRegistry/CFN association probe

> _Note: this issue was identified against the code base at commit `2d8e5ac28e0b90b67e06be860948cea48d2b4817`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One net-new root cause, grounded in the single failing check of this cross-service suite. The check id's leading token names two services (`cloudformation` and `service-catalog-appregistry`); the defective component is CloudFormation's stack output-evaluation / DescribeStacks serialization path. Both constituent services are stood up in this build (CloudFormation and ServiceCatalog AppRegistry both appear in the bootstrapped set), so this is a fidelity finding, not a bootstrap gap.

Cause 1 of 1: a stack whose template declares an `Outputs` section reaches terminal status CREATE_COMPLETE, but DescribeStacks returns a Stack carrying no `Outputs` list, so no declared output value can be read back. Here the template is a single `AWS::SNS::Topic` named `ProbeTopic` with `Outputs: TopicArn: Value: !Ref ProbeTopic`.

AWS: DescribeStacks returns, for a stack whose template declares outputs, a Stack with an `Outputs` list holding one Output per declared output. Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): `Stack` member `Outputs` -> list of `Output{OutputKey, OutputValue, Description, ExportName}`; DescribeStacks output `Stacks` -> `Stacks`. That `{"Ref": "<AWS::SNS::Topic logical id>"}` resolves to the topic ARN is recorded behavior only (CFN resource-type attribute mapping is not in the Smithy model); the modeled part is that declared outputs appear as Output entries at all.

Emulator: after the stack transitions to CREATE_COMPLETE, `describe_stacks(StackName=...)["Stacks"][0]` has no usable Outputs — the harness builds its output map from `stack.get("Outputs", [])` and the requested key is simply absent, so reading the declared output raises `KeyError: 'TopicArn'` (check_cloudformation__service_catalog_appregistry_appregistry_sync_unassociated_cfn_stack_is_no_action.py line 68, `topic_arn = _outputs(deployed)["TopicArn"]`). Stack creation reported success, so this is not a create failure surfacing late: the output evaluation step produced nothing.

Why: Outputs are the documented contract by which a stack hands identifiers (here an SNS topic ARN) to its consumers. A client that deploys a stack and then addresses the provisioned resource by a stack output gets a missing key instead of an ARN, so every post-deploy lookup, cross-stack wiring and `!Ref`/`!GetAtt`-derived identifier is unobtainable even though the deploy is reported as complete. In this suite the failure also makes the entire cross-service property under test unreachable: nothing after line 68 ran, so the SNS tag-propagation control, AppRegistry `SyncResource` CFN_STACK NO_ACTION behavior and the RESOURCE_TAG_VALUE rejection were never exercised and are neither confirmed nor refuted by this replay. Whether the `AWS::SNS::Topic` resource itself is actually provisioned by the stack is likewise unverified here, since the check never reached an SNS call.

Fix: output evaluation in the CloudFormation stack create/describe path — resolve `Ref` and `Fn::GetAtt` against the provisioned resource's attributes and populate `Stack.Outputs` in the DescribeStacks serializer, in the CloudFormation service (services/service-com-amazonaws-cloudformation-cloudformation).

Related open issue, which does NOT cover this: 20261022T031509Z-xray-cloudformation-never-provisions-aws-xray-group-aws-xray.md is filed under service `xray` and its Cause 2 of 3 describes the same missing-Outputs symptom, but exclusively for templates whose resources are AWS::XRay::Group / AWS::XRay::SamplingRule, and its trace_ids are the five `xray.*` checks. This check uses an AWS::SNS::Topic template and is not listed there and was not reported as owned, so it is filed separately and shows the gap is in the generic output-evaluation layer rather than in an X-Ray resource provider.

Traces: cloudformation__service-catalog-appregistry.appregistry-sync-unassociated-cfn-stack-is-no-action.

## Evidence
Check cloudformation__service-catalog-appregistry.appregistry-sync-unassociated-cfn-stack-is-no-action, verdict `error`:

```
File "check_cloudformation__service_catalog_appregistry_appregistry_sync_unassociated_cfn_stack_is_no_action.py", line 68, in run
    topic_arn = _outputs(deployed)["TopicArn"]
KeyError: 'TopicArn'
```

The harness's `_deploy_cfn` only returns after polling `describe_stacks(StackName=name)["Stacks"][0]["StackStatus"]` to a terminal value and asserting it is exactly `CREATE_COMPLETE`, then builds `outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}`. The KeyError therefore proves the stack was reported CREATE_COMPLETE while its DescribeStacks Stack carried no Output for the declared `TopicArn`.

Template under test (declares exactly one output):
```yaml
Resources:
  ProbeTopic:
    Type: AWS::SNS::Topic
    Properties:
      TopicName: !Ref TopicName
Outputs:
  TopicArn:
    Value: !Ref ProbeTopic
```

Expected (AWS): HTTP 200 DescribeStacks with Stacks[0].Outputs == [{OutputKey: "TopicArn", OutputValue: <ARN of the created topic>}] — one Output entry per declared output, per the modeled Stack.Outputs member.
Actual (emulator): Stacks[0] has no Outputs entry for TopicArn (empty/absent Outputs list), so the declared output value cannot be read.

Not asserted by this evidence: the existence of the SNS topic, the SyncResource NO_ACTION result and the RESOURCE_TAG_VALUE ValidationException — the check aborted before any of those calls.

## Steps to Reproduce
1. Boot the emulator (run_behavioral_suite builds and boots it).
2. Call cloudformation.create_stack with StackName=<name>, Capabilities=[], Parameters=[{ParameterKey: "TopicName", ParameterValue: <unique name>}] and a TemplateBody whose Resources is a single `AWS::SNS::Topic` (`ProbeTopic`, TopicName: !Ref TopicName) and whose Outputs declares `TopicArn: {Value: !Ref ProbeTopic}`.
3. Poll cloudformation.describe_stacks(StackName=<name>)["Stacks"][0]["StackStatus"] until terminal; observe CREATE_COMPLETE.
4. Inspect that same Stack object: there is no Output with OutputKey "TopicArn" (no populated "Outputs" list at all), so `{o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}["TopicArn"]` raises KeyError. AWS returns the topic ARN here.
5. Re-replay cloudformation__service-catalog-appregistry.appregistry-sync-unassociated-cfn-stack-is-no-action to confirm the fix; note that fixing Outputs will then expose the rest of that check (SNS tag propagation onto the topic, AppRegistry SyncResource CFN_STACK NO_ACTION, RESOURCE_TAG_VALUE rejection), which this replay never reached.
