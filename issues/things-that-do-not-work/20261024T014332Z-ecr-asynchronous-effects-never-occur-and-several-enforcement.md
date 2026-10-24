Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.e2e-pull-through-cache-full-flow ecr.e2e-pull-through-cache-rule-consumption-flow ecr.e2e-repository-full-lifecycle-scan-and-replicate ecr.replicated-image-is-byte-identical-in-the-destination-region ecr.e2e-replication-configuration-drives-replication-status ecr.signing-configuration-enforced-verification ecr.e2e-pull-through-cache-validate-update-cycle ecr.validate-pull-through-cache-rule-secret-not-found-rejected ecr.put-image-dangling-references-rejected ecr.manifest-list-child-reference-integrity ecr.e2e-registry-policy-cross-account-pull-consequence ecr.scan-not-found-before-scan-started ecr.ecsnrt-sdk-pushed-image-reports-manifest-digest-and-blob-sizes ecr.oci-manifest-media-type-negotiation ecr.deploy-registry-scanning-configuration-enforced
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.e2e-pull-through-cache-full-flow|_pull_with_retry#9|Timeout|time.sleep(interval_s)
Issue-Key: ecr.e2e-pull-through-cache-rule-consumption-flow|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: ecr.e2e-repository-full-lifecycle-scan-and-replicate|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: ecr.replicated-image-is-byte-identical-in-the-destination-region|run#31|AssertionError|assert dest_repo["repositoryArn"].split(":")[3] == dest_region, (
Issue-Key: ecr.e2e-replication-configuration-drives-replication-status|run#20|Timeout|time.sleep(15)
Issue-Key: ecr.signing-configuration-enforced-verification|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: ecr.e2e-pull-through-cache-validate-update-cycle|run#14|AssertionError|assert before["isValid"] is False, f"expected bogus-credential rule to fail validation: {before!r}"
Issue-Key: ecr.validate-pull-through-cache-rule-secret-not-found-rejected|run#12|AssertionError|assert broken_result["isValid"] is False, f"expected deleted-secret rule to fail validation: {broken_result!r}"
Issue-Key: ecr.put-image-dangling-references-rejected|_error_code#27|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ecr.manifest-list-child-reference-integrity|run#29|AssertionError|assert deleted.get("imageIds", []) == [], (
Issue-Key: ecr.e2e-registry-policy-cross-account-pull-consequence|_poll_until_denied#16|AssertionError|raise AssertionError(
Issue-Key: ecr.scan-not-found-before-scan-started|run#7|AssertionError|assert code == "ScanNotFoundException", f"expected ScanNotFoundException, got {code}"
Issue-Key: ecr.ecsnrt-sdk-pushed-image-reports-manifest-digest-and-blob-sizes|run#10|AssertionError|assert d["imageSizeInBytes"] == want_size, (
Issue-Key: ecr.oci-manifest-media-type-negotiation|run#18|KeyError|assert detail["artifactMediaType"] == OCI_CONFIG, (
Issue-Key: ecr|CreateStack|ValidationError
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# ECR asynchronous effects never occur and several enforcement paths are absent: pull-through mirroring, cross-region replication, signing status, PTC validation, manifest-list integrity, authorization, scan findings, image metadata and a CFN resource type (9 causes)

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetImage

## Description
Nine causes, all in ECR. Related open issues that do NOT cover these checks: 20261022T234922Z-ecr-completelayerupload-is-a-501-stub-cfn-provisioned.md, 20261023T153036Z-ecr-image-metadata-fields-are-missing-or-wrong.md, 20261022T234922Z-ecr-ecs-iam-ecr-setrepositorypolicy-accepts-a-policy-whose-principal-names.md.

Cause 1 of 9: a pull-through cache rule never mirrors the upstream image, so the cached repository never yields it.
AWS: recorded behavior only. With a pull-through cache rule for prefix P, a BatchGetImage against `P/<upstream/path>` causes ECR to import the upstream manifest on demand and return it (`images[0].imageManifest` non-empty), creating a real repository that survives deletion of the rule.
Emulator: CreatePullThroughCacheRule and ValidatePullThroughCacheRule answer, but BatchGetImage on the cached repository never returns an image. Each of these checks sits in a positive poll (90-180 s budgets, BatchGetImage every 5 s) waiting for the mirrored image and is cut off by the harness's 60 s cap with no image ever observed.
Why: every pull-through-cache workflow is unusable — a client sees a rule that validates but a cache that never serves content, and anything built on a mirrored image (scanning it, re-tagging it, depending on it in a build) cannot proceed.
Fix: the on-demand upstream import behind BatchGetImage/BatchImportUpstreamImage for repositories matching a pull-through cache rule prefix, and the implicit creation of the mirrored repository.
Traces: ecr.e2e-pull-through-cache-full-flow, ecr.e2e-pull-through-cache-rule-consumption-flow, ecr.e2e-repository-full-lifecycle-scan-and-replicate.

Cause 2 of 9: cross-region replication is not modelled — a destination-Region client is served the source Region's registry, and replication status never becomes terminal.
AWS: Smithy: DescribeImageReplicationStatus returns ImageReplicationStatus{region, registryId, status (ReplicationStatus enum), failureCode}. With a replication configuration to Region R, the image appears in R and `status` for region R settles to COMPLETE (or FAILED); a client built for R mints Region-R ARNs (`arn:aws:ecr:R:<account>:repository/<name>`).
Emulator: a client addressed at the destination Region is answered with a repository whose ARN names the source Region (recorded: `arn:aws:ecr:us-west-2:...` when us-east-1 was addressed), i.e. one registry is served regardless of the Region in the request; and DescribeImageReplicationStatus never reports a terminal status for the destination Region (the check's 360 s poll observes nothing terminal before the harness cap).
Why: a client cannot verify or consume replication at all; worse, cross-Region isolation is absent, so a caller believes it is reading the destination registry while it is reading the source — resources appear to exist in Regions where they were never created.
Fix: Region scoping of the registry state (the ARN/registry resolution must come from the request's Region) and the replication engine driving DescribeImageReplicationStatus.
Traces: ecr.replicated-image-is-byte-identical-in-the-destination-region, ecr.e2e-replication-configuration-drives-replication-status.

Cause 3 of 9: image signing status is never produced after a matching signing configuration.
AWS: Smithy: DescribeImageSigningStatus returns ImageSigningStatus{signingProfileArn, status (IN_PROGRESS|COMPLETE|FAILED), failureCode, failureReason}; failureReason is documented as present when status is FAILED. With a signing configuration whose rule matches the repository, pushing an image creates a signing job and the status settles (here to FAILED, naming the unresolvable signing profile).
Emulator: DescribeImageSigningStatus returns no signingStatuses entry (or never leaves IN_PROGRESS) for an image pushed under a matching rule; the check's 180 s poll never sees a settled entry.
Why: a client that gates deployment on an image's signing status can never read one, and a non-compliant image is indistinguishable from a compliant one.
Fix: the signing-job state machine behind PutSigningConfiguration/PutImage and the DescribeImageSigningStatus handler.
Traces: ecr.signing-configuration-enforced-verification.

Cause 4 of 9: ValidatePullThroughCacheRule always reports isValid=true, ignoring the rule's credentials.
AWS: Smithy: ValidatePullThroughCacheRuleResponse{isValid (default false), failure}. AWS actually exercises the rule's upstream credential: a rule whose credentialArn names a Secrets Manager secret that does not exist (or whose contents are bogus) validates as isValid=false with a non-empty `failure`.
Emulator: returns `isValid: True, failure: ''` for a rule whose credential secret was deleted and for a rule created with deliberately bogus credentials.
Why: validation is the documented pre-flight for a cache rule; a client is told a rule is good and then gets no images from it (see cause 1), with no diagnosis.
Fix: the ValidatePullThroughCacheRule handler — resolve credentialArn against Secrets Manager state and report isValid/failure accordingly.
Traces: ecr.e2e-pull-through-cache-validate-update-cycle, ecr.validate-pull-through-cache-rule-secret-not-found-rejected.

Cause 5 of 9: manifest-list referential integrity is not enforced on write or on delete.
AWS: Smithy: PutImage declares ReferencedImagesNotFoundException (HTTP 400); a manifest list whose child manifests are not present in the repository is rejected. BatchDeleteImage does not delete a child manifest that a stored manifest list still references — it reports it under `failures` and leaves it in place.
Emulator: PutImage accepts a manifest-list manifest whose referenced child digests were never pushed and returns HTTP 200 with the stored image; BatchDeleteImage deletes a child that a manifest list references, returning it in `imageIds`.
Why: the registry can hold manifest lists that resolve to nothing, and a prune of child digests silently breaks a multi-arch tag — a docker pull of that tag fails against a registry that reported both operations as successful.
Fix: PutImage validation (resolve every child descriptor of a manifest list before storing) and the BatchDeleteImage handler's reference check.
Traces: ecr.put-image-dangling-references-rejected, ecr.manifest-list-child-reference-integrity.

Cause 6 of 9: ECR control-plane calls are not authorization-gated — a principal with no permissions succeeds.
AWS: Smithy: CreateRepository is a sigv4 operation; AWS evaluates identity and resource policy and answers HTTP 400 `AccessDeniedException` for a principal with neither an identity-policy grant nor a registry-policy grant; after a registry policy grants ecr:CreateRepository to that principal the same call succeeds, and deleting the policy denies it again.
Emulator: CreateRepository with the credentials of a freshly created IAM role that has no attached policy and no registry-policy grant returns HTTP 200 with the created repository, so the grant/revoke consequence can never be observed.
Why: the whole authorization dimension is invisible — a client cannot test least-privilege wiring, and code that expects AccessDeniedException sees success.
Fix: the request-authorization layer for ECR (identity + registry/repository resource-policy evaluation) ahead of the CreateRepository handler.
Traces: ecr.e2e-registry-policy-cross-account-pull-consequence.

Cause 7 of 9: DescribeImageScanFindings is an unimplemented scaffold stub.
AWS: Smithy: DescribeImageScanFindings errors include ScanNotFoundException and ImageNotFoundException (HTTP 400 each). For an image that exists but was never scanned AWS answers ScanNotFoundException; for a digest that was never pushed it answers the different ImageNotFoundException.
Emulator: answers error code `NotImplemented` (`operation ... is scaffolded but not implemented in this emulator`) for both, so the two cases are indistinguishable and no scan read path exists.
Why: a client polling scan findings gets an unmodelled error code its SDK cannot classify, and cannot tell a missing image from an unscanned one.
Fix: implement the DescribeImageScanFindings handler (services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/describe_image_scan_findings.rs) with the ScanNotFound/ImageNotFound distinction.
Traces: ecr.scan-not-found-before-scan-started.

Cause 8 of 9: image metadata is computed/returned wrongly — imageSizeInBytes is inflated and artifactMediaType is absent.
AWS: Smithy: ImageDetail.imageSizeInBytes (long) is documented as the size of the image in the repository, and ImageDetail.artifactMediaType is the artifact media type of the image (for an OCI image, the config's media type). For an SDK-pushed single-layer image, imageSizeInBytes equals the pushed blob sizes (layer + config), and DescribeImages reports artifactMediaType for an OCI manifest.
Emulator: reports imageSizeInBytes 423 where layer+config is 299 (it appears to count the manifest bytes as well), and omits artifactMediaType entirely from the image detail, so reading it raises KeyError.
Why: clients that budget storage or that negotiate on media type read a wrong number and a missing field; media-type negotiation for OCI artifacts cannot be performed at all.
Fix: the image-detail projection used by DescribeImages/PutImage state (size accumulation from the manifest's layers+config, and population of artifactMediaType from the manifest config media type).
Traces: ecr.ecsnrt-sdk-pushed-image-reports-manifest-digest-and-blob-sizes, ecr.oci-manifest-media-type-negotiation.

Cause 9 of 9: CloudFormation does not recognise AWS::ECR::RegistryScanningConfiguration.
AWS: recorded behavior only. CreateStack with a template declaring AWS::ECR::RegistryScanningConfiguration is accepted and the stack applies the registry scanning configuration (observable via GetRegistryScanningConfiguration).
Emulator: CreateStack is refused with HTTP 400 `ValidationError`: `Template format error: Unrecognized resource types: [AWS::ECR::RegistryScanningConfiguration]`, so the configuration can never be declared as infrastructure.
Why: any template that manages registry scanning fails at submit time, before any resource is created.
Fix: register the AWS::ECR::RegistryScanningConfiguration resource provider in the CloudFormation resource-type registry and route it to the ECR registry-scanning state.
Traces: ecr.deploy-registry-scanning-configuration-enforced.

## Evidence
Cause 1 — checks time out inside `_poll_until(lambda: _try_batch_get_image(ecr, '<prefix>/docker/library/hello-world', 'latest'), timeout_s=180)` / `_pull_with_retry(..., timeout_s=90)`: no image ever returned for a prefix with a valid rule (ValidatePullThroughCacheRule reported isValid=True).
Cause 2 — AssertionError: the destination repository ARN 'arn:aws:ecr:us-west-2:000000000000:repository/sim2real-ecr-repl-yes-cdfc2a3d28b6' does not name Region 'us-east-1'; and DescribeImageReplicationStatus never reaches {COMPLETE, FAILED} for the destination region within the check's 360 s poll.
Cause 3 — DescribeImageSigningStatus returns no settled signingStatuses entry within the check's 180 s poll; expected one entry with signingProfileArn = the configured profile, status FAILED and a non-empty failureReason naming the missing profile.
Cause 4 — AssertionError: expected deleted-secret rule to fail validation: {'ecrRepositoryPrefix': 'sim2realptcb-...', 'credentialArn': 'arn:aws:secretsmanager:...:secret:ecr-pullthroughcache/...', 'isValid': True, 'failure': ''}; same for a bogus-credential rule.
Cause 5 — PutImage returned HTTP 200 for a manifest list referencing child digests never pushed (expected ReferencedImagesNotFoundException); BatchDeleteImage returned imageIds=[{'imageDigest': 'sha256:3a43798e...'}] for a child referenced by a stored manifest list (expected it under failures, undeleted).
Cause 6 — AssertionError: expected a ClientError, but the call succeeded and returned {'repository': {...'repositoryName': 'sim2real-ecr-regpol-pre-394c3aa32e69'...}, 'ResponseMetadata': {'HTTPStatusCode': 200}}; expected AccessDeniedException for a role with no identity policy and no registry-policy grant.
Cause 7 — AssertionError: expected ScanNotFoundException, got NotImplemented (and ImageNotFoundException likewise unreachable).
Cause 8 — AssertionError: imageSizeInBytes is 423, expected layer+config = 299; KeyError: 'artifactMediaType' when reading the image detail of an OCI manifest.
Cause 9 — botocore ClientError (ValidationError) on CreateStack: Template format error: Unrecognized resource types: [AWS::ECR::RegistryScanningConfiguration].

## Steps to Reproduce
Cause 1: create_pull_through_cache_rule(ecrRepositoryPrefix=P, upstreamRegistryUrl='public.ecr.aws', upstreamRegistry='ecr-public'); poll batch_get_image(repositoryName=f'{P}/docker/library/hello-world', imageIds=[{'imageTag':'latest'}]).
Cause 2: put_replication_configuration to a second Region, push an image, then (a) describe_repositories through a client for the destination Region and inspect repositoryArn, (b) poll describe_image_replication_status for that Region.
Cause 3: put_signing_configuration with a rule whose repositoryFilters match the repository and a syntactically valid but nonexistent signer profile ARN; push an image; poll describe_image_signing_status.
Cause 4: create a pull-through cache rule with credentialArn pointing at a Secrets Manager secret, delete the secret (or use bogus credentials), then validate_pull_through_cache_rule(ecrRepositoryPrefix=P).
Cause 5: put_image with a manifest-list manifest whose child digests were never pushed; separately push a manifest list plus its children and call batch_delete_image on a child digest.
Cause 6: create an IAM role with no attached policies, assume it, and call create_repository with the assumed credentials while no registry policy grants ecr:CreateRepository.
Cause 7: push an image to a repository with scanOnPush=false and call describe_image_scan_findings on its digest.
Cause 8: push a single-layer image via InitiateLayerUpload/UploadLayerPart/CompleteLayerUpload/PutImage and compare describe_images imageSizeInBytes with layer+config sizes; push an OCI manifest and read artifactMediaType.
Cause 9: cloudformation.create_stack with a template declaring AWS::ECR::RegistryScanningConfiguration.
