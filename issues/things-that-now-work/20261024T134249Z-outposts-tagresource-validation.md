Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7bcb53c96ea0ea6c32faf644dec1ae2940515247

# TagResource resolves a ResourceArn by resource id alone: the partition, Region and account id in the ARN are ignored

_Filed: 2026-10-24T13:42:49Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `7bcb53c96ea0ea6c32faf644dec1ae2940515247`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The tagging path reads a `ResourceArn` with `crate::state::resource_arn_parts`, which splits the ARN, checks only that the service field equals `outposts`, and explicitly discards the partition, Region and account-id fields (`let _partition`, `let _region`, `let _account`). The resource type + bare id are then looked up in the store.

Consequence: an ARN that names a DIFFERENT account's (or a different partition's/Region's) resource but happens to carry a resource id this emulator minted is accepted and the local resource is mutated. `arn:aws-us-gov:outposts:us-gov-west-1:999999999999:outpost/op-<id created here>` tags the Outpost that belongs to the emulator account in us-east-1, and `ListTagsForResource` with the same forged ARN reads those tags back (same helper, `src/ops/list_tags_for_resource.rs` line ~152).

Expected (real Outposts): an ARN is an absolute identity. An ARN whose account id is not the caller's, or whose partition/Region does not match the endpoint being called, names no resource the caller can tag — AWS refuses it (`NotFoundException`/`AccessDeniedException`), it never falls back to matching on the resource id alone.

Actual: HTTP 200 and the tags are written to the local resource.

## Evidence

Spec: the modelled `Arn` shape pattern `^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:([a-z\d-]+)/)[a-z]{2,8}-[a-f0-9]{17}$` gives the partition, Region and 12-digit account their own fields, and `TagResource` declares `NotFoundException` (404) and `AccessDeniedException` (403). Well-established AWS behaviour: a resource ARN identifies partition+Region+account+resource, and one account cannot tag another account's resource.

Handler: `src/state.rs` `resource_arn_parts` lines ~1240-1248 discards partition/region/account and keys only on `(resource_type, resource_id)`; `src/ops/tag_resource.rs` lines ~153-194 then writes into `store.tags_mut(&resource_type, &resource_id)`.

Tests: `tests/all/resource_tagging.rs::tagging_an_arn_that_names_no_resource_is_not_found_and_writes_nothing` (lines ~247-300) covers only ARNs whose resource ID does not exist — including a cross-partition case that uses an ABSENT site id — so no test covers a foreign partition/Region/account ARN carrying an id the store does hold.

## Steps to Reproduce

1. CreateSite, then CreateOutpost; keep the returned OutpostId (minted under the emulator account in the request's Region).
2. POST /tags/arn%3Aaws-us-gov%3Aoutposts%3Aus-gov-west-1%3A999999999999%3Aoutpost%2F<OutpostId> with {"Tags":{"Env":"dev"}} — a foreign partition, Region and account.
3. Observe HTTP 200 instead of a refusal.
4. GetOutpost on the real OutpostId: it now reports the tag written through the forged ARN.
5. GET /tags/<same forged ARN> also returns those tags, confirming the ARN's account/Region/partition are never checked.
