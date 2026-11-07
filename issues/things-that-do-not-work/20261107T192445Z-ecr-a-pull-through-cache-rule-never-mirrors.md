Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.e2e-pull-through-cache-full-flow
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.e2e-pull-through-cache-full-flow|_pull_with_retry#9|AssertionError|raise AssertionError(f"pull-through-cache image never became available: {last!r}")
Issue-Commit: ab92470025dd8eeb501814706ada4aa2b617e26c

# ECR: a pull-through cache rule never mirrors upstream — BatchGetImage on the cached repository is refused RepositoryNotFoundException instead of importing the image (1 cause)

> _Note: this issue was identified against the code base at commit `ab92470025dd8eeb501814706ada4aa2b617e26c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetImage

## Description
ONE net-new root cause. Related open issue that does NOT carry this replay key (the suite report does not mark this check OWNED; that issue recorded the check as a harness TIMEOUT inside the pull poll, while the emulator now answers a refusal instead of hanging): 20261024T014332Z-ecr-asynchronous-effects-never-occur-and-several-enforcement.md.

Cause 1 of 1: with a valid pull-through cache rule in place, the first read of a cached repository is refused as not-found instead of triggering the on-demand upstream import.
AWS: Smithy: BatchGetImage takes a required repositoryName (pattern allows the slash-separated cached path) and declares RepositoryNotFoundException and UnableToGetUpstreamImageException (HTTP 400 each); CreatePullThroughCacheRule registers the prefix for an upstream registry (here upstreamRegistryUrl 'public.ecr.aws', upstreamRegistry 'ecr-public'). Recorded behavior: a BatchGetImage against '<prefix>/docker/library/hello-world' with imageIds=[{imageTag:'latest'}] makes ECR create the mirrored repository on demand and return exactly one image whose imageId.imageTag is 'latest' and whose imageManifest is non-empty. The repository exists from then on; if the upstream cannot be fetched, AWS reports UnableToGetUpstreamImageException — never a bare RepositoryNotFoundException for a path covered by a registered rule.
Emulator: ValidatePullThroughCacheRule reports isValid=true for the freshly created rule, but every BatchGetImage on the cached path is refused HTTP 400 RepositoryNotFoundException with message "The repository with name '<prefix>/docker/library/hello-world' does not exist in the registry with id '<account>'" (fixed text with the repository name from the request and the registry id from state) for the whole retry window, so the check fails with "pull-through-cache image never became available". No repository is ever created for the prefix and no upstream fetch is attempted.
Why: the entire pull-through-cache feature is unusable while reporting itself healthy — a client creates a rule, validation says it is good, and every pull fails with an error that says the repository does not exist, pointing the user at a non-existent naming mistake rather than at an unimplemented mirror. Anything built on a mirrored image (scanning, retagging, an ECS/EKS task pulling through the cache) cannot proceed, and the modeled upstream-failure class is never used.
Fix: the on-demand upstream import behind BatchGetImage (and BatchImportUpstreamImage) for repository names matching a registered pull-through cache rule prefix, including implicit creation of the mirrored repository and the UnableToGetUpstreamImageException path when the upstream fetch fails.
Traces: ecr.e2e-pull-through-cache-full-flow.

## Evidence
[mismatch] ecr.e2e-pull-through-cache-full-flow — AssertionError: pull-through-cache image never became available: RepositoryNotFoundException("An error occurred (RepositoryNotFoundException) when calling the BatchGetImage operation: The repository with name 'sim2realptc-<id>/docker/library/hello-world' does not exist in the registry with id '<account>'"). The rule had just been created for the prefix and ValidatePullThroughCacheRule returned isValid=true. Expected: images[0].imageId.imageTag == 'latest' with a non-empty imageManifest.

## Steps to Reproduce
ecr create_pull_through_cache_rule(ecrRepositoryPrefix=P, upstreamRegistryUrl='public.ecr.aws', upstreamRegistry='ecr-public'); validate_pull_through_cache_rule(ecrRepositoryPrefix=P) -> isValid true; then batch_get_image(repositoryName=f'{P}/docker/library/hello-world', imageIds=[{'imageTag':'latest'}]) with retries — every attempt is refused RepositoryNotFoundException and no repository for the prefix is ever created.
