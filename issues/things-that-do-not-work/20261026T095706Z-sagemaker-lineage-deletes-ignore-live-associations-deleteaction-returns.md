Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.lineage-entity-association-lifecycle
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.lineage-entity-association-lifecycle|_error#21|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: bb568fea88423ace8b4c8a03ddba0a6ee612948a

# SageMaker lineage deletes ignore live associations: DeleteAction returns 200 for an action that is still the source of an association instead of ValidationException "Cannot delete entity with associations."

> _Note: this issue was identified against the code base at commit `bb568fea88423ace8b4c8a03ddba0a6ee612948a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAction

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: the SageMaker lineage entity deletes perform no referential-integrity check against the association graph, so an entity that is still an endpoint of a live association is deleted and answered 200.

AWS: Smithy models DeleteAction with input ActionName (ExperimentEntityName, @length 1..120, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$), output ActionArn, and exactly one modeled error, ResourceNotFound (HTTP 400, "Resource being access is not found."). The model says NOTHING about the association guard — ValidationException is not in DeleteAction's modeled error list — so this requirement is recorded behavior only: real SageMaker refuses to delete a lineage entity (Action/Context/Artifact, created by CreateAction/CreateContext/CreateArtifact and linked by AddAssociation) while that entity is still the Source of a live association, answering HTTP 400 with error code `ValidationException` and the message `Cannot delete entity with associations.` That message is entirely FIXED text for this error class — it carries no value from the request or from state (no entity name, ARN or count is interpolated). Real AWS does not silently cascade the delete: the association survives the refusal, and only after an explicit DeleteAssociation(SourceArn, DestinationArn) does the DeleteAction succeed. Associations whose SOURCE was a since-deleted entity stop being listed by ListAssociations, while an unrelated association sourced from a still-existing entity is untouched.

Emulator: DeleteAction on an action that is still the Source of an association created by AddAssociation succeeds. The replay recorded HTTP 200 with body {"ActionArn": "arn:aws:sagemaker:<region>:<account>:action/<action-name>"} (the recorded run's ARN value fills the template; the ARN itself is not the requirement) where a 400 ValidationException was required. services/service-com-amazonaws-sagemaker-sagemaker/src/ops/delete_action.rs removes the entry from `store.actions` and returns ActionArn with no consultation of the association graph at all; its only refusal path is ResourceNotFound for an unknown name. The sibling lineage deletes have the identical shape — services/service-com-amazonaws-sagemaker-sagemaker/src/ops/delete_context.rs likewise does `store.contexts.remove(&key)` with no association lookup — so the gap is the whole lineage-delete family, not one operation; the grounding check exercises it through DeleteAction.

Why: a client relying on AWS's behavior gets two wrong answers. First, code that uses the refusal as the referential-integrity signal (catch ValidationException -> DeleteAssociation -> retry the delete) never sees the refusal, so the cleanup-ordering branch is dead and the bug it guards against is never caught in the emulator. Second, and worse, the lineage graph is left inconsistent: the entity is gone while an association still names it, so a subsequent ListAssociations/DescribeAction pairing returns an edge pointing at an entity that no longer exists — a state real SageMaker makes unreachable. Negative-path tests for lineage cleanup pass against the emulator and fail against AWS.

Fix: belongs in the DeleteAction handler's validation step in services/service-com-amazonaws-sagemaker-sagemaker/src/ops/delete_action.rs — before removing the entry from `store.actions`, resolve the entity ARN and query the association store for any association whose SourceArn is that ARN, and return ValidationException with the fixed message above when one exists, leaving the store untouched. The same guard belongs in the sibling lineage deletes (delete_context.rs, delete_artifact.rs) and, if the association index is not currently queryable by endpoint ARN, in the shared lineage state that holds the associations.

Traces: sagemaker.lineage-entity-association-lifecycle — creates an Action, a Context and an Artifact, adds action->context and context->artifact associations, then calls DeleteAction on the action and asserts the call is rejected with ValidationException / "Cannot delete entity with associations."

No currently open issue covers this: the open SageMaker issues concern job execution (training/processing/pipeline runtimes), CloudFormation provisioning, S3-artifact validation, control-plane 501 stubs (including 20261025T101032Z-sagemaker-associatetrialcomponent-stub.md for AssociateTrialComponent) and Feature Store ordering — none of them touches the Action/Context/Artifact association graph or DeleteAction.

## Evidence
Check: sagemaker.lineage-entity-association-lifecycle (verdict: mismatch).

Expected (real AWS, recorded behavior; DeleteAction on an entity that is still a live association source):
  HTTP 400
  Error.Code    = "ValidationException"
  Error.Message = "Cannot delete entity with associations."   <- 100% fixed text, no request/state values
  side effect: none — ListAssociations(SourceArn=<action arn>) still returns 1 summary

Actual (emulator):
  HTTP 200
  body = {"ActionArn": "arn:aws:sagemaker:us-west-2:000000000000:action/sim2real-behav-action-6e83f7a29ce7"}
  (region/account/name are this run's values filling the ARN template, not part of the requirement)
  side effect: the action is removed from state while the association naming it survives

Replay failure:
  File "check_sagemaker_lineage_entity_association_lifecycle.py", line 70, in run
    code, message = _error(lambda: sm.delete_action(ActionName=action_name))
  File "check_sagemaker_lineage_entity_association_lifecycle.py", line 112, in _error
    raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ActionArn': 'arn:aws:sagemaker:us-west-2:000000000000:action/sim2real-behav-action-6e83f7a29ce7', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Model citation: Smithy com.amazonaws.sagemaker#DeleteAction — input {ActionName: ExperimentEntityName, required, length 1..120}; output {ActionArn}; errors = [ResourceNotFound (client, HTTP 400)]. ValidationException is NOT modeled for DeleteAction, hence "recorded behavior only" for the guard and its message.

Code citation: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/delete_action.rs lines 147-158 — `store.actions.remove(&arn)`; the only error branch is ResourceNotFound; no association lookup exists. Same shape in services/service-com-amazonaws-sagemaker-sagemaker/src/ops/delete_context.rs lines 141-149.

## Steps to Reproduce
1. Boot the emulator and point a botocore sagemaker client at it.
2. action_arn = sm.create_action(ActionName=<name>, Source={"SourceUri": "s3://<bucket>/<name>"}, ActionType="ModelDeployment")["ActionArn"]
3. context_arn = sm.create_context(ContextName=<name2>, Source={"SourceUri": "s3://<bucket>/<name2>", "SourceType": "Endpoint"}, ContextType="Endpoint")["ContextArn"]
4. sm.add_association(SourceArn=action_arn, DestinationArn=context_arn, AssociationType="ContributedTo")
5. Confirm the edge exists: len(sm.list_associations(SourceArn=action_arn)["AssociationSummaries"]) == 1
6. sm.delete_action(ActionName=<name>)
   EXPECTED: ClientError, HTTP 400, Code "ValidationException", Message "Cannot delete entity with associations.", and the association from step 4 still listed.
   ACTUAL: HTTP 200 with {"ActionArn": ...}; the action is deleted while the association naming it remains.
7. For the positive half AWS also requires: after sm.delete_association(SourceArn=action_arn, DestinationArn=context_arn), delete_action succeeds, describe_action then returns ResourceNotFound, and the unrelated context->artifact association still lists 1 summary.

Replay command: re-run behavioral check sagemaker.lineage-entity-association-lifecycle.
