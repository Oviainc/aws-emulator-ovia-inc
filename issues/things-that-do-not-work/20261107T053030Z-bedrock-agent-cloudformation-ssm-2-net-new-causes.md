Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__cloudformation__ssm.kbcfn-non-replacing-kb-update-succeeds-while-kb-ids-imported bedrock-agent__cloudformation__ssm.kbcfn-embedding-model-replacement-rejected-while-kb-ids-imported
Issue-Key: bedrock-agent__cloudformation__ssm.kbcfn-non-replacing-kb-update-succeeds-while-kb-ids-imported|run#7|AssertionError|assert status == "UPDATE_COMPLETE", (
Issue-Key: bedrock-agent__cloudformation__ssm.kbcfn-embedding-model-replacement-rejected-while-kb-ids-imported|run#9|AssertionError|assert len(hits) == 1, (
Issue-Commit: dcaafa445ca563c5947bd284ba4541134d1f4f9e

# bedrock-agent__cloudformation__ssm: 2 net-new causes — the in-use-export guard cancels an UpdateStack that does not change the exported value, and the 'Cannot update export … in use by …' cancellation reason is recorded three times

> _Note: this issue was identified against the code base at commit `dcaafa445ca563c5947bd284ba4541134d1f4f9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
TWO net-new root causes of service `bedrock-agent__cloudformation__ssm` (the heading the suite report lists these checks under). Each is grounded in its own failing behavioral check that the replay report left UNMARKED (neither ALREADY FILED nor OWNED). A fixer must close BOTH before submitting.

Context on what is already filed: open issue 20261102T104226Z-bedrock-agent-cloudformation-ssm-5-net-new-causes.md lists these same two check ids, but it records the OPPOSITE emulator behavior — its Cause 1 says an update that changes an in-use export is wrongly ALLOWED to complete with no cancellation reason at all. The emulator now cancels; what diverges is WHICH updates it cancels and HOW MANY TIMES it records the reason, so both causes below are new observations of the current build and neither is covered by that issue's replay keys. Each cause names the related open issue it sits beside.

=== Cause 1 of 2: an UpdateStack that leaves every in-use export's VALUE unchanged is cancelled and rolled back instead of completing ===
AWS: recorded behavior only. The cloudformation Smithy model expresses nothing about cross-stack export protection — it supplies only the operations used to observe it (Smithy: UpdateStack(StackName required, TemplateBody, Parameters, Capabilities) -> StackId, modeled errors InsufficientCapabilitiesException/TokenAlreadyExistsException only; DescribeStackEvents -> StackEvents, paginated). Recorded behavior: an imported export pins a VALUE, not a stack. An update of the exporting stack is cancelled ONLY when the exported value would change or the Export would disappear; an in-place update (every changed property RequiresRecreation: Never — here an AWS::Bedrock::KnowledgeBase Description and Name edit) leaves the minted knowledgeBaseId, and therefore the Output/Export value, identical, so the stack reaches UPDATE_COMPLETE, the Outputs keep their previous values, ListImports still names the importing stack, and the resource read-back reports the new name/description.
Emulator: the same in-place update ends UPDATE_ROLLBACK_COMPLETE, with the stack-level reason 'Update canceled. Cannot update export <kb-stack>-EscalationWriteupsKBKnowledgeBaseId as it is in use by <agent-stack>.' — the guard fires even though no exported value had to move. Hypothesis: the guard is comparing physical ids that the emulator derives from the template Name (open issue 20261102T104226Z… Cause 5 records that id-tracks-Name defect for this very check), so a pure rename appears to the guard as a changed export; it may equally be that the guard fires on any update of a stack with a registered importer without diffing the resolved value. Either way the fix must make a value-preserving update complete.
Why: a client relying on AWS's behavior edits descriptions, tags and names on an exporting stack freely while consumers import it; export protection is supposed to catch only changes that would move the value under a consumer. Against the emulator every routine in-place update of any stack that has an importer is refused and rolled back, so the exporting stack becomes permanently un-updatable until every consumer is torn down — deploy pipelines fail on a change that AWS applies.
Fix: the in-use-export pre-condition on the UpdateStack execution path in the cloudformation service (services/service-com-amazonaws-cloudformation-cloudformation) — resolve the post-update Outputs/Exports, compare each in-use export's value with its current value, and cancel only when a value changes or the export is removed; resolve exported resource ids from the service-minted physical id so a non-replacing rename does not register as a change.
Traces: bedrock-agent__cloudformation__ssm.kbcfn-non-replacing-kb-update-succeeds-while-kb-ids-imported.

=== Cause 2 of 2: the update-cancelled-by-an-in-use-export reason is recorded THREE times among the stack-level events instead of once ===
AWS: recorded behavior only. One cancelled update produces ONE stack-level explanation: among the DescribeStackEvents entries whose LogicalResourceId is the stack itself and which carry a ResourceStatusReason, exactly one matches `Cannot update export <export-name> as it is in use by <importer-stack>.` — fixed text `Cannot update export `, ` as it is in use by ` and the terminating `.`, with `<export-name>` the exporter's export name and `<importer-stack>` the name of the stack holding the import, both taken from state. The stack's own 'User Initiated' reasons are one per user-initiated operation.
Emulator: on a correctly cancelled replacing update (embedding-model change on an imported KB stack, which AWS does refuse) the status UPDATE_ROLLBACK_COMPLETE and the sentence are right, but the cancellation reason appears THREE times in the stack-level reasons, so a client counting failures sees three cancellations where AWS reports one. Only the number of stack-level events carrying the reason is wrong.
Why: DescribeStackEvents is the audit trail deploy tooling reads to decide what went wrong and whether one root cause or several resources failed. Duplicated stack-level reasons inflate that count, break 'exactly one cancellation' assertions, and misattribute a resource-level observation to the stack.
Fix: the stack-event recorder in the cloudformation service's update/rollback path — emit the cancellation reason once, on the rollback transition, and attach any per-resource copy to the failing resource's LogicalResourceId instead of appending the stack-level reason on every transition that observes the in-use export. NOTE: open issue 20261102T104226Z… Cause 3 reports the identical triple-emission for the sibling 'No export named <name> found.' rollback reason; that issue carries only the kbcfn-import-of-a-missing-kb-export-rejected replay key, not this one. If the two share one recorder, one fix may close both — re-replay this check to confirm.
Traces: bedrock-agent__cloudformation__ssm.kbcfn-embedding-model-replacement-rejected-while-kb-ids-imported.

## Evidence
Cause 1 — bedrock-agent__cloudformation__ssm.kbcfn-non-replacing-kb-update-succeeds-while-kb-ids-imported [mismatch]:
  AssertionError (check line 43): "an in-place update of an imported KB stack ended 'UPDATE_ROLLBACK_COMPLETE': ['Update canceled. Cannot update export sim2real-kbcfn-9a8bc693cba3-bedrock-kb-EscalationWriteupsKBKnowledgeBaseId as it is in use by sim2real-kbcfn-9a8bc693cba3-bedrock-agent.', (same sentence), (same sentence)]"
  Expected: StackStatus 'UPDATE_COMPLETE', both KnowledgeBaseId Outputs unchanged, ListImports on each export still == [agent stack], GetKnowledgeBase reporting the edited description/name.
  Actual: 'UPDATE_ROLLBACK_COMPLETE' with an export-in-use cancellation, although the only changed properties (Description, Name) are RequiresRecreation: Never and AWS keeps the exported id. The stack and export names quoted are this run's generated names filling the template slots, not part of the requirement.

Cause 2 — bedrock-agent__cloudformation__ssm.kbcfn-embedding-model-replacement-rejected-while-kb-ids-imported [mismatch]:
  AssertionError (check line 48): "expected exactly one 'Cannot update export ... in use by ...' cancellation, got reasons ['Update canceled. Cannot update export <kb-stack>-EscalationWriteupsKBKnowledgeBaseId as it is in use by <agent-stack>.', (same sentence), (same sentence), 'User Initiated', 'User Initiated']"
  Expected: exactly ONE stack-level reason matching ^.*Cannot update export (\S+) as it is in use by (\S+?)\.?$ .
  Actual: three identical copies of it. The status (UPDATE_ROLLBACK_COMPLETE) and the sentence's wording — fixed text 'Cannot update export ' / ' as it is in use by ' / terminating '.', with the export name and importer stack name taken from state — are correct; the event count is not.

## Steps to Reproduce
0. git_clone, then run_behavioral_suite and read the `### bedrock-agent__cloudformation__ssm` section of the failing-check list; neither check below is marked ALREADY FILED or OWNED.

Shared setup (both causes): CreateStack a 'KB stack' with Capabilities=['CAPABILITY_IAM'] from a template holding an AWS::S3Vectors::VectorBucket, two AWS::S3Vectors::Index (1024-d, cosine), an AWS::IAM::Role and two AWS::Bedrock::KnowledgeBase resources (S3_VECTORS storage, embeddingModelArn arn:aws:bedrock:<region>::foundation-model/amazon.titan-embed-text-v2:0) whose Outputs export each KnowledgeBaseId as '${AWS::StackName}-<OutputKey>'; wait for CREATE_COMPLETE. Then CreateStack an 'agent stack' whose AWS::SSM::Parameter Value Fn::Joins both ids via Fn::ImportValue '${KbStackName}-<OutputKey>'; wait for CREATE_COMPLETE. ListImports on each export must name the agent stack.

Cause 1: UpdateStack the KB stack with the SAME embedding model, an edited Description on one knowledge base and a new Generation suffix (all RequiresRecreation: Never). Observe UPDATE_ROLLBACK_COMPLETE with an 'Update canceled. Cannot update export … as it is in use by …' reason. Required: UPDATE_COMPLETE, the two KnowledgeBaseId Outputs byte-identical to before, each export still imported by the agent stack, and the knowledge base reporting its new description and name.

Cause 2: UpdateStack the KB stack with EmbeddingModelId=cohere.embed-english-v3 and a new Generation suffix (a replacing change AWS does refuse). The status UPDATE_ROLLBACK_COMPLETE is correct; now page DescribeStackEvents for the KB stack, keep every event whose LogicalResourceId == the stack name and which has a ResourceStatusReason, and count the reasons matching 'Cannot update export <export> as it is in use by <importer>.' — observe three, required exactly one.

Re-replay both: the check ids in trace_ids.
