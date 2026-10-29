Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 3a66b075702e523a87f5e33a7353c602a74f8e6d

# ServiceQuotaExceededException is reported with an invented quotaCode, an empty resourceId and an emulator-only ceiling of 500 contacts

_Filed: 2026-10-24T19:15:44Z_
_Operation: CreateEmailContact | Class: error-response_

> _Note: this issue was identified against the code base at commit `3a66b075702e523a87f5e33a7353c602a74f8e6d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateEmailContact is the only operation in this service that declares `ServiceQuotaExceededException` (HTTP 402), and the modelled shape requires `message`, `resourceId`, `resourceType`, `serviceCode` and `quotaCode` — the last two documented as "The code for the service quota exceeded in Service Quotas" / "The code for the service quota in Service Quotas". The emulator does produce a 402 with the right error code, but three details of that refusal do not correspond to anything the real service could answer:

1. The ceiling it triggers at is an emulator resource guard, not the service's per-account email-contact quota. `crate::state::MAX_EMAIL_CONTACTS = 500` is documented in-tree as "Emulator resource guard, NOT a published AWS quota". AWS publishes a per-account email-contact quota for AWS User Notifications Contacts that is far lower (documented as 10 email contacts per account), so a client that creates contacts until the account quota is reached keeps getting 201 Created from the emulator where the real service answers 402 ServiceQuotaExceededException. Expected: the quota ceiling is the service's published per-account email-contact quota; actual: 500 creates all succeed.
2. `quotaCode` is reported as the literal `L-EMAILCONTACTS`. Service Quotas quota codes are `L-` followed by eight uppercase hex characters (e.g. `L-1216C47A`); `L-EMAILCONTACTS` can never be looked up through the Service Quotas API the member's own documentation links to, so a client that round-trips `quotaCode` into `servicequotas:GetServiceQuota` fails against the emulator.
3. `resourceId` is emitted as the empty string (`resource_id: String::new()`), although `ServiceQuotaExceededException$resourceId` is `@required` and documented as "The ID of the resource that exceeds the service quota". A required identity member answered as "" carries no resource identity at all.

All three are observable in the single 402 body this service can produce.

## Evidence

Spec: smithy_query(query="shape", shape="ServiceQuotaExceededException") — client error, httpError 402, members message/resourceId/resourceType/serviceCode/quotaCode all `@required`, with resourceId documented as "The ID of the resource that exceeds the service quota" and quotaCode as "The code for the service quota in Service Quotas" (linking to the Service Quotas user guide). smithy_query(query="operation", operation="CreateEmailContact") binds this error to CreateEmailContact only.

Handler/state: services/service-com-amazonaws-notificationscontacts-notificationscontacts/src/state.rs lines 167-176 define `MAX_EMAIL_CONTACTS = 500` with the comment "Emulator resource guard, NOT a published AWS quota" and `EMAIL_CONTACTS_QUOTA_CODE = "L-EMAILCONTACTS"`. src/ops/create_email_contact.rs lines 177-191 raise the 402 with `resource_id: String::new()`, `service_code: ARN_SERVICE`, `quota_code: EMAIL_CONTACTS_QUOTA_CODE`.

Test coverage: tests/all/email_contacts.rs `the_store_refuses_to_grow_past_its_cap` creates MAX_EMAIL_CONTACTS (500) contacts — every one answers 201 — and then asserts only the 402 status and the `ServiceQuotaExceededException` code; it asserts nothing about quotaCode, serviceCode or resourceId, and it pins the emulator-only 500 ceiling. No existing issue covers (CreateEmailContact, error-response): the open notificationscontacts issues cover tag-key validation, the activation-code lifetime/attempt limit, cross-service delete propagation and error-message wording.

## Steps to Reproduce

1. Start the emulator and call CreateEmailContact repeatedly with distinct name/emailAddress pairs (e.g. name=filler-0000..filler-0010, emailAddress=filler-NNNN@example.com).
2. Observe that creation keeps returning 201 well past the service's published per-account email-contact quota; the first refusal only arrives on the 501st contact.
3. Keep creating until the 501st CreateEmailContact is refused, and inspect the 402 body: `quotaCode` is "L-EMAILCONTACTS" (not an `L-<8 hex>` Service Quotas code) and `resourceId` is the empty string even though the member is @required.
4. Optionally feed the returned quotaCode/serviceCode into servicequotas:GetServiceQuota — the code does not resolve.
