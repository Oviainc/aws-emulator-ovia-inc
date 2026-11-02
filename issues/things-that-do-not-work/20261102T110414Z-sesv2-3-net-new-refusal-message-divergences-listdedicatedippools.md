Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.list-dedicated-ip-pools-forged-next-token-rejected sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree sesv2.concurrent-contact-creation-exactly-one-succeeds
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|ListDedicatedIpPools|BadRequestException
Issue-Key: sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree|run#23|AssertionError|assert get_error == gone, (
Issue-Key: sesv2.concurrent-contact-creation-exactly-one-succeeds|run#18|AssertionError|assert all(
Issue-Commit: fbef683f21970efb4631f2658975dd3b2c5af139

# sesv2: 3 net-new refusal-message divergences — ListDedicatedIpPools forged-NextToken, GetTenant not-found after delete, and the CreateContact duplicate under contention

> _Note: this issue was identified against the code base at commit `fbef683f21970efb4631f2658975dd3b2c5af139`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDedicatedIpPools

## Description
Three distinct net-new root causes in the sesv2 surface. In all three the emulator DOES answer the operation and refuses with the right HTTP status and error code; the Error.Message text is the emulator's own wording rather than AWS's, so every client that surfaces, logs or matches the documented sentence mis-handles an otherwise-correct refusal. No 501 stub is involved in any of them.

Cause 1 of 3: ListDedicatedIpPools refuses a forged/opaque NextToken with the wrong message sentence.
AWS: Smithy (com.amazonaws.sesv2): ListDedicatedIpPools GET /v2/email/dedicated-ip-pools, input {NextToken (httpQuery 'NextToken'), PageSize (httpQuery 'PageSize')}, paginated {inputToken NextToken, outputToken NextToken, pageSize PageSize}; modeled errors BadRequestException(400) and TooManyRequestsException(429). Recorded behavior for the un-decodable-cursor class: BadRequestException/400 with Message 'Invalid next token.' — the whole sentence is FIXED text; it has no variable part (the offending token is never echoed).
Emulator: the status (400) and the code (BadRequestException) are right, but the message reads 'Token is invalid.' where AWS words it 'Invalid next token.'
Why: a client that branches on, or a fixture that asserts, SES's documented cursor-rejection sentence treats a correctly refused forged cursor as an unknown failure; paging code that distinguishes 'cursor expired/invalid' from other BadRequestException causes cannot do so.
Fix: the NextToken decode/validation refusal in the ListDedicatedIpPools handler of the sesv2 service (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/list_dedicated_ip_pools.rs), and the shared cursor-validation helper it calls if the sentence is authored there — every paginated sesv2 list must emit 'Invalid next token.'
Traces: ses.list-dedicated-ip-pools-forged-next-token-rejected (the positive control in the same check — the pool is listed when called without a token — passes).

Cause 2 of 3: GetTenant on a tenant deleted while it still held a resource association refuses with the emulator's tenant not-found sentence.
AWS: Smithy (com.amazonaws.sesv2): GetTenant POST /v2/email/tenants/get, input {TenantName (required, @length min 1)}, errors BadRequestException(400), NotFoundException(404), TooManyRequestsException(429); ListTenantResources POST /v2/email/tenants/resources/list has the same input and the same error set. Recorded behavior for the deleted/unknown-tenant class: NotFoundException with Message 'The requested tenant <NAME> does not exist.' — fixed text 'The requested tenant <' + '> does not exist.', the only variable part being the TenantName taken verbatim from the request and wrapped in literal angle brackets.
Emulator: the 404 and NotFoundException are right, but the message reads 'Tenant <name> does not exist.' (no brackets, different lead-in) where AWS words it 'The requested tenant <name> does not exist.' The tenant name shown in the replay output is a per-run generated value filling the template, not part of it.
Why: a client cannot match SES's documented missing-tenant sentence, and tooling that parses the bracketed tenant name out of the message gets nothing; here it also masks whether the tenant-keyed index (ListTenantResources) was torn down with the tenant, because the check stops at the GetTenant message.
Fix: the shared tenant lookup/refusal construction behind the GetTenant and ListTenantResources handlers in the sesv2 service (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_tenant.rs, .../list_tenant_resources.rs) must emit 'The requested tenant {TenantName} does not exist.' with the name angle-bracketed, for a never-created name and for a deleted one alike. Open issue 20261029T202037Z-sesv2-5-net-new-behavioral-root-causes-four.md reports the same wording defect as its Cause 3 for the GetTenant/DeleteTenant unknown-name paths, but it does not carry this check's replay key and its Traces do not include this check — the path exercised here is a tenant deleted while holding an association, reached through ListTenantResources' tenant-keyed view; fixing that cause should fix this one, and this issue records the check that must be re-replayed to prove it.
Traces: sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree (everything before it passes: shared membership across two tenants is accepted, both views are inverses, and DeleteTenant on a tenant still holding an association succeeds).

Cause 3 of 3: the CreateContact duplicate-address refusal is an emulator-authored sentence that also names the contact list.
AWS: Smithy (com.amazonaws.sesv2): CreateContact POST /v2/email/contact-lists/{ContactListName}/contacts, input {ContactListName (httpLabel, required), EmailAddress (required), TopicPreferences, UnsubscribeAll (default false), AttributesData}, errors AlreadyExistsException(400), BadRequestException(400), NotFoundException(404), TooManyRequestsException(429). Recorded behavior for the duplicate-address class, including the losers of a concurrent race: AlreadyExistsException/400 with Message '<EMAIL> already exists in List.' — fixed text ' already exists in List.' (capital 'L'), the single variable part being the EmailAddress from the request; the contact-list name is NOT named. A loser may alternatively fail with ConcurrentModificationException.
Emulator: exactly one of six concurrent CreateContact calls for one address succeeds (that invariant holds) and every loser is refused AlreadyExistsException, but the message reads 'Contact with email address: <email> already exists in list: <list-name>.' — a different sentence that additionally leaks the list name. The email/list values in the replay output are per-run generated values filling the template.
Why: a client that retries or de-duplicates on SES's documented duplicate-contact sentence does not recognise the conflict, so a benign race is surfaced as an unknown error.
Fix: the duplicate refusal message in the CreateContact handler of the sesv2 service (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_contact.rs) must emit '{EmailAddress} already exists in List.' and must not name the list. The open WIRE issue 20261029T171854Z-sesv2-contact-plane-fabricates-topicdefaultpreferences-drops-empty-tags-and.md records the same sentence as its Cause 2 from a recorded fixture step; it carries no behavioral replay key and does not cover this check, which grounds the same defect on the concurrent-contention path.
Traces: sesv2.concurrent-contact-creation-exactly-one-succeeds.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator; all three are verdict [mismatch] (the call was answered and refused, only the message text diverges).

Cause 1 — ses.list-dedicated-ip-pools-forged-next-token-rejected:
expected (AWS) BadRequestException with Message 'Invalid next token.'; actual AssertionError: expected message 'Invalid next token.', got 'Token is invalid.' The BadRequestException code assertion immediately before it passed.

Cause 2 — sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree:
expected (AWS) ('NotFoundException', 'The requested tenant <TENANT> does not exist.'); actual AssertionError: GetTenant(sim2real-behav-t2-3c6dd4e5e360) after DeleteTenant returned ('NotFoundException', 'Tenant sim2real-behav-t2-3c6dd4e5e360 does not exist.'). The name is a per-run generated value; the divergence is the lead-in 'The requested tenant ' and the angle brackets around the name.

Cause 3 — sesv2.concurrent-contact-creation-exactly-one-succeeds:
expected (AWS) every loser of the 6-way race to report ('AlreadyExistsException', '<EMAIL> already exists in List.') or ConcurrentModificationException; actual AssertionError: unexpected failures under contention: five entries all reading ('AlreadyExistsException', 'Contact with email address: sim2real-behav-contact-race-9a33d98831d0@example.com already exists in list: sim2real-behav-clist-race-922f1d37da51.'). Exactly one call succeeded, so only the refusal wording diverges.

## Steps to Reproduce
Cause 1:
1. sesv2.create_dedicated_ip_pool(PoolName='p1', ScalingMode='STANDARD').
2. sesv2.list_dedicated_ip_pools(PageSize=1, NextToken='c2ltMnJlYWwtZm9yZ2VkLW5leHQtdG9rZW4=') — a token the emulator never issued.
3. Inspect Error.Code/Error.Message: code BadRequestException is right, message is 'Token is invalid.' instead of AWS's 'Invalid next token.'
4. Re-replay ses.list-dedicated-ip-pools-forged-next-token-rejected.

Cause 2:
1. sesv2.create_tenant(TenantName='t2'); sesv2.create_configuration_set(ConfigurationSetName='cs'); sesv2.create_tenant_resource_association(TenantName='t2', ResourceArn='arn:aws:ses:<region>:<account>:configuration-set/cs').
2. sesv2.delete_tenant(TenantName='t2') (it succeeds while the association is still held).
3. sesv2.get_tenant(TenantName='t2') and sesv2.list_tenant_resources(TenantName='t2'); inspect Error.Message — 'Tenant t2 does not exist.' instead of 'The requested tenant <t2> does not exist.'
4. Re-replay sesv2.a-resource-belongs-to-at-most-one-tenant-and-both-views-agree.

Cause 3:
1. sesv2.create_contact_list(ContactListName='l1').
2. Fire six concurrent sesv2.create_contact(ContactListName='l1', EmailAddress='x@example.com') calls.
3. Exactly one succeeds; inspect each loser's Error.Message — 'Contact with email address: x@example.com already exists in list: l1.' instead of AWS's 'x@example.com already exists in List.'
4. Re-replay sesv2.concurrent-contact-creation-exactly-one-succeeds.
