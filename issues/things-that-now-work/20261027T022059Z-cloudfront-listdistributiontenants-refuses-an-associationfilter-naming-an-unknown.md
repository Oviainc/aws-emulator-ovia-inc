Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.list-distribution-tenants-entity-not-found-for-invalid-connection-group
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|ListDistributionTenants|InvalidArgument
Issue-Commit: 96aab6874df7daa64c84ed9b117b617756506559

# cloudfront ListDistributionTenants refuses an AssociationFilter naming an unknown connection group with InvalidArgument instead of returning an empty page

> _Note: this issue was identified against the code base at commit `96aab6874df7daa64c84ed9b117b617756506559`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDistributionTenants

## Description
REOPENS 20261023T140640Z-cloudfront-listdistributiontenants-stub.md: its fix left the call refused.

Cause 1 of 1: ListDistributionTenants validates AssociationFilter ids against stored resources and refuses the call, where AWS does not validate the filter at all and simply serves an empty page.
Failing check: cloudfront.list-distribution-tenants-entity-not-found-for-invalid-connection-group

AWS: Smithy (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json): ListDistributionTenants is POST /2020-05-31/distribution-tenants with success code 200; input member AssociationFilter targets DistributionTenantAssociationFilter, whose members DistributionId and ConnectionGroupId are plain `string` with NO length, pattern or enum constraint and no documented existence requirement ("The ID of the connection group to filter by. You can find distribution tenants associated with a specific connection group."). Output DistributionTenantList is a plain list. The operation does model InvalidArgument (400) and EntityNotFound (404), but neither is documented as the answer to a filter id that resolves to nothing. Recorded behavior only for the filter semantics: the check records that on real AWS (confirmed 2026-09-16 against a leased account) BOTH a ConnectionGroupId that never existed AND a real, freshly created, still tenant-less connection group id return HTTP 200 with DistributionTenantList == [], and no error is ever raised. So AWS treats an unresolvable AssociationFilter id as an ordinary filter that matches nothing, not as a bad argument.

Emulator: the call is refused. Replay output: botocore.errorfactory.InvalidArgument: An error occurred (InvalidArgument) when calling the ListDistributionTenants operation: The association filter id is invalid. That is HTTP 400, error code `InvalidArgument`, message `The association filter id is invalid.` — the message is entirely fixed text with no variable part, so it does not even name the offending id. AWS's answer for this request class carries no error at all: the required response is HTTP 200 with an empty DistributionTenantList (and no NextMarker). The divergence is therefore the whole response: 200/empty-list vs 400/InvalidArgument/"The association filter id is invalid.".

Why: a client enumerating the tenants of a connection group cannot distinguish "no tenants" from "bad request". Code that pages tenants per connection group — a drift detector, a teardown sweep, a dashboard that lists tenants for each connection group it knows — gets a ClientError exception instead of an empty page for every connection group that is unknown to, or lagging in, the emulator's store, so it aborts or reports a spurious configuration fault where AWS would have returned cleanly. It also makes a filter over a legitimately empty connection group indistinguishable from a typo, and any retry/backoff wrapper keyed on 4xx-vs-200 takes the wrong branch.

Fix: the operation's validation inside the handler, services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/list_distribution_tenants.rs. The handler computes an `unresolvable` predicate from `store.distribution(id).is_none()` / `!store.holds_connection_group(id)` and returns InvalidArgument from it; the existence precondition does not belong in this listing at all — the filter values should feed only the per-tenant match that follows, so an id matching no stored tenant yields an empty page. Note the same predicate also covers the AssociationFilter.DistributionId branch; the replayed check only exercises the ConnectionGroupId branch, so for DistributionId this is Hypothesis: the identical over-validation refuses an unknown distribution id the same way and should be removed with it. The MaxItems and Marker validation in the same handler is not implicated by this check and should be left alone.

Traces: cloudfront.list-distribution-tenants-entity-not-found-for-invalid-connection-group (the check's docstring records the corrected AWS contract; its own id still names the disproven EntityNotFound premise). Emulator code: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/list_distribution_tenants.rs lines 182-193 (the `unresolvable` check and the `crate::state::invalid_association_filter()` refusal).

## Evidence
Expected (real AWS, recorded 2026-09-16): ListDistributionTenants with AssociationFilter={"ConnectionGroupId": "<id that never existed>"} -> HTTP 200, DistributionTenantList == []. The same call with a real, freshly created, tenant-less connection group id -> HTTP 200, DistributionTenantList == []. No ClientError in either case.

Actual (emulator): the first call is refused —
  File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.InvalidArgument: An error occurred (InvalidArgument) when calling the ListDistributionTenants operation: The association filter id is invalid.
HTTP status 400, code `InvalidArgument`, message `The association filter id is invalid.` (all fixed text; no request- or state-derived values). The check never reaches its second assertion.

Root cause in the checkout (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/list_distribution_tenants.rs):
  let unresolvable = distribution_filter.is_some_and(|id| store.distribution(id).is_none())
      || connection_group_filter.is_some_and(|id| !store.holds_connection_group(id));
  if unresolvable { return Err(... InvalidArgument(crate::state::invalid_association_filter()) ...); }

Smithy model: DistributionTenantAssociationFilter.ConnectionGroupId is an unconstrained `string`; ListDistributionTenants success code is 200; no modeled error is documented for an unresolvable filter id.

This operation was previously filed as an unimplemented scaffold stub (20261023T140640Z-cloudfront-listdistributiontenants-stub.md, now marked fixed); that implementation added the filter existence check, so the call is still refused for this request class.

## Steps to Reproduce
1. Boot the cloudfront emulator from this checkout.
2. Create a connection group: CreateConnectionGroup(Name=<unique>, Enabled=true) and keep its ConnectionGroup.Id.
3. Call ListDistributionTenants(AssociationFilter={"ConnectionGroupId": "<an id that was never created>"}).
   Observe HTTP 400 InvalidArgument "The association filter id is invalid."
   Expected: HTTP 200 with DistributionTenantList == [] and no NextMarker.
4. Call ListDistributionTenants(AssociationFilter={"ConnectionGroupId": "<the id from step 2>"}).
   Expected (and required): HTTP 200 with DistributionTenantList == [] — the same shape as step 3, proving the empty result is ordinary filtering.
5. Re-replay the behavioral check cloudfront.list-distribution-tenants-entity-not-found-for-invalid-connection-group to confirm both calls return 200 with an empty list.
