Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53domains.domain-scoped-operations-require-domain-in-account route53domains.unsupported-tld-rejected-consistently-across-surfaces
Issue-Shape-Id: com.amazonaws.route53domains#Route53Domains_v20140515
Issue-Key: route53domains.domain-scoped-operations-require-domain-in-account|run#5|AssertionError|assert message == expected_message, (
Issue-Key: route53domains.unsupported-tld-rejected-consistently-across-surfaces|run#6|AssertionError|assert message == expected_message, f"CheckDomainAvailability: unexpected message {message!r}"
Issue-Commit: 7e082a9dfef7650059a714e1b731386462df20dc

# route53domains refusal messages use emulator-invented wording: the unknown-domain InvalidInput and the UnsupportedTLD message both diverge from AWS (2 causes)

> _Note: this issue was identified against the code base at commit `7e082a9dfef7650059a714e1b731386462df20dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDomainDetail

## Description
Two net-new root causes in this service's shared refusal-message helpers (services/service-com-amazonaws-route53domains-route53domains-v20140515/src/state.rs). Both are message-text divergences on refusals whose HTTP status (400) and error code are already correct, so a client that routes on the code is fine but any client, test or assertion that reads the message text sees text AWS never produces.

Cause 1 of 2: the InvalidInput refusal of a domain that is not registered to the calling account uses the emulator's own sentence instead of AWS's.
AWS: every domain-scoped Route 53 Domains operation (GetDomainDetail, ListTagsForDomain, GetContactReachabilityStatus, DisableDomainTransferLock, DeleteDomain) refuses a name the account does not hold with HTTP 400 and error code InvalidInput — Smithy: com.amazonaws.route53domains#InvalidInput, errorKind client, httpError 400, and it is a service-common error bound to every operation; its documentation is "the request might specify a domain name that doesn't belong to the account that submitted the request". The message wording is recorded behavior only: the recording shows the fixed text "Invalid request. Problems: Domain " + <domain name> + " not found in account." — i.e. the fixed parts are the "Invalid request. Problems: Domain " prefix and the " not found in account." suffix (with the trailing period), and the only variable part is the domain name taken from the request's DomainName member. The same message is served by every domain-scoped operation, which is what makes it checkable across five operations at once.
Emulator: serves InvalidInput with the message "Domain <name> is not registered to this account" — different fixed text, no "Invalid request. Problems: " prefix, no trailing period. Observed on GetDomainDetail: expected 'Invalid request. Problems: Domain <name>.com not found in account.', got 'Domain <name>.com is not registered to this account'. The check stops at the first operation, so the remaining four domain-scoped operations are served the same wrong text by the same helper.
Why: a client that parses or asserts on the refusal message — including AWS's own documented troubleshooting text and any test pinned to AWS's wording — cannot recognise the "not found in account" condition, and a human reading the message gets prose no AWS log or runbook contains.
Fix: the shared message helper crate::state::unknown_domain_message, which every domain-scoped operation's handler calls when the store has no such domain.
Traces: route53domains.domain-scoped-operations-require-domain-in-account.

Cause 2 of 2: the UnsupportedTLD refusal message names the whole domain name in the emulator's own sentence instead of naming the TLD in AWS's.
AWS: an unsupported top-level domain is refused with HTTP 400 and error code UnsupportedTLD — Smithy: com.amazonaws.route53domains#UnsupportedTLD, errorKind client, httpError 400, declared on CheckDomainAvailability, CheckDomainTransferability, ListPrices and most other operations; doc "Amazon Route 53 does not support this top-level domain (TLD)." The message wording is recorded behavior only: the recording shows the fixed text "The " + <tld> + " TLD is not supported" — the fixed parts are the "The " prefix and the " TLD is not supported" suffix, and the single variable part is the top-level domain itself (the label(s) after the first label of the requested DomainName, or the Tld member for ListPrices), NOT the whole domain name and NOT quoted. The same message is served by every surface that takes a TLD, which is what the check asserts across CheckDomainAvailability, CheckDomainTransferability and ListPrices.
Emulator: CheckDomainAvailability returns code UnsupportedTLD (correct) with the message "Amazon Route 53 does not support the top-level domain of '<full.domain.name>'" — the fixed text is the emulator's own, and the variable part is the entire requested domain name in single quotes rather than just the TLD.
Why: the message no longer tells the caller which TLD was rejected in the form AWS reports it, so a client that extracts the offending TLD from the message, or asserts AWS's wording, breaks; and the three surfaces cannot be shown to agree because none of them uses AWS's text.
Fix: the shared message helper crate::state::unsupported_tld_message and its call sites in each operation's refuse_domain_name — it must be passed/derive the TLD rather than the raw domain name. (The same check also exercises ListPrices and CheckDomainTransferability, which are still unimplemented scaffold stubs covered by their own stub issues; this cause is about the message text the implemented surface already serves.)
Traces: route53domains.unsupported-tld-rejected-consistently-across-surfaces.

## Evidence
Check route53domains.domain-scoped-operations-require-domain-in-account [mismatch]:
  AssertionError: GetDomainDetail: expected 'Invalid request. Problems: Domain sim2real-behav-r53d-missing-77cbef28dfa5.com not found in account.', got 'Domain sim2real-behav-r53d-missing-77cbef28dfa5.com is not registered to this account'
  (the domain name itself is the per-run variable part; the divergence is in the fixed text around it)
  Error.Code was InvalidInput on both sides — only Error.Message diverges.
  Source: services/.../src/state.rs:297-299 unknown_domain_message(); call site services/.../src/ops/get_domain_detail.rs:228-231.

Check route53domains.unsupported-tld-rejected-consistently-across-surfaces [mismatch]:
  AssertionError: CheckDomainAvailability: unexpected message "Amazon Route 53 does not support the top-level domain of 'sim2real-behav-r53d-ad0504c44587.zzinvalidtld'"
  expected message: "The zzinvalidtld TLD is not supported"
  Error.Code was UnsupportedTLD on both sides — only Error.Message diverges.
  Source: services/.../src/state.rs:292-294 unsupported_tld_message(); call site services/.../src/ops/check_domain_availability.rs:147-152.

Smithy (com-amazonaws-route53domains-route53domains-v20140515.json): InvalidInput -> client error, httpError 400, service-common (bound to every operation); UnsupportedTLD -> client error, httpError 400, declared on CheckDomainAvailability / CheckDomainTransferability / ListPrices / GetDomainDetail and others.

## Steps to Reproduce
Cause 1:
1. aws route53domains get-domain-detail --domain-name <a-name-this-account-does-not-hold>.com --region us-east-1
2. Observe HTTP 400 InvalidInput with message 'Domain <name> is not registered to this account'.
3. AWS answers 400 InvalidInput with 'Invalid request. Problems: Domain <name> not found in account.'
4. Repeat with list-tags-for-domain, get-contact-reachability-status, disable-domain-transfer-lock and delete-domain on the same name: all five must carry the identical AWS message.

Cause 2:
1. aws route53domains check-domain-availability --domain-name <label>.zzinvalidtld --region us-east-1
2. Observe HTTP 400 UnsupportedTLD with message "Amazon Route 53 does not support the top-level domain of '<label>.zzinvalidtld'".
3. AWS answers 400 UnsupportedTLD with 'The zzinvalidtld TLD is not supported' (the TLD only, unquoted).
4. The same message is expected from check-domain-transferability --domain-name <label>.zzinvalidtld and from list-prices --tld zzinvalidtld.
5. A supported-TLD request (check-domain-availability --domain-name <label>.com) must still answer AVAILABLE afterwards.
