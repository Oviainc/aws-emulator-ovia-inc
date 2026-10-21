Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the BCM Dashboards emulator

_Filed: 2026-10-21T21:18:14Z_
_Operation: service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 15 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` with zero persisted state, so no client workflow works at all. Implement a FIRST minimal slice: the stateful lifecycle of the primary resource, the dashboard.

Happy path to implement, in order (awsJson1_0, `POST /` with `X-Amz-Target: AWSBCMDashboardsService.<Op>`):

1. **CreateDashboard** — input `name` (required, 2..50, pattern `^(?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]$`), `description` (optional, 1..200), `widgets` (required list<Widget>, 0..20), `resourceTags` (optional list<ResourceTag>, 0..200). Output: `arn` (required) — must be a freshly minted, unique ARN matching `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$` (account id from the request context, resource id a new lowercase/hyphen id). Store the dashboard under that ARN with `type = CUSTOM`, `createdAt = updatedAt = now`, the supplied widgets (applying Widget member defaults: `width`=4, `height`=7, `horizontalOffset`=0, and minting a 32-hex-char `id` for any widget that omits one) and the tags.
2. **GetDashboard** — input `arn` (required). Output must be read back from the stored record: `arn`, `name`, `description`, `type` (CUSTOM), `widgets` (exactly what was stored, including minted widget ids and defaulted width/height), `createdAt`, `updatedAt`. Unknown ARN => `ResourceNotFoundException` (HTTP 404); malformed ARN / constraint violation => `ValidationException` (HTTP 400).
3. **ListDashboards** — input `maxResults` (1..100, default 20), `nextToken` (1..8192). Output `dashboards` (required list<DashboardReference>: `arn`, `name`, `description`, `type`, `createdAt`, `updatedAt`) built by projecting the stored dashboards, plus `nextToken` when and ONLY when more remain. Pagination must be real: slice at `maxResults`, encode the cursor in `nextToken`, resume from it, and omit `nextToken` on the last page. `maxResults` outside 1..100 or an uninterpretable `nextToken` => `ValidationException`.
4. **UpdateDashboard** — input `arn` (required), `name` (required), `description`, `widgets` (optional: absent means leave widgets untouched, present means replace wholesale). Mutate the stored record in place, bump `updatedAt`, return `arn`. Unknown ARN => `ResourceNotFoundException`. A subsequent GetDashboard must show the new name/description/widgets and a strictly newer `updatedAt`.
5. **DeleteDashboard** — input `arn` (required), output `arn`. Remove the record (and its tags/resource policy) from state; a following GetDashboard must return `ResourceNotFoundException` and ListDashboards must no longer include it.

NONE of these may return a canned or echoed response. Every field in every output must be computed from the persisted record — in particular GetDashboard/ListDashboards must never echo request input, and CreateDashboard must not return a random ARN it did not store. Hollow 200s are failures.

## State Model

Add to `ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState` in `src/state.rs`:

- `dashboards: BTreeMap<String /* dashboard ARN */, Dashboard>` — BTreeMap so ListDashboards has a stable, deterministic order across pages.
- `struct Dashboard { arn: String, name: String, description: Option<String>, dashboard_type: DashboardType /* CUSTOM */, widgets: Vec<Widget>, created_at: SystemTime, updated_at: SystemTime, resource_tags: Vec<ResourceTag> }`
- `struct Widget { id: String /* 32 hex chars */, title: String, description: Option<String>, width: i32 /* default 4 */, height: i32 /* default 7 */, horizontal_offset: i32 /* default 0 */, configs: Vec<WidgetConfig> /* 1..2, stored verbatim */ }`
- a monotonically increasing `next_dashboard_seq: u64` (or equivalent) used to mint unique dashboard resource ids so two CreateDashboard calls with the same `name` get different ARNs.

Out of scope for this first slice (stay 501, filed as individual gap issues later): CreateScheduledReport, GetScheduledReport, ListScheduledReports, UpdateScheduledReport, DeleteScheduledReport, ExecuteScheduledReport, GetResourcePolicy, TagResource, UntagResource, ListTagsForResource. Note the tags supplied to CreateDashboard should already be persisted on the record so the tagging operations have state to read when they are implemented.

## Evidence

OPERATIONS.md (crate root) documents all 15 operations with their members: CreateDashboard (`name`/`description`/`widgets`/`resourceTags` -> required `arn`), GetDashboard (@readonly, `arn` -> `arn`/`name`/`description`/`type`/`widgets`/`createdAt`/`updatedAt`, ResourceNotFoundException 404), ListDashboards (@paginated inputToken=nextToken outputToken=nextToken items=dashboards, `maxResults` range 1..100 default 20), UpdateDashboard (`arn`+`name` required, ResourceNotFoundException 404), DeleteDashboard (`arn` -> `arn`). `smithy_query shape=Widget,DashboardReference` gives the Widget defaults (width 4, height 7, horizontalOffset 0, 32-hex `id`, required `title` and `configs` 1..2) and DashboardReference's required members used by ListDashboards. Census: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 15 files, including create_dashboard.rs, get_dashboard.rs, list_dashboards.rs, update_dashboard.rs and delete_dashboard.rs — every handler still returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) and no test exercises any dashboard state.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `bcm-dashboards` at it.
2. CreateDashboard with name="MyCostDashboard", description="demo", and one widget {title: "Spend", configs: [<one config>]} — expect HTTP 200 with a unique `arn` matching the dashboard ARN pattern (actual today: HTTP 501 NotImplemented).
3. GetDashboard with that arn — expect the stored name, description, type=CUSTOM, the widget with a minted 32-hex `id` and defaulted width=4/height=7, and createdAt/updatedAt timestamps.
4. ListDashboards with maxResults=1 — expect the dashboard in `dashboards` as a DashboardReference; create a second dashboard and confirm page 1 returns 1 item plus a `nextToken`, page 2 returns the other item with no `nextToken`.
5. UpdateDashboard on the first arn with name="MyCostDashboard2" — expect 200, then GetDashboard shows the new name and a newer `updatedAt` while `createdAt` is unchanged.
6. DeleteDashboard on that arn — expect 200 echoing the arn; then GetDashboard returns ResourceNotFoundException (HTTP 404) and ListDashboards no longer lists it.
7. GetDashboard on a well-formed but never-created arn — expect ResourceNotFoundException (404); GetDashboard with arn="not-an-arn" — expect ValidationException (400).
