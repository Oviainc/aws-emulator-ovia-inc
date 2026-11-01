Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.deploy-domain-behaves-after-cfn-create codeartifact.deploy-repository-behaves-with-external-connection codeartifact.domain-permissions-policy-server-enforced-on-repository-actions codeartifact.domain-policy-deny-overrides-repository-allow codeartifact.repository-permissions-policy-enforced-for-cross-principal-access codeartifact.repository-policy-deny-scopes-to-the-named-action codeartifact.publish-to-read-only-upstream-repository-blocked codeartifact.delete-domain-permissions-policy-revision-mismatch-conflict codeartifact.disposed-package-version-is-terminal codeartifact.list-packages-origin-control-prefix-and-format-filters-partition-the-repository codeartifact.copy-from-upstream-without-include-flag-rejected codeartifact.publish-duplicate-immutable-version-rejected
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|DescribeDomain|ResourceNotFoundException
Issue-Key: codeartifact|GetRepositoryEndpoint|ResourceNotFoundException
Issue-Key: codeartifact.domain-permissions-policy-server-enforced-on-repository-actions|_error#22|AssertionError|raise AssertionError(f"expected AccessDeniedException but the call SUCCEEDED: {result!r}")
Issue-Key: codeartifact.domain-policy-deny-overrides-repository-allow|_error#18|AssertionError|raise AssertionError(
Issue-Key: codeartifact.repository-permissions-policy-enforced-for-cross-principal-access|_error#21|AssertionError|raise AssertionError(f"expected the call to be denied, but it SUCCEEDED and returned {result!r}")
Issue-Key: codeartifact.repository-policy-deny-scopes-to-the-named-action|_error#18|AssertionError|raise AssertionError(
Issue-Key: codeartifact.publish-to-read-only-upstream-repository-blocked|run#18|AssertionError|assert refusal == expected, (
Issue-Key: codeartifact|DeleteDomainPermissionsPolicy|ConflictException
Issue-Key: codeartifact.disposed-package-version-is-terminal|run#19|AssertionError|assert record["revision"] == revision, (
Issue-Key: codeartifact.list-packages-origin-control-prefix-and-format-filters-partition-the-repository|_error#38|AssertionError|raise AssertionError(
Issue-Key: codeartifact.copy-from-upstream-without-include-flag-rejected|run#20|AssertionError|assert refusal == expected, (
Issue-Key: codeartifact.publish-duplicate-immutable-version-rejected|run#18|AssertionError|assert refusal == expected, f"expected {expected!r}, got {refusal!r}"
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# CodeArtifact: CFN-provisioned domains absent, resource policies never evaluated, origin-block publish mis-classified, and four more control-plane divergences

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishPackageVersion

## Description
Eight distinct root causes in the CodeArtifact emulator, all reached through the codeartifact service (every request below is answered — none of these is a missing operation implementation). Numbered so each can be closed individually; a fixer must close all eight.

Related already-open issue (does NOT cover any cause below): 20261022T171312Z-codeartifact-no-iam-authorization-on-reads-publishpackageversion-digest.md. Its Cause 1 is identity-policy authorization on DescribeRepository, its Cause 3 was CFN Outputs serialized without an OutputValue. Outputs now resolve and the CFN checks fail one layer deeper (Cause 1 here), and resource-based policy evaluation (Cause 2 here) is a different decision point than identity-policy evaluation on reads.

Cause 1 of 8: a CloudFormation-created AWS::CodeArtifact::Domain / ::Repository does not exist in CodeArtifact.
AWS: a stack that reaches CREATE_COMPLETE has actually created its resources; the domain named by AWS::CodeArtifact::Domain is thereafter describable through the CodeArtifact API (DescribeDomain returns status Active and the same ARN the stack's !GetAtt Domain.Arn output resolved to) and can host CreateRepository, and GetRepositoryEndpoint works for a stack-created repository (recorded behavior only — the CloudFormation model does not describe per-resource-type provisioning; the recording shows the stack's resources are live CodeArtifact resources).
Emulator: the stack reaches CREATE_COMPLETE and its Outputs now resolve (DomainName output equals the requested name), but the domain was never written into the CodeArtifact store: DescribeDomain answers ResourceNotFoundException 404 ("Domain not found. Domain '<name>' owned by account '<acct>' does not exist.") and GetRepositoryEndpoint answers the same 404 for the stack's repository. The CFN resource types are resolving attributes without provisioning anything.
Why: any infrastructure-as-code flow is untestable end to end — a client deploys its stack, sees CREATE_COMPLETE, then every CodeArtifact call against the deployed domain 404s, which is indistinguishable from a deploy that silently failed.
Fix: the CloudFormation stack provisioner's resource handlers for the AWS::CodeArtifact::Domain and AWS::CodeArtifact::Repository types — they must create the resource in the CodeArtifact service state (and delete it on stack delete), not only resolve GetAtt attributes.
Traces: codeartifact.deploy-domain-behaves-after-cfn-create, codeartifact.deploy-repository-behaves-with-external-connection.

Cause 2 of 8: domain and repository permissions policies are stored but never enforced — neither their Allow scoping nor their Deny.
AWS: CodeArtifact resource policies are evaluated server-side on every domain/repository-scoped call. A domain policy that Allows one named role and nobody else makes ListRepositoriesInDomain fail for a sibling principal with AccessDeniedException (Smithy: ListRepositoriesInDomain errors -> AccessDeniedException, httpError 403, "The operation did not succeed because of an unauthorized access attempt"); an explicit Deny in the domain policy overrides a repository-policy Allow for the same action (ListPackages -> AccessDeniedException 403); a repository policy that does not name a principal denies that principal GetRepositoryEndpoint (403); and a repository-policy Deny scoped to a single action denies exactly that action (DescribePackageVersion -> AccessDeniedException 403) while leaving the others working.
Emulator: every one of those four calls returns HTTP 200 with data (ListRepositoriesInDomain returned the repository list, ListPackages returned [], GetRepositoryEndpoint returned the endpoint URL, DescribePackageVersion returned the version). PutDomainPermissionsPolicy/PutRepositoryPermissionsPolicy accept and echo the document, but no authorization decision is ever made from it.
Why: every negative authorization test a client writes passes vacuously. A team validating that a domain policy fences a repository off, or that an explicit Deny beats a downstream Allow, gets green against an emulator that would have served the data to an unauthorized principal — the privilege defect ships.
Fix: resource-policy evaluation in the CodeArtifact request pipeline (the authorization decision ahead of the operation handlers), reading the stored domain/repository policy documents, with the AccessDeniedException path wired in the affected read handlers; the stored-policy state lives in the service's state module.
Traces: codeartifact.domain-permissions-policy-server-enforced-on-repository-actions, codeartifact.domain-policy-deny-overrides-repository-allow, codeartifact.repository-permissions-policy-enforced-for-cross-principal-access, codeartifact.repository-policy-deny-scopes-to-the-named-action.

Cause 3 of 8: a publish refused by the package origin configuration is classified as an authorization failure.
AWS: when a package's origin controls are set to publish: BLOCK, PublishPackageVersion is refused with ValidationException (HTTP 400; Smithy: PublishPackageVersion errors -> ValidationException httpError 400, "The operation did not succeed because a parameter in the request was sent with an invalid value"), recorded message example 'This package is configured to block new versions from being published.' The requirement is the 400/ValidationException classification: this is a configuration-of-the-request refusal, not an identity decision.
Emulator: answers AccessDeniedException (HTTP 403), message example 'Publishing package <pkg> is blocked by its package origin configuration.'
Why: a client cannot tell a blocked-origin repository from a missing IAM grant. Retry/credential-refresh logic keyed on 403 fires on a condition no credential change can fix, and code that keys remediation on ValidationException never runs.
Fix: the origin-restriction check in the PublishPackageVersion handler (services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/publish_package_version.rs) must raise the modeled ValidationException instead of AccessDeniedException.
Traces: codeartifact.publish-to-read-only-upstream-repository-blocked.

Cause 4 of 8: DeleteDomainPermissionsPolicy is not idempotent when a policyRevision is supplied.
AWS: DeleteDomainPermissionsPolicy on a domain that holds no policy succeeds (HTTP 200) and answers the bare ResourcePolicy envelope with no revision member, even when the caller passes a policyRevision — the optimistic lock applies only to a policy that is actually attached. (Smithy: DeleteDomainPermissionsPolicy.policyRevision is optional, documented as "The current revision of the resource policy to be deleted... used for optimistic locking"; output member policy is optional. The idempotent-success leg is recorded behavior.)
Emulator: answers ConflictException (HTTP 409, message 'Provided policy version conflicts with the server') for the re-delete, because the revision comparison treats "no policy attached" as "revision does not match" and runs before the nothing-to-remove case. The handler's own module doc states the intended contract ("A domain that holds no policy has nothing to remove, which is the delete's goal already met: the call succeeds"), so the ordering contradicts the design.
Why: cleanup and teardown code that deletes a policy twice (or races another deleter) gets a 409 on a no-op, so idempotent provisioning and cleanup paths fail against the emulator where they succeed on AWS.
Fix: the revision/optimistic-lock branch of the DeleteDomainPermissionsPolicy handler (src/ops/delete_domain_permissions_policy.rs) — skip the lock when no policy is attached and return the empty envelope.
Traces: codeartifact.delete-domain-permissions-policy-revision-mismatch-conflict.

Cause 5 of 8: DisposePackageVersions mints a new version revision instead of retaining the published one.
AWS: DisposePackageVersions "Deletes the assets in package versions and sets the package versions' status to Disposed" (Smithy doc) — the version RECORD is retained, so DescribePackageVersion afterwards still reports the revision minted at publish. Disposal is a status/asset change, not a replacement of the version.
Emulator: after disposal, DescribePackageVersion reports a different revision than PublishPackageVersion returned, because the revision is DERIVED from the version's mutable content (the handler clears stored.assets and sets status, then reads stored.revision() back), so clearing the assets changes the identity of the record.
Why: the revision is the join key clients use for optimistic concurrency (versionRevisions on Dispose/Copy/UpdatePackageVersionsStatus) and for correlating EventBridge packageVersionRevision values. A revision that moves under a disposal invalidates every stored reference and makes revision-pinned calls fail for reasons the client cannot explain.
Fix: version-revision minting in the CodeArtifact state module — the revision must be a stored field assigned at publish (and only re-minted when the version's content is genuinely republished), not recomputed from current assets/status; the Dispose handler (src/ops/dispose_package_versions.rs) must report the retained revision.
Traces: codeartifact.disposed-package-version-is-terminal.

Cause 6 of 8: ListPackages accepts a namespace filter with no format.
AWS: ListPackages.namespace is documented as a namespace PREFIX filter whose meaning depends on the package format ("The namespace of a Maven package version is its groupId... of an npm or Swift package version is its scope" — Smithy: ListPackagesRequest.namespace), and format is the member that selects which of those it means. Hypothesis (the failing check is an unblessed DRAFT and neither the model nor a recording states the refusal): a namespace filter with no format is rejected with ValidationException (HTTP 400; Smithy: ListPackages errors -> ValidationException httpError 400), recorded-as-expected message example 'PackageFormat cannot be null when PackageNamespace is not null'. The 400/ValidationException classification is the requirement; the message text is the check's expectation, not an observation.
Emulator: returns HTTP 200 and a full package listing for list_packages(domain, repository, namespace='sim2real') with no format, so the under-specified filter is silently accepted.
Why: a client that omits format gets an answer the service would have refused, so a request that is invalid on AWS looks correct in testing and the filter's format-dependent meaning is never pinned.
Fix: input validation in the ListPackages handler (src/ops/list_packages.rs) — the cross-member rule namespace-requires-format.
Traces: codeartifact.list-packages-origin-control-prefix-and-format-filters-partition-the-repository.

Cause 7 of 8: the not-found refusal of a copy that would need includeFromUpstream does not identify the package it looked for.
AWS: CopyPackageVersions of a version present only in an upstream, without includeFromUpstream, is refused with ResourceNotFoundException (HTTP 404; Smithy: CopyPackageVersions errors -> ResourceNotFoundException httpError 404) and the message is built from the request — it names the package FORMAT, the NAMESPACE and the package name alongside the repository, e.g. "The generic package 'sim2real-upstream-only' with namespace 'sim2real' does not exist in repository '<repo>'." The rule (message identifies the addressed format/namespace/package/repository) is the requirement; the exact phrasing is an example.
Emulator: same code and status, but the message carries only the bare package name and repository ('Package sim2real-upstream-only not found in repository <repo>.'), dropping format and namespace — the two members that disambiguate which package was addressed.
Why: a package is addressed by its (format, namespace, name) triple, so a message that omits two thirds of the key cannot tell a caller whether it mistyped the namespace or the format. Clients that surface or parse the refusal lose that information.
Fix: the not-found message builder used by the copy path (the shared package-not-found message helper in the CodeArtifact state module, used by src/ops/copy_package_versions.rs) must render the full addressed triple.
Traces: codeartifact.copy-from-upstream-without-include-flag-rejected.

Cause 8 of 8: the duplicate-publish conflict message does not name the addressed version or the status rule.
AWS: re-publishing an existing Published version is refused with ConflictException (HTTP 409; Smithy: PublishPackageVersion errors -> ConflictException httpError 409) and the message is built from the request and the stored state — it names the format, the full namespace/package/version path and the blocking status, e.g. "Cannot update existing generic package version 'sim2real-ns/sim2real-pkg/1.0.0' with status 'Published'. Only 'Unfinished' package versions can be updated." The rule (message identifies format + namespace/package/version + current status, and states that only Unfinished versions are updatable) is the requirement; the phrasing is an example.
Emulator: same code and status, message 'Package version sim2real-pkg 1.0.0 is Published and cannot be published to.' — no format, no namespace, and no statement of the Unfinished-only rule.
Why: clients and CI logs rely on the refusal to say which version in which namespace collided and why; without the namespace/format the message does not identify the resource, and without the status rule a caller cannot discover that the republish would have been allowed with unfinished=true.
Fix: the conflict-message construction in the PublishPackageVersion handler (src/ops/publish_package_version.rs).
Traces: codeartifact.publish-duplicate-immutable-version-rejected.

## Evidence
Cause 1 — codeartifact.deploy-domain-behaves-after-cfn-create:
  stack reached CREATE_COMPLETE and outputs['DomainName'] == the requested name, then
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeDomain operation: Domain not found. Domain 'sim2real-behav-cfndom-05948dd61e95' owned by account '000000000000' does not exist.
  codeartifact.deploy-repository-behaves-with-external-connection:
  botocore.errorfactory.ResourceNotFoundException: ... when calling the GetRepositoryEndpoint operation: Domain not found. Domain 'sim2real-behav-cfndom-c0eae77c4a62' owned by account '000000000000' does not exist.
  (Template: AWS::CodeArtifact::Domain, plus AWS::CodeArtifact::Repository with DependsOn: Domain.)

Cause 2 — codeartifact.domain-permissions-policy-server-enforced-on-repository-actions:
  AssertionError: expected AccessDeniedException but the call SUCCEEDED: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'repositories': [{'name': 'repo-0d915e98dbb5', 'domainName': 'sim2real-cadompol-8ca0356f2218', ...}]}
  — caller was a sibling role named in neither an identity policy nor the domain policy.
  codeartifact.domain-policy-deny-overrides-repository-allow:
  AssertionError: the call SUCCEEDED where the domain policy denies it and returned [] (ListPackages, with an explicit domain-policy Deny over a repository-policy Allow).
  codeartifact.repository-permissions-policy-enforced-for-cross-principal-access:
  AssertionError: expected the call to be denied, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'repositoryEndpoint': 'https://sim2real-carepopol-...-000000000000.d.codeartifact.us-west-2.amazonaws.com/npm/repo-82a001a5ea96/'} (GetRepositoryEndpoint for a principal the repository policy does not name).
  codeartifact.repository-policy-deny-scopes-to-the-named-action:
  AssertionError: the call SUCCEEDED where the repository policy denies it — there is no error code (DescribePackageVersion under a repository-policy Deny naming that action).

Cause 3 — codeartifact.publish-to-read-only-upstream-repository-blocked:
  expected ('ValidationException', 'This package is configured to block new versions from being published.')  [HTTP 400]
  got      ('AccessDeniedException', 'Publishing package mirrored-pkg-702fe12c0b6a is blocked by its package origin configuration.')  [HTTP 403]

Cause 4 — codeartifact.delete-domain-permissions-policy-revision-mismatch-conflict:
  The stale-revision leg and the correct-revision delete behaved; the escaped exception is the idempotent re-delete:
  botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the DeleteDomainPermissionsPolicy operation: Provided policy version conflicts with the server
  — raised by delete_domain_permissions_policy(domain=D, policyRevision=<the revision of the already-deleted policy>) on a domain that now holds no policy; AWS answers 200 with a policy envelope carrying no 'revision'.

Cause 5 — codeartifact.disposed-package-version-is-terminal:
  AssertionError: disposal changed the revision to 'IERwYGRCImZCNOrRIFW8hDXIXYe21ihZlxXer+tGokM=', expected the published '7a23Bv4sC0Df9Vje6LJ/QcWkV2Jy7Ny4A2Sg1RubrR8=' — the record is retained, not replaced.
  (Values are examples; the requirement is that the revision does not change across DisposePackageVersions.)

Cause 6 — codeartifact.list-packages-origin-control-prefix-and-format-filters-partition-the-repository:
  AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'packages': [{'format': 'generic', 'namespace': 'sim2real', 'package': 'alpha-one-...'}, ...]}
  for list_packages(domain=D, repository=R, namespace='sim2real') with no format; expected ValidationException (400).

Cause 7 — codeartifact.copy-from-upstream-without-include-flag-rejected:
  got      ('ResourceNotFoundException', 'Package sim2real-upstream-only not found in repository sim2real-behav-ca-dn-f1e567989f0e.')
  expected ('ResourceNotFoundException', "The generic package 'sim2real-upstream-only' with namespace 'sim2real' does not exist in repository 'sim2real-behav-ca-dn-f1e567989f0e'.")
  — code and status agree; the message omits the request-derived format and namespace.

Cause 8 — codeartifact.publish-duplicate-immutable-version-rejected:
  got      ('ConflictException', 'Package version sim2real-pkg 1.0.0 is Published and cannot be published to.')
  expected ('ConflictException', "Cannot update existing generic package version 'sim2real-ns/sim2real-pkg/1.0.0' with status 'Published'. Only 'Unfinished' package versions can be updated.")

## Steps to Reproduce
Cause 1:
1. cloudformation.create_stack with a template declaring AWS::CodeArtifact::Domain (DomainName from a parameter) and an Output !GetAtt Domain.Name / Domain.Arn; wait for CREATE_COMPLETE.
2. codeartifact.describe_domain(domain=<that name>) -> observe ResourceNotFoundException 404 instead of a domain with status Active and the ARN the stack output resolved to.
3. Variant: add AWS::CodeArtifact::Repository (DependsOn Domain) and call get_repository_endpoint(domain, repository, format='npm') -> same 404.

Cause 2:
1. create_domain(domain=D), create_repository(domain=D, repository=R) with the privileged client.
2. iam.create_role for two sibling roles trusted by the account root; grant neither any CodeArtifact identity permission; sts.assume_role into the second one.
3. put_domain_permissions_policy(domain=D, policyDocument=<Allow codeartifact:ListRepositoriesInDomain to role 1 only>), then call list_repositories_in_domain(domain=D) as role 2 -> observe HTTP 200 instead of AccessDeniedException (403).
4. Deny leg: put_repository_permissions_policy(Allow codeartifact:ListPackages to the principal) plus put_domain_permissions_policy(explicit Deny of codeartifact:ListPackages for the same principal), then list_packages(domain=D, repository=R) -> observe HTTP 200.
5. Repository-policy legs: with a repository policy that does not name the principal, call get_repository_endpoint(...) as that principal -> HTTP 200; with a repository policy denying only codeartifact:DescribePackageVersion, call describe_package_version(...) -> HTTP 200.

Cause 3:
1. create_domain/create_repository; publish_package_version(format='generic', namespace='sim2real', package=P, packageVersion='1.0.0', ...).
2. put_package_origin_configuration(..., package=P, restrictions={'publish': 'BLOCK', 'upstream': 'ALLOW'}).
3. publish_package_version(..., package=P, packageVersion='2.0.0', ...) -> observe AccessDeniedException 403 instead of ValidationException 400.

Cause 4:
1. create_domain(domain=D); put_domain_permissions_policy(domain=D, policyDocument=<any>) and keep policy['revision'] as REV.
2. delete_domain_permissions_policy(domain=D, policyRevision=REV) -> succeeds (correct).
3. Call it again with the same policyRevision=REV on the now policy-less domain -> observe ConflictException 409 instead of HTTP 200 with a policy envelope that carries no 'revision'.

Cause 5:
1. create_domain/create_repository; rev = publish_package_version(format='generic', namespace='sim2real', package=P, packageVersion='1.0.0', ...)['versionRevision'].
2. dispose_package_versions(domain=D, repository=R, format='generic', namespace='sim2real', package=P, versions=['1.0.0']).
3. describe_package_version(... packageVersion='1.0.0')['packageVersion']['revision'] -> observe a different value than rev.

Cause 6:
1. create_domain/create_repository; publish one generic package with namespace='sim2real'.
2. list_packages(domain=D, repository=R, namespace='sim2real') with no format argument -> observe HTTP 200 and a listing instead of ValidationException (400).

Cause 7:
1. create_domain; create upstream repository U and downstream D with U as an upstream of D.
2. publish a generic package 'sim2real-upstream-only' (namespace 'sim2real') into U only.
3. copy_package_versions(domain, sourceRepository=D, destinationRepository=<third repo>, format='generic', namespace='sim2real', package='sim2real-upstream-only', versions=['1.0.0']) with includeFromUpstream unset -> observe a ResourceNotFoundException whose message names neither the format nor the namespace.

Cause 8:
1. create_domain/create_repository; publish_package_version(format='generic', namespace='sim2real-ns', package='sim2real-pkg', packageVersion='1.0.0', ...).
2. Repeat the identical publish -> observe ConflictException whose message omits the format, the namespace and the Unfinished-only rule.
