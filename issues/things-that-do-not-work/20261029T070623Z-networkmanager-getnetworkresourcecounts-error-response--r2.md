Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 3a384b4a12755a7fa8fedde9fe2fff5778558e98

# The unsupported-ResourceType refusal is hand-rolled instead of going through crate::state::field_rejection, so it reports a Fields[].Message its two siblings never emit for the same rule

_Filed: 2026-10-29T07:06:23Z_
_Operation: GetNetworkResourceCounts | Class: error-response_

> _Note: this issue was identified against the code base at commit `3a384b4a12755a7fa8fedde9fe2fff5778558e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Four handlers of this crate enforce the same rule on the same request member: a `ResourceType` outside the set `crate::state::NETWORK_RESOURCE_TYPES` enumerates is refused with a `ValidationException` naming that member. Three of them build the refusal through `crate::state::field_rejection`, the crate's single construction for "a refusal of the single input member `member`", which by definition puts the SAME prose in the top-level `Message` and in `Fields[0].Message`.

`GetNetworkResourceCounts` is the one that does not. It constructs the `ValidationException` literally, with a request-built `Message` and a DIFFERENT, constant `Fields[0].Message` ("must be a supported network resource type"), and it spells the `Reason` with the bare literal `"FieldValidationFailed"` instead of `crate::state::REASON_FIELD_VALIDATION_FAILED` that the siblings pass. Its own comment claims the rule is shared with those siblings, and `get_network_resource_relationships.rs` says in so many words that this is "the same rule `GetNetworkResources` and `GetNetworkResourceCounts` apply to the same member" — yet a client that keys on `Fields[].Message` gets two different answers for one rule depending on which of the three operations refused it.

This is the sibling half of a defect already fixed in this very handler: the previously filed and fixed issue 20261028T185537Z-networkmanager-getnetworkresourcecounts-error-response routed the handler's OTHER hand-rolled refusal (the unresolvable `GlobalNetworkId`) through `crate::state::unknown_reference`; the `ResourceType` refusal a few lines above it was left hand-rolled. Routing it through `crate::state::field_rejection("ResourceType", crate::state::REASON_FIELD_VALIDATION_FAILED, <the same message>)` — exactly the call its siblings make — leaves the status, the error code and every existing assertion unchanged.

## Evidence

The hand-rolled refusal (the path that skips the helper):
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_network_resource_counts.rs:162-178
  "if !crate::state::is_network_resource_type(resource_type) {
       return Err(GetNetworkResourceCountsErrorModeled::ValidationException(
           ValidationException {
               message: format!(
                   \"ResourceType {resource_type:?} is not one of the supported network resource types: {}\",
                   crate::state::NETWORK_RESOURCE_TYPES.join(\", \")
               ),
               reason: Some(\"FieldValidationFailed\".to_string()),
               fields: Some(vec![ValidationExceptionField {
                   name: \"ResourceType\".to_string(),
                   message: \"must be a supported network resource type\".to_string(),
               }]),
           },
       )"

The sibling paths that DO call the helper for the identical rule (kind B, other side):
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_network_resources.rs:191-204
  "if let Some(resource_type) = input.resource_type.as_deref() {
       if !crate::state::is_network_resource_type(resource_type) {
           return Err(GetNetworkResourcesErrorModeled::ValidationException(
               crate::state::field_rejection(
                   \"ResourceType\",
                   crate::state::REASON_FIELD_VALIDATION_FAILED,
                   format!(
                       \"ResourceType {resource_type:?} is not one of the supported network resource types: {}\",
                       crate::state::NETWORK_RESOURCE_TYPES.join(\", \")
                   ),
               ),
           )"
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_network_resource_relationships.rs:195-208 makes the same `crate::state::field_rejection("ResourceType", crate::state::REASON_FIELD_VALIDATION_FAILED, ...)` call, and its comment states the rule is shared:
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_network_resource_relationships.rs:189-194
  "// A `ResourceType` outside the set the member's documentation enumerates
   // (`crate::state::NETWORK_RESOURCE_TYPES`) names no resource kind at all, so it is
   // refused on the member rather than answered with an empty page — the same rule
   // `GetNetworkResources` and `GetNetworkResourceCounts` apply to the same member."
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_network_telemetry.rs:186-199 refuses its own `ResourceType` through the same helper.

What the helper guarantees, and what the hand-rolled copy breaks:
services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:5439-5449
  "/// The modelled `ValidationException` (HTTP 400) for a refusal of the single input member
   /// `member`, carrying `reason` and one `Fields` entry explaining `message`.
   pub fn field_rejection(member: &str, reason: &str, message: String) -> ValidationException {
       ValidationException {
           message: message.clone(),
           reason: Some(reason.to_string()),
           fields: Some(vec![ValidationExceptionField {
               name: member.to_string(),
               message,
           }]),
       }"

And the handler's own comment asserting it applies the shared rule:
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_network_resource_counts.rs:156-160
  "// A `ResourceType` outside the set the member's documentation enumerates
   // (`crate::state::NETWORK_RESOURCE_TYPES`) names no resource kind this service
   // counts, so it is refused on the member rather than silently answered with a
   // zero count."

Test coverage (nothing pins the current strings, so the fix is response-compatible with the suite):
services/service-com-amazonaws-networkmanager-networkmanager/tests/all/network_resource_counts.rs:109-120 drives seven unsupported-`resourceType` cases and asserts only `status == 400` and `error_code(&body) == "ValidationException"`; it reads neither `Message` nor `Fields`.
services/service-com-amazonaws-networkmanager-networkmanager/tests/all/network_resource_counts.rs:270-281 does the same for the paged variant.

## Steps to Reproduce

1. CreateGlobalNetwork and note its GlobalNetworkId.
2. Call GET /global-networks/{id}/network-resource-count?resourceType=sites (an unsupported type).
3. Observed: HTTP 400 ValidationException whose Message is "ResourceType \"sites\" is not one of the supported network resource types: attachment, connect-peer, ..." but whose Fields = [{Name: "ResourceType", Message: "must be a supported network resource type"}] — a field message that differs from the top-level message.
4. Call GET /global-networks/{id}/network-resources?resourceType=sites for contrast (GetNetworkResources, same rule, same member).
5. Observed there: HTTP 400 ValidationException whose Fields[0].Message is the full "ResourceType \"sites\" is not one of the supported network resource types: ..." string, identical to the top-level Message, because the handler goes through crate::state::field_rejection.
6. Call GET /global-networks/{id}/network-resource-relationships?resourceType=sites — same as step 5.
7. One service therefore answers two different Fields[].Message values for one rule on one member, decided only by which of the three reads was called.
