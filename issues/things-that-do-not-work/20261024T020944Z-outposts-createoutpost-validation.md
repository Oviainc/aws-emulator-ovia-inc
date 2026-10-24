Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# CreateOutpost accepts Availability Zone identifiers that name no zone, and contradicting AvailabilityZone/AvailabilityZoneId pairs

_Filed: 2026-10-24T02:09:44Z_
_Operation: CreateOutpost | Class: validation_

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler's only zone check is a REGION-PREFIX check. `crate::state::availability_zone_region_refusal` refuses a zone name that does not start with the request's region code and a zone id that does not start with the region's abbreviation; nothing checks that the value names a zone that actually exists, and nothing checks that a request naming BOTH identities names the SAME zone.

Consequences, all answered 200 with the value persisted:

1. `AvailabilityZoneId=usw2-az0` in `us-west-2` is accepted. AWS zone ids are `<region abbreviation>-az<n>` with n starting at 1; `usw2-az0` is not a zone of any region.
2. `AvailabilityZone=us-west-2j` in `us-west-2` is accepted. `us-west-2` publishes zones a-d; AWS answers a zone it cannot resolve in the region with the same `ValidationException` the emulator already renders for a cross-region zone (`<value> does not match your region.`).
3. A request naming `AvailabilityZone=us-east-1a` AND `AvailabilityZoneId=use1-az2` is accepted and the Outpost is stored with BOTH values as supplied, so `GetOutpost`/`ListOutposts` report a record whose two zone identities name two different zones. An AZ name maps to exactly one AZ id for a given account, so this pair describes a zone that cannot exist, and it also contradicts the emulator's own documented name<->id bijection (`…a` <-> `…-az1`) that it uses to fill in the missing identity when only one is supplied.

Expected: `ValidationException` (HTTP 400) for a zone identifier that names no zone of the request's region, and for a name/id pair that does not name the same zone. Actual: HTTP 200 and a persisted Outpost carrying the unusable (or self-contradicting) zone identity.

## Evidence

Spec: the model declares `AvailabilityZone` (pattern `^([a-zA-Z]+-){1,3}([a-zA-Z]+)?(\d+[a-zA-Z]?)?$`) and `AvailabilityZoneId` (pattern `^[a-zA-Z]+\d-[a-zA-Z]+\d$`) on `CreateOutpostInput` and `ValidationException` (HTTP 400) among its errors; the operation doc says "You can specify either an Availability one or an AZ ID", i.e. the two identities name one zone. Well-established AWS semantics: an Availability Zone's name and its zone ID are a 1:1 per-account mapping, and zone ids are numbered from az1.

Handler: `src/ops/create_outpost.rs` lines ~173-244 — the only refusal is `crate::state::availability_zone_region_refusal`; `src/state.rs` `zone_id_in_region` returns true for anything with the region abbreviation plus `-` (so `usw2-az0` passes), `zone_name_in_region` returns true for any non-digit suffix after the region code (so `us-west-2j` passes), and the resolution block then stores whatever the caller sent with no cross-check between the two members.

Tests: `tests/all/site_and_outpost_projection.rs::an_unresolvable_zone_leaves_the_other_identity_absent` pins the current accepting behaviour for `us-west-2j` and `usw2-az0`; `tests/all/error_classification.rs::create_outpost_refuses_an_availability_zone_of_another_region` only covers CROSS-region values. No test covers a contradicting in-region name/id pair.

## Steps to Reproduce

1. CreateSite with `{"Name":"zone site"}` and keep the returned SiteId.
2. Sign requests for us-west-2 and call CreateOutpost with `{"Name":"az0","SiteId":<site>,"AvailabilityZoneId":"usw2-az0"}` — observe HTTP 200 instead of a 400 ValidationException.
3. Call CreateOutpost with `{"Name":"zone-j","SiteId":<site>,"AvailabilityZone":"us-west-2j"}` — observe HTTP 200 for a zone that does not exist in us-west-2.
4. Sign for us-east-1 and call CreateOutpost with `{"Name":"mismatch","SiteId":<site>,"AvailabilityZone":"us-east-1a","AvailabilityZoneId":"use1-az2"}` — observe HTTP 200.
5. GetOutpost on the last Outpost: the response reports AvailabilityZone `us-east-1a` together with AvailabilityZoneId `use1-az2`, two different zones.
