Issue-Source: trace-sweep-divergence
Issue-Trace: route53domains-check-domain-availability
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53domains-check-domain-availability route53domains-domain-availability-and-transferability-checks route53domains-domain-input-validation route53domains-domain-and-operation-not-found route53domains-domain-reads-require-ownership route53domains-transfer-and-operation-tracking-sibling-surfaces route53domains-domain-mutations-require-ownership route53domains-list-operations-filters
Issue-Shape-Id: com.amazonaws.route53domains#Route53Domains_v20140515
Issue-Key: route53domains|CheckDomainAvailability|4xx->4xx|$.message
Issue-Key: route53domains|GetDomainDetail|4xx->4xx|$.message
Issue-Key: route53domains|GetOperationDetail|4xx->4xx|$.message
Issue-Key: route53domains|UpdateDomainContact|4xx->4xx|$.message
Issue-Key: route53domains|DeleteDomain|4xx->4xx|$.message
Issue-Key: route53domains|DisableDomainAutoRenew|4xx->4xx|$.message
Issue-Key: route53domains|ListOperations|4xx->4xx|$.message
Issue-Commit: 63738de0101ebf6c323b1ab59b247c4705f0deaf

# route53domains refusal and constraint-violation message text diverges from AWS wording on seven surfaces

> _Note: this issue was identified against the code base at commit `63738de0101ebf6c323b1ab59b247c4705f0deaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CheckDomainAvailability

## Description
SEVEN net-new root causes in this service's refusal and constraint-violation message text. In every one of the seven the HTTP status (400) and the error code are already correct and ONLY the message diverges, so a client that routes on the code is unaffected while any client, log consumer, runbook or assertion that reads the message sees text AWS never produces. All seven causes must be closed before this issue is resolved.

Related open issue: 20261109T193137Z-route53domains-refusal-messages-use-emulator-invented-wording-the.md (behavioral corpus) names the same two shared helpers as Causes 1 and 2/5/6 below, but it carries no wire replay key, so none of the wire steps listed here is covered or exercised by it. It does NOT cover Causes 3, 4 and 7 at all.

Cause 1 of 7 — UnsupportedTLD message names the whole domain instead of the TLD (CheckDomainAvailability).
AWS: an unsupported top-level domain is refused with HTTP 400 and error code UnsupportedTLD — Smithy: com.amazonaws.route53domains#UnsupportedTLD, errorKind client, httpError 400, declared on CheckDomainAvailability (and GetDomainDetail, DeleteDomain, DisableDomainAutoRenew, UpdateDomainContact); doc "Amazon Route 53 does not support this top-level domain (TLD)." The message wording is recorded behavior only: the recording shows the fixed text "The " + <tld> + " TLD is not supported". The fixed parts are the leading "The " and the trailing " TLD is not supported"; the single variable part is the TOP-LEVEL DOMAIN of the requested DomainName (the labels after the first), unquoted — not the whole domain name.
Emulator: code UnsupportedTLD (correct) with the message "Amazon Route 53 does not support the top-level domain of '<full.domain.name>'" — the emulator's own fixed text, and the variable part is the entire requested domain name in single quotes.
Why: a client that extracts the offending TLD from the message cannot find it, and the message is prose no AWS log or document contains.
Fix: crate::state::unsupported_tld_message and its call sites in each operation's refuse_domain_name — the helper must be given (or derive) the TLD rather than the raw domain name.
Traces: route53domains-check-domain-availability#task#2 (cluster also covers task#3, task#4, route53domains-domain-availability-and-transferability-checks#task#3 and route53domains-domain-input-validation#task#1..#3).

Cause 2 of 7 — unknown-domain InvalidInput message on GetDomainDetail.
AWS: a domain-scoped read of a name the account does not hold is refused with HTTP 400 and error code InvalidInput — Smithy: com.amazonaws.route53domains#InvalidInput, errorKind client, httpError 400, declared on GetDomainDetail; shape doc: "the request might specify a domain name that doesn't belong to the account that submitted the request". Message wording is recorded behavior only: the fixed text is "Invalid request. Problems: Domain " + <domain name> + " not found in account." — fixed prefix "Invalid request. Problems: Domain ", fixed suffix " not found in account." including the trailing period; the only variable part is the DomainName taken from the request.
Emulator: InvalidInput (correct) with "Domain <name> is not registered to this account" — different fixed text, no "Invalid request. Problems: " prefix, no trailing period.
Why: a caller cannot recognise the documented not-found condition from the message, and AWS's troubleshooting wording never appears.
Fix: the shared helper crate::state::unknown_domain_message (services/service-com-amazonaws-route53domains-route53domains-v20140515/src/state.rs:297-299), called by every domain-scoped handler.
Traces: route53domains-domain-and-operation-not-found#task#1 (cluster also covers route53domains-domain-reads-require-ownership#task#1).

Cause 3 of 7 — unknown operation id message on GetOperationDetail.
AWS: an operationId that names no operation submitted by this account is refused with HTTP 400 and error code InvalidInput — Smithy: GetOperationDetail declares only InvalidInput (client, httpError 400); the member doc says the item "might refer to the ID of an operation that is already completed". Message wording is recorded behavior only: the fixed text is "Invalid request. Problems: No operation found for operationId : " + <operationId> — note the space before the colon; the only variable part is the OperationId from the request.
Emulator: InvalidInput (correct) with "Value '<id>' at 'operationId' failed to satisfy constraint: Member must be the id of an operation submitted by this account" — a boundary-validator-style constraint sentence instead of the "Invalid request. Problems: ..." refusal AWS serves for a lookup miss.
Why: the emulator mislabels a state lookup miss as an input-constraint violation, so a client that distinguishes malformed input from an unknown id by message shape draws the wrong conclusion.
Fix: the GetOperationDetail handler's not-found branch (services/service-com-amazonaws-route53domains-route53domains-v20140515/src/ops/get_operation_detail.rs) — it should build its message from a shared "Invalid request. Problems: ..." helper in crate::state alongside unknown_domain_message, not from the constraint-message formatter.
Traces: route53domains-domain-and-operation-not-found#task#2 (cluster also covers route53domains-domain-input-validation#task#7 and route53domains-transfer-and-operation-tracking-sibling-surfaces#task#3).

Cause 4 of 7 — enum constraint-violation message on UpdateDomainContact is missing AWS's error-count prefix and echoes a sensitive value.
AWS: a ContactDetail.ContactType outside the modeled enum is refused with HTTP 400 InvalidInput — Smithy: ContactType enum values [PERSON, COMPANY, ASSOCIATION, PUBLIC_BODY, RESELLER]; UpdateDomainContact.AdminContact targets ContactDetail, which is @sensitive. AWS's constraint-violation message format is "<N> validation error(s) detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]" — the recording shows "1 validation error detected: Value at 'adminContact.contactType' failed to satisfy constraint: Member must satisfy enum value set: [ASSOCIATION, RESELLER, PERSON, PUBLIC_BODY, COMPANY]". Fixed text: the "<N> validation error(s) detected: " prefix, "Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]"; variable parts: the error count, the member path from the request, and the enum value set. Because the member is @sensitive the OFFENDING VALUE IS NOT ECHOED — "Value at", not "Value '<v>' at". Hypothesis: the bracketed value order in the recording is the service's own and is not the model's declaration order; the orderings differ, so a fixer should match the recorded set ordering for this member but treat only the prefix and the value-omission as hard requirements.
Emulator: "Value 'BOGUS' at 'adminContact.contactType' failed to satisfy constraint: Member must satisfy enum value set: [PERSON, COMPANY, ASSOCIATION, PUBLIC_BODY, RESELLER]" — no "1 validation error detected: " prefix, the sensitive value is echoed back, and the value set is in model-declaration order.
Why: clients and tests that parse AWS's "N validation error(s) detected:" envelope (the SDKs' and the CLI's documented validation format) fail to match, and echoing a @sensitive member's value leaks it into logs AWS keeps it out of.
Fix: the boundary validator's constraint-message formatter that the request walk runs before handle — it must emit the error-count prefix, suppress the value for @sensitive members, and render the enum value set for this member in the service's order.
Traces: route53domains-domain-and-operation-not-found#task#5.

Cause 5 of 7 — unknown-domain InvalidInput message on DeleteDomain (same helper as Cause 2, separate surface/step).
AWS: HTTP 400 InvalidInput (Smithy: DeleteDomain declares InvalidInput, client, httpError 400) with the fixed text "Invalid request. Problems: Domain " + <domain name> + " not found in account." — recorded behavior only; the only variable part is the requested DomainName. A name registered by someone else is refused the same way as one that does not exist, because the condition is "not in this account".
Emulator: InvalidInput with "Domain <name> is not registered to this account".
Why: as Cause 2 — a mutation refusal carries wording no AWS client recognises.
Fix: crate::state::unknown_domain_message (state.rs:297-299) and the DeleteDomain handler call site.
Traces: route53domains-domain-mutations-require-ownership#task#2.

Cause 6 of 7 — unknown-domain InvalidInput message on DisableDomainAutoRenew (same helper, third surface/step).
AWS: HTTP 400 InvalidInput (Smithy: DisableDomainAutoRenew declares InvalidInput and UnsupportedTLD, both client/400) with the same fixed text "Invalid request. Problems: Domain " + <domain name> + " not found in account."; recorded behavior only, variable part is the requested DomainName.
Emulator: InvalidInput with "Domain <name> is not registered to this account".
Why: as Cause 2; every domain-scoped operation must carry the identical AWS message, which is what makes the refusal recognisable across surfaces.
Fix: crate::state::unknown_domain_message (state.rs:297-299) and the DisableDomainAutoRenew handler call site.
Traces: route53domains-domain-mutations-require-ownership#task#5.

Cause 7 of 7 — range constraint-violation message on ListOperations is missing AWS's error-count prefix.
AWS: MaxItems above the modeled maximum is refused with HTTP 400 InvalidInput — Smithy: ListOperations.MaxItems targets PageMaxItems with @range(max: 100), and the operation declares only InvalidInput (client, httpError 400). The message is AWS's standard validation envelope: "<N> validation error(s) detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must have value less than or equal to <max>" — the recording shows "1 validation error detected: Value '101' at 'maxItems' failed to satisfy constraint: Member must have value less than or equal to 100". Fixed text: the "<N> validation error detected: " prefix and the "failed to satisfy constraint: Member must have value less than or equal to " clause; variable parts: the error count, the submitted value, the lowercase member name, and the modeled maximum (100 here, from the model).
Emulator: the identical sentence WITHOUT the leading "1 validation error detected: " — "Value '101' at 'maxItems' failed to satisfy constraint: Member must have value less than or equal to 100".
Why: the envelope is the part clients and the CLI's own tests match on and the only part that reports how many violations were found, so a request with several bad members cannot be told from one with a single bad member.
Fix: the boundary validator's constraint-message formatter (same component as Cause 4) — the prefix is missing for every constraint refusal this service serves, not just this member.
Traces: route53domains-list-operations-filters#task#2 (cluster also covers task#3 and task#4).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, 60 steps, 4 passed). All seven causes: expected and actual HTTP status both 400, error code matches, $.message diverges.

Cause 1 — route53domains-check-domain-availability#task#2, CheckDomainAvailability, 400->400:
  expected $.message: 'The doesnotexisttld TLD is not supported'
  actual   $.message: "Amazon Route 53 does not support the top-level domain of 'sim2real-probe-rtdavail-unsupported.doesnotexisttld'"
  (the domain label and TLD are per-fixture values; the divergence is the fixed text and the choice of variable)
  Cluster of 7 steps: check-domain-availability#task#2..#4, domain-availability-and-transferability-checks#task#3, domain-input-validation#task#1..#3.
  Source: state.rs:292-294 unsupported_tld_message(); call site ops/check_domain_availability.rs:147-152.

Cause 2 — route53domains-domain-and-operation-not-found#task#1, GetDomainDetail, 400->400:
  expected $.message: 'Invalid request. Problems: Domain <name>.com not found in account.'
  actual   $.message: 'Domain <name>.com is not registered to this account'
  Cluster also covers domain-reads-require-ownership#task#1. Source: state.rs:297-299.

Cause 3 — route53domains-domain-and-operation-not-found#task#2, GetOperationDetail, 400->400:
  expected $.message: 'Invalid request. Problems: No operation found for operationId : <operationId>'
  actual   $.message: "Value '<operationId>' at 'operationId' failed to satisfy constraint: Member must be the id of an operation submitted by this account"
  Cluster also covers domain-input-validation#task#7, transfer-and-operation-tracking-sibling-surfaces#task#3.

Cause 4 — route53domains-domain-and-operation-not-found#task#5, UpdateDomainContact, 400->400:
  expected $.message: "1 validation error detected: Value at 'adminContact.contactType' failed to satisfy constraint: Member must satisfy enum value set: [ASSOCIATION, RESELLER, PERSON, PUBLIC_BODY, COMPANY]"
  actual   $.message: "Value 'BOGUS' at 'adminContact.contactType' failed to satisfy constraint: Member must satisfy enum value set: [PERSON, COMPANY, ASSOCIATION, PUBLIC_BODY, RESELLER]"
  ('BOGUS' is the fixture's submitted value; AWS omits it because ContactDetail is @sensitive.)

Cause 5 — route53domains-domain-mutations-require-ownership#task#2, DeleteDomain, 400->400:
  expected $.message: 'Invalid request. Problems: Domain google.com not found in account.'
  actual   $.message: 'Domain google.com is not registered to this account'

Cause 6 — route53domains-domain-mutations-require-ownership#task#5, DisableDomainAutoRenew, 400->400:
  expected $.message: 'Invalid request. Problems: Domain google.com not found in account.'
  actual   $.message: 'Domain google.com is not registered to this account'

Cause 7 — route53domains-list-operations-filters#task#2, ListOperations, 400->400:
  expected $.message: "1 validation error detected: Value '101' at 'maxItems' failed to satisfy constraint: Member must have value less than or equal to 100"
  actual   $.message: "Value '101' at 'maxItems' failed to satisfy constraint: Member must have value less than or equal to 100"
  (identical but for the missing envelope prefix) Cluster also covers task#3, task#4.

Smithy (services/service-com-amazonaws-route53domains-route53domains-v20140515/smithy-model/com-amazonaws-route53domains-route53domains-v20140515.json): InvalidInput -> client, httpError 400; UnsupportedTLD -> client, httpError 400; ContactType enum [PERSON, COMPANY, ASSOCIATION, PUBLIC_BODY, RESELLER]; ContactDetail @sensitive; PageMaxItems @range(max: 100).

## Steps to Reproduce
Region us-east-1 against the emulator endpoint.

Cause 1: aws route53domains check-domain-availability --domain-name probe.doesnotexisttld
  -> expect 400 UnsupportedTLD, message 'The doesnotexisttld TLD is not supported' (TLD only, unquoted).

Cause 2: aws route53domains get-domain-detail --domain-name <a-name-this-account-does-not-hold>.com
  -> expect 400 InvalidInput, message 'Invalid request. Problems: Domain <name> not found in account.'

Cause 3: aws route53domains get-operation-detail --operation-id <an-id-never-issued>
  -> expect 400 InvalidInput, message 'Invalid request. Problems: No operation found for operationId : <id>' (note the space before the colon).

Cause 4: aws route53domains update-domain-contact --domain-name <held-name>.com --admin-contact ContactType=BOGUS
  -> expect 400 InvalidInput, message "1 validation error detected: Value at 'adminContact.contactType' failed to satisfy constraint: Member must satisfy enum value set: [ASSOCIATION, RESELLER, PERSON, PUBLIC_BODY, COMPANY]" — with the count prefix and WITHOUT the submitted value.

Cause 5: aws route53domains delete-domain --domain-name google.com
  -> expect 400 InvalidInput, message 'Invalid request. Problems: Domain google.com not found in account.'

Cause 6: aws route53domains disable-domain-auto-renew --domain-name google.com
  -> expect the same 400 InvalidInput message as Cause 5.

Cause 7: aws route53domains list-operations --max-items 101
  -> expect 400 InvalidInput, message "1 validation error detected: Value '101' at 'maxItems' failed to satisfy constraint: Member must have value less than or equal to 100".
