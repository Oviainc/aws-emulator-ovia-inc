Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree|run#26|AssertionError|assert second_tenant in owners, (
Issue-Commit: 6b193e67e38a36506ad31c413899507363d5dc88

# sesv2 DeleteTenant cascades the resource-keyed tenant index: ListResourceTenants no longer names the deleted tenant, where AWS leaves the dangling reference

> _Note: this issue was identified against the code base at commit `6b193e67e38a36506ad31c413899507363d5dc88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTenant

## Description
One cause.

Cause 1 of 1: DeleteTenant removes the association rows that back BOTH tenant/resource views, so after the delete ListResourceTenants(ResourceArn) reports no tenants at all; AWS tears down only the tenant-keyed forward index and leaves the resource-keyed reverse index naming the deleted tenant. Failing check: sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree.

AWS: Smithy (com.amazonaws.sesv2#SimpleEmailService_v2) models DeleteTenant as POST /v2/email/tenants/delete, input `TenantName` (target TenantName, required, @length min 1), empty output DeleteTenantResponse, success 200, modeled errors BadRequestException (400), NotFoundException (404), TooManyRequestsException (429); its model documentation says "Delete an existing tenant. When you delete a tenant, its associations with resources are removed, but the resources themselves are not deleted." ListResourceTenants is modeled as POST /v2/email/resources/tenants/list, input `ResourceArn` (required, @length min 1) + PageSize/NextToken, output `ResourceTenants` (ResourceTenantMetadataList) + NextToken, paginated. The model does not state what the two views do after a delete, so this leg is recorded behavior only: the recording shows that after DeleteTenant on a tenant that still held an association, the tenant-keyed views refuse — GetTenant(TenantName=<deleted>) and ListTenantResources(TenantName=<deleted>) both answer HTTP 404 error code `NotFoundException` — while the resource-keyed view does NOT cascade: ListResourceTenants(ResourceArn=<an ARN the deleted tenant held>) still answers HTTP 200 with a ResourceTenants entry whose `TenantName` is the deleted tenant. The two views are deliberately not inverses after a delete; the reverse-index reference is left dangling.

Emulator: the delete leg itself now behaves (200, empty body, no in-use refusal) and the tenant-keyed views correctly 404 the deleted name, but the handler also cascades the reverse index: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/delete_tenant.rs drops every association row of the deleted tenant (`store.tenant_resource_associations.retain(|_, association| association.tenant_name != tenant_name)`), and ListResourceTenants reads that very same map keyed by (ResourceArn, TenantName) (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/list_resource_tenants.rs), so it answers 200 with an EMPTY `ResourceTenants` list for the ARN the deleted tenant held. The check fails on its last leg with AssertionError: "ListResourceTenants(<configuration-set ARN>) no longer names deleted tenant <tenant> among [] — expected the dangling reference real AWS leaves in the resource-keyed index" (the ARN, tenant name, region and account in that line are per-run values from the request and emulator state; the requirement is that the deleted tenant name is still present in ResourceTenants).

Why: a client that reads the two indices after a tenant teardown gets the opposite model of SES's state. Code written against AWS must treat GetTenant/ListTenantResources 404 as authoritative and tolerate (or itself garbage-collect) a tenant name that ListResourceTenants still reports for a resource; against this emulator that reconciliation path never fires, so the bug — and any cleanup or audit logic that depends on spotting the dangling owner on the resource side — is only discovered on real AWS. Conversely, an inventory that enumerates resource owners via ListResourceTenants sees a resource silently lose an owner entry on the emulator where AWS keeps it.

Fix: the DeleteTenant handler and the tenant/association state it owns. Tenant deletion must remove the tenant and its tenant-keyed association entries (so GetTenant and ListTenantResources resolve the deleted name to NotFoundException) without removing the resource-keyed entries ListResourceTenants serves — i.e. the single (ResourceArn, TenantName) association map that currently backs both views must be split, or the row retained and filtered only on the tenant-keyed read path. Other tenants' associations on the same resource, and the resources themselves, stay untouched as they do today. Note the already-open issue 20261027T125725Z-sesv2-deletetenant-refuses-with-501-notimplemented-while-the.md: it was filed when DeleteTenant answered 501 and its replay key is the delete-leg assertion; the refusal is gone, but the post-delete reverse-index behavior it also described is still wrong and is what this check now fails on.

Traces: sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree (final leg, `assert second_tenant in owners`); cascade at services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/delete_tenant.rs:178-180; reverse-index read at services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/list_resource_tenants.rs:191-205.

## Evidence
Expected (AWS): with tenant T2 associated to configuration-set ARN A, DeleteTenant(TenantName=T2) -> HTTP 200 empty body; then GetTenant(TenantName=T2) -> HTTP 404 `NotFoundException`; ListTenantResources(TenantName=T2) -> HTTP 404 `NotFoundException`; ListResourceTenants(ResourceArn=A) -> HTTP 200 whose `ResourceTenants` STILL contains an entry with TenantName=T2 (the dangling reverse-index reference).

Actual (emulator): DeleteTenant -> HTTP 200 (correct), GetTenant/ListTenantResources -> 404 NotFoundException (correct), but ListResourceTenants(ResourceArn=A) -> HTTP 200 with `ResourceTenants: []`. Replay failure: check_sesv2_a_resource_belongs_to_at_most_one_tenant_and_both_views_agree.py line 93 — "AssertionError: ListResourceTenants(arn:aws:ses:us-west-2:000000000000:configuration-set/sim2real-behav-control-<id>) no longer names deleted tenant sim2real-behav-t2-<id> among [] — expected the dangling reference real AWS leaves in the resource-keyed index". Fixed text of the requirement is the presence of the deleted tenant name in ResourceTenants; the ARN, tenant name, account and region are per-run values. Source: delete_tenant.rs retains out every association whose `tenant_name` equals the deleted tenant, and list_resource_tenants.rs serves its page from that same `tenant_resource_associations` map.

## Steps to Reproduce
1. Boot the sesv2 emulator.
2. CreateTenant(TenantName=T1); CreateTenant(TenantName=T2).
3. CreateConfigurationSet(ConfigurationSetName=CS); form A = arn:aws:ses:<region>:<account>:configuration-set/CS.
4. CreateTenantResourceAssociation(TenantName=T2, ResourceArn=A); confirm ListTenantResources(TenantName=T2) reports A and ListResourceTenants(ResourceArn=A) names T2.
5. DeleteTenant(TenantName=T2) -> 200.
6. GetTenant(TenantName=T2) and ListTenantResources(TenantName=T2) -> 404 NotFoundException (as AWS).
7. ListResourceTenants(ResourceArn=A): observe `ResourceTenants: []`; AWS still returns an entry with TenantName=T2.
8. Replay sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree to confirm.
