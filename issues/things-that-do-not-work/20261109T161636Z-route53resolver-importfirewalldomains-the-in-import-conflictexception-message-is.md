Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.import-firewall-domains-conflict-while-in-progress-rejected
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.import-firewall-domains-conflict-while-in-progress-rejected|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 4d9e3fb251011fe781d884d5ecb8db5bb1e34e6b

# route53resolver ImportFirewallDomains: the in-import ConflictException Message is emulator-authored — no [RSLVR-02305] code, no domain-list id, no remediation clause

> _Note: this issue was identified against the code base at commit `4d9e3fb251011fe781d884d5ecb8db5bb1e34e6b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportFirewallDomains

## Description
Cause 1 of 1: a second ImportFirewallDomains issued while the target DNS Firewall domain list is still IMPORTING is refused with the right code and status but a generic, emulator-authored message. Failing check id: route53resolver.import-firewall-domains-conflict-while-in-progress-rejected.

AWS: Smithy: ImportFirewallDomains takes FirewallDomainListId (required, ResourceId, length 1-64), Operation (required enum) and DomainFileUrl (required, length 1-1024), and models ConflictException as a client error with HTTP 400. Recorded behavior for the overlap case: the refusal Message is '[RSLVR-02305] The DNS Firewall domain list is in importing status: "<FirewallDomainListId>". Try again after the import completes.' Fixed text: '[RSLVR-02305] The DNS Firewall domain list is in importing status: ' and '. Try again after the import completes.'; the only variable part is the double-quoted FirewallDomainListId taken from the request (the rslvr-fdl-… id seen in any single run fills that slot and is not itself part of the template). The rejected call must not partially apply: the first import still completes with exactly its own domains, and a later import against the now-idle list succeeds.

Emulator: status 400 and code ConflictException are correct, but the Message is the fixed sentence "The requested state transition isn't valid." — no RSLVR-02305 prefix, no domain-list id, no remediation clause, and nothing distinguishing it from any other state-transition conflict the emulator raises.

Why: clients and operators key retry/backoff on the RSLVR code and parse the domain-list id out of the Message to know which list is busy. A generic state-transition sentence names no resource, so RSLVR-02305-matching retry logic never fires and automation cannot tell an in-progress import apart from an unrelated conflict; assertions on the recorded refusal text fail.

Fix: the conflict path of ImportFirewallDomains in the route53resolver handler (and the shared RSLVR-coded error message rendering it uses) — build the ConflictException Message from the RSLVR-02305 template with the requested FirewallDomainListId interpolated, instead of the generic state-transition string.

Traces: route53resolver.import-firewall-domains-conflict-while-in-progress-rejected. Note: the open issue 20261108T160732Z-route53resolver-route-53-resolver-dns-firewall-domain-plane-malformed.md describes this same message defect as its Cause 2, but the replay key it carries for this check no longer matches (the suite does not report this check as owned by it), so this check is re-filed to keep it gated; if that issue is fixed, re-replaying the check in trace_ids is the confirmation and this issue can be closed with it.

## Evidence
route53resolver.import-firewall-domains-conflict-while-in-progress-rejected
  setup: CreateFirewallDomainList, then two back-to-back ImportFirewallDomains(Operation=REPLACE, DomainFileUrl=s3://<bucket>/domains.txt) with a 3,000-domain file so the first is still IMPORTING.
  expected: ClientError code ConflictException, HTTP 400, Message '[RSLVR-02305] The DNS Firewall domain list is in importing status: "<FirewallDomainListId>". Try again after the import completes.'
  actual:   code ConflictException (correct), Message "The requested state transition isn't valid."
  assertion: AssertionError: expected message '[RSLVR-02305] The DNS Firewall domain list is in importing status: "rslvr-fdl-910a2dec89025cc1". Try again after the import completes.', got "The requested state transition isn't valid." (the rslvr-fdl-… id is the id of the list created by that run and fills the template's one variable slot)

## Steps to Reproduce
1. s3:CreateBucket and PutObject a domain file with ~3,000 lines (e.g. sim2real-behav-<i>.example.com) at key domains.txt.
2. route53resolver:CreateFirewallDomainList(CreatorRequestId=<unique>, Name=<unique>) and note Id.
3. ImportFirewallDomains(FirewallDomainListId=<id>, Operation=REPLACE, DomainFileUrl=s3://<bucket>/domains.txt) — accepted, Status IMPORTING.
4. Immediately call the identical ImportFirewallDomains again and inspect Error.Code and Error.Message of the refusal: code ConflictException is right, but the Message is "The requested state transition isn't valid." instead of the [RSLVR-02305] template naming the domain-list id.
5. Replay: run_behavioral_suite and re-run route53resolver.import-firewall-domains-conflict-while-in-progress-rejected.
