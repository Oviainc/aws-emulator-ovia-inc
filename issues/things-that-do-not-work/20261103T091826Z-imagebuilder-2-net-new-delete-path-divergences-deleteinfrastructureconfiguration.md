Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: imagebuilder.delete-infrastructure-configuration-in-use-rejected imagebuilder.ibp-delete-component-referenced-by-a-recipe-rejected
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder.delete-infrastructure-configuration-in-use-rejected|run#10|AssertionError|assert code == "ResourceDependencyException", f"expected ResourceDependencyException, got {code!r}"
Issue-Key: imagebuilder.ibp-delete-component-referenced-by-a-recipe-rejected|run#12|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Commit: c1544a9590ded4504bea161b6ee6981cbcc184a7

# imagebuilder: 2 net-new delete-path divergences — DeleteInfrastructureConfiguration answers 501 NotImplemented, and DeleteComponent's ResourceDependencyException message is emulator-authored

> _Note: this issue was identified against the code base at commit `c1544a9590ded4504bea161b6ee6981cbcc184a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteInfrastructureConfiguration

## Description
TWO net-new root causes on Image Builder delete paths, both grounded in failing behavioral checks. Both causes must be closed before this issue is submitted.

Scope note: open issue 20261102T085510Z-imagebuilder-image-builder-accepts-creates-and-deletes-that-aws.md (wire corpus) describes DeleteComponent/DeleteImageRecipe answering HTTP 200 for a dependency-protected delete; the emulator now DOES refuse DeleteComponent, so cause 2 below is the surviving wording defect and that issue does not carry this check's replay key. Open issue 20261101T001020Z-imagebuilder-six-operations-answer-501-notimplemented-where-aws.md enumerates six 501-stub operations and does NOT include DeleteInfrastructureConfiguration, so cause 1 is net-new.

Cause 1 of 2: DeleteInfrastructureConfiguration is a scaffold stub and answers 501 NotImplemented, so the dependency refusal, the delete itself and the not-found semantics after deletion are all absent.
AWS: Smithy: DeleteInfrastructureConfiguration takes required infrastructureConfigurationArn (@httpQuery, InfrastructureConfigurationArn pattern ^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws):infrastructure-configuration/[a-z0-9-_]+$), answers 200 with {infrastructureConfigurationArn, requestId}, and models ResourceDependencyException (client, @httpError 400, documented "You have attempted to mutate or delete a resource with a dependency that prohibits this action. See the error message for more details."). The operation's own documentation states: "You can't delete a configuration that an image pipeline still references. The request fails with ResourceDependencyException." Recorded behavior for the message: 400 ResourceDependencyException with "The resource ARN '<infrastructureConfigurationArn>' has dependent resources" — fixed text "The resource ARN '" and "' has dependent resources"; the only variable part is the ARN taken verbatim from the request. The refused delete has no effect: GetInfrastructureConfiguration still answers 200 with that arn. Once the referencing pipeline (and recipe) are deleted, the delete succeeds; a subsequent GetInfrastructureConfiguration is refused ResourceNotFoundException with "The resource ARN '<arn>' was not found." (fixed text "The resource ARN '", "' was not found."; ARN from the request), and a second DeleteInfrastructureConfiguration on the now-dead ARN is refused with the same ResourceNotFoundException code and message rather than succeeding silently.
Emulator: every DeleteInfrastructureConfiguration request is refused with error code 'NotImplemented' (HTTP 501, body message "operation DeleteInfrastructureConfiguration is scaffolded but not implemented in this emulator") — services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/delete_infrastructure_configuration.rs returns Scaffolded::not_implemented("DeleteInfrastructureConfiguration") from handle(). So neither the dependency refusal nor the successful delete nor the post-delete not-found exists.
Why: 'NotImplemented'/501 is not a member of any SDK's imagebuilder error union, is not catchable as a modeled client error and is not retriable; a caller cannot distinguish "still referenced by a pipeline" from "service broken", and teardown code can never delete an infrastructure configuration at all, so every dependency-ordered cleanup sequence fails.
Fix: implement the handler in services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/delete_infrastructure_configuration.rs together with the reverse-reference lookup in the service state (pipelines referencing the infrastructure configuration) and the not-found path, returning the statuses, error codes and message templates above instead of the scaffold's 501.
Traces: imagebuilder.delete-infrastructure-configuration-in-use-rejected.

Cause 2 of 2: DeleteComponent refuses a dependency-protected delete with the right error code but an emulator-authored message instead of AWS's ResourceDependencyException wording.
AWS: Smithy: ResourceDependencyException is a modeled error of DeleteComponent (client, @httpError 400). Recorded behavior for the message of this error class: "Resource dependency error: The resource ARN '<componentBuildVersionArn>' has other resources depended on it." — fixed text "Resource dependency error: The resource ARN '" and "' has other resources depended on it."; the only variable part is the component build-version ARN taken verbatim from the request. AWS does not name the referencing recipe in the message. (Same template as the DeleteImageRecipe refusal.)
Emulator: the code is correct ('ResourceDependencyException') but the message is invented and has a different shape and different variable parts — "The component <componentBuildVersionArn> can't be deleted because the image recipe <imageRecipeArn> includes it." It interpolates a second value (the referencing image-recipe ARN) that AWS's message never carries, and drops the "Resource dependency error: ..." prefix and the "' has other resources depended on it." suffix entirely.
Why: the error message is part of the contract for this error class; clients, log scrapers and test assertions that match AWS's documented dependency-error text do not match, and tooling that parses the single quoted ARN out of the message reads the wrong field count. The divergence also leaks emulator-internal relationship detail that real AWS withholds.
Fix: the message construction for the dependency refusal in the DeleteComponent handler (services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/delete_component.rs) and any shared dependency-error helper it uses must emit AWS's template with only the requested ARN substituted; the same template applies to DeleteImageRecipe.
Traces: imagebuilder.ibp-delete-component-referenced-by-a-recipe-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator (service heading: ### imagebuilder).

Cause 1 — imagebuilder.delete-infrastructure-configuration-in-use-rejected [mismatch]:
After creating an IAM instance profile, an infrastructure configuration, an image recipe and a DISABLED image pipeline that references the configuration, DeleteInfrastructureConfiguration(infrastructureConfigurationArn=<that arn>) is expected to be refused 400 ResourceDependencyException with "The resource ARN '<arn>' has dependent resources".
  AssertionError: expected ResourceDependencyException, got 'NotImplemented'
The stub body is services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/delete_infrastructure_configuration.rs line 180: Err(crate::scaffolded::Scaffolded::not_implemented("DeleteInfrastructureConfiguration")), with the SCAFFOLDED marker still on line 4. Because the operation never succeeds either, the check's positive control (delete pipeline, delete recipe, delete configuration, then GetInfrastructureConfiguration -> 404 ResourceNotFoundException "The resource ARN '<arn>' was not found.", and a repeat delete -> the same ResourceNotFoundException) is unreachable.

Cause 2 — imagebuilder.ibp-delete-component-referenced-by-a-recipe-rejected [mismatch]:
DeleteComponent(componentBuildVersionArn=<build referenced by an image recipe>) returns the expected code 'ResourceDependencyException' but the wrong message.
  AssertionError: expected message "Resource dependency error: The resource ARN '<component-build-version-arn>' has other resources depended on it.", got "The component <component-build-version-arn> can't be deleted because the image recipe <image-recipe-arn> includes it."
(The ARNs in that recorded line are per-run generated values; the requirement is the template, with the single ARN taken from the request.)

Model (smithy_query, services/service-com-amazonaws-imagebuilder-imagebuilder/smithy-model/com-amazonaws-imagebuilder-imagebuilder.json): ResourceDependencyException = structure, client error, httpError 400, member message:ErrorMessage; it is a modeled error of both DeleteComponent and DeleteInfrastructureConfiguration, each of which succeeds with HTTP 200 and {arn, requestId}.

## Steps to Reproduce
Cause 1 (DeleteInfrastructureConfiguration 501):
1. iam.CreateRole + CreateInstanceProfile + AddRoleToInstanceProfile for a unique name.
2. imagebuilder.CreateInfrastructureConfiguration(name=<unique>, instanceProfileName=<that profile>, clientToken=<uuid>) -> note infrastructureConfigurationArn.
3. imagebuilder.CreateImageRecipe(name=<unique>, semanticVersion='1.0.0', parentImage='arn:aws:imagebuilder:<region>:aws:image/amazon-linux-2-x86/x.x.x', clientToken=<uuid>).
4. imagebuilder.CreateImagePipeline(name=<unique>, imageRecipeArn=<step 3>, infrastructureConfigurationArn=<step 2>, status='DISABLED', clientToken=<uuid>).
5. imagebuilder.DeleteInfrastructureConfiguration(infrastructureConfigurationArn=<step 2>) — expect 400 ResourceDependencyException "The resource ARN '<arn>' has dependent resources"; the emulator answers 501 'NotImplemented'.
6. GetInfrastructureConfiguration(<arn>) — expect 200 with that arn (refused delete had no effect).
7. DeleteImagePipeline, DeleteImageRecipe, then DeleteInfrastructureConfiguration(<arn>) — expect 200.
8. GetInfrastructureConfiguration(<arn>) and DeleteInfrastructureConfiguration(<arn>) again — expect 404 ResourceNotFoundException "The resource ARN '<arn>' was not found." both times.

Cause 2 (DeleteComponent dependency message):
1. imagebuilder.CreateComponent(name=<unique>, semanticVersion='1.0.0', platform='Linux', data=<valid document with build and validate phases>) -> componentBuildVersionArn.
2. imagebuilder.CreateImageRecipe(name=<unique>, semanticVersion='1.0.0', parentImage='arn:aws:imagebuilder:<region>:aws:image/amazon-linux-2-x86/x.x.x', components=[{componentArn: <step 1>}]).
3. imagebuilder.DeleteComponent(componentBuildVersionArn=<step 1>) — expect 400 ResourceDependencyException with message exactly "Resource dependency error: The resource ARN '<step 1 arn>' has other resources depended on it."; the emulator returns the correct code with an invented message naming the recipe.
4. GetComponent(<step 1>) — expect state.status 'ACTIVE' (the refused delete left it intact).
5. DeleteImageRecipe(<step 2>), then DeleteComponent(<step 1>) — expect success, and GetComponent(<step 1>) -> ResourceNotFoundException "The following required resource 'Component' cannot be found: '<arn>'.".
6. Re-replay the checks listed in trace_ids.
