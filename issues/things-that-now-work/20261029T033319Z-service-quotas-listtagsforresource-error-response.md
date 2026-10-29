Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 2bb16450f75bc57f8f5e668b05a4384e1672b4e8

# A ResourceARN that passes its @pattern but is not a quota ARN is refused in a second, member-and-rule wording the crate says this member never gets

_Filed: 2026-10-29T03:33:19Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `2bb16450f75bc57f8f5e668b05a4384e1672b4e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states ONE rule for how a `ResourceARN` this service will not accept as a value is worded: the service's own application layer answers with a single illegal-value sentence that names neither the member nor the rule, and which validator answers is "a property of the MEMBER, not of the operation or of the value".

The tagging handlers break that rule for every `ResourceARN` that satisfies the modelled `AmazonResourceName` pattern/length but is not an applied-quota ARN of this service — e.g. `arn:aws:s3:us-east-1:123456789012:bucket/x` (another service's `arnNamespace`) or `arn:aws:servicequotas:us-east-1:123456789012:notaquota` (a resource part that is not `<ServiceCode>/<QuotaCode>`). Both match `^arn:aws(-[\w]+)*:*:.+:[0-9]{12}:.+$`, so the boundary validator passes them and `crate::state::resolve_quota_arn` returns `ResourceArnError::Malformed`. The handler then answers `IllegalArgumentException` with `crate::state::malformed_resource_arn_message`, which NAMES the member and spells out the rule and echoes the caller's string back — exactly the wording the crate says this member is never refused with.

So the same member, the same error code and the same fact ("this is not an ARN I can read") get two different sentences, and which one a caller gets is decided by the VALUE, not by the member. The identical duplication exists in `TagResource` and `UntagResource`, which map the same `Malformed` arm the same way; one fix covers all three (the `Malformed` arm should state the crate's one illegal-value sentence). The `NoSuchQuota` arm is a different fact (`NoSuchResourceException`) and is not part of this finding.

## Evidence

The stated rule — `tests/all/resource_tags.rs:12-20`:
```
//! The rule, asserted over a TABLE of inputs rather than the one recorded case: this
//! service runs THREE validators, and which one answers is a property of the MEMBER,
//! not of the operation or of the value (`src/service.rs`:
//! `FRONT_END_VALIDATED_MEMBERS`, `SERVICE_OWN_VALIDATED_MEMBERS`).
//!
//! * `ResourceARN` is checked by the service's own application layer, which states ONE
//!   sentence for an illegal value and names neither the member nor the rule —
//!   recorded for an ARN outside its `@pattern`, and asserted here for every
//!   constraint that member carries, on all three tagging operations;
```

and `src/service.rs:145-170`:
```
/// The input members Service Quotas checks in its OWN application layer, which states
/// one sentence for an illegal value ([`ILLEGAL_VALUE_MESSAGE`]) and names
/// neither the member nor the rule — the third validator in the split above.
...
const SERVICE_OWN_VALIDATED_MEMBERS: &[&str] = &["ResourceARN"];
...
/// Recorded for the resource ARN; it covers every member in
/// [`SERVICE_OWN_VALIDATED_MEMBERS`].
const ILLEGAL_VALUE_MESSAGE: &str =
    "The request failed because you provided an illegal value for a parameter.";
```

The code that breaks it — `src/ops/list_tags_for_resource.rs:139-148`:
```
            .map_err(|error| match error {
                crate::state::ResourceArnError::Malformed => {
                    ListTagsForResourceErrorModeled::IllegalArgumentException(
                        IllegalArgumentException {
                            message: Some(crate::state::malformed_resource_arn_message(
                                &input.resource_arn,
                            )),
                        },
                    )
                }
```
with the second wording at `src/state.rs:1795-1800`:
```
pub fn malformed_resource_arn_message(resource_arn: &str) -> String {
    format!(
        "ResourceARN '{resource_arn}' is not an applied quota ARN of the form \
         arn:<partition>:{ARN_NAMESPACE}:<region>:<account>:<ServiceCode>/<QuotaCode>"
    )
}
```
and the reachable `Malformed` arm at `src/state.rs:1544-1556` (`namespace != ARN_NAMESPACE`, or `resource.split_once('/')` failing) — both reached only AFTER the modelled pattern passed.

The same mapping is duplicated at `src/ops/tag_resource.rs:150-157` and `src/ops/untag_resource.rs:136-143`.

Test coverage: `tests/all/resource_tags.rs:91-136` (`an_illegal_resource_arn_is_refused_in_the_services_own_words`) tables only values that break the member's MODELLED constraints (`"not-an-arn"`, a missing account segment, a short account id, one char, the empty string, one char past 1011) and asserts `Some(ILLEGAL_VALUE_MESSAGE)` for all of them; no row carries a pattern-passing ARN of another namespace, so nothing pins the handler's second wording.

## Steps to Reproduce

1. Start the emulator and sign a request with a us-east-1 SigV4 credential scope.
2. POST / with `X-Amz-Target: ServiceQuotasV20190624.ListTagsForResource` and body {"ResourceARN":"not-an-arn"} — observe HTTP 400 IllegalArgumentException with Message "The request failed because you provided an illegal value for a parameter."
3. POST / with the same target and body {"ResourceARN":"arn:aws:s3:us-east-1:123456789012:bucket/x"} (it satisfies the modelled AmazonResourceName pattern, so it reaches the handler) — observe HTTP 400 IllegalArgumentException with Message "ResourceARN 'arn:aws:s3:us-east-1:123456789012:bucket/x' is not an applied quota ARN of the form arn:<partition>:servicequotas:<region>:<account>:<ServiceCode>/<QuotaCode>": a second wording for the same member and the same code, naming the member, the rule and the caller's value.
4. Repeat step 3 against TagResource and UntagResource — the same second wording.
