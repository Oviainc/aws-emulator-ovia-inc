Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.tenant-resource-association-unknown-tenant-rejected
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.tenant-resource-association-unknown-tenant-rejected|run#9|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Commit: 388872f1987f74fe9682288e62ee8e18a6dfb8b9

# sesv2 CreateTenantResourceAssociation answers 501 NotImplemented instead of 404 NotFoundException for an unknown tenant, and the tenant-resource association surface is unusable

> _Note: this issue was identified against the code base at commit `388872f1987f74fe9682288e62ee8e18a6dfb8b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTenantResourceAssociation

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: SESv2 `CreateTenantResourceAssociation` never performs tenant resolution — it refuses every call, including the negative case, with a generic 501 `NotImplemented`, so the modeled rejection for a non-existent tenant is unreachable and the association surface has no behavior at all.

AWS: Smithy model `services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json` binds `CreateTenantResourceAssociation` to `POST /v2/email/tenants/resources` with success code 200, input members `TenantName` (required, @length min 1) and `ResourceArn` (required, @length min 1), and models the errors `AlreadyExistsException` (400), `BadRequestException` (400), `NotFoundException` (404) and `TooManyRequestsException` (429). When `TenantName` names a tenant that does not exist in the account, real AWS refuses the request with HTTP 404 and `Error.Code` = `NotFoundException`. Recorded behavior for the message: the fixed text is `The requested tenant <` + `> does not exist.`, i.e. `The requested tenant <{TenantName}> does not exist.`, where the only variable part is the tenant name copied verbatim from the request's `TenantName` (the recording's value is a generated `sim2real-tenant-unknown-...` name and is NOT part of the requirement). The refusal has no effect: `ListResourceTenants` for the target resource ARN still returns an empty `ResourceTenants` list, and a subsequently created real tenant can be associated with that same resource ARN and then appears in `ListResourceTenants`.

Emulator: the call is refused with `Error.Code` = `NotImplemented` (the scaffold stub refusal, HTTP 501) and the message `operation CreateTenantResourceAssociation is scaffolded but not implemented in this emulator`. The replay asserts on the error code and fails with `AssertionError: expected NotFoundException, got 'NotImplemented'`. Because the operation never runs, the positive leg of the check (associate the same configuration-set ARN with a real tenant, then observe it in `ListResourceTenants`) is unreachable as well.

Why: a client cannot tell "this tenant does not exist" from "the service is broken". `NotFoundException` is a modeled, retry-never client error that SDKs and application code branch on (create-the-tenant-then-retry, or surface a 404 to the caller); `NotImplemented` is an unmodeled server-class fault, so botocore raises a `ClientError` with a code no `CreateTenantResourceAssociation` error handler matches, and retry/backoff logic treats it as a transient server failure. Multi-tenant SESv2 provisioning flows — create tenant, associate configuration sets / email identities with it, list a resource's tenants — cannot run at all against the emulator.

Fix: implement the operation handler `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_tenant_resource_association.rs`: resolve `TenantName` against the tenant store before any mutation and, when it is absent, return the modeled `NotFoundException` (HTTP 404) with the message templated from the request's `TenantName`; when the tenant exists, record the association in the service state so `ListResourceTenants` for that `ResourceArn` reports it, and return the empty 200 `CreateTenantResourceAssociationResponse`. The existing-association case must return the modeled `AlreadyExistsException` rather than a second row.

Related open issue, which does NOT cover this: `20261025T214624Z-sesv2-7-net-new-behavioral-divergences-listemailidentities-unreachable.md` groups other SESv2 fidelity gaps but does not mention `CreateTenantResourceAssociation`, tenant resolution, or the `NotFoundException` tenant refusal.

Traces: failing check `sesv2.tenant-resource-association-unknown-tenant-rejected` (verdict: mismatch), group `error-semantics`, tags `axis:rejection`, `concern:negative-classes`, `plane:control`.

## Evidence
Check: sesv2.tenant-resource-association-unknown-tenant-rejected (mismatch)

Replay output:
  File "check_sesv2_tenant_resource_association_unknown_tenant_rejected.py", line 34, in run
    assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
  AssertionError: expected NotFoundException, got 'NotImplemented'

Expected (real AWS, per the recorded check and the Smithy model):
  HTTP 404
  Error.Code    = NotFoundException
  Error.Message = "The requested tenant <{TenantName}> does not exist."
                  fixed text: "The requested tenant <" ... "> does not exist."
                  variable part: {TenantName}, copied from the request
  Effect: none — ListResourceTenants(ResourceArn=<cfg-set arn>) returns ResourceTenants == []
  Positive control: CreateTenant(<real>) then CreateTenantResourceAssociation(TenantName=<real>,
                   ResourceArn=<same arn>) succeeds (200, empty body) and <real> then appears in
                   ListResourceTenants(ResourceArn=<same arn>).

Actual (emulator):
  HTTP 501
  Error.Code    = NotImplemented
  Error.Message = "operation CreateTenantResourceAssociation is scaffolded but not implemented in this emulator"
  Effect: the operation has no implementation, so neither the negative refusal nor the positive
          association path exists; the rest of the check is unreachable.

Smithy citation: CreateTenantResourceAssociation — http POST /v2/email/tenants/resources, code 200;
input TenantName (required, @length min 1), ResourceArn (required, @length min 1);
errors AlreadyExistsException(400), BadRequestException(400), NotFoundException(404),
TooManyRequestsException(429).

## Steps to Reproduce
1. Point an SESv2 client at the emulator in the target region.
2. sesv2.create_configuration_set(ConfigurationSetName=<cs>) and build its ARN:
   arn:aws:ses:<region>:<account>:configuration-set/<cs>
3. Call sesv2.create_tenant_resource_association(TenantName=<a-tenant-name-that-was-never-created>,
   ResourceArn=<that arn>).
   Expected: ClientError with HTTP 404, Error.Code == "NotFoundException" and
   Error.Message == "The requested tenant <" + <that tenant name> + "> does not exist."
   Actual:   ClientError with HTTP 501 and Error.Code == "NotImplemented".
4. Call sesv2.list_resource_tenants(ResourceArn=<that arn>) and assert ResourceTenants == []
   (the refused call must leave no association).
5. Positive control: sesv2.create_tenant(TenantName=<real>), then
   sesv2.create_tenant_resource_association(TenantName=<real>, ResourceArn=<that arn>) — expected to
   return 200 with an empty body — then sesv2.list_resource_tenants(ResourceArn=<that arn>) and
   assert <real> is among the returned TenantName values.
6. Equivalent: replay the behavioral check sesv2.tenant-resource-association-unknown-tenant-rejected.
