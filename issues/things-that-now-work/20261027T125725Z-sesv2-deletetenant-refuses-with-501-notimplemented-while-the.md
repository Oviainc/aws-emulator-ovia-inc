Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree|run#21|AssertionError|assert delete_error is None, (
Issue-Commit: 16c20ea7d22d9314dc8611d90389344009516a2f

# sesv2 DeleteTenant refuses with 501 NotImplemented while the tenant holds resource associations; AWS deletes the tenant unconditionally and leaves the resource-keyed tenant index dangling

> _Note: this issue was identified against the code base at commit `16c20ea7d22d9314dc8611d90389344009516a2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTenant

## Description
One cause.

Cause 1 of 1: DeleteTenant is unreachable, so the tenant/resource association state machine cannot be exercised past the delete leg. Failing check: sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree.

AWS: Smithy (com.amazonaws.sesv2, SimpleEmailService_v2) models DeleteTenant as POST /v2/email/tenants/delete, input `TenantName` (target TenantName, required, @length min 1), empty output shape DeleteTenantResponse, success status 200, and the modeled errors BadRequestException (HTTP 400), NotFoundException (HTTP 404) and TooManyRequestsException (HTTP 429). The operation's own model documentation states: "Delete an existing tenant. When you delete a tenant, its associations with resources are removed, but the resources themselves are not deleted." So a tenant that still holds associations is NOT a refusal case: there is no modeled conflict/in-use error for DeleteTenant at all, and the delete succeeds with 200 and an empty body. Recorded behavior (this check) further pins the post-delete views: the tenant-keyed forward index is torn down with the tenant — GetTenant(TenantName=<deleted>) and ListTenantResources(TenantName=<deleted>) both refuse with error code `NotFoundException` (HTTP 404) — while the resource-keyed reverse index is NOT cascaded: ListResourceTenants(ResourceArn=<an ARN the deleted tenant held>) still returns a ResourceTenants entry whose `TenantName` is the deleted tenant. The two views are therefore deliberately NOT inverses after a delete.

Emulator: every DeleteTenant call is refused with HTTP 501 and error code `NotImplemented`, message "operation DeleteTenant is scaffolded but not implemented in this emulator" (fixed text; the variable part is the operation name `DeleteTenant`). The handler is an unmodified scaffold: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/delete_tenant.rs returns Err(crate::scaffolded::Scaffolded::not_implemented("DeleteTenant")) and still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker. Nothing is persisted, so no tenant is ever removed. The legs before the delete passed in this check: CreateTenant, CreateConfigurationSet, CreateTenantResourceAssociation for two tenants sharing one configuration-set ARN, DeleteTenantResourceAssociation and both list views all behaved as AWS does — the divergence is confined to the delete leg and its aftermath.

Why: a client that provisions a tenant and later tears it down can never complete the teardown — DeleteTenant is a hard 501, which botocore surfaces as ClientError(NotImplemented) rather than any modeled SES error, so retry/branching logic written against BadRequestException/NotFoundException/TooManyRequestsException never matches. Tenants accumulate forever, a tenant name can never be reused, and cleanup paths that call DeleteTenant on a tenant still holding associations (the normal case — AWS does not require you to disassociate first) fail where AWS succeeds. Because the operation never runs, the emulator also cannot reproduce AWS's asymmetric index teardown, so code that reads ListResourceTenants after a delete (and must tolerate a dangling tenant name there, while treating GetTenant/ListTenantResources 404 as authoritative) is never exercised and will be written wrong against the emulator.

Fix: implement the DeleteTenant handler and the tenant state it owns. The handler must (a) resolve TenantName in the service's tenant store and return NotFoundException (404) when it is absent, (b) otherwise remove the tenant and its entries from the tenant-keyed association index unconditionally — no in-use/conflict refusal, the resources themselves untouched — and return 200 with an empty body, and (c) leave the resource-keyed reverse index entries in place so ListResourceTenants still names the deleted tenant. GetTenant and ListTenantResources must then resolve the deleted name to NotFoundException. This is the same stub named by the already-open issue 20261027T083802Z-sesv2-deletetenant-stub.md, whose Issue-Trace-Ids carries only sesv2.tenant-create-get-list-delete-lifecycle and therefore does not cover this check; that issue asks only that the stub be implemented, while this one additionally states the post-delete index behavior (tenant-keyed views 404, resource-keyed view keeps the dangling tenant) that this check asserts. Fix both together and re-replay both check ids.

Traces: sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree (assertion: "DeleteTenant(<tenant>) was refused with 'NotImplemented' while it still held ['arn:aws:ses:<region>:<account>:configuration-set/<name>']; expected it to succeed unconditionally"); handler at services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/delete_tenant.rs:151.

## Evidence
Expected (AWS): DeleteTenant(TenantName=<tenant that still holds >=1 resource association>) -> HTTP 200, empty body, tenant deleted and its associations removed (resources themselves kept). Then GetTenant(TenantName=<tenant>) -> HTTP 404 error code NotFoundException; ListTenantResources(TenantName=<tenant>) -> HTTP 404 error code NotFoundException; ListResourceTenants(ResourceArn=<ARN the tenant held>) -> HTTP 200 whose ResourceTenants still contains an entry with TenantName=<deleted tenant>.

Actual (emulator): DeleteTenant -> HTTP 501, error code `NotImplemented`, message "operation DeleteTenant is scaffolded but not implemented in this emulator". The check fails at check_sesv2_a_resource_belongs_to_at_most_one_tenant_and_both_views_agree.py line 76 with: AssertionError: DeleteTenant(sim2real-behav-t2-459d424b4869) was refused with 'NotImplemented' while it still held ['arn:aws:ses:us-west-2:000000000000:configuration-set/sim2real-behav-control-c1e31014896f']; expected it to succeed unconditionally. (The tenant name, configuration-set name, region and account in that line are per-run values from the request/emulator state, not part of the requirement; the requirement is the 200-with-empty-body success and the subsequent NotFoundException/dangling-reverse-index behavior.) Source: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/delete_tenant.rs:151 returns Scaffolded::not_implemented("DeleteTenant").

## Steps to Reproduce
1. Boot the sesv2 emulator.
2. CreateTenant(TenantName=T1) and CreateTenant(TenantName=T2).
3. CreateConfigurationSet(ConfigurationSetName=CS) and form its ARN arn:aws:ses:<region>:<account>:configuration-set/CS.
4. CreateTenantResourceAssociation(TenantName=T2, ResourceArn=<CS ARN>) and confirm ListTenantResources(TenantName=T2) reports it.
5. Call DeleteTenant(TenantName=T2) while that association is still held. Observe HTTP 501 / error code NotImplemented instead of HTTP 200 with an empty body.
6. (Blocked by step 5 today) Call GetTenant(TenantName=T2) and ListTenantResources(TenantName=T2) and expect error code NotFoundException; call ListResourceTenants(ResourceArn=<CS ARN>) and expect ResourceTenants to still contain TenantName=T2.
7. Replay sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree to confirm.
