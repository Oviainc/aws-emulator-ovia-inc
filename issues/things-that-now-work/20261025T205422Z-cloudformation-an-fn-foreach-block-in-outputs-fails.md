Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.cfn-foreach-language-extensions-expands-to-24-working-queues-and-rejects-missing-capability
Issue-Key: cloudformation__sns__sqs.cfn-foreach-language-extensions-expands-to-24-working-queues-and-rejects-missing-capability|run#8|AssertionError|assert stack["StackStatus"] == "CREATE_COMPLETE", (
Issue-Commit: cc62856ae07f45464bafde33e00f4ba8357575c6

# cloudformation: an Fn::ForEach block in Outputs fails the whole stack CREATE_FAILED — the AWS::LanguageExtensions collection given as !Ref to a CommaDelimitedList parameter is not resolved

> _Note: this issue was identified against the code base at commit `cc62856ae07f45464bafde33e00f4ba8357575c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
This issue carries 1 net-new root cause in CloudFormation's AWS::LanguageExtensions transform. The open issue 20261025T015306Z-cloudformation-template-transforms-and-intrinsics-are-not-processed.md lists this same check id under Cause 1 ('Fn::ForEach is not expanded', emulator recorded only the hand-written Sentinel resource), but it does not carry this replay key and does not describe this failure mode: the emulator no longer silently drops the loop — it now fails the stack outright, and it does so on the Outputs-section loop with an emulator-authored reason. That open issue is named here so the two are not fixed twice, but the behaviour below is a different defect in the same processor and is not covered by its description.

Cause 1 of 1: an Fn::ForEach whose collection is `!Ref <CommaDelimitedList parameter>` is not resolvable by the transform, and the stack is failed instead.

AWS: with `Transform: AWS::LanguageExtensions` and CreateStack Capabilities=['CAPABILITY_AUTO_EXPAND'], an `Fn::ForEach::<name>` block takes [<loop variable>, <collection>, <output/resource template>] and expands to one entry per collection element before graph construction. The collection may be given as `!Ref` to a template Parameter of Type CommaDelimitedList (here `Ids`, defaulted to the 24 single-letter slots a..x), and the expansion applies to the Outputs section exactly as it does to Resources — so the recorded template yields 24 queues plus the non-loop Sentinel topic, 24 generated Outputs (`Urla`..`Urlx`), and the stack reaches CREATE_COMPLETE. Recorded behavior only for the terminal status wording: the Smithy model carries no transform semantics; the recording shows the stack reaching CREATE_COMPLETE for this template.

Emulator: the stack reaches CREATE_FAILED and the stack-status reason is `Fn::ForEach::Urls: the collection is not a list this service can resolve from the template alone`. That text is emulator-authored prose — 'the collection is not a list this service can resolve from the template alone' appears in no AWS message template — so even on the (hypothetical) templates where AWS does refuse an unresolvable collection, the refusal would not match AWS's wording. The failing section is the Outputs loop (`Fn::ForEach::Urls`), which means the transform reaches the Outputs section but cannot dereference `!Ref Ids` to the parameter's comma-delimited list value there.

Why: a client deploying any language-extensions template whose loop collection comes from a CommaDelimitedList parameter — the form the AWS documentation itself uses — gets a hard CREATE_FAILED instead of a deployed stack, so none of the 24 queues, none of the generated Outputs, and not even the unrelated Sentinel topic survive. A client that handles stack failure by reading the reason gets a string it cannot map to any AWS condition.

Fix: the AWS::LanguageExtensions / Fn::ForEach expansion pass in the cloudformation template pipeline must resolve the loop collection through the same intrinsic evaluator used elsewhere (so `Ref` to a CommaDelimitedList parameter yields that parameter's list) and must apply the expansion to Outputs as well as Resources, ahead of graph construction; it must not fail the stack for a collection AWS resolves.

Traces: cloudformation__sns__sqs.cfn-foreach-language-extensions-expands-to-24-working-queues-and-rejects-missing-capability.

## Evidence
Replayed suite, check cloudformation__sns__sqs.cfn-foreach-language-extensions-expands-to-24-working-queues-and-rejects-missing-capability (mismatch).

Template (abridged): `Transform: AWS::LanguageExtensions`; `Parameters: Ids: {Type: CommaDelimitedList, Default: a,b,...,x}`; `Resources: 'Fn::ForEach::Fanned': [Slot, !Ref Ids, {'Q${Slot}': AWS::SQS::Queue ...}]` plus a non-loop `Sentinel: AWS::SNS::Topic`; `Outputs: 'Fn::ForEach::Urls': [Slot, !Ref Ids, {'Url${Slot}': {Value: !Ref {'Fn::Sub': 'Q${Slot}'}}}]`. CreateStack is sent with Capabilities=['CAPABILITY_AUTO_EXPAND'].

Expected: the stack reaches StackStatus 'CREATE_COMPLETE' (then 24 distinct, independently usable queues and 24 Url* Outputs).

Actual: `AssertionError: the Fn::ForEach stack ended 'CREATE_FAILED': 'Fn::ForEach::Urls: the collection is not a list this service can resolve from the template alone'` (check line 189, after _wait_stack_status saw the terminal state).

The failing status reason names the Outputs-section loop key, and the quoted reason text is emulator prose, not an AWS message template.

## Steps to Reproduce
1. Boot the emulator and replay cloudformation__sns__sqs.cfn-foreach-language-extensions-expands-to-24-working-queues-and-rejects-missing-capability.
2. Direct repro: cloudformation CreateStack with Capabilities=['CAPABILITY_AUTO_EXPAND'] and a template that declares `Transform: AWS::LanguageExtensions`, a Parameter `Ids` of Type CommaDelimitedList with a multi-element Default, a Resources entry `'Fn::ForEach::Fanned': [Slot, !Ref Ids, {'Q${Slot}': {Type: AWS::SQS::Queue, Properties: {QueueName: !Sub '${Prefix}-${Slot}'}}}]`, one non-loop resource, and an Outputs entry `'Fn::ForEach::Urls': [Slot, !Ref Ids, {'Url${Slot}': {Value: !Ref {'Fn::Sub': 'Q${Slot}'}}}]`.
3. Poll DescribeStacks until terminal. AWS: CREATE_COMPLETE. Emulator: CREATE_FAILED with StackStatusReason 'Fn::ForEach::Urls: the collection is not a list this service can resolve from the template alone'.
4. Narrow the cause: remove only the Outputs Fn::ForEach block and re-create — the failure reason no longer names Urls, isolating the unresolved `!Ref <CommaDelimitedList>` collection in the Outputs expansion.
