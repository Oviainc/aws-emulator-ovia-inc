Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.server-side-field-level-encryption-enforced-on-request cloudfront.signed-cookies-custom-policy-admits-wildcard-and-expired-policy-refused
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.server-side-field-level-encryption-enforced-on-request|_post#9|Timeout|time.sleep(10)
Issue-Key: cloudfront.signed-cookies-custom-policy-admits-wildcard-and-expired-policy-refused|_poll_until#34|Timeout|time.sleep(interval_s)
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# cloudfront: the edge data plane serves nothing, so field-level encryption on the request path and signed-cookie admission/expiry can never be exercised

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
This issue bundles TWO root causes in the cloudfront service, both reached through the distribution's edge domain. BOTH must be closed before submitting.

Cause 1 of 2: a POST through a distribution whose cache behavior has a FieldLevelEncryptionId never reaches an origin, so the designated form field is never encrypted on the request path.
AWS: recorded behavior only for the data plane (CloudFront's edge is not described by the Smithy control-plane model; the control-plane side is Smithy: CreateDistribution -> POST /2020-05-31/distribution, 201, DefaultCacheBehavior.FieldLevelEncryptionId binds the config, and CreateFieldLevelEncryptionConfig/Profile declare which field names are encrypted with which public key). What the recording requires: the distribution's DomainName answers an HTTPS request; a form POST through it arrives at the origin with the field NOT designated for encryption passed through verbatim and the DESIGNATED field replaced by a long base64 ciphertext blob (over 100 characters), never the plaintext the client sent.
Emulator: no response is ever obtained through the distribution's edge domain — the check consumes its whole budget and is cut off at the 60 s cap without a single edge response, so neither the passthrough field nor the encrypted field can be observed. (The control-plane calls that precede it — CreatePublicKey, CreateFieldLevelEncryptionProfile, CreateFieldLevelEncryptionConfig, CreateDistribution — are what the check spends its budget on; the edge request after them never completes.)
Why: field-level encryption exists precisely so that sensitive form fields are unreadable at the origin. An emulator that stores the FLE config and serves nothing lets a client build and "verify" a configuration whose entire effect is absent; a client that trusts it ships plaintext PII to its origin in production.
Fix: the cloudfront crate's edge/data-plane request path — the distribution's DomainName must be resolvable and answer requests, route them to the configured origin per the matching cache behavior, and apply the behavior's FieldLevelEncryptionId (encrypting the configured ContentTypeProfile/QueryArgProfile field names with the profile's public key) before forwarding.
Traces: cloudfront.server-side-field-level-encryption-enforced-on-request.

Cause 2 of 2: a signed-cookie custom policy is never evaluated, because the same edge domain answers nothing.
AWS: recorded behavior only (data plane). What the recording requires: with a behavior carrying TrustedKeyGroups, a request bearing CloudFront-Policy/CloudFront-Signature/CloudFront-Key-Pair-Id cookies over a custom policy whose Resource is a wildcard is admitted for BOTH objects under that prefix with one cookie set; a request with no cookies is refused 403 with an XML error body; and a correctly signed but EXPIRED policy is refused 403 with a DIFFERENT error Code from the no-credential case (documented as "403 Access Denied", so `AccessDenied` against the no-cookie case's `MissingKey`).
Emulator: the check is cut off at the 60 s cap with no edge response at all, so neither the admission nor either refusal class is observable.
Why: signed cookies are an authorization boundary. A distribution that answers nothing cannot be distinguished from one that admits everything, and a client cannot confirm that expiry is enforced or that its wildcard policy covers the prefix it intends — the exact failure mode that silently publishes private content.
Fix: the same cloudfront edge/data-plane path as cause 1, plus signed-URL/signed-cookie verification on it: parse the CloudFront-Policy/Signature/Key-Pair-Id credentials, verify the signature against the behavior's TrustedKeyGroups public keys, evaluate the custom policy's Resource/DateLessThan/IpAddress conditions, and emit the 403 XML refusal with the distinct error Code for each refusal class.
Traces: cloudfront.signed-cookies-custom-policy-admits-wildcard-and-expired-policy-refused.

Related open issues describe the missing edge data plane for OTHER checks and do not carry these two: 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md, 20261024T182555Z-cloudfront-s3-distributions-over-s3-origins-serve-nothing.md and 20261024T102105Z-cloudfront-kms-s3-a-cloudfront-distribution-with-an-oac-over-an.md.

## Evidence
Cause 1 — cloudfront.server-side-field-level-encryption-enforced-on-request [mismatch]:
  Error: Check timed out after 60.0s
  The check creates a public key, an FLE profile and an FLE config, attaches the config to a distribution's cache behavior, waits for Deployed, then POSTs `secret_field=<value>&other_field=<value>` (content-type application/x-www-form-urlencoded) to the distribution domain and reads the origin's echo. Expected: HTTP 200 from the edge, the non-designated field echoed verbatim, and the designated field echoed as a base64 ciphertext longer than 100 characters. Actual: no edge response is ever produced; the check is killed at the 60 s cap.

Cause 2 — cloudfront.signed-cookies-custom-policy-admits-wildcard-and-expired-policy-refused [mismatch]:
  Error: Check timed out after 60.0s
  The check creates a public key and key group, a distribution with TrustedKeyGroups and the managed CachingDisabled policy, waits for Deployed, then issues raw HTTPS GETs with CloudFront-Policy/CloudFront-Signature/CloudFront-Key-Pair-Id cookies. Expected: HTTP 200 for the wildcard-policy cookie set on both objects under the prefix; HTTP 403 with a non-empty XML error Code for the no-cookie case (`MissingKey`) and a DIFFERENT non-empty Code for the correctly-signed-but-expired policy. Actual: no edge response at all; the check is killed at the 60 s cap.

In both cases the failure is the absence of any response from the distribution's edge domain, not a wrong value in one. Hosts, generated domain names and ids differ per run and are not requirements.

## Steps to Reproduce
Cause 1:
1. CreatePublicKey, CreateFieldLevelEncryptionProfile naming one field (e.g. secret_field) against that key, CreateFieldLevelEncryptionConfig referencing the profile for the form content type.
2. CreateDistribution with a custom origin that echoes the request body, and DefaultCacheBehavior.FieldLevelEncryptionId = <the config id>; poll GetDistribution until Status == Deployed.
3. POST `secret_field=<plaintext>&other_field=<plaintext>` with content-type application/x-www-form-urlencoded to https://<DomainName>/post.
4. Expect HTTP 200, other_field echoed verbatim, secret_field echoed as base64 ciphertext (>100 chars). Observe: no response from the edge domain; the request never completes.

Cause 2:
1. CreatePublicKey + CreateKeyGroup, then CreateDistribution whose behavior sets TrustedKeyGroups to that group and CachePolicyId to the managed CachingDisabled policy; wait for Deployed.
2. GET https://<DomainName>/<object> with no CloudFront-* cookies -> expect HTTP 403 with an XML error Code (MissingKey).
3. GET two objects under one prefix with cookies signed over a custom policy whose Resource is https://<DomainName>/<prefix>/* -> expect HTTP 200 for both.
4. GET with cookies signed over a policy whose DateLessThan is in the past -> expect HTTP 403 with a non-empty Code DIFFERENT from step 2's.
5. Observe: none of steps 2-4 produces any response.

Re-replay cloudfront.server-side-field-level-encryption-enforced-on-request and cloudfront.signed-cookies-custom-policy-admits-wildcard-and-expired-policy-refused to confirm.
