Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.op-rrset-malformed-ip-rejected route53.op-weighted-rrset-without-set-identifier-rejected route53.rrset-alias-excludes-ttl-and-resource-records route53.rrset-cname-at-zone-apex-rejected route53.rrset-cname-conflicts-with-same-name-record route53.rrset-latency-requires-region-and-set-identifier route53.txt-record-unquoted-value-with-spaces-rejected
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.op-rrset-malformed-ip-rejected|_client_error#10|AssertionError|raise AssertionError(f"expected the change to be rejected, but it succeeded: {result!r}")
Issue-Key: route53.op-weighted-rrset-without-set-identifier-rejected|_error_code_and_message#9|AssertionError|raise AssertionError(f"expected {what} to be rejected, but the call succeeded")
Issue-Key: route53.rrset-alias-excludes-ttl-and-resource-records|_expect_error#19|AssertionError|raise AssertionError(f"expected {what} to be rejected, but the call succeeded")
Issue-Key: route53.rrset-cname-at-zone-apex-rejected|_expect_error#9|AssertionError|raise AssertionError(f"expected {what} to be rejected, but the call succeeded")
Issue-Key: route53.rrset-cname-conflicts-with-same-name-record|_expect_error#7|AssertionError|raise AssertionError(f"expected {what} to be rejected, but the call succeeded")
Issue-Key: route53.rrset-latency-requires-region-and-set-identifier|_expect_error#15|AssertionError|raise AssertionError(f"expected {what} to be rejected, but the call succeeded")
Issue-Key: route53.txt-record-unquoted-value-with-spaces-rejected|_error#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# ChangeResourceRecordSets performs no record-set validation: seven classes of request AWS refuses are accepted with 200 PENDING

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangeResourceRecordSets

## Description
ChangeResourceRecordSets accepts every change batch whose request shape satisfies the Smithy model, so the record-set semantics Route 53 enforces inside the batch are not evaluated at all. Seven independent validation rules are missing; each is its own cause below. Smithy model for ChangeResourceRecordSets (services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json) declares both error classes used below: `InvalidChangeBatch` (HTTP 400) and `InvalidInput` (HTTP 400); success is HTTP 200 with ChangeInfo.

Cause 1 of 7 — an A record whose value is not a valid IPv4 address.
AWS: refuses the batch with HTTP 400 `InvalidChangeBatch`; recorded message (example) "[Invalid Resource Record: 'FATAL problem: ARRDATAIllegalIPv4Address (Value is not a valid IPv4 address) encountered with 'not-an-ip-address'']" — the rule is per-RDATA syntax validation of the submitted value, and the message is built from the request (it quotes the offending value).
Emulator: HTTP 200 with ChangeInfo {Status: PENDING} — the record is accepted.
Traces: route53.op-rrset-malformed-ip-rejected.

Cause 2 of 7 — a weighted record set that supplies `Weight` but no `SetIdentifier`.
AWS: refuses with HTTP 400 `InvalidInput`; recorded message (example) "Invalid request: Missing field 'SetIdentifier' in Change with [Action=CREATE, Name=<name>, Type=A, SetIdentifier=null]" — a request-shape fault naming the missing field, built from the request.
Emulator: the CREATE succeeds.
Traces: route53.op-weighted-rrset-without-set-identifier-rejected.

Cause 3 of 7 — alias record sets must carry exactly one of AliasTarget / TTL+ResourceRecords.
AWS: an alias record that also specifies `TTL`, an alias record that also specifies `ResourceRecords`, and a non-alias record with `ResourceRecords` but no `TTL` are all refused with HTTP 400 `InvalidInput` by one "exactly one of" rule (reported as "more than one" in the first two cases and "none" in the third), the message naming the record.
Emulator: all three are accepted (the check fails on the first: CREATE of an alias record that also specifies a TTL succeeds).
Traces: route53.rrset-alias-excludes-ttl-and-resource-records.

Cause 4 of 7 — a CNAME at the zone apex.
AWS: refuses with HTTP 400 `InvalidChangeBatch`; recorded message (example) "[RRSet of type CNAME with DNS name <zone>. is not permitted at apex in zone <zone>.]" — built from the request (the name and zone).
Emulator: the apex CNAME CREATE succeeds.
Traces: route53.rrset-cname-at-zone-apex-rejected.

Cause 5 of 7 — a CNAME colliding with an existing record of another type at the same name (and the mirror case, an A at a name that already holds a CNAME).
AWS: refuses with HTTP 400 `InvalidChangeBatch`; recorded message (example) "[RRSet of type CNAME with DNS name <name> is not permitted because a conflicting RRSet of type A with the same DNS name already exists in zone <zone>]" — built from the request and the colliding set.
Emulator: the conflicting CREATE succeeds, leaving a CNAME coexisting with an A at one name.
Traces: route53.rrset-cname-conflicts-with-same-name-record.

Cause 6 of 7 — latency routing: `Region` without `SetIdentifier`, and an unresolvable region value.
AWS: `Region` with no `SetIdentifier` is refused with HTTP 400 `InvalidInput` ("Missing field 'SetIdentifier' ..."), while a syntactically valid but unknown region is refused one stage later with HTTP 400 `InvalidChangeBatch`, recorded message (example) "[Cannot find region 'xx-fake-1'. No changes made.]". Two distinct stages, two distinct error classes.
Emulator: the CREATE with Region and no SetIdentifier succeeds, so neither stage is exercised.
Traces: route53.rrset-latency-requires-region-and-set-identifier.

Cause 7 of 7 — a TXT value that is not enclosed in quotation marks.
AWS: refuses with HTTP 400 `InvalidChangeBatch`; recorded message (example) "[Invalid Resource Record: 'FATAL problem: InvalidCharacterString (Value should be enclosed in quotation marks) encountered with 'v=spf1 include:example.net ~all'']" — character-string syntax validation, message built from the submitted value.
Emulator: HTTP 200 with ChangeInfo {Status: PENDING}; the unquoted TXT record is stored.
Traces: route53.txt-record-unquoted-value-with-spaces-rejected.

Why: a client that relies on Route 53 to reject these batches silently gets a "successful" change for a zone state AWS would never allow — a CNAME at an apex, a CNAME beside an A, an unquoted TXT string, an A record holding a non-IP, weighted/latency sets with no discriminator. Deployment tooling (CDK/Terraform/CFN) treats the 200 as proof the record is valid, so the divergence surfaces much later as a DNS answer that cannot exist in AWS, and negative-path tests written against AWS pass vacuously here.

Fix: the ChangeResourceRecordSets handler and its change-batch validation — services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/change_resource_record_sets.rs. Request-shape faults (missing SetIdentifier, the alias exactly-one rule) must be raised as `InvalidInput`; batch-evaluation faults (bad RDATA, apex CNAME, type conflicts, unknown region, unquoted character strings) as `InvalidChangeBatch`, and in every case the batch must have no effect on zone state.

## Evidence
Cause 1 — expected InvalidChangeBatch (400); actual: AssertionError: expected the change to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'ChangeInfo': {'Id': '/change/CYLK4UNXEP225RB', 'Status': 'PENDING', ...}}
Cause 2 — expected InvalidInput (400); actual: AssertionError: expected CREATE of a weighted record at weighted.<zone> with a Weight but no SetIdentifier to be rejected, but the call succeeded.
Cause 3 — expected InvalidInput (400); actual: AssertionError: expected CREATE of an alias record bad-ttl.<zone> that also specifies a TTL to be rejected, but the call succeeded.
Cause 4 — expected InvalidChangeBatch (400); actual: AssertionError: expected CREATE of a CNAME at the zone apex <zone> to be rejected, but the call succeeded.
Cause 5 — expected InvalidChangeBatch (400); actual: AssertionError: expected CREATE of a CNAME at a-first.<zone>, which already holds an A to be rejected, but the call succeeded.
Cause 6 — expected InvalidInput (400); actual: AssertionError: expected CREATE of a latency record lonely.<zone> with a Region but no SetIdentifier to be rejected, but the call succeeded.
Cause 7 — expected InvalidChangeBatch (400); actual: AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'ChangeInfo': {'Id': '/change/C6SX53VC4MLWGUD', 'Status': 'PENDING', ...}}

## Steps to Reproduce
For each cause, create a public hosted zone (CreateHostedZone with HostedZoneConfig.PrivateZone=false) and submit a single-change batch via ChangeResourceRecordSets:
1. CREATE A record with ResourceRecords=[{"Value": "not-an-ip-address"}], TTL 60 -> expect 400 InvalidChangeBatch, got 200 PENDING.
2. CREATE A record with Weight=10 and no SetIdentifier -> expect 400 InvalidInput, got 200.
3. CREATE A record with both AliasTarget and TTL -> expect 400 InvalidInput, got 200 (likewise AliasTarget+ResourceRecords, and ResourceRecords with no TTL).
4. CREATE CNAME whose Name is the zone apex -> expect 400 InvalidChangeBatch, got 200.
5. CREATE A at a-first.<zone>, then CREATE CNAME at the same name -> expect 400 InvalidChangeBatch, got 200.
6. CREATE A record with Region="us-west-2" and no SetIdentifier -> expect 400 InvalidInput, got 200; with SetIdentifier and Region="xx-fake-1" -> expect 400 InvalidChangeBatch.
7. CREATE TXT with ResourceRecords=[{"Value": "v=spf1 include:example.net ~all"}] (unquoted) -> expect 400 InvalidChangeBatch, got 200.
