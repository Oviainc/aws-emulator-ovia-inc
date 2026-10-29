Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.model-package-lifecycle-transitions sagemaker.trial-delete-with-attached-component-rejected sagemaker.create-experiment-duplicate-name-fails sagemaker.create-feature-group-duplicate-name-rejected sagemaker.feature-group-duplicate-name-rejected sagemaker.create-feature-group-malformed-schema-rejected sagemaker.create-model-package-duplicate-name-conflict sagemaker.create-model-package-duplicate-name-version-conflict-rejected sagemaker.create-model-package-duplicate-version-rejected sagemaker.batch-add-cluster-nodes-cluster-not-found sagemaker.batch-add-cluster-nodes-missing-cluster-not-found sagemaker.cluster-node-batch-add-delete-lifecycle sagemaker.cluster-node-batch-lifecycle sagemaker.attach-cluster-node-volume-cluster-not-found sagemaker.attach-cluster-node-volume-missing-node-not-found sagemaker.attach-cluster-node-volume-nonexistent-node-not-found-rejected sagemaker.get-search-suggestions-matches-created-resource
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.model-package-lifecycle-transitions|run#21|AssertionError|raise AssertionError(f"expected a 'does not exist' ValidationException after delete, got {code}: {message}")
Issue-Key: sagemaker.trial-delete-with-attached-component-rejected|run#19|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker.create-experiment-duplicate-name-fails|run#6|AssertionError|assert message == (
Issue-Key: sagemaker.create-feature-group-duplicate-name-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker.feature-group-duplicate-name-rejected|run#11|AssertionError|assert message == (
Issue-Key: sagemaker.create-feature-group-malformed-schema-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker.create-model-package-duplicate-name-conflict|run#11|AssertionError|assert message == f"ModelPackage already exists: {first_arn}", (
Issue-Key: sagemaker.create-model-package-duplicate-name-version-conflict-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.create-model-package-duplicate-version-rejected|run#11|AssertionError|assert message == f"ModelPackage already exists: {first['ModelPackageArn']}", (
Issue-Key: sagemaker.batch-add-cluster-nodes-cluster-not-found|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker.batch-add-cluster-nodes-missing-cluster-not-found|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker.cluster-node-batch-add-delete-lifecycle|run#6|AssertionError|assert add_message == expected_add_message, (
Issue-Key: sagemaker.cluster-node-batch-lifecycle|run#7|AssertionError|assert add_resp["Message"] == expected_add_message, (
Issue-Key: sagemaker.attach-cluster-node-volume-cluster-not-found|run#12|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Key: sagemaker.attach-cluster-node-volume-missing-node-not-found|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker.attach-cluster-node-volume-nonexistent-node-not-found-rejected|run#9|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: sagemaker.get-search-suggestions-matches-created-resource|run#11|AssertionError|assert code_message == (
Issue-Commit: 066b7cda09b446bd4bd9be1c3ecfcbe16940769e

# SageMaker duplicate-create, cluster-node and enum-validation refusal texts/codes diverge from AWS (10 causes)

> _Note: this issue was identified against the code base at commit `066b7cda09b446bd4bd9be1c3ecfcbe16940769e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFeatureGroup

## Description
A second family of refusal divergences, distinct from the not-found prose issue: these are duplicate-create conflicts, HyperPod cluster-node refusals and one enum-validation rendering. Ten causes. As above, the FIXED text of each template is what is quoted and the VARIABLE parts are the names/ARNs taken from the request and the emulator's state. This issue is net-new relative to the open issues 20261029T192819Z-sagemaker-duplicate-create-and-request-refusal-messages-diverge.md, 20261029T192819Z-sagemaker-validationexception-rendering-diverges-ordering-list-constraints-enum.md and 20261029T070612Z-sagemaker-attachclusternodevolume-s-cluster-not-found-message-is.md — none of those carry the replay keys listed here, and causes 8 and 9 below contradict the diagnosis recorded in the last of them.

Cause 1 of 10 — DescribeModelPackage after delete.
AWS: recorded behavior only: ValidationException whose Message says the model package "does not exist".
Emulator: ValidationException 'Could not find model package "<model-package-arn>".' — the generic emulator template again, with no "does not exist" clause.
Why: a client polling for the delete to take effect by matching "does not exist" never observes completion.
Fix: the describe-model-package not-found refusal rendering.
Traces: sagemaker.model-package-lifecycle-transitions.

Cause 2 of 10 — DeleteTrial with components still attached.
AWS: recorded behavior only: "Trial '<trial-arn>' has trial components and cannot be deleted." — fixed text, variable part the trial ARN.
Emulator: "Unable to delete trial <trial-arn> because it has <n> trial component(s) associated with it. All trial components that make up the trial must be deleted first." — an invented, longer template that also exposes a count AWS does not report.
Why: wrong text, and the count makes the message vary with state in a way AWS's does not.
Fix: the delete-trial in-use refusal rendering.
Traces: sagemaker.trial-delete-with-attached-component-rejected.

Cause 3 of 10 — CreateExperiment duplicate name.
AWS: recorded behavior only: ValidationException "Experiment names must be unique within an AWS account and region. Experiment with name (<name>) already exists." — note the leading uniqueness sentence and the PARENTHESISED name.
Emulator: 'Experiment with name "<name>" already exists.' — drops the first sentence and uses double quotes instead of parentheses.
Why: the message no longer states the uniqueness scope AWS reports, so a caller cannot tell an account/region collision from any other conflict.
Fix: the create-experiment duplicate-name refusal rendering.
Traces: sagemaker.create-experiment-duplicate-name-fails.

Cause 4 of 10 — CreateFeatureGroup duplicate name.
AWS: recorded behavior only: "Resource Already Exists: FeatureGroup with name <name> already exists. Choose a different name."
Emulator: "The ID or Name specified is already in use." — a generic template that names neither the resource type nor the name.
Why: the refusal does not identify which feature group collided.
Fix: the create-feature-group duplicate-name refusal rendering.
Traces: sagemaker.create-feature-group-duplicate-name-rejected, sagemaker.feature-group-duplicate-name-rejected.

Cause 5 of 10 — CreateFeatureGroup malformed EventTime feature type.
AWS: recorded behavior only: "Validation Error: invalid event time <event-time-feature-name> feature type provided <provided-type>. Event time type should be either Fractional(Unix timestamp in seconds) or String (ISO-8601 format) type" — variable parts the EventTimeFeatureName and the supplied FeatureType.
Emulator: "EventTimeFeatureName: the feature '<name>' is <type>; an EventTime feature must be one of Fractional, String." — a rewritten template.
Why: wrong wording on a request-shaping error developers read directly.
Fix: the create-feature-group schema validation's refusal rendering.
Traces: sagemaker.create-feature-group-malformed-schema-rejected.

Cause 6 of 10 — CreateModelPackage duplicate.
AWS: recorded behavior only: "ModelPackage already exists: <existing-model-package-arn>" — the refusal names the EXISTING package's own ARN.
Emulator: 'Cannot create already existing model package "<name>".' — rewritten, and it names only the requested name, never the existing ARN.
Why: a client reconciling a retried registration cannot learn which ARN already holds the name.
Fix: the create-model-package duplicate refusal rendering.
Traces: sagemaker.create-model-package-duplicate-name-conflict, sagemaker.create-model-package-duplicate-name-version-conflict-rejected, sagemaker.create-model-package-duplicate-version-rejected.

Cause 7 of 10 — BatchAddClusterNodes against a cluster that does not exist.
AWS: Smithy: BatchAddClusterNodes errors = [ResourceLimitExceeded, ResourceNotFound] (client, HTTP 400). Recorded behavior: Message "Cluster with name <cluster-name> not found" — no trailing period.
Emulator: "Cluster <cluster-name> does not exist."
Why: wrong text on the HyperPod node-scaling path.
Fix: the batch-add-cluster-nodes handler's cluster-lookup refusal rendering.
Traces: sagemaker.batch-add-cluster-nodes-cluster-not-found, sagemaker.batch-add-cluster-nodes-missing-cluster-not-found, sagemaker.cluster-node-batch-add-delete-lifecycle, sagemaker.cluster-node-batch-lifecycle.

Cause 8 of 10 — AttachClusterNodeVolume cluster-not-found Message is MISSING the exception-class prefix AWS sends.
AWS: Smithy: AttachClusterNodeVolume errors = [ResourceNotFound] (client, HTTP 400). Recorded behavior: Message "ClusterNotExistsException: Specified cluster not found" — AWS's own fixed text for this class carries the "ClusterNotExistsException: " prefix.
Emulator: "Specified cluster not found" — the prefix is absent.
Why: a client matching AWS's text misses the class discriminator. NOTE: the open issue 20261029T070612Z-sagemaker-attachclusternodevolume-s-cluster-not-found-message-is.md asserts the OPPOSITE (that the prefix is wrongly present); the replay shows the prefix absent, so that diagnosis should be corrected rather than applied.
Fix: the attach-cluster-node-volume handler's cluster-lookup refusal rendering.
Traces: sagemaker.attach-cluster-node-volume-cluster-not-found, sagemaker.attach-cluster-node-volume-missing-node-not-found.

Cause 9 of 10 — AttachClusterNodeVolume with an unknown NodeId is classified with the wrong error code.
AWS: recorded behavior: a well-formed NodeId that names no node of an EXISTING cluster is refused ValidationException (the node is a request parameter the service validates), not the ResourceNotFound reserved for the cluster lookup.
Emulator: returns error code 'ResourceNotFound'.
Why: the two failures are indistinguishable, so a client retries a cluster-missing path for what is a bad parameter.
Fix: the attach-cluster-node-volume handler's node-lookup classification (which modeled error it raises), not the message alone.
Traces: sagemaker.attach-cluster-node-volume-nonexistent-node-not-found-rejected.

Cause 10 of 10 — GetSearchSuggestions enum-validation message lists the wrong enum value set.
AWS: Smithy: GetSearchSuggestions.Resource targets ResourceType (@enum, 20 values) and the operation models no errors, so the refusal is the generic constraint ValidationException. Recorded behavior: AWS's set for this parameter is LARGER than the modeled enum and in a different order — "1 validation error detected: Value '<supplied>' at 'resource' failed to satisfy constraint: Member must satisfy enum value set: [App, Pipeline, ModelPackageGroup, TrainingJob, InferenceComponent, HubContent, Domain, ExperimentTrialComponent, FeatureMetadata, FeatureGroup, Model, HyperParameterTuningJob, Endpoint, PipelineVersion, PipelineExecution, EndpointConfig, ModelCard, Project, ExperimentTrial, Image, Job, ImageVersion, ModelPackage, Experiment, UserProfile]". The fixed text is the whole sentence plus that bracketed set; only the supplied value is variable.
Emulator: renders the 20 modeled ResourceType values in model declaration order and omits App, InferenceComponent, Domain, EndpointConfig and UserProfile.
Why: a caller reading the refusal is told five legal Resource values are illegal.
Fix: the get-search-suggestions request validation's enum-set rendering.
Traces: sagemaker.get-search-suggestions-matches-created-resource.

## Evidence
Replayed against the emulator, representative assertion per cause:

1. sagemaker.model-package-lifecycle-transitions: expected a 'does not exist' ValidationException after delete, got ValidationException: Could not find model package "<arn>".
2. sagemaker.trial-delete-with-attached-component-rejected: expected "Trial '<arn>' has trial components and cannot be deleted.", got 'Unable to delete trial <arn> because it has 1 trial component(s) associated with it. All trial components that make up the trial must be deleted first.'
3. sagemaker.create-experiment-duplicate-name-fails: unexpected duplicate-name message: 'Experiment with name "<name>" already exists.' (expected 'Experiment names must be unique within an AWS account and region. Experiment with name (<name>) already exists.')
4. sagemaker.create-feature-group-duplicate-name-rejected: expected 'Resource Already Exists: FeatureGroup with name <name> already exists. Choose a different name.', got 'The ID or Name specified is already in use.'
5. sagemaker.create-feature-group-malformed-schema-rejected: expected 'Validation Error: invalid event time event_time feature type provided Integral. Event time type should be either Fractional(Unix timestamp in seconds) or String (ISO-8601 format) type', got "EventTimeFeatureName: the feature 'event_time' is Integral; an EventTime feature must be one of Fractional, String."
6. sagemaker.create-model-package-duplicate-version-rejected: expected the refusal to name the existing package's own ARN ('<arn>'); got message='Cannot create already existing model package "<name>".'
7. sagemaker.batch-add-cluster-nodes-cluster-not-found: expected 'Cluster with name <name> not found', got 'Cluster <name> does not exist.'
8. sagemaker.attach-cluster-node-volume-cluster-not-found: expected 'ClusterNotExistsException: Specified cluster not found', got 'Specified cluster not found'
9. sagemaker.attach-cluster-node-volume-nonexistent-node-not-found-rejected: expected ValidationException, got 'ResourceNotFound'
10. sagemaker.get-search-suggestions-matches-created-resource: emulator message listed [TrainingJob, Experiment, ExperimentTrial, ExperimentTrialComponent, Endpoint, Model, ModelPackage, ModelPackageGroup, Pipeline, PipelineExecution, FeatureGroup, FeatureMetadata, Image, ImageVersion, Project, HyperParameterTuningJob, ModelCard, PipelineVersion, Job, HubContent]; AWS lists 25 values including App, InferenceComponent, Domain, EndpointConfig, UserProfile.

## Steps to Reproduce
Re-replay the checks in trace_ids. Causes 1-6: create the resource, then re-create it with the same name (or delete it and describe it) and compare the refusal Message to the AWS template. Cause 7: BatchAddClusterNodes with a ClusterName that was never created. Causes 8-9: AttachClusterNodeVolume against (a) a cluster ARN that does not exist and (b) an existing cluster with a well-formed but unknown NodeId. Cause 10: GetSearchSuggestions with Resource='sim2real-not-a-real-resource-type' and compare the bracketed enum set.
