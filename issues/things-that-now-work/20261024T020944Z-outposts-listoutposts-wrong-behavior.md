Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# Sites and Outposts are not scoped to the region they were created in, so one region's endpoint lists another region's resources

_Filed: 2026-10-24T02:09:44Z_
_Operation: ListOutposts | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateSite`/`CreateOutpost` read the request's SigV4 credential-scope region only to MINT the ARN (`arn:aws:outposts:<region>:<account>:site/os-…`); the stored records carry no region at all. Every read path ignores the request region entirely (`get_outpost.rs`, `get_site.rs`, `list_outposts.rs`, `list_sites.rs`, `delete_*`, `update_*` all start with `let _ = ctx;`).

So a site created through the us-east-1 endpoint is returned by `ListSites`/`GetSite` signed for eu-west-1, and an Outpost created in us-west-2 is returned by `ListOutposts` signed for ap-southeast-2 — carrying an ARN that names the OTHER region. AWS Outposts is a regional service: each regional endpoint sees only the sites and Outposts of its own region, so `ListOutposts` in a region where nothing was created returns an empty list and `GetOutpost` for another region's id answers `NotFoundException` (404).

Expected: a resource is visible only through the region it was created in; other regions see an empty list / 404. Actual: one global store shared by every region, with cross-region ARNs leaking into responses.

## Evidence

Spec: the model pins the region into every ARN this service returns (`SiteArn` pattern `^arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:site/(os-[a-f0-9]{17})$`, same for `OutpostArn`), and the service's `endpointPrefix`/`sigv4` config is regional — well-established AWS behaviour is that a regional endpoint only serves its own region's resources.

State/handlers: `src/state.rs` `SiteRecord`/`OutpostRecord` have no region member and `ComAmazonawsOutpostsOutpostsolafserviceState` keys both maps by bare id only; `request_region(ctx.parts())` is called only in `create_site.rs` (line ~161) and `create_outpost.rs` (line ~163) for ARN minting. `list_outposts.rs`/`list_sites.rs`/`get_outpost.rs`/`get_site.rs` discard `ctx`.

Tests: `tests/all/error_classification.rs::create_outpost_refuses_an_availability_zone_of_another_region` creates Outposts in seven different regions against ONE store and never lists them, so no test asserts per-region visibility either way.

## Steps to Reproduce

1. Sign for us-east-1: CreateSite `{"Name":"east site"}`, then CreateOutpost `{"Name":"east outpost","SiteId":<site>,"AvailabilityZone":"us-east-1a"}`.
2. Sign the next request for eu-west-1 and call `GET /outposts` — the us-east-1 Outpost is returned, with an `arn:aws:outposts:us-east-1:…` OutpostArn; the real eu-west-1 endpoint returns an empty `Outposts` list.
3. Still signed for eu-west-1, call `GET /sites/<site>` and `GET /outposts/<outpostId>` — both answer 200 instead of the 404 NotFoundException the real regional endpoint returns.
