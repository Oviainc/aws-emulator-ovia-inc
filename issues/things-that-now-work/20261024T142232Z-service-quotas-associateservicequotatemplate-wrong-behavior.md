Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 409cf01b27185a74633e00014402d3a564807f11

# The quota request template plane is unreachable: the organization gate is hardcoded to refuse and no template state exists

_Filed: 2026-10-24T14:22:32Z_
_Operation: AssociateServiceQuotaTemplate | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `409cf01b27185a74633e00014402d3a564807f11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All seven quota-request-template operations are implemented as an unconditional refusal, so none of their modeled success outputs can ever be observed and no template state is ever stored.

Actual (emulator): `AssociateServiceQuotaTemplate`, `DisassociateServiceQuotaTemplate`, `GetAssociationForServiceQuotaTemplate`, `PutServiceQuotaIncreaseRequestIntoTemplate`, `GetServiceQuotaIncreaseRequestFromTemplate`, `DeleteServiceQuotaIncreaseRequestFromTemplate` and `ListServiceQuotaIncreaseRequestsInTemplate` each end in `Err(template_refusal(&region).into())` and bind their input with `let _ = (&input, &self.engine);`. `crate::state::template_refusal` is a pure function of the region: `us-east-1` -> `NoAvailableOrganizationException`, every other region -> `TemplatesNotAvailableInRegionException`. There is no template store in `src/state.rs` at all.

Expected (AWS): the refusal is right only for as long as the calling account belongs to no AWS Organization. Once the account is the management account of an all-features organization with Service Quotas service access enabled, AWS serves the plane at its `us-east-1` home Region: `PutServiceQuotaIncreaseRequestIntoTemplate` stores a `ServiceQuotaIncreaseRequestInTemplate` (`ServiceCode`, `ServiceName`, `QuotaCode`, `QuotaName`, `DesiredValue`, `AwsRegion`, `Unit`, `GlobalQuota`) and returns it; `GetServiceQuotaIncreaseRequestFromTemplate` reads that same entry back and answers `NoSuchResourceException` for a `(ServiceCode, QuotaCode, AwsRegion)` triple that has none; `ListServiceQuotaIncreaseRequestsInTemplate` enumerates the entries, filtered by the optional `ServiceCode`/`AwsRegion` and paginated by `MaxResults`/`NextToken`; `DeleteServiceQuotaIncreaseRequestFromTemplate` removes one entry so the later read 404s; `AssociateServiceQuotaTemplate` flips the association so `GetAssociationForServiceQuotaTemplate` reports `ASSOCIATED`; `DisassociateServiceQuotaTemplate` flips it back (and answers the modeled `ServiceQuotaTemplateNotInUseException` when nothing is associated). None of that is reachable here, so a client exercising the organization-wide template feature sees a refusal forever and seven of this service's 26 operations can never return 2xx.

The gate's own premise is what makes this a divergence rather than a modelling choice: `src/state.rs` justifies the unconditional refusal with "Service Quotas models no organization membership and has no operation that could create or join one (AWS Organizations is a different service, with its own state in its own crate)" — but this repo ships a working AWS Organizations emulator whose `CreateOrganization`, `EnableAllFeatures` and `EnableAWSServiceAccess` handlers are implemented (not scaffolds), so a caller CAN put the emulated account into an all-features organization and AWS would then serve the template plane. Keep the standalone-account refusal (it is trace-verified); make it conditional on the organization state and implement the template store behind it.

## Evidence

Model: the seven operations are bound to the service with success outputs that only a real template store can produce — `ServiceQuotaIncreaseRequestInTemplate` (smithy_query shape) on Put/Get, `ServiceQuotaIncreaseRequestInTemplateList` + `NextToken` on the `@paginated` List, and `ServiceQuotaTemplateAssociationStatus` (`ASSOCIATED | DISASSOCIATED`) on `GetAssociationForServiceQuotaTemplate`; `DisassociateServiceQuotaTemplate` models `ServiceQuotaTemplateNotInUseException` and `PutServiceQuotaIncreaseRequestIntoTemplate` models `QuotaExceededException`/`NoSuchResourceException`, error paths that presuppose template state (OPERATIONS.md lines 27-55, 87-119, 141-145). `AssociateServiceQuotaTemplate`'s own doc: "Associates your quota request template with your organization. When a new Amazon Web Services account is created in your organization, the quota increase requests in the template are automatically applied to the account."

Handlers: `src/ops/associate_service_quota_template.rs` lines 136-143 (`let _ = (&input, &self.engine); Err(template_refusal(&region).into())`) and the identical tail in `disassociate_service_quota_template.rs:142`, `get_association_for_service_quota_template.rs:147`, `put_service_quota_increase_request_into_template.rs:162`, `get_service_quota_increase_request_from_template.rs:157`, `delete_service_quota_increase_request_from_template.rs:152`, `list_service_quota_increase_requests_in_template.rs:169`. `crate::state::template_refusal` (src/state.rs ~lines 1252-1279) decides on the region alone and its doc states no organization can ever exist here.

Cross-service premise is false in this checkout: `services/service-com-amazonaws-organizations-awsorganizationsv20161128/src/ops/` contains non-stub `create_organization.rs`, `enable_all_features.rs` and `enable_aws_service_access.rs` (the only scaffolds in that crate are update_organizational_unit, list_aws_service_access_for_organization, invite_account_to_organization, invite_organization_to_transfer_responsibility, list_handshakes_for_organization, remove_account_from_organization).

Test coverage: `tests/all/support_case_and_templates.rs` `every_template_operation_is_refused_by_the_template_gate` (line 402, rows at 446-450) pins exactly the unconditional refusal, so those assertions must move with the fix; nothing in the suite asserts any template success path, and no existing issue covers it — the fixed issue `20261022T124557Z-service-quotas-ignores-supportcaseallowed-false-...` cause 3 only asked for the standalone-account refusal to replace a 501, which is what is implemented now.

## Steps to Reproduce

1. Boot the emulator and POST `X-Amz-Target: ServiceQuotasV20190624.PutServiceQuotaIncreaseRequestIntoTemplate` at the us-east-1 endpoint with `{"ServiceCode":"vpc","QuotaCode":"L-F678F1CE","AwsRegion":"us-east-1","DesiredValue":1000}` -> HTTP 400 `NoAvailableOrganizationException`; nothing is stored.
2. Through the repo's Organizations emulator, POST `AWSOrganizationsV20161128.CreateOrganization` `{"FeatureSet":"ALL"}` (and `EnableAWSServiceAccess` for `servicequotas.amazonaws.com`) so the emulated account is the management account of an all-features organization.
3. Repeat step 1 -> still HTTP 400 `NoAvailableOrganizationException`, where AWS now returns 200 with the stored `ServiceQuotaIncreaseRequestInTemplate`.
4. POST `AssociateServiceQuotaTemplate` `{}` then `GetAssociationForServiceQuotaTemplate` `{}` -> both refused; `ServiceQuotaTemplateAssociationStatus` (`ASSOCIATED`/`DISASSOCIATED`) is never put on the wire, and `ListServiceQuotaIncreaseRequestsInTemplate` / `GetServiceQuotaIncreaseRequestFromTemplate` / `DeleteServiceQuotaIncreaseRequestFromTemplate` are refused for every input.
