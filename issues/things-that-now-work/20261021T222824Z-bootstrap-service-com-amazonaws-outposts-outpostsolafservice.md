Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 85378260976efce74d968b045ae62c2fe2a03011

# Bootstrap the com-amazonaws-outposts-outpostsolafservice emulator

_Filed: 2026-10-21T22:28:24Z_
_Operation: service-com-amazonaws-outposts-outpostsolafservice | Class: missing_

> _Note: this issue was identified against the code base at commit `85378260976efce74d968b045ae62c2fe2a03011`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 45 handlers under `services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and the service holds no state. The emulator therefore cannot serve any AWS Outposts client call.

This issue scopes the FIRST, SMALL slice: the site + Outpost lifecycle, which is the primary resource pair of this API (`restJson1`, sdkId `Outposts`, signing name `outposts`). Implement these 6 operations statefully, in this order:

1. **CreateSite** — `POST /sites`. Input: `Name` (required, 1..1000, `^[\S ]+$`), `Description`, `Notes`, `Tags`, `OperatingAddress` (Address), `ShippingAddress` (Address), `RackPhysicalProperties`. Must mint a new `SiteId` matching `^os-[a-f0-9]{17}$` and a `SiteArn` matching `^arn:aws:outposts:<region>:<account12>:site/os-[a-f0-9]{17}$`, store the record, and return `Site` populated from what was stored: `SiteId`, `SiteArn`, `AccountId` (12 digits), `Name`, `Description`, `Notes`, `Tags`, `RackPhysicalProperties`, plus `OperatingAddressCountryCode` / `OperatingAddressStateOrRegion` / `OperatingAddressCity` derived from the supplied `OperatingAddress`.

2. **CreateOutpost** — `POST /outposts`. Input: `Name` (required), `SiteId` (required, must reference an existing site or the call fails with `NotFoundException` 404), `Description`, `AvailabilityZone`, `AvailabilityZoneId`, `Tags`, `SupportedHardwareType` (RACK|SERVER). Must mint `OutpostId` matching `^op-[a-f0-9]{17}$` and `OutpostArn` matching `^arn:aws:outposts:<region>:<account12>:outpost/op-[a-f0-9]{17}$`, persist it against the site, and return `Outpost` with `OutpostId`, `OutpostArn`, `OwnerId`, `SiteId`, `SiteArn`, `Name`, `Description`, `LifeCycleStatus` (e.g. `ACTIVE`), `AvailabilityZone`, `AvailabilityZoneId`, `Tags`, `SupportedHardwareType`.

3. **GetOutpost** — `GET /outposts/{OutpostId}`. Must look the stored record up by id OR by ARN (the label pattern accepts both forms) and return the SAME `Outpost` structure that CreateOutpost returned, field for field. Unknown id -> `NotFoundException` (404). Malformed id (pattern mismatch) -> `ValidationException` (400).

4. **GetSite** — `GET /sites/{SiteId}`. Same contract for the site: resolve by id or ARN, return the stored `Site`; 404 when absent, 400 when the identifier is malformed.

5. **ListOutposts** — `GET /outposts`. Returns `Outposts` (list<Outpost>) containing exactly the Outposts currently persisted — no canned list. Honour `MaxResults` (1..1000) by truncating and emitting an opaque `NextToken` that a follow-up call resumes from without duplicating or skipping items; `LifeCycleStatusFilter` / `AvailabilityZoneFilter` / `AvailabilityZoneIdFilter` must filter the stored records. (`ListSites` with its `OperatingAddress*Filter`s may follow the same shape but is optional for this slice.)

6. **DeleteOutpost** — `DELETE /outposts/{OutpostId}`, then **DeleteSite** — `DELETE /sites/{SiteId}`. Both have empty outputs and must actually remove the record so a subsequent Get returns `NotFoundException` (404) and ListOutposts/ListSites no longer includes it. Deleting a site that still has Outposts attached must fail with `ConflictException` (409); deleting an absent id must be `NotFoundException` (404).

NEVER STUBS: each of these must compute its response from the persisted store. Hardcoded `Outpost`/`Site` bodies, echoing the input back without storing it, or minting an id without recording it are all failures of this issue. A create followed by a get/list must return what was created, and a delete must be observable by every later call.

## State Model

In `src/state.rs` (`ComAmazonawsOutpostsOutpostsolafserviceState`, reached via `crate::state::service_state(&*self.engine)` and mutated under its lock):

- `sites: HashMap<String /* SiteId "os-..." */, SiteRecord>` where `SiteRecord { site_id, site_arn, account_id, name, description, notes, tags: HashMap<String,String>, operating_address: Option<Address>, shipping_address: Option<Address>, rack_physical_properties: Option<RackPhysicalProperties>, created_at }`.
- `outposts: HashMap<String /* OutpostId "op-..." */, OutpostRecord>` where `OutpostRecord { outpost_id, outpost_arn, owner_id, site_id, site_arn, name, description, life_cycle_status, availability_zone, availability_zone_id, tags: HashMap<String,String>, supported_hardware_type, created_at }`.
- Insertion order preserved for listing (e.g. an auxiliary `outpost_order: Vec<String>` / `site_order: Vec<String>`, or an ordered map) so pagination tokens are stable across calls.
- A shared id/ARN helper that generates the 17-hex-char suffixes required by the `^op-[a-f0-9]{17}$` / `^os-[a-f0-9]{17}$` patterns and formats ARNs from the request's region and account, plus an identifier resolver that accepts either the bare id or the full ARN form the `@httpLabel` patterns allow.

Everything else (orders, quotes, capacity tasks, assets, catalog items, connections, private connectivity, tagging, Update*) stays 501 for now and will be filed as individual gap issues later.

Done = a realistic client script runs end to end against the emulator with correct, stateful bodies — not merely 2xx.

## Evidence

OPERATIONS.md (crate root) documents all 45 operations, the `restJson1` protocol and sigv4 signing name `outposts`, and gives the exact contracts cited above: CreateSite `POST /sites` -> `Site` (line 75-80), CreateOutpost `POST /outposts` with required `Name`+`SiteId` -> `Outpost` (lines 47-52), GetOutpost `GET /outposts/{OutpostId}` -> `Outpost` with NotFound/Validation errors (lines 131-136), GetSite `GET /sites/{SiteId}` (lines 183-188), ListOutposts `GET /outposts` with `@paginated(inputToken=NextToken, outputToken=NextToken, items=Outposts)` and the three filters (lines 253-259), DeleteOutpost `DELETE /outposts/{OutpostId}` and DeleteSite `DELETE /sites/{SiteId}` with empty outputs and `ConflictException (409)` declared (lines 82-101). The `Outpost` and `Site` member lists and id/ARN patterns come from the Smithy model shapes `com.amazonaws.outposts#Outpost` and `com.amazonaws.outposts#Site`. All-stub status confirmed by `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:'` matching all 45 files in `src/ops/` (the census marker documented in the crate README); no handler has any state access and there are no tests beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS Outposts client at it (`--endpoint-url`, region us-east-1).
2. CreateSite --name "bootstrap-site" --operating-address '{"CountryCode":"US","StateOrRegion":"WA","City":"Seattle","AddressLine1":"410 Terry Ave N","PostalCode":"98109"}' --tags Env=dev  => expect 200 with Site.SiteId matching ^os-[a-f0-9]{17}$, a matching SiteArn, Name/Tags echoed from stored state, and OperatingAddressCity=Seattle. Actual: HTTP 501 NotImplemented.
3. CreateOutpost --name "bootstrap-op" --site-id <SiteId from step 2> --availability-zone us-east-1a --supported-hardware-type RACK  => expect 200 with Outpost.OutpostId matching ^op-[a-f0-9]{17}$, OutpostArn, SiteId/SiteArn of the created site, LifeCycleStatus set. Actual: 501.
4. CreateOutpost with --site-id os-00000000000000000 (never created) => expect NotFoundException with HTTP 404.
5. GetOutpost --outpost-id <OutpostId> => expect 200 returning exactly the Outpost created in step 3 (same Name, Description, SiteId, Tags, SupportedHardwareType); repeat using the full OutpostArn and expect the same record.
6. GetSite --site-id <SiteId> => expect 200 returning the Site created in step 2.
7. ListOutposts => expect the created Outpost present exactly once; create a second Outpost, then ListOutposts --max-results 1 and follow NextToken, expecting the two distinct Outposts with no duplicates or omissions; ListOutposts --availability-zone-filter us-east-1a returns only matching records.
8. DeleteSite --site-id <SiteId> while the Outpost still exists => expect ConflictException with HTTP 409.
9. DeleteOutpost --outpost-id <OutpostId> => expect 200 with an empty body; then GetOutpost on the same id => expect NotFoundException 404, and ListOutposts no longer lists it.
10. DeleteSite --site-id <SiteId> => expect 200; GetSite on the same id => expect NotFoundException 404.
11. GetOutpost --outpost-id not-an-outpost-id => expect ValidationException with HTTP 400.
