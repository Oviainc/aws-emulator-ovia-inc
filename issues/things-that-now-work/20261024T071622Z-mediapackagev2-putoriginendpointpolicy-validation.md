Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# PutOriginEndpointPolicy stores CdnAuthConfiguration secret and role ARNs unvalidated

_Filed: 2026-10-24T07:16:22Z_
_Operation: PutOriginEndpointPolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler validates the policy document (INVALID_POLICY) and then assigns the submitted CdnAuthConfiguration verbatim onto the endpoint. Nothing about the CDN-authorization secrets is checked, although the model dedicates a whole block of ValidationExceptionType values to exactly this input: MALFORMED_SECRET_ARN, SECRET_ARN_RESOURCE_NOT_FOUND, SECRET_FROM_DIFFERENT_ACCOUNT, SECRET_FROM_DIFFERENT_REGION, DUPLICATED_SECRET, TOO_MANY_SECRETS, INVALID_SECRET / INVALID_SECRET_FORMAT, and INVALID_ROLE_ARN / ROLE_ARN_INVALID_FORMAT / ROLE_ARN_NOT_ASSUMABLE for SecretsRoleArn.

Expected: CdnIdentifierSecretArns entries that are not well-formed Secrets Manager ARNs (or that repeat, or that name another region/account) and a SecretsRoleArn that is not a well-formed IAM role ARN are refused with 400 ValidationException carrying the matching discriminator, and the endpoint's existing policy is untouched. Actual: 200, and GetOriginEndpointPolicy reports the malformed configuration back, so CDN authorization appears configured when it could never work. (Note the same class of gap was filed and accepted for mediapackage-vod UpdatePackagingGroup's CDN authorization ARNs.)

## Evidence

Spec: CdnAuthConfiguration{CdnIdentifierSecretArns (required, @length min 1), SecretsRoleArn (required, @length 20..2048)}; PutOriginEndpointPolicy declares ValidationException (400); ValidationExceptionType includes MALFORMED_SECRET_ARN, DUPLICATED_SECRET, SECRET_FROM_DIFFERENT_REGION, SECRET_FROM_DIFFERENT_ACCOUNT, TOO_MANY_SECRETS, INVALID_ROLE_ARN, ROLE_ARN_INVALID_FORMAT, ROLE_ARN_NOT_ASSUMABLE.
Handler: src/ops/put_origin_endpoint_policy.rs lines 238-241 — `endpoint.policy = Some(EndpointPolicy { policy: input.policy, cdn_auth_configuration: input.cdn_auth_configuration })`; the only validation above it is crate::state::is_policy_document (line 189). The comment there says the content is "bounded by the model and refused at the request boundary", i.e. only @length is applied — no ARN shape, duplicate or region check.
Tests: tests/all/origin_endpoint_policy.rs exercises the round-trip and the secret-count bound with well-formed ARNs only (lines 282-999); no test asserts a malformed-ARN refusal.

## Steps to Reproduce

1. Create a channel group, channel and origin endpoint.
2. PutOriginEndpointPolicy with a valid Policy and CdnAuthConfiguration = {CdnIdentifierSecretArns: ['not-an-arn', 'not-an-arn'], SecretsRoleArn: 'arn:aws:iam::123456789012:user/not-a-role-but-long-enough'}.
3. Observe 200; AWS answers 400 ValidationException (MALFORMED_SECRET_ARN / DUPLICATED_SECRET / INVALID_ROLE_ARN).
4. GetOriginEndpointPolicy and observe the malformed CdnAuthConfiguration echoed back as the endpoint's live CDN authorization.
