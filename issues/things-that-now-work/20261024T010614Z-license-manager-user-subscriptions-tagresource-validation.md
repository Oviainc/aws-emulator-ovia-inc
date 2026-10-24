Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Tag keys and values are stored unvalidated: empty/over-long keys and reserved aws: prefixes are accepted

_Filed: 2026-10-24T01:06:14Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`merge_tags` enforces only the COUNT of the merged tag set (<= 50). Nothing validates the individual keys or values, so TagResource answers 200 and persists tags that AWS refuses:

- an empty tag key ("") ;
- a tag key longer than 128 characters, or a value longer than 256 characters;
- a key in the reserved `aws:` namespace, which AWS reserves for system tags and rejects on every tagging API.

Expected: each of those is refused with the operation's modelled `ValidationException` (HTTP 400) and nothing is written. Actual: HTTP 200, and `ListTagsForResource` subsequently reports the illegal tag, so a client that round-trips tags through this emulator sees state the real service would never hold. The same unvalidated path is shared by the `Tags` maps of RegisterIdentityProvider, AssociateUser, StartProductSubscription and CreateLicenseServerEndpoint, which insert the map into `state.tags` directly.

## Evidence

Spec: `Tags` is `map<String,String>` with only `@length(min:0,max:50)` on the map itself (smithy_query shape "Tags"), so nothing at the boundary constrains keys/values; the service-independent AWS tagging rules (key 1-128 chars, value 0-256 chars, `aws:` prefix reserved) are what the real API enforces, and TagResource models `ValidationException` (400) for exactly this class of failure. Handler: `src/ops/tag_resource.rs` lines 136-144; state: `src/state.rs::merge_tags` lines 719-741 checks only the merged count against MAX_TAGS_PER_RESOURCE. Tests: `resource_resolution_refusals.rs::the_tag_set_one_resource_carries_is_bounded` (line 1536) covers the count bound only; no test asserts any per-key/value rule.

## Steps to Reproduce

1. RegisterIdentityProvider and take the returned IdentityProviderArn.
2. Call TagResource PUT /tags/{ResourceArn} with Tags = {"aws:created-by": "x"} — observe HTTP 200; expected 400 ValidationException (the aws: namespace is reserved).
3. Call TagResource with Tags = {"": "x"} — observe HTTP 200; expected 400 ValidationException (empty key).
4. Call TagResource with a 200-character key and a 500-character value — observe HTTP 200; expected 400 ValidationException (key > 128 / value > 256).
5. Call ListTagsForResource for the same ARN and observe all of the illegal tags reported back.
