Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.import-firewall-domains-invalid-s3-uri-rejected route53resolver.import-firewall-domains-conflict-while-in-progress route53resolver.import-firewall-domains-conflict-while-in-progress-rejected route53resolver.firewall-domain-unicode-wildcard-round-trip route53resolver.e2e-firewall-domain-blocklist-lifecycle-flow
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.import-firewall-domains-invalid-s3-uri-rejected|_error#13|AssertionError|raise AssertionError(f"expected a ClientError, the call succeeded and returned {result!r}")
Issue-Key: route53resolver.import-firewall-domains-conflict-while-in-progress|run#14|AssertionError|assert _without_trace_id(message) == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53resolver.import-firewall-domains-conflict-while-in-progress-rejected|run#18|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53resolver.firewall-domain-unicode-wildcard-round-trip|run#14|AssertionError|assert expected_mixed_case_domain in domains, (
Issue-Key: route53resolver|GetFirewallRuleGroupAssociation|ResourceNotFoundException
Issue-Commit: 0a1e14ce9c5d04d4281ea317c037d5ece999a2b8

# Route 53 Resolver DNS Firewall domain plane: malformed ImportFirewallDomains URI accepted, in-import ConflictException message emulator-authored, domains not lowercased, not-found Message omits Trace Id (4 causes)

> _Note: this issue was identified against the code base at commit `0a1e14ce9c5d04d4281ea317c037d5ece999a2b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportFirewallDomains

## Description
Four net-new root causes in the Route 53 Resolver DNS Firewall domain-list plane, all surfaced by behavioral checks that are not covered by any open issue. Related but distinct open issues are named per cause; none of them carries these checks' replay keys.

Cause 1 of 4: ImportFirewallDomains accepts a malformed DomainFileUrl instead of refusing it.
AWS: Smithy: ImportFirewallDomains.DomainFileUrl targets DomainListFileUrl, @required, @length min 1 max 1024; the operation models ValidationException (client error, HTTP 400). AWS requires the value to be an S3 URL/URI; a value that is neither is refused with ValidationException, HTTP 400 and the message "[RSLVR-02038] A valid S3 URL or S3 URI is required." (recorded behavior; the whole sentence is fixed text — this message interpolates nothing from the request), and the target domain list is left untouched (same DomainCount, Status COMPLETE).
Emulator: the call returns HTTP 200 with {"Id":"rslvr-fdl-…","Name":"…","Status":"IMPORTING"} for DomainFileUrl="not-a-valid-s3-uri" — no validation of the URL form at all, and the list is pushed into IMPORTING by a request AWS never started.
Why: a client that relies on AWS refusing a typo'd or non-S3 URI sees a success and a list that reports IMPORTING; it never learns the import can never run, and error-handling paths keyed on ValidationException/RSLVR-02038 are dead code. Worse, the bogus import mutates the list's observable Status.
Fix: ImportFirewallDomains' input validation in the route53resolver handler — validate DomainFileUrl is a well-formed s3:// URI or S3 HTTPS URL before any state transition, and return ValidationException (400) with the RSLVR-02038 message. (The open issue "Implement route53resolver ImportFirewallDomains: unimplemented scaffold stub", 20261025T012620Z-route53resolver-importfirewalldomains-stub.md, is still open but the operation now answers 200, so it does not describe this; 20261108T111412Z-route53resolver-route-53-resolver-dns-firewall-responses-diverge-managed.md covers ImportFirewallDomains status fields, not URI validation.)
Traces: route53resolver.import-firewall-domains-invalid-s3-uri-rejected.

Cause 2 of 4: the ConflictException raised for a second ImportFirewallDomains while the list is still IMPORTING carries an emulator-authored message with no RSLVR code and no resource id.
AWS: Smithy: ImportFirewallDomains models ConflictException (client error, HTTP 400). A second import while the domain list is IMPORTING is refused with ConflictException and the message '[RSLVR-02305] The DNS Firewall domain list is in importing status: "<FirewallDomainListId>". Try again after the import completes.' (recorded behavior). Fixed text: "[RSLVR-02305] The DNS Firewall domain list is in importing status: " and ". Try again after the import completes."; the only variable part is the quoted FirewallDomainListId taken from the request (the rslvr-fdl-… id in the recordings fills that slot and is not part of the template).
Emulator: code is ConflictException, but Message is "The requested state transition isn't valid." — a generic, emulator-authored sentence with no RSLVR-02305 prefix, no domain-list id and no remediation clause.
Why: clients and operators match on the RSLVR code and parse the id out of the message to know which list is busy; a generic state-transition sentence names no resource and cannot be distinguished from any other conflict, so retry logic keyed on RSLVR-02305 never fires.
Fix: ImportFirewallDomains' conflict path in the route53resolver handler — build the ConflictException message from the RSLVR-02305 template with the requested FirewallDomainListId interpolated.
Traces: route53resolver.import-firewall-domains-conflict-while-in-progress, route53resolver.import-firewall-domains-conflict-while-in-progress-rejected.

Cause 3 of 4: UpdateFirewallDomains stores domains without lowercasing them, so ListFirewallDomains returns mixed case.
AWS: Smithy: UpdateFirewallDomains.Domains is @required FirewallDomains, ListFirewallDomains returns the same FirewallDomains shape; the model states no case rule, so recorded behavior only: DNS Firewall canonicalizes each stored domain to lowercase and appends the trailing FQDN dot, while preserving the wildcard "*." prefix and the exact punycode (xn--) encoding. A domain submitted as "CaseTest-<suffix>.Example" is listed back as "casetest-<suffix>.example.".
Emulator: the trailing dot is appended but the case is kept verbatim — ListFirewallDomains returned ['*.xn--mnchen-3ya-<suffix>.example.', 'CaseTest-<suffix>.Example.'] where AWS returns the lowercased form of both.
Why: DNS names are case-insensitive, so a client that stores the canonical lowercase form (or compares a query name against the listed domains) finds no match and concludes the domain is absent from the blocklist; set-membership and idempotent ADD/REMOVE logic built on the listed values breaks.
Fix: the domain canonicalization used by UpdateFirewallDomains (and shared with ImportFirewallDomains) in the route53resolver firewall domain-list state — lowercase each domain as it is stored, alongside the existing trailing-dot normalization. (The open issue 20261029T193214Z-route53resolver-dns-firewall-control-plane-10-net-new.md owns a different case-normalization check; it does not carry this check's replay key.)
Traces: route53resolver.firewall-domain-unicode-wildcard-round-trip.

Cause 4 of 4: the ResourceNotFoundException Message for GetFirewallRuleGroupAssociation on a deleted association omits the trailing X-Ray Trace Id clause.
AWS: Smithy: GetFirewallRuleGroupAssociation models ResourceNotFoundException (client error, HTTP 400). Recorded behavior: the Message is '[RSLVR-02025] Can’t find the resource with ID "<FirewallRuleGroupAssociationId>". Trace Id: "<root trace id>"' — fixed text "[RSLVR-02025] Can’t find the resource with ID ", then the quoted id from the request, then the literal ' Trace Id: ' and the quoted per-request X-Ray root id (shape 1-<8 hex>-<24 hex>; its value is per request and is not itself a requirement, but the presence and shape of the clause are).
Emulator: status 400 and code ResourceNotFoundException are correct and the head is correct, but the Message ends at '[RSLVR-02025] Can’t find the resource with ID "rslvr-frgassoc-…".' — the ' Trace Id: "…"' field is absent entirely, so the message is short by its last field.
Why: clients and support tooling parse the trace id out of the refusal to correlate the call with X-Ray; with the clause missing, that parse fails or yields an empty id, and any assertion on the full recorded refusal shape fails.
Fix: the route53resolver error serializer that renders RSLVR-coded refusal Messages — append the ' Trace Id: "<root>"' clause for the firewall rule-group-association not-found path too. Open issues 20261028T035605Z-route53resolver-route-53-resolver-error-messages-omit-the-trailing.md, 20261102T105111Z-route53resolver-route-53-resolver-refusal-messages-still-omit-the.md and 20261029T234450Z-route53resolver-route-53-resolver-not-found-refusal-messages-fail.md report the same omission on other operations; none carries this check, so the GetFirewallRuleGroupAssociation path must be fixed and re-replayed here.
Traces: route53resolver.e2e-firewall-domain-blocklist-lifecycle-flow.

## Evidence
Cause 1 — route53resolver.import-firewall-domains-invalid-s3-uri-rejected:
  expected: ClientError, code ValidationException, HTTP 400, Message "[RSLVR-02038] A valid S3 URL or S3 URI is required."; list unchanged (DomainCount == 2, Status COMPLETE)
  actual:   HTTP 200, body {'Id': 'rslvr-fdl-…', 'Name': 'sim2real-fdl-…', 'Status': 'IMPORTING'}
  assertion: "expected a ClientError, the call succeeded and returned {...'Status': 'IMPORTING'...}"

Cause 2 — route53resolver.import-firewall-domains-conflict-while-in-progress and ...-conflict-while-in-progress-rejected:
  expected code/message: ConflictException, '[RSLVR-02305] The DNS Firewall domain list is in importing status: "<FirewallDomainListId>". Try again after the import completes.'
  actual: code ConflictException, message "The requested state transition isn't valid."
  assertion: "expected message '[RSLVR-02305] The DNS Firewall domain list is in importing status: \"rslvr-fdl-910a2dec89025cc1\". Try again after the import completes.', got \"The requested state transition isn't valid.\""

Cause 3 — route53resolver.firewall-domain-unicode-wildcard-round-trip:
  added via UpdateFirewallDomains(Operation=ADD): ['*.xn--mnchen-3ya-<suffix>.example', 'CaseTest-<suffix>.Example']
  expected ListFirewallDomains Domains to contain 'casetest-<suffix>.example.'
  actual Domains: ['*.xn--mnchen-3ya-<suffix>.example.', 'CaseTest-<suffix>.Example.']
  assertion: "lowercased domain 'casetest-9688c2e6.example.' not found, got ['*.xn--mnchen-3ya-9688c2e6.example.', 'CaseTest-9688c2e6.Example.']"

Cause 4 — route53resolver.e2e-firewall-domain-blocklist-lifecycle-flow:
  GetFirewallRuleGroupAssociation after DisassociateFirewallRuleGroup
  expected Message: '[RSLVR-02025] Can’t find the resource with ID "<assoc-id>". Trace Id: "1-<8 hex>-<24 hex>"'
  actual Message:   '[RSLVR-02025] Can’t find the resource with ID "rslvr-frgassoc-1d0b14e4db018fed".' (code ResourceNotFoundException, HTTP 400 both correct)
  assertion: "GetFirewallRuleGroupAssociation after delete: unexpected trace id in '[RSLVR-02025] Can’t find the resource with ID \"rslvr-frgassoc-1d0b14e4db018fed\".'"

## Steps to Reproduce
Cause 1: CreateFirewallDomainList; put an object in S3 and ImportFirewallDomains(Operation=REPLACE, DomainFileUrl=s3://<bucket>/domains.txt); poll GetFirewallDomainList to COMPLETE; then call ImportFirewallDomains(FirewallDomainListId=<id>, Operation=REPLACE, DomainFileUrl="not-a-valid-s3-uri") — the emulator answers 200/IMPORTING where AWS answers 400 ValidationException [RSLVR-02038].

Cause 2: CreateFirewallDomainList; upload a domain file large enough that the import is still running (e.g. 3,000–90,000 domains); call ImportFirewallDomains(Operation=REPLACE) twice back to back; inspect Error.Message of the second call.

Cause 3: CreateFirewallDomainList; UpdateFirewallDomains(Operation=ADD, Domains=["*.xn--mnchen-3ya-<suffix>.example", "CaseTest-<suffix>.Example"]); poll to COMPLETE; ListFirewallDomains and compare the returned domains against the lowercased, trailing-dot form.

Cause 4: CreateFirewallDomainList + CreateFirewallRuleGroup + AssociateFirewallRuleGroup to a VPC; wait COMPLETE; DisassociateFirewallRuleGroup; then GetFirewallRuleGroupAssociation(FirewallRuleGroupAssociationId=<id>) and inspect Error.Message for the trailing ' Trace Id: "…"' clause.

Replay: run_behavioral_suite and re-run the checks listed in trace_ids.
