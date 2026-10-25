Issue-Source: trace-sweep-divergence
Issue-Trace: api-pull-through-cache-rule-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-pull-through-cache-rule-lifecycle
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|ValidatePullThroughCacheRule|2xx->2xx|$.failure
Issue-Key: ecr|ValidatePullThroughCacheRule|2xx->2xx|$.isValid
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# ECR ValidatePullThroughCacheRule marks a complete rule invalid, citing its own lack of egress

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ValidatePullThroughCacheRule

## Description
Cause 1 of 1: ValidatePullThroughCacheRule reports a freshly created, well-formed rule as invalid and returns a failure string that describes the emulator rather than the rule.
AWS: Smithy ecr#ValidatePullThroughCacheRule returns isValid (ecr#IsPTCRuleValid, boolean, default false) and failure (ecr#PTCValidateFailure). The recording shows a rule created against an upstream that needs no credentials validating with HTTP 200, isValid=true and an empty failure. Validation is a verdict on the STORED rule (prefix, upstream URL, and the resolvability of credentialArn/customRoleArn when the upstream needs authentication), not a live reachability probe the caller can be blamed for.
Emulator: returns HTTP 200 with isValid=false and failure set to its own prose: "the stored configuration of the pull through cache rule for '<prefix>' is complete, but authentication to the upstream registry could not be verified: this emulator makes no outbound connection to 'public.ecr.aws'". It concedes the stored configuration is complete and then still votes invalid, on the strength of an egress limitation of the emulator itself.
Why: isValid is the one signal a client has for "is this rule usable". Always-false means an operator who just created a correct rule is told it is broken, CI gates that assert isValid fail permanently, and a genuinely broken rule (missing credential, unresolvable secret) is indistinguishable from a healthy one because both report the same verdict and the same emulator-authored failure text. The failure is also not a code a client can branch on.
Fix: the ValidatePullThroughCacheRule handler must compute the verdict from stored rule state alone — isValid=true with no failure when the rule is complete and its credentials (if required) resolve — and must never report its own lack of outbound connectivity as the rule's validity. Related but not covering: open issue 20261022T234922Z-ecr-completelayerupload-is-a-501-stub-cfn-provisioned.md, Cause 4, describes this behaviour from the behavioral suite but carries none of this fixture's replay keys, so this wire step is not gated by it.
Traces: api-pull-through-cache-rule-lifecycle#task#4.

## Evidence
Fixture api-pull-through-cache-rule-lifecycle, step task#4 (ValidatePullThroughCacheRule), status 200 on both sides:
- $.isValid: expected True, got False.
- $.failure: expected '' (empty), got "the stored configuration of the pull through cache rule for 'sim2real-probe-ecr-ptc-8b27664' is complete, but authentication to the upstream registry could not be verified: this emulator makes no outbound connection to 'public.ecr.aws'".
The rule under validation had just been created successfully in an earlier step of the same fixture against upstream public.ecr.aws, which requires no credentials.

## Steps to Reproduce
Replay the wire corpus and re-run api-pull-through-cache-rule-lifecycle#task#4. Manually:
1. CreatePullThroughCacheRule(ecrRepositoryPrefix=P, upstreamRegistryUrl='public.ecr.aws') -> expect 200.
2. ValidatePullThroughCacheRule(ecrRepositoryPrefix=P) -> assert isValid is True and failure is empty.
3. For contrast, point a rule's credentialArn at a secret that does not exist and validate it -> assert isValid is False with a failure that names the credential problem, not the emulator's egress.
