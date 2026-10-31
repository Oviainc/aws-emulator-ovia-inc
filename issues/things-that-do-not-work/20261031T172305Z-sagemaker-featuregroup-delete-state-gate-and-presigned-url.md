Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-feature-group-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-feature-group-lifecycle sagemaker-notebook-instance-error-paths
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DeleteFeatureGroup|4xx->2xx|$
Issue-Key: sagemaker|DeleteFeatureGroup|4xx->2xx|status
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$.CreationTime
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$.Description
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$.EventTimeFeatureName
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$.FeatureDefinitions
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$.FeatureGroupArn
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$.FeatureGroupName
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$.FeatureGroupStatus
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$.OnlineStoreConfig
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$.OnlineStoreReadWriteType
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$.OnlineStoreReplicas
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$.OnlineStoreTotalItemCount
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$.OnlineStoreTotalSizeBytes
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$.RecordIdentifierFeatureName
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|$.ThroughputConfig
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->4xx|status
Issue-Key: sagemaker|DeleteFeatureGroup|2xx->4xx|$
Issue-Key: sagemaker|DeleteFeatureGroup|2xx->4xx|status
Issue-Key: sagemaker|CreatePresignedNotebookInstanceUrl|4xx->2xx|$.__type
Issue-Key: sagemaker|CreatePresignedNotebookInstanceUrl|4xx->2xx|$.message
Issue-Key: sagemaker|CreatePresignedNotebookInstanceUrl|4xx->2xx|status
Issue-Commit: 28fd4137d5c77ba3e39f3c32a85f48747657e064

# SageMaker FeatureGroup delete state gate and presigned-URL existence check are missing

> _Note: this issue was identified against the code base at commit `28fd4137d5c77ba3e39f3c32a85f48747657e064`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFeatureGroup

## Description
This issue bundles 4 causes (3 of them one root fault) with a shared defect: SageMaker handlers act on a resource without first checking its precondition — neither the resource's non-terminal FeatureGroupStatus before deleting it, nor a notebook instance's existence before minting a presigned URL. The delete path therefore destroys a live resource it should have refused, and every later read of that resource diverges; the presigned-URL path hands back a URL for a resource that does not exist.

Cause 1 of 4: DeleteFeatureGroup called while the feature group is still being created. AWS: refuses with HTTP 400, __type 'ValidationException' and the message "Validation Error: FeatureGroup with name <FeatureGroupName> cannot be deleted, because FeatureGroupStatus is '<FeatureGroupStatus>'." — the fixed text is "Validation Error: FeatureGroup with name " / " cannot be deleted, because FeatureGroupStatus is '" / "'."; the variable parts are the FeatureGroupName taken from the request and the group's current FeatureGroupStatus (here 'CREATING') taken from emulator state. Smithy: DeleteFeatureGroup takes only FeatureGroupName (required, @length 1-64, @pattern ^[a-zA-Z0-9]([_-]*[a-zA-Z0-9]){0,63}$), returns Unit and models only ResourceNotFound (httpError 400), so the ValidationException wording for the state gate is recorded behavior only. Emulator: answers HTTP 200 with an empty body and removes the group. Why: a client that creates a feature group and issues a delete before it settles sees the delete succeed instead of being told to wait, and the group is destroyed mid-creation — the opposite outcome from AWS, so retry/backoff logic keyed on the ValidationException never runs. Fix: the DeleteFeatureGroup handler's precondition/state gate (it must consult FeatureGroupStatus and refuse any non-terminal status before touching state). Traces: sagemaker-feature-group-lifecycle#task#3.

Cause 2 of 4: DescribeFeatureGroup polled for the group to finish creating. AWS: HTTP 200 with the group body, FeatureGroupStatus reaching 'Created' and the modeled required members present (FeatureGroupArn, FeatureGroupName, RecordIdentifierFeatureName, EventTimeFeatureName, FeatureDefinitions, CreationTime — Smithy DescribeFeatureGroup output marks these required). Emulator: HTTP 400 with __type 'ResourceNotFound' and "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name <FeatureGroupName>" for the whole poll window (capped at 60s), because the group was destroyed by cause 1. Why: a client waiting for FeatureGroupStatus 'Created' can never proceed; it sees a resource it just created reported as absent. Fix: same component as cause 1 — the group must survive the refused delete and settle to 'Created' on the read path. Traces: sagemaker-feature-group-lifecycle#task#5.

Cause 3 of 4: DeleteFeatureGroup issued legitimately once the group is in a terminal state. AWS: HTTP 200 with an empty body (Smithy output shape Unit). Emulator: HTTP 400 __type 'ResourceNotFound', "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name <FeatureGroupName>" — again a consequence of cause 1 having already removed it. Why: teardown of a feature group fails with a not-found error, so cleanup code that treats a delete failure as fatal aborts. Fix: same component as cause 1. Traces: sagemaker-feature-group-lifecycle#task#6.

Cause 4 of 4: CreatePresignedNotebookInstanceUrl naming a notebook instance that does not exist. AWS: HTTP 400 with __type 'ValidationException' and the message "Notebook Instance <notebook instance ARN> does not exist" — fixed text "Notebook Instance " / " does not exist", with the ARN formed from the NotebookInstanceName in the request plus the request's account and region. Smithy: CreatePresignedNotebookInstanceUrl models NO errors (input NotebookInstanceName required, @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$; output AuthorizedUrl), so this refusal is recorded behavior only. Emulator: HTTP 200 carrying an AuthorizedUrl for the non-existent instance. Why: a caller that mistypes or races a notebook-instance name is handed a URL instead of the ValidationException, so the naming error surfaces much later as an unusable URL rather than at the API boundary. Fix: the CreatePresignedNotebookInstanceUrl handler's existence check, before the URL is minted. Traces: sagemaker-notebook-instance-error-paths#task#2.

Related but NOT covering these steps: open issue 20261029T192819Z-sagemaker-delete-and-stop-paths-lack-not-found.md describes the same family of missing gates (its cause 2 is DescribeFeatureGroup after a delete, fixture step sagemaker-feature-group-lifecycle#task#7) and open issue 20261030T213859Z-sagemaker-stopprocessingjob-accepts-an-absent-processing-job-with.md covers StopProcessingJob; neither owns the four steps filed here.

## Evidence
Cause 1 (sagemaker-feature-group-lifecycle#task#3, DeleteFeatureGroup): expected HTTP 400 {'__type': 'ValidationException', 'message': "Validation Error: FeatureGroup with name <FeatureGroupName> cannot be deleted, because FeatureGroupStatus is 'CREATING'."}; actual HTTP 200 with an empty body {} and the group removed.
Cause 2 (sagemaker-feature-group-lifecycle#task#5, DescribeFeatureGroup): expected HTTP 200 with FeatureGroupStatus 'Created' plus FeatureGroupArn/FeatureGroupName/RecordIdentifierFeatureName/EventTimeFeatureName/FeatureDefinitions/CreationTime/OnlineStoreConfig/ThroughputConfig; actual HTTP 400 {'__type': 'ResourceNotFound', 'Message': "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name <FeatureGroupName>"} for every attempt (poll capped at 60s).
Cause 3 (sagemaker-feature-group-lifecycle#task#6, DeleteFeatureGroup): expected HTTP 200 {}; actual HTTP 400 {'__type': 'ResourceNotFound', 'Message': "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name <FeatureGroupName>"}.
Cause 4 (sagemaker-notebook-instance-error-paths#task#2, CreatePresignedNotebookInstanceUrl): expected HTTP 400 {'__type': 'ValidationException', 'message': 'Notebook Instance <notebook instance ARN> does not exist'}; actual HTTP 200 with an AuthorizedUrl and no __type/message member.

## Steps to Reproduce
Replay the wire corpus (replay_wire_corpus). Representative: fixture sagemaker-feature-group-lifecycle, step task#3. The other three causes are the fixture#step bindings in trace_ids.
1) CreateFeatureGroup, then immediately DeleteFeatureGroup while FeatureGroupStatus is still 'CREATING' -> emulator answers 200 instead of the ValidationException state gate (task#3).
2) Poll DescribeFeatureGroup for FeatureGroupStatus 'Created' -> emulator answers 400 ResourceNotFound for the whole window (task#5).
3) DeleteFeatureGroup once the group should be terminal -> emulator answers 400 ResourceNotFound instead of 200 (task#6).
4) Call CreatePresignedNotebookInstanceUrl with a NotebookInstanceName that was never created -> emulator answers 200 with an AuthorizedUrl instead of 400 ValidationException 'Notebook Instance <arn> does not exist' (sagemaker-notebook-instance-error-paths#task#2).
