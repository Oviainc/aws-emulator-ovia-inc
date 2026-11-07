Issue-Source: trace-sweep-divergence
Issue-Trace: ecr-ecracc-pull-through-cache-for-the-service-image-registry
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecr-ecracc-pull-through-cache-for-the-service-image-registry
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|CreatePullThroughCacheRule|4xx->4xx|$.message
Issue-Key: ecr|CreatePullThroughCacheRule|4xx->2xx|$.__type
Issue-Key: ecr|CreatePullThroughCacheRule|4xx->2xx|$.message
Issue-Key: ecr|CreatePullThroughCacheRule|4xx->2xx|status
Issue-Key: ecr|CreatePullThroughCacheRule|2xx->4xx|$.createdAt
Issue-Key: ecr|CreatePullThroughCacheRule|2xx->4xx|$.ecrRepositoryPrefix
Issue-Key: ecr|CreatePullThroughCacheRule|2xx->4xx|$.registryId
Issue-Key: ecr|CreatePullThroughCacheRule|2xx->4xx|$.upstreamRegistryUrl
Issue-Key: ecr|CreatePullThroughCacheRule|2xx->4xx|status
Issue-Key: ecr|DescribePullThroughCacheRules|2xx->2xx|$.pullThroughCacheRules[*].upstreamRegistryUrl
Issue-Commit: ae442358c61414b2bbb5a7e8dd80b7eec77accf8

# ECR pull-through cache rule upstream-registry validation and storage diverge

> _Note: this issue was identified against the code base at commit `ae442358c61414b2bbb5a7e8dd80b7eec77accf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePullThroughCacheRule

## Description
4 causes in the pull-through cache rule plane: the emulator refuses a URL AWS accepts, accepts a rule AWS refuses, words its unsupported-upstream refusal differently, and does not keep the rule's upstreamRegistryUrl. Related but distinct from the open issue 'ECR: a pull-through cache rule never mirrors upstream' (id 20261107T192445Z-ecr-a-pull-through-cache-rule-never-mirrors.md), which is about pulling THROUGH an existing rule; these four are about creating and reading the rule itself and are not covered by it.

Cause 1 of 4: the unsupported-upstream-URL refusal is worded by the emulator, not by AWS.
AWS: Smithy: CreatePullThroughCacheRule.upstreamRegistryUrl -> Url (required), errors include UnsupportedUpstreamRegistryException ("The specified upstream registry isn't supported", httpError 400) and InvalidParameterException (400); supported upstreams are the UpstreamRegistry enum [ecr, ecr-public, quay, k8s, docker-hub, github-container-registry, azure-container-registry, gitlab-container-registry, chainguard]. A URL belonging to none of them is refused 400 with message, fixed text exactly: "The upstream registry URL <url> is invalid. Specify a valid upstream registry URL and try again". The only variable part is the URL, echoed from the request; there is no trailing period.
Emulator: answers 400 but with its own sentence "upstreamRegistryUrl '<url>' names no supported upstream registry; specify upstreamRegistry" — different fixed text, quotes around the URL, and advice AWS does not give.
Why: a caller or test that matches AWS's refusal text (or shows it to a user) does not recognise the emulator's; the advice to 'specify upstreamRegistry' points at a parameter that would not make the URL supported.
Fix: CreatePullThroughCacheRule's upstream-URL validation message.
Traces: ecr-ecracc-pull-through-cache-for-the-service-image-registry#task#3 (same cluster also covers task#5).

Cause 2 of 4: an upstream registry that requires authentication is accepted with no credentialArn.
AWS: Smithy: CreatePullThroughCacheRule.credentialArn -> CredentialArn @pattern ^arn:aws(-\\w+)*:secretsmanager:...:secret:ecr-pullthroughcache/..., error UnsupportedUpstreamRegistryException httpError 400. When the named upstream requires credentials and no credentialArn is supplied, AWS refuses with 400 UnsupportedUpstreamRegistryException and message, fixed text exactly: "The specified upstream registry requires authentication. Specify a valid Secrets Manager ARN containing the upstream registry credentials and try again." (no variable parts).
Emulator: answers HTTP 200 and creates the rule — no __type and no message at all.
Why: the rule is created in a state AWS never allows, so every later pull through it fails for a reason the caller was never told about at create time, and a negative test for the credential requirement passes when it should fail.
Fix: CreatePullThroughCacheRule's handler/validation — require credentialArn for the upstreams that need it before mutating state.
Traces: ecr-ecracc-pull-through-cache-for-the-service-image-registry#task#4.

Cause 3 of 4: a rule for the public ECR upstream is refused instead of created.
AWS: Smithy: CreatePullThroughCacheRule returns ecrRepositoryPrefix, upstreamRegistryUrl, registryId, createdAt (CreationTimestamp), upstreamRegistry; the UpstreamRegistry enum includes ecr-public, whose registry URL is public.ecr.aws, and no credentialArn is required for it. The call succeeds with HTTP 200 and echoes the prefix from the request, the upstream URL from the request, the caller's registry id and the creation timestamp.
Emulator: answers HTTP 400 — the response carries none of createdAt / ecrRepositoryPrefix / registryId / upstreamRegistryUrl.
Why: the most common pull-through configuration (public.ecr.aws) cannot be created at all, so any workflow that caches public ECR images is blocked at setup.
Fix: CreatePullThroughCacheRule's upstream-registry resolution — recognise the public-ECR upstream URL as supported and create the rule.
Traces: ecr-ecracc-pull-through-cache-for-the-service-image-registry#task#7.

Cause 4 of 4: DescribePullThroughCacheRules does not return the rule's own upstreamRegistryUrl.
AWS: Smithy: DescribePullThroughCacheRules.pullThroughCacheRules -> PullThroughCacheRuleList, each carrying upstreamRegistryUrl (Url). The value read back is the URL the rule was created with, verbatim.
Emulator: reports a different upstream URL than the rule was created with — a docker-hub registry URL in place of the created one.
Why: a caller reconciling its rules (or deciding whether a rule already points at the wanted upstream) sees an upstream it never configured, and an idempotent create/describe loop will keep trying to fix a rule that is already correct. Hypothesis: the describe path substitutes a default upstream rather than reading the stored field, which is why a non-docker-hub upstream comes back as docker-hub.
Fix: the pull-through cache rule state/serializer for DescribePullThroughCacheRules — return the stored upstreamRegistryUrl instead of a default.
Traces: ecr-ecracc-pull-through-cache-for-the-service-image-registry#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture ecr-ecracc-pull-through-cache-for-the-service-image-registry.

Cause 1 — task#3 (cluster also covers task#5) CreatePullThroughCacheRule, 400 -> 400:
  $.message expected 'The upstream registry URL mcr.microsoft.com is invalid. Specify a valid upstream registry URL and try again'
  $.message actual   "upstreamRegistryUrl 'mcr.microsoft.com' names no supported upstream registry; specify upstreamRegistry"
  (mcr.microsoft.com is the URL from the request and fills the template.)

Cause 2 — task#4 CreatePullThroughCacheRule: status expected 400, got 200
  $.__type expected 'UnsupportedUpstreamRegistryException', got None
  $.message expected 'The specified upstream registry requires authentication. Specify a valid Secrets Manager ARN containing the upstream registry credentials and try again.', got None

Cause 3 — task#7 CreatePullThroughCacheRule: status expected 200, got 400
  $.ecrRepositoryPrefix expected 'sim2real-probe-ptc-e5a07c3b', got None
  $.upstreamRegistryUrl expected 'public.ecr.aws', got None
  $.registryId expected '000000000000', got None
  $.createdAt expected a creation timestamp, got None
  (the prefix, the registry id and the timestamp are per-environment values; the 200 with the echoed rule is the requirement.)

Cause 4 — task#8 DescribePullThroughCacheRules, 200 -> 200:
  $.pullThroughCacheRules[0].upstreamRegistryUrl expected 'public.ecr.aws' (the URL the rule was created with), got 'registry-1.docker.io'

## Steps to Reproduce
1. Replay the wire corpus against the emulator (trace set simparity-wire-aws-v1), fixture ecr-ecracc-pull-through-cache-for-the-service-image-registry.
2. Cause 1: CreatePullThroughCacheRule(ecrRepositoryPrefix=<prefix>, upstreamRegistryUrl="mcr.microsoft.com") — a URL matching no UpstreamRegistry value — and compare $.message with AWS's 'The upstream registry URL <url> is invalid. Specify a valid upstream registry URL and try again'.
3. Cause 2: CreatePullThroughCacheRule for an upstream that requires authentication with NO credentialArn; observe 200 and a created rule where AWS answers 400 UnsupportedUpstreamRegistryException with the 'requires authentication' message.
4. Cause 3: CreatePullThroughCacheRule(ecrRepositoryPrefix=<prefix>, upstreamRegistryUrl="public.ecr.aws"); observe 400 where AWS answers 200 echoing ecrRepositoryPrefix/upstreamRegistryUrl/registryId/createdAt.
5. Cause 4: after creating a rule for public.ecr.aws, call DescribePullThroughCacheRules and compare pullThroughCacheRules[0].upstreamRegistryUrl with the URL supplied at create time.
