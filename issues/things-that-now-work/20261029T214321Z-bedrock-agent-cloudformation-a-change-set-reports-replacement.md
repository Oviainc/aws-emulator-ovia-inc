Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__cloudformation.kbcfn-change-set-flags-embedding-model-change-as-kb-replacement bedrock-agent__cloudformation.kbcfn-kb-drift-detection-rejected-as-unsupported-type
Issue-Key: bedrock-agent__cloudformation.kbcfn-change-set-flags-embedding-model-change-as-kb-replacement|run#5|AssertionError|assert (rc["Action"], rc["Replacement"]) == ("Modify", "Conditional"), (
Issue-Key: bedrock-agent__cloudformation.kbcfn-kb-drift-detection-rejected-as-unsupported-type|_err#4|AssertionError|raise AssertionError(f"expected a rejection, but it succeeded: {result!r}")
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# bedrock-agent__cloudformation: a change set reports Replacement='True' for an AWS::Bedrock::KnowledgeBase embedding-model change, and drift detection on that type returns NOT_CHECKED instead of refusing (2 causes)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Two net-new behavioral root causes of service `bedrock-agent__cloudformation`, grounded in two failing behavioral checks. Each cause is independent and must be closed separately.

=== Cause 1 of 2: the ResourceChange for an embedding-model change is Replacement='True' where AWS reports 'Conditional' ===
AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json) models `ResourceChange.Replacement` as the `Replacement` enum and documents the rule exactly: Replacement is derived from the changed property's `ResourceTargetDefinition.RequiresRecreation` together with its `Evaluation` — 'if the RequiresRecreation field is Always and the Evaluation field is Static, Replacement is True. If the RequiresRecreation field is Always and the Evaluation field is Dynamic, Replacement is Conditional.' For an AWS::Bedrock::KnowledgeBase whose embedding-model property changes, the recorded AWS answer is Action='Modify' with Replacement='Conditional' — the recreation is required but the evaluation is Dynamic, so CloudFormation cannot promise replacement up front.
Emulator: the same change comes back Action='Modify' with Replacement='True'.
Why: Replacement is the field a reviewer and every change-set gate reads to decide whether executing the plan destroys data. 'True' asserts the knowledge base WILL be recreated; 'Conditional' says it may be. Automation that blocks on Replacement='True' refuses a change AWS would let through (and conversely a reviewer trusting 'Conditional' elsewhere is mis-served), so the emulator's plan is not the plan AWS would execute.
Fix: the Replacement derivation in the cloudformation service's change-set change-computation — it must honour the modelled rule (RequiresRecreation x Evaluation) for the AWS::Bedrock::KnowledgeBase embedding-model property instead of defaulting a recreating change to 'True'.
Traces: bedrock-agent__cloudformation.kbcfn-change-set-flags-embedding-model-change-as-kb-replacement.

=== Cause 2 of 2: drift detection on an unsupported resource type returns a NOT_CHECKED result instead of refusing ===
AWS: Smithy models DetectStackResourceDrift with required StackName/LogicalResourceId and a required `StackResourceDrift` output whose `StackResourceDriftStatus` enum is [IN_SYNC, MODIFIED, DELETED, NOT_CHECKED, UNKNOWN, UNSUPPORTED], and the model's own documentation states NOT_CHECKED is a value 'CloudFormation doesn't currently return'. Recorded behavior for a resource type that does not support drift detection (AWS::Bedrock::KnowledgeBase) is that the call is REFUSED, not answered — the check asserts a rejection.
Emulator: answers HTTP 200 with StackResourceDrift {LogicalResourceId, PhysicalResourceId, ResourceType='AWS::Bedrock::KnowledgeBase', StackResourceDriftStatus='NOT_CHECKED'} — a success carrying precisely the status the model says is never returned.
Why: a client that asks for drift and gets 200/NOT_CHECKED concludes the resource was examined and is not drifted, when in fact nothing was checked and AWS would have told it the type is unsupported. Drift-reporting dashboards therefore show clean where AWS shows an error or UNSUPPORTED.
Fix: the DetectStackResourceDrift handler in the cloudformation service's drift ops — gate on whether the resource type supports drift detection and refuse for the unsupported ones instead of emitting a NOT_CHECKED result.
Traces: bedrock-agent__cloudformation.kbcfn-kb-drift-detection-rejected-as-unsupported-type.

## Evidence
Cause 1 — bedrock-agent__cloudformation.kbcfn-change-set-flags-embedding-model-change-as-kb-replacement [mismatch], check line 50:
  assert (rc["Action"], rc["Replacement"]) == ("Modify", "Conditional")
  AssertionError: EscalationKB: Action='Modify' Replacement='True'
  Smithy: ResourceChange.Replacement (Replacement enum) — documented derivation: RequiresRecreation=Always + Evaluation=Static -> True; RequiresRecreation=Always + Evaluation=Dynamic -> Conditional.

Cause 2 — bedrock-agent__cloudformation.kbcfn-kb-drift-detection-rejected-as-unsupported-type [mismatch], check line 43:
  AssertionError: expected a rejection, but it succeeded: {'StackResourceDrift': {'StackId': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-kbcfn-91c460c963d0-bedrock-kb/92bd1be3-...', 'LogicalResourceId': 'MetadataKB', 'PhysicalResourceId': 'sim2real-kbcfn-91c460c963d0-metadata-kb-g1', 'ResourceType': 'AWS::Bedrock::KnowledgeBase', 'StackResourceDriftStatus': 'NOT_CHECKED', 'Timestamp': ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Smithy: DetectStackResourceDrift output StackResourceDrift required; StackResourceDriftStatus enum = [IN_SYNC, MODIFIED, DELETED, NOT_CHECKED, UNKNOWN, UNSUPPORTED], with NOT_CHECKED documented as a value CloudFormation does not return.
  Stack/resource names, ARNs and timestamps above are per-run values.

## Steps to Reproduce
Cause 1:
1. Deploy a stack containing an AWS::Bedrock::KnowledgeBase.
2. CreateChangeSet against it with only the knowledge base's embedding-model property changed, and DescribeChangeSet.
3. Read the ResourceChange for that logical id: observe Action='Modify', Replacement='True'. AWS reports Replacement='Conditional'.

Cause 2:
1. On that same stack call DetectStackResourceDrift(StackName=<stack>, LogicalResourceId=<the knowledge base>).
2. Observe HTTP 200 with StackResourceDriftStatus='NOT_CHECKED'. AWS refuses drift detection for this unsupported resource type.
3. Re-replay: bedrock-agent__cloudformation.kbcfn-change-set-flags-embedding-model-change-as-kb-replacement, bedrock-agent__cloudformation.kbcfn-kb-drift-detection-rejected-as-unsupported-type.
