Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.configuration-set-duplicate-create sesv2.e2e-tenant-suppression-blocks-send-then-teardown sesv2.tenant-get-unknown-rejected sesv2.tenant-get-after-delete-not-found sesv2.tenant-delete-unknown-rejected sesv2.put-account-pricing-attributes-unknown-plan-rejected sesv2.send-email-unverified-sender-rejected
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.configuration-set-duplicate-create|run#7|AssertionError|assert err == expected, f'expected {expected}, got {err}'
Issue-Key: sesv2.e2e-tenant-suppression-blocks-send-then-teardown|run#9|AssertionError|assert not_found.get("Message") == not_found_message, (
Issue-Key: sesv2.tenant-get-unknown-rejected|run#7|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: sesv2.tenant-get-after-delete-not-found|run#11|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: sesv2.tenant-delete-unknown-rejected|run#9|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: sesv2.put-account-pricing-attributes-unknown-plan-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sesv2.send-email-unverified-sender-rejected|run#4|AssertionError|raise AssertionError(
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# sesv2: 5 net-new behavioral root causes — four refusal-message wordings diverge (configuration-set duplicate, tenant-scoped suppression not-found, tenant not-found, PricingPlan enum-validation order) and no verified sending identity can ever be discovered

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTenant

## Description
Five distinct net-new root causes in the sesv2 surface, each grounded in its own failing behavioral checks. All are message/behaviour fidelity defects in operations the emulator DOES answer (no 501 stub involved).

Cause 1 of 5 — CreateConfigurationSet's duplicate-name refusal does not bracket the configuration-set name.
AWS: Smithy (com.amazonaws.sesv2): CreateConfigurationSet POST /v2/email/configuration-sets, input {ConfigurationSetName (required), ...}, modeled errors AlreadyExistsException(400), BadRequestException(400), LimitExceededException(400), NotFoundException(404), TooManyRequestsException(429), ConcurrentModificationException(500). Recorded behavior for the duplicate-name class: AlreadyExistsException with Message `Configuration set <NAME> already exists.` — fixed text is `Configuration set <` + `> already exists.` and the only variable part is the ConfigurationSetName taken verbatim from the request, wrapped in literal angle brackets. The same service's NotFoundException on GetConfigurationSet brackets the name identically (`Configuration set <NAME> does not exist.`) and that assertion passed in this check, so the emulator already brackets in the not-found path.
Emulator: code AlreadyExistsException and the 400 are right, but the message drops the angle brackets: got `Configuration set sim2real-behav-cfgset-dup-<id> already exists.` where AWS words it `Configuration set <sim2real-behav-cfgset-dup-<id>> already exists.` (the name shown is a per-run generated value filling the template, not part of it).
Why: clients, log scrapers and SDK-level fixtures that match SES's documented duplicate-name sentence (or parse the bracketed name out of it) mis-handle the refusal; the emulator is internally inconsistent with its own not-found wording for the same resource.
Fix: the CreateConfigurationSet handler's already-exists error construction in the sesv2 service (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_configuration_set.rs) must interpolate the request's ConfigurationSetName into `Configuration set <{name}> already exists.`
Traces: sesv2.configuration-set-duplicate-create.

Cause 2 of 5 — GetSuppressedDestination scoped to a tenant (TenantName query parameter) refuses a missing address with the ACCOUNT-list wording instead of the TENANT-list wording.
AWS: Smithy (com.amazonaws.sesv2): GetSuppressedDestination GET /v2/email/suppression/addresses/{EmailAddress}, input {EmailAddress (httpLabel, required), TenantName (httpQuery `TenantName`, @length min 1)}, errors BadRequestException(400), NotFoundException(404), TooManyRequestsException(429); the SuppressedDestination shape carries a TenantName member that is 'present only when the suppressed destination is on a tenant's suppression list', i.e. the tenant list is a distinct list from the account list. Recorded behavior for the tenant-scoped missing-address class: NotFoundException with Message `Email address ADDRESS does not exist on your tenant suppression list.` — fixed text `Email address ` + ` does not exist on your tenant suppression list.`, the variable part being the EmailAddress from the request path; the account-scoped call (no TenantName) is the one that says `... does not exist on your suppression list.`
Emulator: the NotFoundException code is right, but the tenant-scoped read returns the account-scoped sentence: got `Email address sim2real-tenant-suppressed-<id>@example.com does not exist on your suppression list.` where AWS words it `Email address sim2real-tenant-suppressed-<id>@example.com does not exist on your tenant suppression list.` (the address is a per-run generated value filling the template). Hypothesis: the handler ignores the TenantName query parameter when selecting the not-found message, i.e. tenant-scoped and account-scoped suppression reads share one error path.
Why: a client cannot tell 'absent from this tenant's list' from 'absent from the account list'; tenant-aware tooling (per-tenant suppression dashboards, teardown checks) mis-attributes the refusal, and the divergence also hints the tenant suppression list is not modelled as its own list.
Fix: the GetSuppressedDestination handler in the sesv2 service (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_suppressed_destination.rs) — branch the not-found message on the presence of the TenantName query parameter; its sibling DeleteSuppressedDestination must word the tenant-scoped refusal the same way.
Traces: sesv2.e2e-tenant-suppression-blocks-send-then-teardown (the assertion right after the sandbox-refused PutSuppressedDestination).

Cause 3 of 5 — the tenant not-found sentence is the emulator's own wording, not SES's.
AWS: Smithy (com.amazonaws.sesv2): GetTenant POST /v2/email/tenants/get and DeleteTenant POST /v2/email/tenants/delete, both with input {TenantName (required, @length min 1)} and errors BadRequestException(400), NotFoundException(404), TooManyRequestsException(429). Recorded behavior for the unknown/deleted-tenant class on BOTH operations: NotFoundException with Message `The requested tenant <NAME> does not exist.` — fixed text `The requested tenant <` + `> does not exist.`, the only variable part being the TenantName from the request, wrapped in literal angle brackets.
Emulator: NotFoundException and the status are accepted, but the message reads `Tenant sim2real-tenant-unknown-<id> does not exist.` where AWS words it `The requested tenant <sim2real-tenant-unknown-<id>> does not exist.` (the tenant name is a per-run generated value filling the template). It is identical for a never-created name, for a deleted name read back with GetTenant, and for a second DeleteTenant on an already-deleted name — one shared message, wrong in all three.
Why: every client that surfaces, logs or branches on the documented SES sentence (and every fixture asserting it) mis-classifies a missing tenant; the bracketed name is also what tooling parses the tenant out of.
Fix: the shared tenant lookup/refusal construction behind the GetTenant and DeleteTenant handlers in the sesv2 service (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_tenant.rs, .../delete_tenant.rs) must emit `The requested tenant {TenantName} does not exist.` with the name angle-bracketed.
Traces: sesv2.tenant-get-unknown-rejected, sesv2.tenant-get-after-delete-not-found, sesv2.tenant-delete-unknown-rejected.

Cause 4 of 5 — PutAccountPricingAttributes' enum-validation message lists the PricingPlan values in the wrong order.
AWS: Smithy (com.amazonaws.sesv2): PutAccountPricingAttributes PUT /v2/email/account/pricing-attributes, input {Plan (required, enum PricingPlan)}, errors BadRequestException(400), ConflictException(409), TooManyRequestsException(429). The model declares PricingPlan's values in the order [NONE, ESSENTIALS, PRO, ENTERPRISE], but the recorded refusal for an out-of-enum value is BadRequestException/400 with Message `1 validation error detected: Value at 'plan' failed to satisfy constraint: Member must satisfy enum value set: [ENTERPRISE, ESSENTIALS, PRO, NONE]` — recorded behavior is the requirement here: the fixed text is the whole sentence including the bracketed value list in exactly that order, and the only request-derived part is the member path `'plan'`.
Emulator: the code (BadRequestException) and 400 are right, and the sentence is otherwise identical, but the value set is rendered in the model's declaration order: got `... Member must satisfy enum value set: [NONE, ESSENTIALS, PRO, ENTERPRISE]`. Hypothesis: the validator formats the list by iterating the generated enum rather than emitting AWS's fixed list text.
Why: AWS's validation messages are byte-stable and are matched verbatim by tests, CLI users and support tooling; a differently ordered value set makes an otherwise-correct refusal fail any exact comparison. (The sibling check sesv2.put-account-pricing-attributes-conflict-rejected is owned by the open stub issue 20261026T013649Z-sesv2-putaccountpricingattributes-is-an-unimplemented-scaffold-stub-every.md; this cause is NOT that one — here the operation did validate and refuse, only the message text diverges.)
Fix: the Plan enum validation/error formatting for PutAccountPricingAttributes in the sesv2 service (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_account_pricing_attributes.rs and the shared validation-message helper).
Traces: sesv2.put-account-pricing-attributes-unknown-plan-rejected.

Cause 5 of 5 — no verified sending identity is ever discoverable, so the SendEmail sender-verification gate cannot be exercised at all.
AWS: Smithy (com.amazonaws.sesv2): ListEmailIdentities POST /v2/email/list-identities -> {EmailIdentities: IdentityInfo[], NextToken}, where IdentityInfo carries IdentityName, IdentityType, SendingEnabled and VerificationStatus (enum PENDING|SUCCESS|FAILED|TEMPORARY_FAILURE|NOT_STARTED); SendEmail POST /v2/email/outbound-emails refuses a send from an unverified identity with MessageRejected(400). Recorded behavior: the account holds at least one identity whose VerificationStatus is `SUCCESS` with SendingEnabled true (the DNS-baseline domain), so a send from it returns 200 with a MessageId while a send from an arbitrary unverified address is refused MessageRejected.
Emulator: paging ListEmailIdentities to exhaustion yields no identity with VerificationStatus `SUCCESS`, so the check aborts before any send with `AssertionError: no verified SES sending identity exists in this account`. Nothing is refused — the enumeration simply never reports a verified identity, so neither the positive control (send from a verified identity) nor the negative (MessageRejected for an unverified sender) can be observed. Hypothesis: identities never leave PENDING/NOT_STARTED, so the emulator can never present a verified sender; the related open issues 20261029T085623Z-sesv2-two-causes-the-dns-baseline-hosted-zone.md and 20261029T160438Z-sesv2-sns-sqs-the-ses-sending-surface-is.md report the route53 tag-discovery side of this and carry other check ids — neither covers this check.
Why: the entire outbound sending surface is unreachable for a client: it cannot find a sender to send from, and the emulator's sender-verification gate (the thing that makes an emulator not overly permissive) is unverifiable.
Fix: the sesv2 email-identity state and its verification transition (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/list_email_identities.rs projection of VerificationStatus/SendingEnabled, plus whatever drives an identity to SUCCESS once its DKIM records are published) and the SendEmail sender gate that consumes it.
Traces: sesv2.send-email-unverified-sender-rejected.

## Evidence
Cause 1 — expected (AWS): AlreadyExistsException, Message "Configuration set <{ConfigurationSetName}> already exists." Actual (emulator):
[mismatch] sesv2.configuration-set-duplicate-create — AssertionError: expected ('AlreadyExistsException', 'Configuration set <sim2real-behav-cfgset-dup-39775d4aac43> already exists.'), got ('AlreadyExistsException', 'Configuration set sim2real-behav-cfgset-dup-39775d4aac43 already exists.') (the name is a per-run generated value).

Cause 2 — expected (AWS): GetSuppressedDestination(EmailAddress=X, TenantName=T) for an address absent from that tenant's list -> NotFoundException, Message "Email address {X} does not exist on your tenant suppression list." Actual (emulator):
[mismatch] sesv2.e2e-tenant-suppression-blocks-send-then-teardown — AssertionError: expected message 'Email address sim2real-tenant-suppressed-52845a693e6a@example.com does not exist on your tenant suppression list.', got 'Email address sim2real-tenant-suppressed-52845a693e6a@example.com does not exist on your suppression list.'

Cause 3 — expected (AWS): NotFoundException, Message "The requested tenant <{TenantName}> does not exist." Actual (emulator), three independent paths:
[mismatch] sesv2.tenant-get-unknown-rejected — expected 'The requested tenant <sim2real-tenant-unknown-67ace0dfb6bc> does not exist.', got 'Tenant sim2real-tenant-unknown-67ace0dfb6bc does not exist.'
[mismatch] sesv2.tenant-get-after-delete-not-found — expected 'The requested tenant <sim2real-tenant-97de5d77d866> does not exist.', got 'Tenant sim2real-tenant-97de5d77d866 does not exist.'
[mismatch] sesv2.tenant-delete-unknown-rejected — expected 'The requested tenant <sim2real-tenant-6bc6164033c2...> does not exist.', got 'Tenant sim2real-tenant-unknown-6bc6164033c2 does not exist.'
The NotFoundException code and the effect-absent/reuse assertions pass; only the message text diverges.

Cause 4 — expected (AWS, recorded): BadRequestException/400, Message "1 validation error detected: Value at 'plan' failed to satisfy constraint: Member must satisfy enum value set: [ENTERPRISE, ESSENTIALS, PRO, NONE]". Actual (emulator):
[mismatch] sesv2.put-account-pricing-attributes-unknown-plan-rejected — got "1 validation error detected: Value at 'plan' failed to satisfy constraint: Member must satisfy enum value set: [NONE, ESSENTIALS, PRO, ENTERPRISE]" (same sentence, value set in the model's declaration order).

Cause 5 — expected (AWS): ListEmailIdentities enumerates an identity with VerificationStatus 'SUCCESS' and SendingEnabled true; SendEmail from it returns a MessageId, SendEmail from an unverified address is refused MessageRejected. Actual (emulator):
[mismatch] sesv2.send-email-unverified-sender-rejected — AssertionError: no verified SES sending identity exists in this account; SendEmail requires a verified FROM identity even to send to the mailbox-simulator recipient. No call is refused; the paginated ListEmailIdentities just returns nothing verified.

## Steps to Reproduce
Cause 1:
1. sesv2.create_configuration_set(ConfigurationSetName='cs-dup-1').
2. sesv2.create_configuration_set(ConfigurationSetName='cs-dup-1') again; inspect Error.Message — it lacks the angle brackets AWS puts around the name.
3. Re-replay sesv2.configuration-set-duplicate-create.

Cause 2:
1. sesv2.create_tenant(TenantName='t1').
2. sesv2.get_suppressed_destination(EmailAddress='nobody@example.com', TenantName='t1'); inspect Error.Message — it says 'on your suppression list.' instead of 'on your tenant suppression list.'
3. Re-replay sesv2.e2e-tenant-suppression-blocks-send-then-teardown.

Cause 3:
1. sesv2.get_tenant(TenantName='never-created') -> NotFoundException with 'Tenant never-created does not exist.'
2. sesv2.create_tenant(TenantName='t2'); sesv2.delete_tenant(TenantName='t2'); then get_tenant and a second delete_tenant on 't2' — same wrong sentence.
3. Re-replay sesv2.tenant-get-unknown-rejected, sesv2.tenant-get-after-delete-not-found, sesv2.tenant-delete-unknown-rejected.

Cause 4:
1. With botocore parameter validation disabled, call sesv2.put_account_pricing_attributes(Plan='PLATINUM').
2. Inspect Error.Message: the bracketed enum value set is ordered [NONE, ESSENTIALS, PRO, ENTERPRISE] instead of AWS's [ENTERPRISE, ESSENTIALS, PRO, NONE].
3. Re-replay sesv2.put-account-pricing-attributes-unknown-plan-rejected.

Cause 5:
1. Page sesv2.list_email_identities() to exhaustion; observe no entry with VerificationStatus 'SUCCESS'.
2. Create a domain identity, publish its returned Easy DKIM CNAMEs, and poll get_email_identity: VerifiedForSendingStatus never becomes true / DkimAttributes.Status never becomes SUCCESS.
3. Re-replay sesv2.send-email-unverified-sender-rejected.
