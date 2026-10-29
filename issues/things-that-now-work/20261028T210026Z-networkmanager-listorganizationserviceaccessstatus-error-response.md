Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: f6d3b7b16cb7e1ce86179d792dd5264644759e0e

# An unminted NextToken is refused with Reason `CannotParse` here and `FieldValidationFailed` by all 23 sibling listings, although the handler's own comment says it refuses it "the same way"

_Filed: 2026-10-28T21:00:26Z_
_Operation: ListOrganizationServiceAccessStatus | Class: error-response_

> _Note: this issue was identified against the code base at commit `f6d3b7b16cb7e1ce86179d792dd5264644759e0e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated listing of this crate refuses a `NextToken` that `decode_page_token` cannot parse. 21 of them build the refusal inline with `Reason: "FieldValidationFailed"` and `Fields[0].Message: "the token could not be parsed"`; `GetNetworkResources` and `GetNetworkResourceRelationships` route it through `state::field_rejection(.., REASON_FIELD_VALIDATION_FAILED, ..)`, which also yields `Reason: FieldValidationFailed` (but copies the top-level message into `Fields[0].Message`).

`ListOrganizationServiceAccessStatus` alone passes `REASON_CANNOT_PARSE` instead, so for byte-identical input (`?nextToken=zzz`) it answers the modelled `ValidationException$Reason` enum value `CannotParse` where every sibling answers `FieldValidationFailed`. Its own comment two lines above the `match` asserts the opposite: that it refuses an unminted token "the same way every other paginated listing of this service refuses it".

This is a different symptom from the already-filed empty-`NextToken` divergence (20261028T023710Z-networkmanager-getsites-validation--r2, which is about `?nextToken=` being ACCEPTED here): this one is about the refusal body for a non-empty, unparseable token, which that issue does not mention.

## Evidence

The outlier and the rule it states — src/ops/list_organization_service_access_status.rs:149-165:
```
        // A token that is not one this service minted is refused before anything is
        // reported, the same way every other paginated listing of this service refuses it.
...
            Some(token) => Some(crate::state::decode_page_token(token).ok_or_else(|| {
                ListOrganizationServiceAccessStatusErrorModeled::ValidationException(
                    crate::state::field_rejection(
                        crate::state::FIELD_NEXT_TOKEN,
                        crate::state::REASON_CANNOT_PARSE,
                        "NextToken is not a token this service issued".to_string(),
                    ),
                )
            })?),
```
A sibling answering the same input — src/ops/list_core_network_prefix_list_associations.rs:205-219:
```
        // A token this service did not mint names no position in the listing, so it is
        // refused on the member rather than answered with an empty page — the refusal every
        // other listing of this service gives for the same input.
        let after = match input.next_token.as_deref() {
            Some(token) => Some(crate::state::decode_page_token(token).ok_or_else(|| {
                ListCoreNetworkPrefixListAssociationsErrorModeled::ValidationException(
                    ValidationException {
                        message: "NextToken is not a token this service issued".to_string(),
                        reason: Some(crate::state::REASON_FIELD_VALIDATION_FAILED.to_string()),
```
and 20 more with the identical `reason` (e.g. src/ops/get_sites.rs:163-170, src/ops/list_attachments.rs:165-172, src/ops/get_core_network_change_set.rs:197-204, src/ops/list_connect_peers.rs:152-159).

Both values are modelled members of the same enum, so the difference is observable on the wire — src/state.rs:5416-5423:
```
/// The `Reason` a `ValidationException` of this service carries when one input MEMBER is
/// at fault, and the one it carries when the member's text could not be parsed at all.
pub const REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";
pub const REASON_CANNOT_PARSE: &str = "CannotParse";
```
and `smithy_query shape=ValidationExceptionReason` → `["UnknownOperation", "CannotParse", "FieldValidationFailed", "Other"]`.

Test coverage: tests/all/core_network_change_set.rs:541 (`a_next_token_this_service_did_not_mint_is_refused`) covers the sibling spelling; tests/all/organization_service_access.rs contains no assertion on this handler's refusal `Reason`.

## Steps to Reproduce

1. GET /global-networks/{id}/sites?nextToken=zzzz → 400 ValidationException with `Reason: "FieldValidationFailed"`
2. GET /organizations/service-access?nextToken=zzzz → 400 ValidationException with `Reason: "CannotParse"`
3. Both members are the same modelled `NextToken` refused by the same `decode_page_token` failure, and src/ops/list_organization_service_access_status.rs:149-150 claims the second refuses "the same way every other paginated listing of this service refuses it"
