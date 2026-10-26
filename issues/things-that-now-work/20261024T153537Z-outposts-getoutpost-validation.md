Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# Identifier ARNs are resolved by bare resource id alone: the partition, Region and account id in an OutpostId/SiteId/QuoteIdentifier ARN are ignored

_Filed: 2026-10-24T15:35:37Z_
_Operation: GetOutpost | Class: validation_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every identifier member in this API accepts two forms — the bare id (`op-…`, `os-…`, `oq-…`) or the full resource ARN. The emulator resolves both through `crate::state::resolve_id`, which, for a value starting with `arn:`, keeps only the segment after the LAST `/` and throws away the whole ARN prefix (partition, service, Region, account id, resource type).

Consequence: an ARN that names a resource in a different partition, a different Region or a different ACCOUNT is accepted and silently served from the local store as long as the bare id happens to be one this emulator minted. `GET /outposts/arn%3Aaws-cn%3Aoutposts%3Acn-north-1%3A111122223333%3Aoutpost%2Fop-<local id>` answers 200 with the local Outpost (whose own `OutpostArn` names `arn:aws:outposts:us-east-1:000000000000:…`), instead of the 404 `NotFoundException` the real regional endpoint returns for an ARN that is not one of its own account's resources. The same helper backs `DeleteOutpost`, `UpdateOutpost`, `CreatePrivateConnectivityConfig`, `GetPrivateConnectivityConfig`, `GetRenewalPricing`, `GetCapacityTask`, `StartCapacityTask`, `StartOutpostDecommission`, `ListAssets`, `ListAssetInstances`, `ListBlockingInstancesForCapacityTask`, `GetOutpostSupportedInstanceTypes`, `GetSite`, `DeleteSite`, `UpdateSite`, `UpdateSiteRackPhysicalProperties`, `GetSiteAddress`, `UpdateSiteAddress`, `CreateOutpost` (`SiteId`), `CreateQuote` (`OutpostIdentifier`) and `GetQuote`, so a forged ARN reads — and for the deletes/updates MUTATES — the local resource everywhere.

Expected: an ARN is an absolute identity; one whose account id is not the caller's, or whose partition/Region does not match the endpoint being called, names no resource this endpoint holds and is refused (404 `NotFoundException`, which every one of these operations models). Actual: the ARN prefix is discarded and only the trailing id is matched.

Note this is a DIFFERENT code path from the already-filed `TagResource` ARN issue: that one is about `ResourceArn` and `crate::state::resource_arn_parts`; this one is about the identifier members and `crate::state::resolve_id`, which strips the prefix without even looking at the fields.

## Evidence

Spec: the modelled identifier patterns give the partition, Region and 12-digit account their own fields and admit non-default partitions — `OutpostId`/`OutpostIdentifier` `^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:outpost/)?op-[a-f0-9]{17}$`, `SiteId` `^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:site/)?(os-[a-f0-9]{17})$`, `QuoteIdentifier` `…:quote/)?oq-…` — so a value carrying another account's or another partition's ARN passes boundary validation and reaches the handler. `GetOutpost` declares `NotFoundException` (404) and `AccessDeniedException` (403). Well-established AWS behaviour: an ARN identifies partition+Region+account+resource, and a regional endpoint of one account does not serve another account's ARN.

Handler: `src/state.rs` `resolve_id` lines ~1190-1201 — `let bare = match identifier.starts_with("arn:") { true => identifier.rsplit('/').next()?, … }` — nothing after that inspects the discarded prefix; `resolve_site_id`/`resolve_outpost_id`/`resolve_quote_id` (lines ~1203-1216) are thin wrappers. `src/ops/get_outpost.rs` line ~148 then looks the bare id up in `store.outposts`.

Tests: `tests/all/site_and_outpost_lifecycle.rs` and `tests/all/renewal_pricing.rs` exercise the two identifier forms only with the emulator's OWN ARN (`OUTPOST_ARN_PREFIX` = `arn:aws:outposts:us-east-1:000000000000:outpost/`); no test passes an ARN with a foreign partition, Region or account id carrying a locally minted id, so nothing asserts either behaviour.

## Steps to Reproduce

1. CreateSite `{"Name":"arn identity site"}`, then CreateOutpost `{"Name":"arn identity outpost","SiteId":<SiteId>,"AvailabilityZone":"us-east-1a"}`; keep the returned OutpostId.
2. Call `GET /outposts/arn%3Aaws%3Aoutposts%3Aus-east-1%3A999999999999%3Aoutpost%2F<OutpostId>` (another account's ARN) — observe 200 with the local Outpost instead of 404 NotFoundException.
3. Call `GET /outposts/arn%3Aaws-cn%3Aoutposts%3Acn-north-1%3A111122223333%3Aoutpost%2F<OutpostId>` (another partition and Region) — observe 200 again, with an `OutpostArn` that contradicts the ARN that was asked about.
4. Call `DELETE /outposts/arn%3Aaws-us-gov%3Aoutposts%3Aus-gov-west-1%3A999999999999%3Aoutpost%2F<OutpostId>` — the local Outpost is deleted through a foreign ARN; a following GetOutpost on the real id answers 404.
