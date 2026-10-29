Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.artifact-create-describe-delete sagemaker.artifact-describe-after-delete-rejected sagemaker.artifact-lifecycle-create-describe-delete sagemaker.artifact-lifecycle-transitions sagemaker.context-create-describe-delete sagemaker.context-lifecycle-create-describe-delete sagemaker.context-lifecycle-transitions sagemaker.ai-workload-config-create-delete-lifecycle sagemaker.delete-ai-workload-config-idempotent-not-found-rejected sagemaker.delete-ai-workload-config-in-use-blocks-delete sagemaker.algorithm-lifecycle-create-describe-delete sagemaker.model-card-archived-update-rejected sagemaker.model-card-create-describe-delete sagemaker.model-card-lifecycle-transitions sagemaker.model-card-unicode-content-round-trip sagemaker.hub-create-describe-delete sagemaker.create-partner-app-presigned-url-app-not-found sagemaker.create-partner-app-presigned-url-missing-app sagemaker.delete-model-package-group-policy-removes-policy sagemaker.delete-model-package-group-policy-then-get-not-found sagemaker.e2e-model-package-group-policy-controls-access-lifecycle sagemaker.e2e-model-package-group-policy-full-flow sagemaker.get-model-package-group-policy-missing sagemaker.model-package-group-policy-json-round-trip sagemaker.model-package-group-policy-put-get-delete-consistency sagemaker.model-package-group-policy-put-get-delete-flow sagemaker.model-package-group-policy-put-get-delete-round-trip sagemaker.put-model-package-group-policy-precondition-conflict sagemaker.experiment-trial-component-association-flow sagemaker.trial-and-trial-component-association-lifecycle sagemaker.trial-component-unicode-metadata-round-trip
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.artifact-create-describe-delete|run#17|AssertionError|assert describe_err["Message"] == expected_describe, (
Issue-Key: sagemaker.artifact-describe-after-delete-rejected|run#7|AssertionError|assert describe_err["Message"] == expected_describe, (
Issue-Key: sagemaker.artifact-lifecycle-create-describe-delete|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker.artifact-lifecycle-transitions|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker.context-create-describe-delete|run#16|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.context-lifecycle-create-describe-delete|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.context-lifecycle-transitions|run#17|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.ai-workload-config-create-delete-lifecycle|run#16|AssertionError|assert describe_err["Message"] == expected_describe, (
Issue-Key: sagemaker.delete-ai-workload-config-idempotent-not-found-rejected|run#12|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: sagemaker.delete-ai-workload-config-in-use-blocks-delete|run#11|AssertionError|assert describe_message == expected, f"expected {expected!r}, got {describe_message!r}"
Issue-Key: sagemaker.algorithm-lifecycle-create-describe-delete|run#23|AssertionError|assert tag_err["Message"] == expected_tag, (
Issue-Key: sagemaker.model-card-archived-update-rejected|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.model-card-create-describe-delete|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.model-card-lifecycle-transitions|run#17|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.model-card-unicode-content-round-trip|run#20|AssertionError|assert not_found_message == expected_message, (
Issue-Key: sagemaker.hub-create-describe-delete|run#12|AssertionError|assert message == f"Hub with name {hub_name} does not exist.", f"unexpected DescribeHub message: {message!r}"
Issue-Key: sagemaker.create-partner-app-presigned-url-app-not-found|run#6|AssertionError|assert message == "App entity does not exist.", f"unexpected ResourceNotFound message: {message!r}"
Issue-Key: sagemaker.create-partner-app-presigned-url-missing-app|run#8|AssertionError|assert message == "App entity does not exist.", f"unexpected ResourceNotFound message: {message!r}"
Issue-Key: sagemaker.delete-model-package-group-policy-removes-policy|run#15|AssertionError|assert message == expected_message, f"expected {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.delete-model-package-group-policy-then-get-not-found|run#17|AssertionError|assert msg == expected_message, f"expected {expected_message!r}, got {msg!r}"
Issue-Key: sagemaker.e2e-model-package-group-policy-controls-access-lifecycle|run#23|AssertionError|assert revoked_message == expected, f"expected message {expected!r}, got {revoked_message!r}"
Issue-Key: sagemaker.e2e-model-package-group-policy-full-flow|run#19|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker.get-model-package-group-policy-missing|run#5|AssertionError|assert err.get("Message") == f"Cannot find resource policy for: {group_arn}", (
Issue-Key: sagemaker.model-package-group-policy-json-round-trip|run#14|AssertionError|assert message == f"Cannot find resource policy for: {group_arn}", message
Issue-Key: sagemaker.model-package-group-policy-put-get-delete-consistency|run#15|AssertionError|assert message == f"Cannot find resource policy for: {group_arn}", message
Issue-Key: sagemaker.model-package-group-policy-put-get-delete-flow|run#17|AssertionError|assert msg == f"Cannot find resource policy for: {group_arn}", msg
Issue-Key: sagemaker.model-package-group-policy-put-get-delete-round-trip|run#15|AssertionError|assert message == f"Cannot find resource policy for: {group_arn}", message
Issue-Key: sagemaker.put-model-package-group-policy-precondition-conflict|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.experiment-trial-component-association-flow|run#29|AssertionError|assert tc_message == (
Issue-Key: sagemaker.trial-and-trial-component-association-lifecycle|run#25|AssertionError|assert tc_message == expected_tc, f"expected message {expected_tc!r}, got {tc_message!r}"
Issue-Key: sagemaker|DescribeTrialComponent|ResourceNotFound
Issue-Commit: 066b7cda09b446bd4bd9be1c3ecfcbe16940769e

# SageMaker ResourceNotFound/ValidationException refusal texts are emulator-authored prose across ten resource families (10 causes)

> _Note: this issue was identified against the code base at commit `066b7cda09b446bd4bd9be1c3ecfcbe16940769e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeArtifact

## Description
The emulator answers the right HTTP status and the right error CODE on these not-found paths, but it renders ONE generic emulator-authored Message template per resource family instead of the per-operation text AWS returns. The error Message is part of the contract (Smithy: ResourceNotFound.Message / ConflictException.Message -> FailureReason, @length max 1024), so a client that matches on it breaks. Ten distinct message templates are wrong; each is one cause below. In every template below, the FIXED text is the wording shown and the VARIABLE parts are the resource ARN or name taken from the request and the emulator's own state — never the literal ids quoted from the single replay run.

This issue is net-new relative to the open issues 20261029T192819Z-sagemaker-not-found-refusal-messages-are-emulator-authored, 20261029T192819Z-sagemaker-scheduler-hub-card-and-partner-app-not.md and 20261029T165422Z-sagemaker-describetrialcomponent-s-resourcenotfound-message-uses-the-emulator.md, which name overlapping surfaces; those issues do not carry the replay keys listed here and the behaviour is still live in this replay.

Cause 1 of 10 — DescribeArtifact/DeleteArtifact/UpdateArtifact ResourceNotFound text.
AWS: Smithy: DescribeArtifact errors = [ResourceNotFound] (client, HTTP 400), Message target FailureReason. Recorded behavior: AWS words it "Artifact '<artifact-arn>' does not exist." — fixed text "Artifact '" ... "' does not exist.", variable part the artifact ARN from the request.
Emulator: returns ResourceNotFound with Message 'Could not find artifact "<artifact-arn>".' — different wording and double quotes instead of single.
Why: a client that keys retry/cleanup logic on the AWS wording never matches, so a deleted-artifact probe is treated as an unrecognised failure.
Fix: the artifact handlers' not-found refusal rendering (describe/delete/update artifact in the sagemaker crate's ops), not the status or code.
Traces: sagemaker.artifact-create-describe-delete, sagemaker.artifact-describe-after-delete-rejected, sagemaker.artifact-lifecycle-create-describe-delete, sagemaker.artifact-lifecycle-transitions.

Cause 2 of 10 — DescribeContext ResourceNotFound text omits the quoting AWS applies.
AWS: recorded behavior only: "Context '<context-arn>' does not exist." — the ARN is single-quoted.
Emulator: "Context <context-arn> does not exist." — the quotes around the variable ARN are missing.
Why: a client matching the quoted form mis-parses the entity out of the message.
Fix: the context not-found refusal rendering.
Traces: sagemaker.context-create-describe-delete, sagemaker.context-lifecycle-create-describe-delete, sagemaker.context-lifecycle-transitions.

Cause 3 of 10 — AI workload config not-found text names an ARN where AWS names the config NAME.
AWS: recorded behavior only: "AIWorkloadConfig '<name>' not found." — fixed text with the config NAME from the request.
Emulator: 'Could not find AI workload config "<config-arn>".' — different wording and it substitutes a synthesised ARN for the name the caller supplied.
Why: a caller cannot correlate the refusal with the name it passed.
Fix: the AI-workload-config describe/delete handlers' refusal rendering.
Traces: sagemaker.ai-workload-config-create-delete-lifecycle, sagemaker.delete-ai-workload-config-idempotent-not-found-rejected, sagemaker.delete-ai-workload-config-in-use-blocks-delete.

Cause 4 of 10 — DescribeAlgorithm after delete uses a generic "Resource" noun.
AWS: recorded behavior only: "Algorithm <algorithm-arn> does not exist." — the noun is the resource type, the ARN unquoted.
Emulator: "Resource '<algorithm-arn>' does not exist." — generic noun plus quotes.
Why: the refusal no longer identifies which SageMaker resource type was missing.
Fix: the algorithm not-found refusal rendering.
Traces: sagemaker.algorithm-lifecycle-create-describe-delete.

Cause 5 of 10 — Model card not-found text names the card by name, not by ARN, and rewords.
AWS: recorded behavior only: "Modelcard <model-card-arn> does not exist" — note AWS's single-word noun "Modelcard" and NO trailing period.
Emulator: "Could not find model card '<name>'." — different noun, name instead of ARN, added trailing period.
Why: clients matching AWS's exact text (including the absent final period) never match.
Fix: the model-card describe/update not-found refusal rendering.
Traces: sagemaker.model-card-archived-update-rejected, sagemaker.model-card-create-describe-delete, sagemaker.model-card-lifecycle-transitions, sagemaker.model-card-unicode-content-round-trip.

Cause 6 of 10 — DescribeHub ResourceNotFound text.
AWS: recorded behavior only: "Hub with name <hub-name> does not exist."
Emulator: 'Resource not found: hub "<hub-name>".'
Why: wrong wording on the only signal distinguishing a deleted hub from a permission problem.
Fix: the hub describe handler's refusal rendering.
Traces: sagemaker.hub-create-describe-delete.

Cause 7 of 10 — CreatePartnerAppPresignedUrl not-found text echoes the ARN AWS deliberately omits.
AWS: recorded behavior only: ResourceNotFound with the FIXED, variable-free Message "App entity does not exist." — AWS names no identifier here.
Emulator: "Partner app <partner-app-arn> does not exist." — invents a template and interpolates the requested ARN.
Why: beyond the wording mismatch, the emulator leaks the probed ARN back in a message AWS keeps identifier-free.
Fix: the create-partner-app-presigned-url handler's refusal rendering.
Traces: sagemaker.create-partner-app-presigned-url-app-not-found, sagemaker.create-partner-app-presigned-url-missing-app.

Cause 8 of 10 — GetModelPackageGroupPolicy "no policy attached" text adds quotes and a trailing period.
AWS: recorded behavior only: ValidationException with Message "Cannot find resource policy for: <model-package-group-arn>" — the ARN is bare and there is NO trailing period.
Emulator: 'Cannot find resource policy for: "<model-package-group-arn>".' — quotes the ARN and appends a period.
Why: this is the read that tells a caller a resource policy was successfully deleted; the altered text breaks that detection, and it is the single most-replayed divergence in this service's policy surface.
Fix: the get-model-package-group-policy handler's refusal rendering.
Traces: sagemaker.delete-model-package-group-policy-removes-policy, sagemaker.delete-model-package-group-policy-then-get-not-found, sagemaker.e2e-model-package-group-policy-controls-access-lifecycle, sagemaker.e2e-model-package-group-policy-full-flow, sagemaker.get-model-package-group-policy-missing, sagemaker.model-package-group-policy-json-round-trip, sagemaker.model-package-group-policy-put-get-delete-consistency, sagemaker.model-package-group-policy-put-get-delete-flow, sagemaker.model-package-group-policy-put-get-delete-round-trip.

Cause 9 of 10 — PutModelPackageGroupPolicy on a missing group uses the wrong template.
AWS: Smithy: PutModelPackageGroupPolicy errors = [ConflictException] (client, HTTP 400). Recorded behavior: Message "Cannot find Model Package Group: <model-package-group-arn>".
Emulator: "ModelPackageGroup <model-package-group-arn> does not exist."
Why: a caller distinguishing "group absent" from "policy absent" by message text cannot.
Fix: the put-model-package-group-policy handler's refusal rendering.
Traces: sagemaker.put-model-package-group-policy-precondition-conflict.

Cause 10 of 10 — DescribeTrialComponent/DeleteTrialComponent not-found text.
AWS: recorded behavior only: "Trial component '<trial-component-arn>' does not exist."
Emulator: 'Could not find trial component "<trial-component-arn>".'
Why: same generic-prose substitution; experiment-tracking clients that clean up by probing for the deleted component cannot tell the probe succeeded.
Fix: the trial-component describe/delete not-found refusal rendering. The open issue 20261029T165422Z-sagemaker-describetrialcomponent-s-resourcenotfound-message-uses-the-emulator.md names this surface but does not carry the three replay keys below.
Traces: sagemaker.experiment-trial-component-association-flow, sagemaker.trial-and-trial-component-association-lifecycle, sagemaker.trial-component-unicode-metadata-round-trip.

## Evidence
Replayed against the emulator (suite simparity-behavioral-aws-v1), representative assertion per cause:

1. sagemaker.artifact-create-describe-delete: AssertionError: expected message "Artifact 'arn:aws:sagemaker:<region>:<account>:artifact/<id>' does not exist.", got 'Could not find artifact "arn:aws:sagemaker:<region>:<account>:artifact/<id>".'
2. sagemaker.context-create-describe-delete: expected "Context '<arn>' does not exist.", got 'Context <arn> does not exist.'
3. sagemaker.ai-workload-config-create-delete-lifecycle: expected "AIWorkloadConfig '<name>' not found.", got 'Could not find AI workload config "<arn>".'
4. sagemaker.algorithm-lifecycle-create-describe-delete: expected 'Algorithm <arn> does not exist.', got "Resource '<arn>' does not exist."
5. sagemaker.model-card-create-describe-delete: expected 'Modelcard <arn> does not exist', got "Could not find model card '<name>'."
6. sagemaker.hub-create-describe-delete: expected 'Hub with name <name> does not exist.', got 'Resource not found: hub "<name>".'
7. sagemaker.create-partner-app-presigned-url-app-not-found: expected 'App entity does not exist.', got 'Partner app <arn> does not exist.'
8. sagemaker.get-model-package-group-policy-missing: expected 'Cannot find resource policy for: <arn>', got 'Cannot find resource policy for: "<arn>".'
9. sagemaker.put-model-package-group-policy-precondition-conflict: expected 'Cannot find Model Package Group: <arn>', got 'ModelPackageGroup <arn> does not exist.'
10. sagemaker.trial-and-trial-component-association-lifecycle: expected "Trial component '<arn>' does not exist.", got 'Could not find trial component "<arn>".'

In every case the HTTP status (400) and the error code (ResourceNotFound / ValidationException / ConflictException as modeled) matched; only the Message diverged.

## Steps to Reproduce
Re-replay the checks listed in trace_ids against the emulator. For each cause the shape is identical: create the resource, delete it, then issue the describe/delete/update (or, for cause 8, PutModelPackageGroupPolicy then DeleteModelPackageGroupPolicy then GetModelPackageGroupPolicy; for cause 7, CreatePartnerAppPresignedUrl against a partner-app ARN that was never created) and compare the error Message to the AWS template named in that cause.
