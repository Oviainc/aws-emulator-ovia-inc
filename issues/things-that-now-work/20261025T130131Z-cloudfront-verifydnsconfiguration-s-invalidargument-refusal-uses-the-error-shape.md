Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.verify-dns-configuration-malformed-domain-invalid-argument-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.verify-dns-configuration-malformed-domain-invalid-argument-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 58bb21045c140f54eac821540018fc7201e16d37

# VerifyDnsConfiguration's InvalidArgument refusal uses the error shape's documentation plus an emulator-authored reason instead of CloudFront's "The specified domain name isn't associated with the distribution tenant." sentence

> _Note: this issue was identified against the code base at commit `58bb21045c140f54eac821540018fc7201e16d37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
VerifyDnsConfiguration

## Description
REOPENS 20261024T182555Z-cloudfront-verifydnsconfiguration-stub.md: its fix left the call refused. That issue (now marked fixed) replaced the 501 scaffold with a real handler, and the two units it named now pass; a third unit still fails on a `VerifyDnsConfiguration` refusal, because the handler the fix introduced invents its own `InvalidArgument` message instead of CloudFront's.

This is 1 root cause.

Cause 1 of 1: the `InvalidArgument` message for a `Domain` the distribution tenant does not hold is emulator-authored, and the refusal predicate is narrower than AWS's.

AWS: Smithy (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json) models `VerifyDnsConfiguration` with input members `Identifier` (required) and `Domain` ("The domain name that you're verifying"), output `DnsConfigurationList`, and errors `AccessDenied` (403), `EntityNotFound` (404) and `InvalidArgument` (400). The model does not state the message text — recorded behavior only: for a `Domain` argument that is not one of the named tenant's domains, CloudFront answers HTTP 400 with error code `InvalidArgument` and the message

    The specified domain name isn't associated with the distribution tenant. Specify another domain name and try again.

That message is entirely FIXED text — it has no variable parts, it interpolates neither the offending domain nor the tenant id. The message also states the rule AWS applies: the refusal class is "the `Domain` argument is not associated with this distribution tenant", which a syntactically malformed domain satisfies trivially (no tenant can hold it). A `Domain` the tenant does hold is accepted and reported in `DnsConfigurationList`.

Emulator: for the same call, `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/verify_dns_configuration.rs` answers error code `InvalidArgument` (correct) with the message `An argument is invalid: <reason>`, where the leading clause is the `InvalidArgument` shape's one-line DOCUMENTATION ("An argument is invalid.") and `<reason>` is a locally-worded diagnosis produced by `crate::state::domain_name_refusal`. The replay recorded `'An argument is invalid: the domain not a domain !! .. is a single label, not a domain name'`. Separately, the refusal is decided only by the shape of the string: a syntactically well-formed domain that the tenant does NOT hold is not refused at all — it falls through to the comparison and is reported as a `DnsConfiguration` entry, where AWS refuses the whole call with the sentence above.

Why: a client that classifies this refusal by message — a troubleshooting tool or a test that asserts the documented sentence, which is the only signal distinguishing "you asked about a domain this tenant doesn't own" from the other `InvalidArgument` paths — sees text no AWS region ever returns, so the branch never matches. Worse, because the predicate is shape-only, a caller that passes a valid domain belonging to a DIFFERENT tenant gets a 200 with a `DnsConfiguration` status instead of the 400 refusal, so the caller concludes the domain is attached to the tenant it asked about and skips the association step.

Fix: the operation's handler and its refusal construction — the `invalid_argument()` helper and the pre-lock `Domain` check in `src/ops/verify_dns_configuration.rs` must emit CloudFront's fixed sentence verbatim rather than `format!("An argument is invalid: {reason}")`, and the refusal must be raised for any `Domain` the resolved tenant's stored `domains` list does not contain (which subsumes the malformed-domain case), not only for a string that fails the domain-shape test. No other open issue carries this replay key: the suite marked the neighbouring cloudfront checks OWNED by 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md and 20261025T054735Z-cloudfront-4-net-new-fidelity-divergences-managed-policy.md, and this check is in neither.

Traces: cloudfront.verify-dns-configuration-malformed-domain-invalid-argument-rejected.

## Evidence
Failing check `cloudfront.verify-dns-configuration-malformed-domain-invalid-argument-rejected` [mismatch]:

    AssertionError: expected message "The specified domain name isn't associated with the distribution tenant. Specify another domain name and try again.", got 'An argument is invalid: the domain not a domain !! .. is a single label, not a domain name'

Expected (AWS): HTTP 400, error code `InvalidArgument`, message `The specified domain name isn't associated with the distribution tenant. Specify another domain name and try again.` (fixed text, no interpolated values).
Actual (emulator): HTTP 400, error code `InvalidArgument` (correct), message `An argument is invalid: <emulator-authored reason>` — the error shape's documentation sentence concatenated with a locally-worded diagnosis.

Source of the divergent text: `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/verify_dns_configuration.rs` lines 128-136 (`fn invalid_argument(reason: String)` → `message: Some(format!("An argument is invalid: {reason}"))`), raised from the pre-lock `Domain` check at lines 234-238 via `crate::state::domain_name_refusal`.

The check's positive control (the tenant's own, well-formed domain returning exactly one `DnsConfiguration` whose `Status != "valid-configuration"`) was never reached, because the run stopped on the message assertion.

## Steps to Reproduce
1. Boot the cloudfront emulator.
2. CreateDistributionWithTags for a `tenant-only` (ConnectionMode) distribution with one custom origin and the AWS-managed CachingDisabled cache policy.
3. CreateDistributionTenant on that distribution with a single well-formed domain and `ManagedCertificateRequest={"ValidationTokenHost": "self-hosted"}`; keep the returned tenant `Id`.
4. Call VerifyDnsConfiguration with `Identifier=<tenant id>` and a `Domain` the tenant does not hold — e.g. the syntactically invalid `"not a domain !! .."`.
5. Observe HTTP 400 `InvalidArgument` with message `An argument is invalid: the domain not a domain !! .. is a single label, not a domain name`, where AWS returns the fixed sentence `The specified domain name isn't associated with the distribution tenant. Specify another domain name and try again.`
6. Repeat step 4 with a well-formed domain that belongs to no tenant (or to a different tenant): the emulator answers HTTP 200 with a `DnsConfigurationList` entry instead of refusing with that same `InvalidArgument`.
7. Re-replay `cloudfront.verify-dns-configuration-malformed-domain-invalid-argument-rejected` to confirm the fix (its positive control also exercises the accepted-domain path).
