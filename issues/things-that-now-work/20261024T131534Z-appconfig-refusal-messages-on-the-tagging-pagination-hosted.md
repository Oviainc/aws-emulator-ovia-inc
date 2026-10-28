Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.tag-resource-invalid-arn-rejected appconfig.op-contract-untagResource appconfig.list-extensions-malformed-token-rejected appconfig.op-contract-listDeploymentStrategies appconfig.hosted-configuration-version-versioning-and-stale-conflict appconfig.create-experiment-definition-duplicate-name-conflict-rejected appconfig.update-extension-description-length-exceeded-rejected
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.tag-resource-invalid-arn-rejected|run#8|AssertionError|assert err_tag["Message"] == expected_message, (
Issue-Key: appconfig.op-contract-untagResource|run#14|AssertionError|assert err["Message"] == expected_message, f"expected message {expected_message!r}, got {err['Message']!r}"
Issue-Key: appconfig.list-extensions-malformed-token-rejected|run#5|AssertionError|assert err["Message"] == f"Invalid nextToken parameter {bad_token}.", (
Issue-Key: appconfig.op-contract-listDeploymentStrategies|run#14|AssertionError|assert err["Message"] == f"Invalid nextToken parameter {bad_token}.", (
Issue-Key: appconfig.hosted-configuration-version-versioning-and-stale-conflict|run#13|AssertionError|assert err["Message"] == (
Issue-Key: appconfig.create-experiment-definition-duplicate-name-conflict-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: appconfig.update-extension-description-length-exceeded-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: f04518a3ee65f2e7815d98c430f437f66966ee67

# AppConfig refusal messages on the tagging, pagination, hosted-version-conflict, experiment-conflict and extension-validation paths do not use AWS's request-derived templates (5 causes)

> _Note: this issue was identified against the code base at commit `f04518a3ee65f2e7815d98c430f437f66966ee67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Five distinct root causes, all of the same fault and all grounded in replayed behavioral checks: the HTTP status and the error code already MATCH in every one, and the only divergence is the refusal's Message, which is an emulator-invented sentence instead of AppConfig's request-derived template. These templates are rule-determined, not incidental recorded text: each is built from the request's own values (the ARN and account, the submitted token, the submitted and actual version numbers, the submitted name and application id, the offending member value and its constraint), which is exactly the class the fixed issue 20261023T164118Z-appconfig-refusal-messages-do-not-use-aws-s.md established a shared not-found/conflict message builder for. That issue is NOT a reopen here: its calls are refused as AWS refuses them, and it named only CreateConfigurationProfile, GetEnvironment, UpdateEnvironment, DeleteEnvironment, GetDeploymentStrategy, GetExtension and StartDeployment — none of the operations below. The requirement in each cause is the TEMPLATE, never the recorded identifier values. Numbered; close ALL FIVE before submitting.

=== Cause 1 of 5: the tagging operations' not-found message says 'Arn' where AWS says 'ARN' ===
AWS: TagResource, UntagResource and ListTagsForResource against an AppConfig resource ARN that resolves to nothing are refused with ResourceNotFoundException, HTTP 404, and the single shared template 'Resource with ARN <ResourceArn> could not be found for account <accountId>.' (Smithy: TagResource POST /tags/{ResourceArn}, success code 204, errors BadRequestException 400 / ResourceNotFoundException 404 / InternalServerException 500; the template is recorded behavior only, asserted by two independent checks with two different bogus ARNs).
Emulator: the correct 404 ResourceNotFoundException, but the message is 'Resource with Arn <ResourceArn> could not be found for account <accountId>.' — the literal token 'ARN' is rendered 'Arn'. All three operations share the one wrong literal.
Why: a client or contract test that matches the documented refusal text (log scrapers, error classifiers, snapshot tests) does not match AppConfig's sentence, and the divergence is invisible to code inspection because every other part of the template is right.
Fix: the shared tagging-resource resolution path's not-found message builder (used by TagResource, UntagResource and ListTagsForResource) must emit 'ARN'.
Traces: appconfig.tag-resource-invalid-arn-rejected, appconfig.op-contract-untagResource.

=== Cause 2 of 5: malformed-NextToken refusals do not echo the token in AWS's template ===
AWS: a paginated AppConfig list called with a syntactically invalid NextToken is refused with BadRequestException, HTTP 400, and the template 'Invalid nextToken parameter <NextToken>.' — the submitted token is echoed verbatim (Smithy: ListExtensions GET /extensions and ListDeploymentStrategies GET /deploymentstrategies, both paginated with inputToken/outputToken NextToken, errors BadRequestException 400 / InternalServerException 500; the template is recorded behavior only, asserted by two checks with two different bad tokens).
Emulator: the correct 400 BadRequestException, but with per-operation invented sentences that echo nothing — 'NextToken is not a token this service issued for an extension listing.' (ListExtensions) and 'NextToken is not a pagination token this operation issued.' (ListDeploymentStrategies). Two different literals in two handlers for one rule.
Why: the echo is what lets a caller see WHICH token was rejected when several pagination loops are in flight; a client that logs or parses the refusal loses that, and the message shape differs per operation where AWS's is uniform.
Fix: one shared pagination-token validation path for the appconfig list operations, rendering AWS's template with the submitted token.
Traces: appconfig.list-extensions-malformed-token-rejected, appconfig.op-contract-listDeploymentStrategies.

=== Cause 3 of 5: CreateHostedConfigurationVersion's stale-LatestVersionNumber conflict message ===
AWS: supplying a stale LatestVersionNumber is refused with ConflictException, HTTP 409, and the template 'Expected latest version <submitted> does not match actual latest version <current>' (Smithy: CreateHostedConfigurationVersion, LatestVersionNumber is the Latest-Version-Number header, errors include ConflictException 409; the sentence is recorded behavior only, but it is built from the submitted and actual version numbers, both visible in the request/state).
Emulator: the correct 409 ConflictException, but the message is 'LatestVersionNumber <submitted> does not match the latest hosted configuration version <current> of this configuration profile.'
Why: the optimistic-concurrency guard is the one AppConfig refusal callers routinely parse to decide between retry and abort; the emulator's sentence shape is one AppConfig never emits.
Fix: the optimistic-lock branch of CreateHostedConfigurationVersion must render AWS's template from the two version numbers.
Traces: appconfig.hosted-configuration-version-versioning-and-stale-conflict.

=== Cause 4 of 5: CreateExperimentDefinition's duplicate-name conflict message ===
AWS: a second experiment definition with a Name already in use under the same application is refused with ConflictException, HTTP 409, and the template "Experiment definition with name '<Name>' already exists for application <ApplicationId>" — the name is quoted and the existing definition's id is NOT disclosed (Smithy: CreateExperimentDefinition, errors include ConflictException 409; the sentence is recorded behavior only, built from the request's Name and ApplicationIdentifier).
Emulator: the correct 409 ConflictException, but the message is 'An experiment definition named <Name> already exists in application <ApplicationId> with id <existingId>.' — a different sentence shape that additionally reveals the colliding definition's id, which AWS's refusal withholds.
Why: beyond the shape difference, logic built against the emulator can read an id out of the refusal that is simply absent in production.
Fix: CreateExperimentDefinition's duplicate-name guard must render AWS's template from the request's Name and application id only.
Traces: appconfig.create-experiment-definition-duplicate-name-conflict-rejected.

=== Cause 5 of 5: UpdateExtension's length-validation message elides the offending value ===
AWS: a Description longer than the modeled maximum is refused with BadRequestException, HTTP 400, and the standard AWS validation template "1 validation error detected: Value '<the full offending value>' at 'description' failed to satisfy constraint: Member must have length less than or equal to 1024" — the offending value is echoed in full (Smithy: UpdateExtension PATCH /extensions/{ExtensionIdentifier}, Description @length max 1024, errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404; the validation-message form is AWS's standard request-derived constraint template).
Emulator: the correct 400 BadRequestException and the correct constraint clause, but the echoed value is truncated and replaced with a byte-count annotation — "Value 'xxxx…... [1025 bytes]' at 'description' failed to satisfy constraint: …".
Why: the echo is part of the request-derived rule, so a client asserting or displaying AWS's validation text sees a sentence AppConfig never produces; the annotation is an emulator artifact with no counterpart in production.
Fix: the appconfig request-validation message renderer must echo the full offending member value rather than abbreviating it.
Traces: appconfig.update-extension-description-length-exceeded-rejected.

## Evidence
Behavioral replay (simparity-behavioral-ga-prod-v1). In every cause below the status and error code matched and only $.Message diverged. The ARNs, account ('000000000000'), ids, tokens and version numbers are the run's values and are examples; the requirement is each cause's template.

Cause 1 — appconfig.tag-resource-invalid-arn-rejected [mismatch]: AssertionError: TagResource: expected message 'Resource with ARN arn:aws:appconfig:us-west-2:000000000000:application/zzzzzzz could not be found for account 000000000000.', got 'Resource with Arn arn:aws:appconfig:us-west-2:000000000000:application/zzzzzzz could not be found for account 000000000000.' Same divergence via appconfig.op-contract-untagResource for UntagResource (expected 'Resource with ARN arn:aws:appconfig:us-west-2:000000000000:application/0000000 …', got 'Resource with Arn …'). Code ResourceNotFoundException matched in both.

Cause 2 — appconfig.list-extensions-malformed-token-rejected [mismatch]: AssertionError: unexpected message: 'NextToken is not a token this service issued for an extension listing.' (expected 'Invalid nextToken parameter not-a-real-continuation-token.'). appconfig.op-contract-listDeploymentStrategies [mismatch]: AssertionError: unexpected message: 'NextToken is not a pagination token this operation issued.' (expected 'Invalid nextToken parameter not-a-real-pagination-token!!.'). Code BadRequestException matched in both; the token-less positive controls succeeded.

Cause 3 — appconfig.hosted-configuration-version-versioning-and-stale-conflict [mismatch]: AssertionError: unexpected message: 'LatestVersionNumber 1 does not match the latest hosted configuration version 2 of this configuration profile.' (expected 'Expected latest version 1 does not match actual latest version 2'). Code ConflictException matched, and the stale write correctly left no new version.

Cause 4 — appconfig.create-experiment-definition-duplicate-name-conflict-rejected [mismatch]: AssertionError: expected message "Experiment definition with name 'sim2real-behav-dupexp-88aaf3f158e4' already exists for application ho65pb1", got 'An experiment definition named sim2real-behav-dupexp-88aaf3f158e4 already exists in application ho65pb1 with id q92lkf0.' Code ConflictException matched.

Cause 5 — appconfig.update-extension-description-length-exceeded-rejected [mismatch]: expected "1 validation error detected: Value '<1025 x characters>' at 'description' failed to satisfy constraint: Member must have length less than or equal to 1024", got the same sentence with the value rendered as "'xxxx…... [1025 bytes]'". Code BadRequestException matched, the extension's Description and VersionNumber were correctly left unchanged.

## Steps to Reproduce
Point an appconfig client at the emulator's data listener and compare only the Message of each refusal (status and code already match).

Cause 1: tag_resource(ResourceArn='arn:aws:appconfig:<region>:<account>:application/zzzzzzz', Tags={'k':'v'}) -> 404 ResourceNotFoundException; expect 'Resource with ARN <arn> could not be found for account <account>.' Repeat with untag_resource(TagKeys=['k']) and list_tags_for_resource on the same bogus ARN.

Cause 2: list_extensions(NextToken='not-a-real-continuation-token') -> 400 BadRequestException; expect 'Invalid nextToken parameter not-a-real-continuation-token.'. list_deployment_strategies(NextToken='not-a-real-pagination-token!!') -> 400; expect 'Invalid nextToken parameter not-a-real-pagination-token!!.'.

Cause 3: create an application and a hosted configuration profile; create_hosted_configuration_version twice (v1, v2); then create_hosted_configuration_version(..., LatestVersionNumber=1) -> 409 ConflictException; expect 'Expected latest version 1 does not match actual latest version 2'.

Cause 4: create an application, environment and AWS.AppConfig.FeatureFlags profile; create_experiment_definition(Name=N, ...) then create_experiment_definition with the same Name -> 409 ConflictException; expect "Experiment definition with name 'N' already exists for application <appId>" and no colliding id in the text.

Cause 5: create_extension(...); update_extension(ExtensionIdentifier=<id>, Description='x'*1025) -> 400 BadRequestException; expect the full 1025-character value echoed inside the standard validation sentence.

Re-replay every check in trace_ids to confirm all five causes are closed.
