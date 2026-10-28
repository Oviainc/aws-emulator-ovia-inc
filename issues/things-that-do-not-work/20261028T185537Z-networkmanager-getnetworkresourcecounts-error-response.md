Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 293c15726352cf981422bfe1d7cee5f6a5389f12

# GetNetworkResourceCounts hand-rolls the unresolvable-GlobalNetworkId refusal instead of the crate's one unknown_reference construction, so it reports Fields[].Name="GlobalNetworkId" and a request-built message

_Filed: 2026-10-28T18:55:37Z_
_Operation: GetNetworkResourceCounts | Class: error-response_

> _Note: this issue was identified against the code base at commit `293c15726352cf981422bfe1d7cee5f6a5389f12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetNetworkResourceCounts` models no `ResourceNotFoundException`, so it is the one operation of this crate that answers a `GlobalNetworkId` naming no stored global network with a `ValidationException`. It builds that exception by hand rather than through `crate::state::unknown_reference`, the single construction this crate documents and uses for "a reference that names no stored resource".

Two consequences, both divergences from the rule the crate states about itself:

1. The `Fields[].Name` it reports is the wire (PascalCase) spelling `GlobalNetworkId`, while `crate::state::FIELD_GLOBAL_NETWORK_ID` is `globalNetworkId` and the doc above it says Network Manager names the offending request member "in its own lowerCamelCase field naming, not in the wire (PascalCase) spelling". Every other refusal in this crate for an unresolvable `GlobalNetworkId` (CreateSite, CreateDevice, CreateConnection, CreateCoreNetwork) reports `globalNetworkId`, and `GetCoreNetwork` carries the same lowerCamelCase rule into its own `ResourceId`. A client keying on `Fields[].Name` therefore sees two different names for the same member depending on which operation refused it.

2. The `Message` is built from the request ("GlobalNetworkId <id> does not name a global network of this account") and the field message is a third string ("must name an existing global network"), where `unknown_reference` emits the constant "Global network not found." as BOTH the top-level `Message` and the field's `Message`.

No test pins either string: the suite asserts only the status and the error code for this case, so routing the refusal through `crate::state::unknown_reference(crate::state::FIELD_GLOBAL_NETWORK_ID, crate::state::GLOBAL_NETWORK_KIND)` leaves every existing assertion green.

## Evidence

The hand-rolled refusal:
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_network_resource_counts.rs:202-217
  "if !store.global_networks.contains_key(&input.global_network_id) {
       return Err(GetNetworkResourceCountsErrorModeled::ValidationException(
           ValidationException {
               message: format!(
                   \"GlobalNetworkId {} does not name a global network of this account\",
                   input.global_network_id
               ),
               reason: Some(\"FieldValidationFailed\".to_string()),
               fields: Some(vec![ValidationExceptionField {
                   name: \"GlobalNetworkId\".to_string(),
                   message: \"must name an existing global network\".to_string(),
               }]),
           },
       )"

The crate's stated rule and its one construction (kind B, other side):
services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:366-373
  "/// The field names this service reports in `ValidationException$Fields[].Name` for the
   /// parent reference a create carries.
   ///
   /// Network Manager names the offending REQUEST MEMBER in its own lowerCamelCase field
   /// naming, not in the wire (PascalCase) spelling: a recording of `CreateSite` under an
   /// unknown global network answers `globalNetworkId` ...
   pub const FIELD_GLOBAL_NETWORK_ID: &str = \"globalNetworkId\";"
services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:493-503
  "pub fn unknown_reference(member: &str, kind: &str) -> ValidationException {
       let message = format!(\"{} not found.\", kind_label(kind));
       ValidationException { message: message.clone(), reason: Some(\"FieldValidationFailed\"...),
           fields: Some(vec![ValidationExceptionField { name: member.to_string(), message }]) } }"
The sibling callers that go through it for the same member:
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/create_site.rs:177-185 and src/ops/create_core_network.rs:196-204 both pass `crate::state::FIELD_GLOBAL_NETWORK_ID`.
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_core_network.rs:140-147 restates the rule: "the NAME of the member the request addressed the core network through — `coreNetworkId`, in the lowerCamelCase spelling this service names its request members in".

Test coverage (nothing pins the current strings):
services/service-com-amazonaws-networkmanager-networkmanager/tests/all/network_resource_counts.rs:258-282 drives the three unresolvable-global-network cases and asserts only `status == 400` and `error_code(&body) == "ValidationException"`; it reads no `Fields` and no `Message`.
services/service-com-amazonaws-networkmanager-networkmanager/tests/all/unknown_parent_reference.rs:10-11 and :37 pin the other side: "one `Fields` entry whose `Name` is that member (the service's own lowerCamelCase field name)" / `const FIELD_GLOBAL_NETWORK_ID: &str = "globalNetworkId";`.

## Steps to Reproduce

1. CreateGlobalNetwork and then DeleteGlobalNetwork it (or pick an id nothing was ever stored under, e.g. global-network-00000000000000099).
2. Call GET /global-networks/{that id}/network-resource-count?resourceType=site.
3. Observed: HTTP 400 ValidationException with Message "GlobalNetworkId global-network-00000000000000099 does not name a global network of this account" and Fields = [{Name: "GlobalNetworkId", Message: "must name an existing global network"}].
4. Call POST /global-networks/{the same unresolvable id}/sites (CreateSite) for contrast.
5. Observed there: HTTP 400 ValidationException with Message "Global network not found." and Fields = [{Name: "globalNetworkId", Message: "Global network not found."}].
6. The same unresolvable member is reported under two different Fields[].Name spellings and two different messages by one service.
