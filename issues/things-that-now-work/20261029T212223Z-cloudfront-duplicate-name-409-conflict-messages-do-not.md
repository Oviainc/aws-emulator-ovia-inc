Issue-Source: trace-sweep-divergence
Issue-Trace: cfetag-connection-group-etag-chain
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfetag-connection-group-etag-chain cloudfront-connection-group-errors cloudfront-connection-group-lifecycle cfetag-key-group-etag-chain cloudfront-key-group-error-paths cfetag-origin-access-control-etag-chain cloudfront-origin-access-control-lifecycle cfetag-response-headers-policy-etag-chain cloudfront-response-headers-policy-duplicate-name-error cloudfront-response-headers-policy-error-paths cloudfront-response-headers-policy-errors cloudfront-response-headers-policy-lifecycle cloudfront-cache-policy-duplicate-name-error cloudfront-cache-policy-error-paths cloudfront-cache-policy-errors cloudfront-cache-policy-full-lifecycle cloudfront-cache-policy-lifecycle cloudfront-connection-function-errors cloudfront-connection-function-lifecycle cloudfront-create-origin-request-policy-duplicate-name-error cloudfront-origin-request-policy-error-paths cloudfront-origin-request-policy-errors cloudfront-origin-request-policy-lifecycle cloudfront-distribution-tenant-error-paths cloudfront-distribution-tenant-errors-and-filters
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateConnectionGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateKeyGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateOriginAccessControl|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateResponseHeadersPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateCachePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateConnectionFunction|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateOriginRequestPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateDistributionTenant|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront duplicate-name 409 conflict messages do not use AWS wording

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnectionGroup

## Description
Eight causes, one shared fault: the emulator detects the duplicate-name conflict and returns the correct HTTP status and error code, but its `Error.Message` is its own invented prose instead of the fixed sentence AWS returns for that error class. AWS's message for each of these conflicts is FIXED TEXT with NO variable part - it names neither the resource name nor the generated id. The emulator instead interpolates the request's name and/or the colliding id and appends remediation prose, so no two of them match AWS.

Fix for every cause: the error-construction site for the *AlreadyExists family in each operation's handler/validation must emit AWS's fixed sentence and must NOT interpolate the requested name or the existing resource id.

Cause 1 of 8: CreateConnectionGroup duplicate name.
AWS: 409 with Error.Code `EntityAlreadyExists` and the fixed message `A connection group with the same name already exists in this AWS account.` (recorded behavior; Smithy models EntityAlreadyExists -> 409 on CreateConnectionGroup). No variable part.
Emulator: 409, but message `A connection group with the name <Name> already exists in this account` - the requested name is interpolated and the sentence differs.
Why: a client matching on the documented message string (or asserting it in a test) sees an unrecognised message, and the name leak makes the message non-deterministic across accounts.
Fix: CreateConnectionGroup's uniqueness check in the connection-group handler.
Traces: cfetag-connection-group-etag-chain#task#2 (also cloudfront-connection-group-errors#task#3, cloudfront-connection-group-lifecycle#task#5).

Cause 2 of 8: CreateKeyGroup duplicate name.
AWS: 409 `KeyGroupAlreadyExists`, fixed message `Another key group with the same name already exists within the aws account.`
Emulator: 409 with `A key group with this name already exists. You must provide a unique name. To modify an existing key group, use UpdateKeyGroup.` - entirely different sentence plus remediation prose AWS does not send.
Why: same as cause 1.
Fix: CreateKeyGroup's uniqueness check.
Traces: cfetag-key-group-etag-chain#task#3 (also cloudfront-key-group-error-paths#task#4).

Cause 3 of 8: CreateOriginAccessControl duplicate name.
AWS: 409 `OriginAccessControlAlreadyExists`, fixed message `An origin access control with the same name already exists.` (Smithy: CreateOriginAccessControl errors include OriginAccessControlAlreadyExists -> 409).
Emulator: 409 with `An origin access control with the specified parameters already exists: <Id>` - different sentence and it leaks the existing id.
Why: the id is a per-run generated value, so the message is not reproducible; clients keying on AWS's sentence miss it.
Fix: CreateOriginAccessControl's uniqueness check.
Traces: cfetag-origin-access-control-etag-chain#task#2 (also cloudfront-origin-access-control-lifecycle#task#3).

Cause 4 of 8: CreateResponseHeadersPolicy duplicate name.
AWS: 409 `ResponseHeadersPolicyAlreadyExists`, fixed message `Another response headers policy with the same name already exists within the aws account.`
Emulator: 409 with `A response headers policy with the name <Name> already exists in this account. You must provide a unique name. To modify an existing response headers policy, use UpdateResponseHeadersPolicy.`
Why: same as cause 1.
Fix: CreateResponseHeadersPolicy's uniqueness check.
Traces: cfetag-response-headers-policy-etag-chain#task#2 (also cloudfront-response-headers-policy-duplicate-name-error#task#2, cloudfront-response-headers-policy-error-paths#task#3, cloudfront-response-headers-policy-errors#task#2, cloudfront-response-headers-policy-lifecycle#task#2).

Cause 5 of 8: CreateCachePolicy duplicate name.
AWS: 409 `CachePolicyAlreadyExists` (Smithy: CreateCachePolicy errors include CachePolicyAlreadyExists -> 409), fixed message `Another cache policy with the same name already exists within the aws account.`
Emulator: 409 with `A cache policy with the name <Name> already exists in this account. You must provide a unique name. To modify an existing cache policy, use UpdateCachePolicy.`
Why: same as cause 1.
Fix: CreateCachePolicy's uniqueness check.
Traces: cloudfront-cache-policy-duplicate-name-error#task#2 (also cloudfront-cache-policy-error-paths#task#3, cloudfront-cache-policy-errors#task#2, cloudfront-cache-policy-full-lifecycle#task#2, cloudfront-cache-policy-lifecycle#task#2).

Cause 6 of 8: CreateConnectionFunction duplicate name.
AWS: 409 `EntityAlreadyExists`, fixed message `A connection function with the same name already exists in this AWS account.`
Emulator: 409 with `The entity already exists. You must provide a unique entity: a connection function with the name <Name> already exists in this Amazon Web Services account` - a generic prefix plus the interpolated name.
Why: same as cause 1.
Fix: CreateConnectionFunction's uniqueness check.
Traces: cloudfront-connection-function-errors#task#4 (also cloudfront-connection-function-lifecycle#task#5).

Cause 7 of 8: CreateOriginRequestPolicy duplicate name.
AWS: 409 `OriginRequestPolicyAlreadyExists`, fixed message `Another origin request policy with the same name already exists within the aws account.`
Emulator: 409 with `An origin request policy with the name <Name> already exists in this account. You must provide a unique name. To modify an existing origin request policy, use UpdateOriginRequestPolicy.`
Why: same as cause 1.
Fix: CreateOriginRequestPolicy's uniqueness check.
Traces: cloudfront-create-origin-request-policy-duplicate-name-error#task#2 (also cloudfront-origin-request-policy-error-paths#task#3, cloudfront-origin-request-policy-errors#task#2, cloudfront-origin-request-policy-lifecycle#task#2).

Cause 8 of 8: CreateDistributionTenant duplicate name.
AWS: 409 `EntityAlreadyExists`, fixed message `A distribution tenant with the same name already exists in this AWS account.`
Emulator: 409 with `A distribution tenant with the name <Name> already exists in this account`.
Why: same as cause 1.
Fix: CreateDistributionTenant's uniqueness check.
Traces: cloudfront-distribution-tenant-error-paths#task#3 (also cloudfront-distribution-tenant-errors-and-filters#task#4).

Related open issue (different causes, does not cover these): 20261029T175823Z-cloudfront-10-refusal-fidelity-divergences-duplicate-name-precondition.md.

## Evidence
All eight clusters match on status and code and diverge only on $.ErrorResponse.Error.Message.

1. CreateConnectionGroup 409->409: expected 'A connection group with the same name already exists in this AWS account.', got 'A connection group with the name sim2real-probe-cfetag-cg-e721327c already exists in this account'.
2. CreateKeyGroup 409->409: expected 'Another key group with the same name already exists within the aws account.', got 'A key group with this name already exists. You must provide a unique name. To modify an existing key group, use UpdateKeyGroup.'
3. CreateOriginAccessControl 409->409: expected 'An origin access control with the same name already exists.', got 'An origin access control with the specified parameters already exists: E7IYCGPKTANSF3'.
4. CreateResponseHeadersPolicy 409->409: expected 'Another response headers policy with the same name already exists within the aws account.', got 'A response headers policy with the name sim2real-probe-cfetag-rhp-e3001efc already exists in this account. You must provide a unique name. ...'
5. CreateCachePolicy 409->409: expected 'Another cache policy with the same name already exists within the aws account.', got 'A cache policy with the name sim2real-probe-cloudfront-cachepolicydup-c509aa11 already exists in this account. ...'
6. CreateConnectionFunction 409->409: expected 'A connection function with the same name already exists in this AWS account.', got 'The entity already exists. You must provide a unique entity: a connection function with the name ... already exists in this Amazon Web Services account'.
7. CreateOriginRequestPolicy 409->409: expected 'Another origin request policy with the same name already exists within the aws account.', got 'An origin request policy with the name ... already exists in this account. ...'
8. CreateDistributionTenant 409->409: expected 'A distribution tenant with the same name already exists in this AWS account.', got 'A distribution tenant with the name ... already exists in this account'.

In every case the names/ids quoted above are per-run probe values filling AWS's template; the requirement is AWS's fixed sentence, which contains no name or id at all.

## Steps to Reproduce
1. Replay the wire corpus (replay_wire_corpus) against the emulator.
2. For each cause, create the resource once, then issue the same Create* call again with the same Name (or, for cause 3, the same OriginAccessControlConfig).
3. Observe the 409 body: Error.Code is right, Error.Message is the emulator's own prose with the probe name/id interpolated, not AWS's fixed sentence.
4. Representative step: fixture cfetag-connection-group-etag-chain, step task#2 (CreateConnectionGroup).
