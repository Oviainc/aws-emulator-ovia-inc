Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.describe-image-signing-status-missing-repo-rejected ecr.describe-image-signing-status-missing-image-rejected ecr.describe-image-signing-status-not-found ecr.describe-image-replication-status-not-found ecr.describe-image-replication-status-missing-image-rejected ecr.list-image-referrers-repo-not-found-rejected ecr.list-image-referrers-repository-not-found-rejected ecr.deregister-exclusion-not-found ecr.deregister-pull-time-exclusion-not-found-rejected ecr.deregister-pull-time-update-exclusion-nonexistent-rejected ecr.deregister-pull-time-update-exclusion-not-found-rejected ecr.deregister-pull-time-update-exclusion-unknown-rejected ecr.update-pull-through-cache-rule-not-found-rejected ecr.validate-pull-through-cache-rule-not-found ecr.validate-pull-through-cache-rule-not-found-rejected ecr.delete-repository-creation-template-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.describe-image-signing-status-missing-repo-rejected|run#4|AssertionError|assert code == "RepositoryNotFoundException", f"expected RepositoryNotFoundException, got {code!r}"
Issue-Key: ecr.describe-image-signing-status-missing-image-rejected|run#5|AssertionError|assert code == "ImageNotFoundException", code
Issue-Key: ecr.describe-image-signing-status-not-found|run#6|AssertionError|assert code == "ImageNotFoundException", f"expected ImageNotFoundException, got {code}"
Issue-Key: ecr.describe-image-replication-status-not-found|run#7|AssertionError|assert code_repo == "RepositoryNotFoundException", (
Issue-Key: ecr.describe-image-replication-status-missing-image-rejected|run#4|AssertionError|assert code == "ImageNotFoundException", code
Issue-Key: ecr.list-image-referrers-repo-not-found-rejected|run#4|AssertionError|assert code == "RepositoryNotFoundException", code
Issue-Key: ecr.list-image-referrers-repository-not-found-rejected|run#5|AssertionError|assert code == "RepositoryNotFoundException", code
Issue-Key: ecr.deregister-exclusion-not-found|run#5|AssertionError|assert code == "ExclusionNotFoundException", (
Issue-Key: ecr.deregister-pull-time-exclusion-not-found-rejected|run#6|AssertionError|assert code == "ExclusionNotFoundException", (
Issue-Key: ecr.deregister-pull-time-update-exclusion-nonexistent-rejected|run#5|AssertionError|assert err == "ExclusionNotFoundException", f"expected ExclusionNotFoundException, got {err}"
Issue-Key: ecr.deregister-pull-time-update-exclusion-not-found-rejected|run#5|AssertionError|assert code == "ExclusionNotFoundException", code
Issue-Key: ecr.deregister-pull-time-update-exclusion-unknown-rejected|run#5|AssertionError|assert code == "ExclusionNotFoundException", code
Issue-Key: ecr.update-pull-through-cache-rule-not-found-rejected|run#9|AssertionError|assert code == "PullThroughCacheRuleNotFoundException", code
Issue-Key: ecr.validate-pull-through-cache-rule-not-found|run#4|AssertionError|assert code == "PullThroughCacheRuleNotFoundException", (
Issue-Key: ecr.validate-pull-through-cache-rule-not-found-rejected|run#4|AssertionError|assert code == "PullThroughCacheRuleNotFoundException", (
Issue-Key: ecr.delete-repository-creation-template-nonexistent-rejected|run#4|AssertionError|assert err == "TemplateNotFoundException", f"expected TemplateNotFoundException, got {err}"
Issue-Commit: ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3

# ECR: seven modeled operations answer NotImplemented instead of their modeled responses/errors (image status reads, pull-time exclusions, PTC rule update/validate, creation-template delete)

> _Note: this issue was identified against the code base at commit `ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImageSigningStatus

## Description
Seven ECR operations that the behavioral suite exercises are scaffold stubs: every call is answered with a non-modeled error whose code is `NotImplemented` and whose message is "operation <Op> is scaffolded but not implemented in this emulator". None of these seven is covered by an ALREADY-FILED stub issue in this replay (the filed ones name RegisterPullTimeUpdateExclusion, CreatePullThroughCacheRule, CreateRepositoryCreationTemplate, StartLifecyclePolicyPreview, GetSigningConfiguration, DeleteSigningConfiguration, etc. - the operations below are different handlers). Each cause below is a distinct operation and a distinct handler file; all seven must be closed before this issue is submitted.

Cause 1 of 7: DescribeImageSigningStatus
AWS: Smithy DescribeImageSigningStatus - input {repositoryName (required), imageId (required), registryId}; output {repositoryName, imageId, registryId, signingStatuses}; modeled errors ImageNotFoundException (client, httpError 400), RepositoryNotFoundException (client, 400), InvalidParameterException (400), ValidationException (400), ServerException (500). An unknown repository must be refused with RepositoryNotFoundException; a known repository with an absent digest/tag must be refused with ImageNotFoundException.
Emulator: every call returns error code `NotImplemented` ("operation DescribeImageSigningStatus is scaffolded but not implemented in this emulator"), so the checks' assertions on RepositoryNotFoundException / ImageNotFoundException observe 'NotImplemented'.
Why: a client cannot distinguish "repository missing" from "image missing" from "not signed"; SDK typed-exception handling (ImageNotFoundException/RepositoryNotFoundException) never fires, and retry classification treats the unknown code as non-modeled.
Fix: the DescribeImageSigningStatus operation handler plus the repository/image lookup in ECR state that resolves repositoryName and imageId before projecting signingStatuses.
Traces: ecr.describe-image-signing-status-missing-repo-rejected, ecr.describe-image-signing-status-missing-image-rejected, ecr.describe-image-signing-status-not-found.

Cause 2 of 7: DescribeImageReplicationStatus
AWS: Smithy DescribeImageReplicationStatus - input {repositoryName (required), imageId (required), registryId}; output {repositoryName, imageId, replicationStatuses}; modeled errors ImageNotFoundException (400), RepositoryNotFoundException (400), InvalidParameterException (400), ValidationException (400), ServerException (500). Nonexistent repository -> RepositoryNotFoundException; existing repository with an unknown tag -> ImageNotFoundException.
Emulator: error code `NotImplemented` for every call.
Why: replication-status polling loops see an unmodeled error and cannot tell a missing repository from a missing image.
Fix: the DescribeImageReplicationStatus operation handler and its repository/image resolution in ECR state.
Traces: ecr.describe-image-replication-status-not-found, ecr.describe-image-replication-status-missing-image-rejected.

Cause 3 of 7: ListImageReferrers
AWS: Smithy ListImageReferrers - input {repositoryName (required), subjectId (required), registryId, filter, nextToken, maxResults (range 1-50)}; output {referrers, nextToken}; modeled errors RepositoryNotFoundException (400), InvalidParameterException (400), UnableToListUpstreamImageReferrersException (400), ValidationException (400), ServerException (500). A call naming a repository that does not exist must be refused with RepositoryNotFoundException.
Emulator: error code `NotImplemented` for every call.
Why: referrer discovery (attestations/signatures) is unavailable and the not-found contract is unobservable.
Fix: the ListImageReferrers operation handler and its repository resolution/pagination in ECR state.
Traces: ecr.list-image-referrers-repo-not-found-rejected, ecr.list-image-referrers-repository-not-found-rejected.

Cause 4 of 7: DeregisterPullTimeUpdateExclusion
AWS: Smithy DeregisterPullTimeUpdateExclusion - input {principalArn (required, length 0-200, pattern ^arn:aws(-[a-z]+)*:iam::[0-9]{12}:(role|user)/...)}; output {principalArn}; modeled errors ExclusionNotFoundException (client, httpError 400, doc "The specified pull time update exclusion was not found."), InvalidParameterException (400), LimitExceededException (400), ValidationException (400), ServerException (500). Deregistering a principal that was never registered must be refused with ExclusionNotFoundException, and the call must not be a silent no-op success.
Emulator: error code `NotImplemented` for every call; the five checks that assert ExclusionNotFoundException all observe 'NotImplemented'.
Why: idempotency/cleanup code that keys off ExclusionNotFoundException (and the checks' own teardown paths that swallow exactly that code) breaks, and the "never registered" vs "already removed" distinction is lost.
Fix: the DeregisterPullTimeUpdateExclusion operation handler and the pull-time-exclusion set in ECR registry state.
Traces: ecr.deregister-exclusion-not-found, ecr.deregister-pull-time-exclusion-not-found-rejected, ecr.deregister-pull-time-update-exclusion-nonexistent-rejected, ecr.deregister-pull-time-update-exclusion-not-found-rejected, ecr.deregister-pull-time-update-exclusion-unknown-rejected.

Cause 5 of 7: UpdatePullThroughCacheRule
AWS: Smithy UpdatePullThroughCacheRule - input {ecrRepositoryPrefix (required, length 2-30), credentialArn, customRoleArn, registryId}; output {ecrRepositoryPrefix, registryId, updatedAt, credentialArn, customRoleArn, upstreamRepositoryPrefix}; modeled errors PullThroughCacheRuleNotFoundException (client, 400, doc "The pull through cache rule was not found. Specify a valid pull through cache rule and try again."), InvalidParameterException (400), SecretNotFoundException (400), UnableToAccessSecretException (400), UnableToDecryptSecretValueException (400), ValidationException (400), ServerException (500). Updating a prefix with no rule must be refused with PullThroughCacheRuleNotFoundException.
Emulator: error code `NotImplemented` for every call.
Why: callers cannot distinguish "no such rule" from a secret/permission problem, and the modeled 400 class is never produced.
Fix: the UpdatePullThroughCacheRule operation handler and the pull-through-cache-rule table in ECR registry state.
Traces: ecr.update-pull-through-cache-rule-not-found-rejected.

Cause 6 of 7: ValidatePullThroughCacheRule
AWS: Smithy ValidatePullThroughCacheRule - input {ecrRepositoryPrefix (required), registryId}; output {ecrRepositoryPrefix, registryId, upstreamRegistryUrl, credentialArn, customRoleArn, upstreamRepositoryPrefix, isValid (default false), failure}; modeled errors PullThroughCacheRuleNotFoundException (400), InvalidParameterException (400), ValidationException (400), ServerException (500). Validating a prefix that was never created must be refused with PullThroughCacheRuleNotFoundException rather than returning isValid=false or an unmodeled error.
Emulator: error code `NotImplemented` for every call.
Why: a client cannot tell an unregistered prefix from a rule that exists but fails validation - exactly the distinction this operation exists for.
Fix: the ValidatePullThroughCacheRule operation handler and the same pull-through-cache-rule state lookup.
Traces: ecr.validate-pull-through-cache-rule-not-found, ecr.validate-pull-through-cache-rule-not-found-rejected.

Cause 7 of 7: DeleteRepositoryCreationTemplate
AWS: Smithy DeleteRepositoryCreationTemplate - input {prefix (required, length 1-256)}; output {registryId, repositoryCreationTemplate}; modeled errors TemplateNotFoundException (client, 400), InvalidParameterException (400), ValidationException (400), ServerException (500). Deleting a template prefix that does not exist must be refused with TemplateNotFoundException, and a re-delete of a deleted template must be refused the same way.
Emulator: error code `NotImplemented` for every call. (The sibling CreateRepositoryCreationTemplate stub is separately filed; this is the delete handler, which that issue does not name.)
Why: template lifecycle code cannot detect the already-deleted state and a delete of a typo'd prefix looks like an unmodeled server fault.
Fix: the DeleteRepositoryCreationTemplate operation handler and the repository-creation-template store in ECR registry state.
Traces: ecr.delete-repository-creation-template-nonexistent-rejected.

## Evidence
Replay output, all from the behavioral suite's ecr group:

- ecr.describe-image-signing-status-missing-repo-rejected [mismatch]: AssertionError: expected RepositoryNotFoundException, got 'NotImplemented'
- ecr.describe-image-signing-status-missing-image-rejected [mismatch]: assert code == "ImageNotFoundException" -> AssertionError: NotImplemented
- ecr.describe-image-signing-status-not-found [mismatch]: AssertionError: expected ImageNotFoundException, got NotImplemented
- ecr.describe-image-replication-status-not-found [mismatch]: AssertionError: expected RepositoryNotFoundException for nonexistent repo 'sim2real-ecr-noexist-<id>', got 'NotImplemented'
- ecr.describe-image-replication-status-missing-image-rejected [mismatch]: assert code == "ImageNotFoundException" -> AssertionError: NotImplemented
- ecr.list-image-referrers-repo-not-found-rejected [mismatch]: assert code == "RepositoryNotFoundException" -> AssertionError: NotImplemented
- ecr.list-image-referrers-repository-not-found-rejected [mismatch]: assert code == "RepositoryNotFoundException" -> AssertionError: NotImplemented
- ecr.deregister-exclusion-not-found [mismatch]: AssertionError: expected ExclusionNotFoundException deregistering a never-registered principal, got 'NotImplemented'
- ecr.deregister-pull-time-exclusion-not-found-rejected [mismatch]: same assertion, got 'NotImplemented'
- ecr.deregister-pull-time-update-exclusion-nonexistent-rejected [mismatch]: AssertionError: expected ExclusionNotFoundException, got NotImplemented
- ecr.deregister-pull-time-update-exclusion-not-found-rejected [mismatch]: assert code == "ExclusionNotFoundException" -> AssertionError: NotImplemented
- ecr.deregister-pull-time-update-exclusion-unknown-rejected [mismatch]: assert code == "ExclusionNotFoundException" -> AssertionError: NotImplemented
- ecr.update-pull-through-cache-rule-not-found-rejected [mismatch]: AssertionError: NotImplemented
- ecr.validate-pull-through-cache-rule-not-found [mismatch]: AssertionError: expected PullThroughCacheRuleNotFoundException for an unregistered prefix, got 'NotImplemented'
- ecr.validate-pull-through-cache-rule-not-found-rejected [mismatch]: AssertionError: expected PullThroughCacheRuleNotFoundException for a never-created prefix, got 'NotImplemented'
- ecr.delete-repository-creation-template-nonexistent-rejected [mismatch]: AssertionError: expected TemplateNotFoundException, got NotImplemented

Expected (AWS, per Smithy model of com.amazonaws.ecr): a client error with httpError 400 and the modeled code for each case - RepositoryNotFoundException / ImageNotFoundException / ExclusionNotFoundException / PullThroughCacheRuleNotFoundException / TemplateNotFoundException.
Actual (emulator): error code `NotImplemented`, message "operation <Op> is scaffolded but not implemented in this emulator", for every call to the seven operations above.

## Steps to Reproduce
1. Boot the emulator and point an ECR client at it.
2. Cause 1: ecr describe-image-signing-status --repository-name <never-created> --image-id imageTag=v1  -> expect RepositoryNotFoundException; then create a repository and repeat for an absent tag -> expect ImageNotFoundException.
3. Cause 2: ecr describe-image-replication-status --repository-name <never-created> --image-id imageTag=v1 -> expect RepositoryNotFoundException; repeat against an existing repository with an absent tag -> expect ImageNotFoundException.
4. Cause 3: ecr list-image-referrers --repository-name <never-created> --subject-id imageDigest=sha256:... -> expect RepositoryNotFoundException.
5. Cause 4: ecr deregister-pull-time-update-exclusion --principal-arn arn:aws:iam::<account>:role/<never-registered> -> expect ExclusionNotFoundException (not a no-op 200).
6. Cause 5: ecr update-pull-through-cache-rule --ecr-repository-prefix <no-such-prefix> -> expect PullThroughCacheRuleNotFoundException.
7. Cause 6: ecr validate-pull-through-cache-rule --ecr-repository-prefix <no-such-prefix> -> expect PullThroughCacheRuleNotFoundException.
8. Cause 7: ecr delete-repository-creation-template --prefix <no-such-prefix> -> expect TemplateNotFoundException.
9. Re-replay the checks in trace_ids to confirm.
