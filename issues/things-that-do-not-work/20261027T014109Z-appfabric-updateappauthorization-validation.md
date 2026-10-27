Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: bed9f8e2d85c054efb957ff95014081869f003aa

# UpdateAppAuthorization re-points an authorization's tenant without checking the ingestion associated with it, orphaning the ingestion and bypassing DeleteAppAuthorization's documented precondition

_Filed: 2026-10-27T01:41:09Z_
_Operation: UpdateAppAuthorization | Class: validation_

> _Note: this issue was identified against the code base at commit `bed9f8e2d85c054efb957ff95014081869f003aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate ties an ingestion to an app authorization by the (application, tenant) pair: `CreateIngestion` refuses a pair the bundle holds no CONNECTED authorization for, and `DeleteAppAuthorization` refuses to delete an authorization while the bundle still holds an ingestion for that same pair (it calls `bundle.ingestion_for(&app, &tenant)` for exactly that). `UpdateAppAuthorization` owns the `tenant` member and rewrites `tenant_identifier` in place, but it never consults `ingestion_for` — it only runs the sibling collision check `authorization_for`. So a PATCH that moves a connected authorization from `tenant-a` to `tenant-b` silently breaks the association: the ingestion for (app, tenant-a) is left in the store with no app authorization at all — a state `CreateIngestion` would have refused to create — and the authorization, whose pair now matches no ingestion, becomes deletable, so the precondition the model documents for `DeleteAppAuthorization` ("You must delete the associated ingestion before you can delete an app authorization") is bypassed without the ingestion ever being deleted. One handler in this crate calls the helper that enforces the invariant; the handler that breaks the invariant does not.

## Evidence

Invariant side — services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/delete_app_authorization.rs:216-228: "// The precondition the operation documents: \"You must delete the associated ingestion before you can delete an app authorization.\"" followed by `if let Some(ingestion) = bundle.ingestion_for(&app, &tenant) { return Err(...precondition_not_met(...)) }`; and its module header at delete_app_authorization.rs:21-25: "an authorization whose (application, tenant) pair the bundle still holds an ingestion for is refused ... That pair is what ties the two together: an ingestion exists only for a pair whose app authorization has completed the connect handshake".
State helper — src/state.rs:319-331: "The pair is what ties an ingestion to an app authorization: an ingestion is created for an (`app`, `tenantId`) pair and only for a pair whose authorization has completed the connect handshake ... so the ingestion this answers is the one ASSOCIATED with that authorization — what a delete of the authorization has to find gone first." (`pub fn ingestion_for`).
Breaking side — src/ops/update_app_authorization.rs:237-270: the only cross-record check taken is `if let Some(held) = bundle.authorization_for(&app, &tenant.tenant_identifier)` (the at-most-one-per-pair check), then `record.tenant_identifier = tenant.tenant_identifier; record.tenant_display_name = tenant.tenant_display_name;` — `ingestion_for` is never called (grep over the crate shows `ingestion_for` used only in delete_app_authorization.rs:218).
Create side — src/ops/create_ingestion.rs:250-262 refuses a pair with no connected authorization: `if parent.and_then(|bundle| bundle.connected_authorization(&input.app, &input.tenant_id)).is_none() { return Err(...app_authorization_not_connected(...)) }`.
Test coverage: tests/all/update_app_authorization_contract.rs exercises tenant moves (the `tenant only` / `a pair another authorization holds` tables at lines 325-342 and 744-791) but never creates an ingestion for the authorization under test, so no existing test pins the current behaviour; tests/all/app_authorization_deletion.rs contains no PATCH at all (grep for PATCH/update: no matches).

## Steps to Reproduce

1. POST /appbundles -> note appBundle.arn
2. POST /appbundles/{bundle}/appauthorizations with app=SLACK, authType=apiKey, credential.apiKeyCredential.apiKey=k, tenant={tenantIdentifier:tenant-a,tenantDisplayName:a} -> 201, note appAuthorization.appAuthorizationArn
3. POST /appbundles/{bundle}/appauthorizations/{auth}/connect -> 200 (status Connected)
4. POST /appbundles/{bundle}/ingestions with app=SLACK, tenantId=tenant-a, ingestionType=auditLog -> 201
5. PATCH /appbundles/{bundle}/appauthorizations/{auth} with {"tenant":{"tenantIdentifier":"tenant-b","tenantDisplayName":"b"}} -> 200 (accepted, no check against the associated ingestion)
6. DELETE /appbundles/{bundle}/appauthorizations/{auth} -> 204, even though the ingestion created in step 4 still exists
7. GET /appbundles/{bundle}/ingestions -> the ingestion for (SLACK, tenant-a) is still listed while the bundle holds no app authorization for that pair — a state CreateIngestion refuses to create
