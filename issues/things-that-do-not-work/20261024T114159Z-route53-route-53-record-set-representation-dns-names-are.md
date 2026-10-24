Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.rrset-wildcard-name-returned-octal-escaped route53.rrset-weighted-siblings-require-distinct-set-identifiers route53.op-delete-apex-soa-rejected
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.rrset-wildcard-name-returned-octal-escaped|run#10|AssertionError|assert wildcard_escaped in names, (
Issue-Key: route53.rrset-weighted-siblings-require-distinct-set-identifiers|run#16|AssertionError|assert msg == (
Issue-Key: route53.op-delete-apex-soa-rejected|run#19|AssertionError|assert msg == expected_message, f"expected message {expected_message!r}, got {msg!r}"
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Route 53 record-set representation: DNS names are not normalized in responses, and two InvalidChangeBatch discriminators are wrong

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListResourceRecordSets

## Description
Three independent defects in how the emulator represents record sets and describes record-set refusals. The error CODE is correct in causes 2 and 3 (`InvalidChangeBatch`, HTTP 400); what diverges is the discriminator text, and in both cases the rule that produces it is known (it is built from the request's record identity, or it states the constraint AWS enforces), so it is the part a client keys on to tell these refusals apart.

Cause 1 of 3: ListResourceRecordSets returns DNS names un-normalized.
AWS: record-set names come back in Route 53's canonical presentation form — non-printable/special octets are octal-escaped (a wildcard label `*` is returned as `\\052`) and names are case-folded to lower case; both spellings of a name refer to ONE record set (creating `\\052.<zone>` after `*.<zone>` is a duplicate, and the escaped form is accepted verbatim as a DELETE). Recorded behavior of the check: a zone created with `*.<zone>` and `MiXeD.<zone>` lists back as `\\052.<zone>` and `mixed.<zone>`.
Emulator: ListResourceRecordSets returns the submitted spelling verbatim — `['<zone>', '<zone>', '*.<zone>', 'MiXeD.<zone>']` — so neither the octal escape nor the lower-casing is applied, and the duplicate/DELETE round-trip semantics that depend on the normalization cannot hold.
Why: a client comparing the names it created against the names Route 53 returns (every reconcile loop in DNS tooling does) sees a name that never matches, and a DELETE submitted with the canonical name AWS handed it is treated as naming a different record set.
Fix: the record-set serializer / state key for the route53 record store — services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/list_resource_record_sets.rs together with the name normalization used by services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/change_resource_record_sets.rs (one canonical form on write, that same form on read).
Traces: route53.rrset-wildcard-name-returned-octal-escaped.

Cause 2 of 3: the duplicate-CREATE discriminator omits SetIdentifier, i.e. record-set identity is keyed on (Name, Type) instead of the triple (Name, Type, SetIdentifier).
AWS: a duplicate CREATE is refused with HTTP 400 `InvalidChangeBatch` whose message echoes the FULL identity of the colliding record set; for a weighted set that includes the set identifier — recorded form "[Tried to create resource record set [name='<name>', type='A', set-identifier='blue'] but it already exists]".
Emulator: the same refusal is reported as "[Tried to create resource record set [name='wt.<zone>', type='A'] but it already exists]" — no `set-identifier=` component, the (Name, Type) form.
Why: the echo is how a client learns WHICH sibling collided; with the identifier dropped, a canary/blue-green deployment that reuses one identifier cannot tell that refusal from a collision on the whole name+type group. It also suggests identity is tracked as a pair rather than the triple, which is the difference between a legal weighted group and an overwrite.
Fix: the duplicate-detection branch of the ChangeResourceRecordSets handler — services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/change_resource_record_sets.rs (key record sets on Name+Type+SetIdentifier and name all three in the InvalidChangeBatch discriminator).
Traces: route53.rrset-weighted-siblings-require-distinct-set-identifiers.

Cause 3 of 3: DELETE of the apex SOA is refused under the wrong rule.
AWS: HTTP 400 `InvalidChangeBatch` stating the zone-level constraint it violates — recorded message "[A HostedZone must contain exactly one SOA record.]" (the rule is the SOA cardinality constraint, not a protection on "default" records).
Emulator: HTTP 400 `InvalidChangeBatch` with "[Tried to delete resource record set [name='<zone>', type='SOA'] but it is one of the hosted zone's own default NS and SOA records, which cannot be deleted]".
Why: the code and the effect (the SOA survives) match, so this is the narrowest of the three; it still misreports the constraint, and a client that parses the batch message to distinguish a cardinality violation from a protected-record refusal classifies it wrongly.
Fix: the apex-record branch of the ChangeResourceRecordSets handler — services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/change_resource_record_sets.rs.
Traces: route53.op-delete-apex-soa-rejected.

## Evidence
Cause 1 — AssertionError: the wildcard must be returned as '\\052.sim2real-behav-zone822dfb496cb8.example.test.', got ['sim2real-behav-zone822dfb496cb8.example.test.', 'sim2real-behav-zone822dfb496cb8.example.test.', '*.sim2real-behav-zone822dfb496cb8.example.test.', 'MiXeD.sim2real-behav-zone822dfb496cb8.example.test.'] (the mixed-case name is also returned unfolded).
Cause 2 — AssertionError: expected the duplicate-CREATE discriminator naming the triple, got "[Tried to create resource record set [name='wt.sim2real-behav-zone80f09d74085e.example.test.', type='A'] but it already exists]"; expected "[Tried to create resource record set [name='wt.<zone>', type='A', set-identifier='blue'] but it already exists]". (The positive half passed: two weighted siblings with distinct SetIdentifiers do coexist and round-trip.)
Cause 3 — AssertionError: expected message '[A HostedZone must contain exactly one SOA record.]', got "[Tried to delete resource record set [name='sim2real-behav-chk-op-soa04b6abbeeee4.sim2real-probe-zone.com.', type='SOA'] but it is one of the hosted zone's own default NS and SOA records, which cannot be deleted]" (code InvalidChangeBatch in both).

## Steps to Reproduce
1. CreateHostedZone (public). ChangeResourceRecordSets CREATE A records named "*.<zone>" and "MiXeD.<zone>"; call ListResourceRecordSets and observe the names returned verbatim instead of "\\052.<zone>" / "mixed.<zone>".
2. In the same zone, CREATE two weighted A records at wt.<zone> with SetIdentifiers "blue" and "green"; then CREATE a third reusing SetIdentifier "blue" and read the InvalidChangeBatch message — it omits set-identifier='blue'.
3. List the new zone's record sets, take the apex SOA and submit it as a DELETE; read the InvalidChangeBatch message.
