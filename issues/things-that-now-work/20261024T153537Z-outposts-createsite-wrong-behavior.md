Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# Minted SiteArn/OutpostArn always use the `aws` partition, even for a request signed for GovCloud or China

_Filed: 2026-10-24T15:35:37Z_
_Operation: CreateSite | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateSite` and `CreateOutpost` read the request's SigV4 credential-scope Region (`crate::state::request_region`) and interpolate it into the ARN they mint, but the PARTITION is hardcoded: `crate::state::site_arn` / `outpost_arn` call `::engine::emulator_arn`, which formats `arn:aws:{service}:{region}:{account}:{resource}` unconditionally.

So a `CreateSite` signed for `us-gov-west-1` answers `SiteArn: arn:aws:outposts:us-gov-west-1:000000000000:site/os-…`, and a `CreateOutpost` in `cn-north-1` answers `OutpostArn: arn:aws:outposts:cn-north-1:…`. Real AWS mints `arn:aws-us-gov:outposts:us-gov-west-1:…` in GovCloud and `arn:aws-cn:outposts:cn-north-1:…` in the China Regions — the partition is a function of the Region, never the literal `aws`.

The wrong partition is observable in `CreateSite`/`GetSite`/`ListSites` (`SiteArn`), in `CreateOutpost`/`GetOutpost`/`ListOutposts`/`UpdateOutpost` (`OutpostArn`, `SiteArn`) and in `CreateQuote`/`GetQuote` (`Quote$OutpostArn`, copied from the Outpost record). Any client that round-trips the returned ARN into another partition-aware API, or asserts the ARN it got back, sees a value the real service never emits for that Region.

Expected: the partition is derived from the Region (`us-gov-*` -> `aws-us-gov`, `cn-*` -> `aws-cn`, everything else -> `aws`), exactly as the modelled patterns' `aws([a-z-]+)?` branch allows. Actual: the `aws` partition for every Region.

## Evidence

Spec: every ARN shape in this model admits a partition suffix — `SiteArn` `^arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:site/(os-[a-f0-9]{17})$`, `OutpostArn` `^arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:outpost/op-[a-f0-9]{17}$`, `Arn` `^(arn:aws([a-z-]+)?:outposts:…)` — which exists precisely because Outposts is available in partitions whose ARNs are not `arn:aws:`. Well-established AWS behaviour: resources in `us-gov-*` carry `arn:aws-us-gov:` and in `cn-*` carry `arn:aws-cn:`.

Handlers/state: `src/state.rs` `site_arn` lines ~1164-1170 and `outpost_arn` lines ~1173-1179 both delegate to `::engine::emulator_arn`, whose only ARN format string is `arn:aws:{service}:{region}:{EMULATOR_ACCOUNT_ID}:{resource}` (`vendor/server/src/lib.rs` line ~162); `src/ops/create_site.rs` line ~180 and `src/ops/create_outpost.rs` line ~252 pass `request_region(ctx.parts())` as the Region, so the Region IS honoured while the partition is not.

Tests: `tests/all/availability_zone_identity.rs` creates Outposts signed for `us-gov-west-1` (its `REGIONS` table) but only ever asserts the Availability Zone identities, never the minted ARN; `tests/all/site_and_outpost_lifecycle.rs` pins `arn:aws:outposts:us-east-1:000000000000:…` prefixes for the default Region only. No test asserts the partition for a non-`aws` Region either way.

## Steps to Reproduce

1. Sign a request for `us-gov-west-1` and call `POST /sites` with `{"Name":"govcloud site"}`.
2. Observe the answered `Site.SiteArn` is `arn:aws:outposts:us-gov-west-1:000000000000:site/os-…`; the real GovCloud endpoint answers `arn:aws-us-gov:outposts:us-gov-west-1:…:site/os-…`.
3. Still signed for `us-gov-west-1`, call `POST /outposts` with `{"Name":"govcloud outpost","SiteId":<SiteId>,"AvailabilityZone":"us-gov-west-1a"}` and observe `OutpostArn` / `SiteArn` in the `aws` partition too.
4. Repeat signed for `cn-north-1`: the ARNs are `arn:aws:outposts:cn-north-1:…` instead of `arn:aws-cn:outposts:cn-north-1:…`.
5. GetSite / ListOutposts report the same wrong-partition ARNs, so the value persists rather than being a one-off in the create reply.
