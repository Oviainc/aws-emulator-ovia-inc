Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.ecracc-describe-unknown-repository-rejected ecr.ecsdeploy-repository-config-round-trips ecr.pull-through-cache-rule-upstream-repository-prefix-read-back ecr.e2e-signing-configuration-drives-signing-status
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.ecracc-describe-unknown-repository-rejected|run#12|AssertionError|assert [r["repositoryName"] for r in twice] == [name, name], (
Issue-Key: ecr.ecsdeploy-repository-config-round-trips|run#11|AssertionError|assert re.fullmatch(fr"arn:aws:kms:{ctx.region}:{ctx.account}:key/[0-9a-f-]+", kms_key_arn or ""), (
Issue-Key: ecr.pull-through-cache-rule-upstream-repository-prefix-read-back|run#7|AssertionError|assert _fields(created_root) == expected_root, (
Issue-Key: ecr.e2e-signing-configuration-drives-signing-status|run#21|Timeout|time.sleep(2)
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# ECR control-plane representation diverges: DescribeRepositories deduplicates repeated names, CreateRepository(KMS) never resolves the default managed key, CreatePullThroughCacheRule invents upstreamRegistry, and signing never completes (4 causes)

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRepositories

## Description
Four net-new root causes in the ecr emulator, all grounded in failing behavioral checks of this suite. Each cause is independent and must be closed separately.

=== Cause 1 of 4: DescribeRepositories deduplicates a repository name listed twice ===
AWS: Smithy: DescribeRepositories.repositoryNames -> RepositoryNameList, @length min 1 max 100; the list is NOT declared @uniqueItems, and output `repositories` is a RepositoryList. Recorded behavior: real ECR projects the REQUESTED name list onto the response, so a name supplied twice in one call yields two Repository entries (one per requested element), in request order. Errors modelled for the op: InvalidParameterException 400, RepositoryNotFoundException 400, ServerException 500.
Emulator: DescribeRepositories(repositoryNames=[N, N]) answers 200 with exactly ONE Repository entry for N. The handler iterates the STORED repositories and keeps those whose name is contained in the requested list (`wanted.contains(name)`), so the response has at most one entry per stored repository regardless of how many times the caller named it.
Why: a client that zips its requested name list against `repositories` positionally, or that counts entries to confirm every requested name was answered, silently mis-pairs repositories or concludes a requested name was dropped. The response length is a function of the stored set instead of the request.
Fix: the DescribeRepositories handler's candidate projection — build the result by walking the requested `repositoryNames` in order (one output entry per requested element) and fall back to walking the store only when `repositoryNames` is absent.
Traces: ecr.ecracc-describe-unknown-repository-rejected — "AssertionError: the same name listed twice must not be deduplicated; got [{... 'repositoryName': 'sim2real-ecracc-repo-...'}]" (one entry where two were required). Note the same check also asserts the RepositoryNotFoundException message for a missing name among valid ones; that assertion was never reached, so this issue makes no claim about it.

=== Cause 2 of 4: CreateRepository with encryptionType=KMS and no kmsKey does not resolve the account's default ECR managed key ===
AWS: Smithy: EncryptionConfiguration.kmsKey (@length 1..2048), documented as "If you use the KMS encryption type, specify the KMS key to use for encryption. The alias, key ID, or full ARN of the KMS key can be specified. ... If no key is specified, the default Amazon Web Services managed KMS key for Amazon ECR will be used." Real ECR therefore RESOLVES that default managed key at create time and returns the created repository with encryptionConfiguration = {encryptionType: "KMS", kmsKey: "arn:aws:kms:<region>:<account>:key/<key-id>"}, and the same resolved value is read back by DescribeRepositories. The ARN is a template: fixed prefix arn:aws:kms:, then the request's region and the caller's account, then :key/ and the key's own id — never a literal id from one run.
Emulator: CreateRepository(encryptionConfiguration={encryptionType: "KMS"}) returns encryptionConfiguration with encryptionType=KMS but kmsKey ABSENT (None), so no key is ever resolved or stored and DescribeRepositories reports the same key-less configuration.
Why: a client that reads repository.encryptionConfiguration.kmsKey to learn which CMK protects the repository (to grant kms:Decrypt to a puller, to audit encryption-at-rest, or to assert the key is in-region) gets nothing and cannot distinguish "KMS with the AWS managed key" from a broken configuration.
Fix: the CreateRepository handler's encryption-configuration resolution — resolve the account's default AWS-managed ECR key to a key ARN when encryptionType is KMS/KMS_DSSE and kmsKey is omitted, persist the resolved value in the repository state so DescribeRepositories round-trips it. Related but distinct open issue: 20261024T182018Z-ecr-kms-ecr-createrepository-with-encryptiontype-kms-creates-no-kms.md covers the missing KMS GRANT; this cause is the missing resolved kmsKey on the response/stored record.
Traces: ecr.ecsdeploy-repository-config-round-trips — "AssertionError: the create request never supplied a kmsKey, so the response must resolve the account's default managed key to a real key ARN; got None".

=== Cause 3 of 4: CreatePullThroughCacheRule returns an upstreamRegistry the request never supplied ===
AWS: Smithy: CreatePullThroughCacheRule input members ecrRepositoryPrefix (required), upstreamRegistryUrl (required), upstreamRepositoryPrefix, upstreamRegistry (enum UpstreamRegistry: ecr, ecr-public, quay, k8s, docker-hub, github-container-registry, azure-container-registry, gitlab-container-registry, chainguard); output carries ecrRepositoryPrefix, upstreamRegistryUrl, createdAt, registryId, upstreamRegistry, credentialArn, customRoleArn, upstreamRepositoryPrefix. Recorded behavior only: when the request omits upstreamRegistry, real ECR's CreatePullThroughCacheRule response omits upstreamRegistry — the response members are exactly ecrRepositoryPrefix, registryId, upstreamRegistryUrl, upstreamRepositoryPrefix (plus createdAt). ECR does not synthesize the enum from the URL on this response.
Emulator: for CreatePullThroughCacheRule(ecrRepositoryPrefix=<p>, upstreamRegistryUrl="public.ecr.aws", upstreamRepositoryPrefix="ROOT") with no upstreamRegistry, the response carries an EXTRA member upstreamRegistry="ecr-public". The handler derives it with `crate::state::upstream_registry_for_url(...)` whenever the member is absent and then stores and echoes that invented value.
Why: a client comparing the created rule against the rule it asked for (an exact-equality read-back, an idempotency check, or a CloudFormation/IaC drift comparison) sees a member it never set and reports drift, and a client that treats the presence of upstreamRegistry as "the caller pinned the upstream registry" draws the wrong conclusion.
Fix: the CreatePullThroughCacheRule handler — do not populate the response (or the stored record's) upstreamRegistry from a URL inference; carry it only when the request supplied it. The existing URL lookup should stay confined to deciding UnsupportedUpstreamRegistryException, not to filling the output member.
Traces: ecr.pull-through-cache-rule-upstream-repository-prefix-read-back — "AssertionError: expected CreatePullThroughCacheRule to return {'ecrRepositoryPrefix': ..., 'registryId': ..., 'upstreamRegistryUrl': 'public.ecr.aws', 'upstreamRepositoryPrefix': 'ROOT'}, got {... 'upstreamRegistry': 'ecr-public' ...}". The prefix/registry/URL values in that message are this run's generated values; the requirement is only the SET of members.

=== Cause 4 of 4: a PutSigningConfiguration rule never produces a terminal signing status for a matched pushed image ===
AWS: Smithy: DescribeImageSigningStatus output `signingStatuses` -> ImageSigningStatusList of ImageSigningStatus{signingProfileArn, status, failureCode, failureReason}; `status` is documented as IN_PROGRESS (signing in progress), COMPLETE (signature generated), FAILED (see failureCode/failureReason). Recorded behavior: with a signing configuration whose rule matches a repository by WILDCARD_MATCH and names an AWS Signer signing profile, a PutImage into that repository starts one signing job per matching rule, so DescribeImageSigningStatus converges to exactly ONE entry whose signingProfileArn is the configured profile ARN and whose status reaches the terminal value COMPLETE.
Emulator: after PutSigningConfiguration({rules:[{signingProfileArn, repositoryFilters:[{filter:<repo>, filterType:WILDCARD_MATCH}]}]}) and a successful PutImage into that repository, DescribeImageSigningStatus never returns a terminal entry — the check polls it in a loop and never breaks, so the check hits the harness cap ("Check timed out after 60.0s"). No signing job is ever recorded for the pushed image, so neither COMPLETE nor FAILED is ever observable.
Why: a client that pushes an image and waits for its signature before promoting it (an admission gate, a deploy pipeline step, a compliance check) blocks forever instead of being told the signature completed or failed; signing-on-push is unobservable rather than merely slow.
Fix: ECR's signing state and the PutImage path — on a PutImage whose repository matches a rule in the stored signing configuration, record one signing job per matching rule and advance it to a terminal status that DescribeImageSigningStatus reports (with signingProfileArn echoing the rule's profile). This is the same defect family as the already-open issue 20261027T072618Z-ecr-signing-on-push-never-records-a-signing.md (DescribeImageSigningStatus stays empty for a matched image); that issue is OPEN, not resolved, and this check was not marked as owned by it, so it is filed here and should be closed together with it.
Traces: ecr.e2e-signing-configuration-drives-signing-status — "Error: Check timed out after 60.0s" while polling describe_image_signing_status for a terminal status.

## Evidence
Suite: simparity-behavioral-aws-v1, 196/347 passed; service heading '### ecr' 178/212 (84%), 34 failed, of which 30 are already filed/owned and these 4 are net-new.

1) ecr.ecracc-describe-unknown-repository-rejected [mismatch]
   Expected: describe_repositories(repositoryNames=[name, name]) -> repositories with repositoryName == [name, name] (two entries, request order).
   Actual: one entry only — "AssertionError: the same name listed twice must not be deduplicated; got [{'repositoryArn': 'arn:aws:ecr:us-west-2:000000000000:repository/sim2real-ecracc-repo-741ae042f5f2', 'registryId': '000000000000', 'repositoryName': 'sim2real-ecracc-repo-741ae042f5f2', ... }]".
   Code: services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/describe_repositories.rs lines 199-204 filter the STORE by `wanted.is_empty() || wanted.contains(name)`, so the response length is bounded by the stored set, not the request list.

2) ecr.ecsdeploy-repository-config-round-trips [mismatch]
   Request: create_repository(repositoryName=..., imageScanningConfiguration={'scanOnPush': False}, encryptionConfiguration={'encryptionType': 'KMS'}).
   Expected: created repository's encryptionConfiguration.kmsKey matches arn:aws:kms:<region>:<account>:key/<uuid>, and DescribeRepositories returns the identical encryptionConfiguration.
   Actual: "AssertionError: the create request never supplied a kmsKey, so the response must resolve the account's default managed key to a real key ARN; got None".
   Smithy: EncryptionConfiguration.kmsKey doc — "If no key is specified, the default Amazon Web Services managed KMS key for Amazon ECR will be used."

3) ecr.pull-through-cache-rule-upstream-repository-prefix-read-back [mismatch]
   Request: create_pull_through_cache_rule(ecrRepositoryPrefix=<prefix>, upstreamRegistryUrl='public.ecr.aws', upstreamRepositoryPrefix='ROOT') — upstreamRegistry NOT supplied.
   Expected members (ignoring createdAt/ResponseMetadata): {'ecrRepositoryPrefix', 'registryId', 'upstreamRegistryUrl', 'upstreamRepositoryPrefix'}.
   Actual: additionally 'upstreamRegistry': 'ecr-public' — HTTP 200, content-type application/x-amz-json-1.1.
   Code: services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/create_pull_through_cache_rule.rs lines 243-259 fall back to `crate::state::upstream_registry_for_url(&input.upstream_registry_url)` when the member is absent, and lines 315/328 store and echo it.

4) ecr.e2e-signing-configuration-drives-signing-status [mismatch]
   Sequence: signer.put_signing_profile(platformId='Notation-OCI-SHA384-ECDSA') -> ecr.create_repository -> ecr.put_signing_configuration({'rules': [{'signingProfileArn': <arn>, 'repositoryFilters': [{'filter': <repo>, 'filterType': 'WILDCARD_MATCH'}]}]}) -> layer uploads + ecr.put_image(imageTag='sign-target') -> poll ecr.describe_image_signing_status(repositoryName=<repo>, imageId={'imageTag': 'sign-target'}).
   Expected: signingStatuses converges to exactly one entry with signingProfileArn == the configured profile ARN and status == 'COMPLETE' (or the terminal 'FAILED' with failureCode/failureReason).
   Actual: no entry ever reaches a terminal status; the poll loop never breaks and the check ends with "Error: Check timed out after 60.0s".

## Steps to Reproduce
Against the booted emulator, region us-west-2:

Cause 1 — DescribeRepositories dedupe:
  1. aws ecr create-repository --repository-name repo-dedupe
  2. aws ecr describe-repositories --repository-names repo-dedupe repo-dedupe
     Observe: one entry in `repositories`. AWS returns two (one per requested element, request order).

Cause 2 — CreateRepository KMS default key:
  1. aws ecr create-repository --repository-name repo-kms --encryption-configuration encryptionType=KMS --image-scanning-configuration scanOnPush=false
     Observe: repository.encryptionConfiguration == {"encryptionType":"KMS"} with no kmsKey.
  2. aws ecr describe-repositories --repository-names repo-kms — same key-less configuration.
     AWS returns kmsKey = arn:aws:kms:<region>:<account>:key/<key-id> for the account's default ECR managed key, on both the create response and the describe read-back.

Cause 3 — CreatePullThroughCacheRule invented upstreamRegistry:
  1. aws ecr create-pull-through-cache-rule --ecr-repository-prefix ptc-root --upstream-registry-url public.ecr.aws --upstream-repository-prefix ROOT   (do NOT pass --upstream-registry)
     Observe: the 200 response carries "upstreamRegistry": "ecr-public". AWS omits that member when the request did not supply it.

Cause 4 — signing never completes:
  1. aws signer put-signing-profile --profile-name sig_profile --platform-id Notation-OCI-SHA384-ECDSA
  2. aws ecr create-repository --repository-name repo-sign
  3. aws ecr put-signing-configuration --signing-configuration '{"rules":[{"signingProfileArn":"<profile arn>","repositoryFilters":[{"filter":"repo-sign","filterType":"WILDCARD_MATCH"}]}]}'
  4. Push a minimal image into repo-sign (InitiateLayerUpload / UploadLayerPart / CompleteLayerUpload for the config and layer blobs, then PutImage with imageTag=sign-target).
  5. Poll: aws ecr describe-image-signing-status --repository-name repo-sign --image-id imageTag=sign-target
     Observe: no signingStatuses entry ever reaches COMPLETE or FAILED; the poll never terminates.
