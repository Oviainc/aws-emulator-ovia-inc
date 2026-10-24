Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7bcb53c96ea0ea6c32faf644dec1ae2940515247

# Resource Groups state is not partitioned by Region: a group created in one Region resolves, lists and blocks name reuse in every other Region

_Filed: 2026-10-24T13:38:25Z_
_Operation: CreateGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7bcb53c96ea0ea6c32faf644dec1ae2940515247`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Resource Groups is a Regional service: a group lives in the Region whose endpoint created it, a group name only has to be unique within one Region of one account, and a caller signing for another Region sees neither the group nor its name.

The emulator holds every group in ONE global map (`ComAmazonawsResourcegroupsArdiState::groups`, a `BTreeMap` keyed by bare group name) with no Region component in the key and no Region stored on the record other than inside the minted ARN. Two observable divergences follow:

1. Name reuse across Regions is refused. CreateGroup for the name `demo` with a `us-east-1` credential scope stores `demo`; a second CreateGroup for `demo` with an `eu-west-1` credential scope answers 400 BadRequestException "Cannot create group: group already exists". Expected: 200, creating an independent `eu-west-1` group whose ARN carries `eu-west-1` — the model documents the uniqueness scope as per-Region, not per-account.

2. Cross-Region reads and listings succeed. GetGroup / GetGroupQuery / GetGroupConfiguration / ListGroupResources / ListGroupingStatuses / UpdateGroup / DeleteGroup all resolve a bare name (and the stored ARN) regardless of the Region the request was signed for, and ListGroups in `eu-west-1` enumerates groups created in `us-east-1` (returning `GroupArn`s that name a different Region). Expected: a group created in another Region is invisible — NotFoundException (404) for the single-group readers and absent from the listing — and a group ARN naming a Region other than the one the request addressed does not resolve.

The effect is both a false conflict (step 1 blocks a legitimate create) and state leakage (a caller scoped to one Region observes and can delete another Region's groups).

## Evidence

Spec: the model's own documentation for `CreateGroupInput$Name` states "A resource group name must be unique within each Amazon Web Services Region in your Amazon Web Services account" (reproduced in `src/ops/create_group.rs` header, lines ~75-81); every group identifier the service models is Region-qualified (`GroupArn` pattern requires `[a-z]{2}(-[a-z]+)+-\d{1}`), and `resource-groups` is a Regional endpoint (`endpointPrefix: resource-groups`, per-Region signing scope).

Handler/state: `src/state.rs` declares `pub groups: BTreeMap<String, GroupRecord>` keyed by `GroupRecord::name` only; `lookup()` matches a bare name with no Region check and matches an ARN only against the stored ARN. `src/ops/create_group.rs` refuses a duplicate purely on `store.groups.contains_key(&input.name)` (no Region in the key) while reading the Region only to mint the ARN (`crate::state::request_region(ctx.parts())`). `src/ops/get_group.rs`, `get_group_query.rs`, `get_group_configuration.rs`, `update_group.rs`, `delete_group.rs`, `group_resources.rs`, `list_group_resources.rs`, `list_grouping_statuses.rs` all ignore `ctx` for Region (`let _ = ctx;` in the readers) and resolve through the global map; `src/ops/list_groups.rs` iterates `store.groups.values()` with no Region predicate.

Tests: `tests/all/resource_group_lifecycle.rs::a_group_arn_carries_the_scope_region_and_the_emulator_account` (line ~314) explicitly asserts that a group created under one credential-scope Region is readable with a `eu-central-1` scope, i.e. the suite pins the global behaviour; `names_are_unique_and_the_reserved_prefix_is_refused` (line ~358) asserts name uniqueness without any Region dimension. No test covers per-Region isolation or same-name-in-two-Regions.

## Steps to Reproduce

1. Sign a request for us-east-1 and POST /groups with {"Name":"demo"} — 200, GroupArn carries us-east-1.
2. Sign a request for eu-west-1 and POST /groups with {"Name":"demo"} — observe 400 BadRequestException "Cannot create group: group already exists"; expected 200 with an eu-west-1 group of the same name.
3. Sign a request for eu-west-1 and POST /get-group with {"Group":"demo"} — observe 200 returning the us-east-1 group (its GroupArn names us-east-1); expected 404 NotFoundException.
4. Sign a request for eu-west-1 and POST /groups-list with {} — observe the us-east-1 group in GroupIdentifiers/Groups; expected an empty listing.
5. Sign a request for eu-west-1 and POST /delete-group with {"Group":"demo"} — observe 200 deleting another Region's group; expected 404 with the us-east-1 group untouched.
