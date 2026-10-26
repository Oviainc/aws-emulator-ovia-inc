Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.route53-geolocation-routing-is-evaluated-against-the-edns0-client-subnet route53.test-dns-answer-invalid-record-type-rejected
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.route53-geolocation-routing-is-evaluated-against-the-edns0-client-subnet|_poll#8|Timeout|time.sleep(interval_s)
Issue-Key: route53.test-dns-answer-invalid-record-type-rejected|run#14|AssertionError|assert err.get("Message") == (
Issue-Commit: cb933d25eac9e1275ee3ecb224ec471b338d603c

# Route 53 TestDNSAnswer: geolocation routing is not evaluated against EDNS0ClientSubnetIP, and the invalid-RecordType refusal lists the enum value set in model order rather than AWS's (2 causes)

> _Note: this issue was identified against the code base at commit `cb933d25eac9e1275ee3ecb224ec471b338d603c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestDNSAnswer

## Description
Two independent net-new causes in the route53 TestDNSAnswer path. Both are grounded in failing checks of this replay that no open issue claims.

Cause 1 of 2 — TestDNSAnswer does not resolve a geolocation record group per client subnet.
AWS: TestDNSAnswer computes the answer the way the authoritative name servers would, and EDNS0ClientSubnetIP/EDNS0ClientSubnetMask supply the apparent client location used for geolocation routing. With three sibling A record sets at one name, distinguished only by SetIdentifier + GeoLocation (CountryCode 'DE', CountryCode 'JP', and the CountryCode '*' default), a query carrying a German client subnet returns ONLY the DE record, a Japanese subnet ONLY the JP record, and a subnet matching neither country falls through to the '*' default; after the '*' default is DELETED, that same unmatched subnet is answered NODATA (ResponseCode 'NOERROR' with RecordData []) because the name still exists, while DE and JP keep answering normally. Smithy: TestDNSAnswer.EDNS0ClientSubnetIP -> IPAddress (httpQuery 'edns0clientsubnetip'), EDNS0ClientSubnetMask -> SubnetMask (httpQuery 'edns0clientsubnetmask'); output RecordData/ResponseCode are required. The per-country expected values and the NODATA outcome are recorded behavior (the check documents them as measured against real AWS 2026-09-10).
Emulator: the check never observes the geolocation-matched answer and is killed by the harness deadline ('Check timed out after 60.0s') while polling TestDNSAnswer from the German client subnet for RecordData == ['203.0.113.80']. The preceding steps are not the blocker: the sibling check route53.test-dns-answer-invalid-record-type-rejected shows ChangeResourceRecordSets reaching INSYNC via GetChange and a plain TestDNSAnswer resolving a record in the same run, so the wait that never converges is the geolocation-discriminated answer. Hypothesis: the emulator ignores EDNS0ClientSubnetIP entirely and treats the geo group as an unordered record set (returning all siblings, an arbitrary sibling, or nothing), so no client subnet ever selects its country's record.
Why: a client cannot test or rely on geolocation routing at all. Three queries differing only in client subnet must produce three different answers; an emulator that returns an arbitrary sibling sends traffic to the wrong regional endpoint, and one that returns nothing makes a correctly configured geo zone look broken. The deletion half of the contract is equally unobservable: removing the '*' default must turn the unmatched subnet into NOERROR/no-records rather than keep serving a sibling the geo rules do not match.
Fix: the TestDNSAnswer handler's resolution logic in the route53 crate (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/test_dns_answer.rs and the record-set resolution it calls) — select among GeoLocation siblings using EDNS0ClientSubnetIP/Mask, fall through to CountryCode '*' when no country/continent sibling matches, and return NOERROR with an empty RecordData when neither a match nor a default exists.
Traces: route53.route53-geolocation-routing-is-evaluated-against-the-edns0-client-subnet.

Cause 2 of 2 — the invalid-RecordType refusal message lists the enum value set in Smithy declaration order instead of AWS's order.
AWS: an unsupported RecordType is refused with HTTP 400, code InvalidInput, Type Sender, and the message '1 validation error detected: Value '<the value sent>' at 'recordType' failed to satisfy constraint: Member must satisfy enum value set: [AAAA, A, SVCB, CAA, NS, SOA, TLSA, SPF, HTTPS, MX, PTR, CNAME, DS, TXT, SSHFP, SRV, NAPTR]'. Fixed text: the '1 validation error detected: Value ... at 'recordType' failed to satisfy constraint: Member must satisfy enum value set: [...]' frame AND the bracketed value list including its order; variable part: only the rejected value, taken from the request. Smithy: RecordType targets com.amazonaws.route53#RRType, whose declaration order is [SOA, A, TXT, NS, CNAME, MX, NAPTR, PTR, SRV, SPF, AAAA, CAA, DS, TLSA, SSHFP, SVCB, HTTPS] — i.e. NOT the order AWS prints; the modeled error is InvalidInput (400).
Emulator: status and code are right (400 / InvalidInput / Type Sender) but the message renders the value set in the model's declaration order: '... Member must satisfy enum value set: [SOA, A, TXT, NS, CNAME, MX, NAPTR, PTR, SRV, SPF, AAAA, CAA, DS, TLSA, SSHFP, SVCB, HTTPS]'. The emulator is clearly deriving the list from the Smithy member order rather than from AWS's fixed ordering.
Why: clients and test suites that assert on the validation message (the only place the accepted set is reported) see a message that never matches AWS, so the refusal is not byte-compatible even though the code is.
Fix: the enum-validation message rendering for TestDNSAnswer.RecordType in the route53 crate's validation layer (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/test_dns_answer.rs and the shared enum-constraint message builder) — emit AWS's published value order for RRType rather than the model's member order.
Traces: route53.test-dns-answer-invalid-record-type-rejected.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline.

Cause 1 — check route53.route53-geolocation-routing-is-evaluated-against-the-edns0-client-subnet (verdict: mismatch).
Setup: one public hosted zone; UPSERT of three A record sets at geo.<zone>, TTL 60, same name/type, distinguished by SetIdentifier/GeoLocation — ('de', CountryCode 'DE', 203.0.113.80), ('jp', CountryCode 'JP', 203.0.113.81), ('default', CountryCode '*', 203.0.113.82); ChangeResourceRecordSets waited to INSYNC.
Expected (AWS): test_dns_answer(HostedZoneId=<zone>, RecordName=geo.<zone>, RecordType='A', EDNS0ClientSubnetIP='62.155.0.0', EDNS0ClientSubnetMask='24') -> RecordData ['203.0.113.80'], ResponseCode 'NOERROR'; with EDNS0ClientSubnetIP='210.130.0.0' -> ['203.0.113.81']; with EDNS0ClientSubnetIP='8.8.8.0' -> ['203.0.113.82'] (the '*' default). After DELETE of the '*' default: the 8.8.8.0 query -> ResponseCode 'NOERROR' with RecordData [], while 62.155.0.0 still -> ['203.0.113.80'] and the surviving group is exactly {de, jp} / {DE, JP}.
Actual (emulator): 'Error: Check timed out after 60.0s' — the poll 'TestDNSAnswer(geo.<zone>) from a German client subnet to return the DE record' (budget 240s, interval 5s) never satisfied RecordData == ['203.0.113.80'], so the harness deadline fired before any assertion was reached. No per-country selection was ever observed.
Control that isolates it to geo resolution: in route53.test-dns-answer-invalid-record-type-rejected (same replay) ChangeResourceRecordSets reached INSYNC through GetChange and TestDNSAnswer returned RecordType 'A' with the created record, so zone creation, change propagation and plain TestDNSAnswer all work in this build.

Cause 2 — check route53.test-dns-answer-invalid-record-type-rejected (verdict: mismatch).
Call: TestDNSAnswer with HostedZoneId=<zone>, RecordName=<zone>, RecordType='NOTAREALTYPE' (botocore client-side param validation bypassed so the server-side check is exercised).
Expected (AWS): Code 'InvalidInput', Message "1 validation error detected: Value 'NOTAREALTYPE' at 'recordType' failed to satisfy constraint: Member must satisfy enum value set: [AAAA, A, SVCB, CAA, NS, SOA, TLSA, SPF, HTTPS, MX, PTR, CNAME, DS, TXT, SSHFP, SRV, NAPTR]".
Actual (emulator): {'Message': "1 validation error detected: Value 'NOTAREALTYPE' at 'recordType' failed to satisfy constraint: Member must satisfy enum value set: [SOA, A, TXT, NS, CNAME, MX, NAPTR, PTR, SRV, SPF, AAAA, CAA, DS, TLSA, SSHFP, SVCB, HTTPS]", 'Code': 'InvalidInput', 'Type': 'Sender'} — AssertionError on the message.
Model evidence (smithy_query on services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json): RRType enum values in declaration order are exactly the list the emulator printed; TestDNSAnswer errors are InvalidInput(400) and NoSuchHostedZone(404); EDNS0ClientSubnetIP/EDNS0ClientSubnetMask are modeled httpQuery inputs of TestDNSAnswer.

No open issue claims either check: the TestDNSAnswer stub issue (20261024T114159Z-route53-testdnsanswer-stub.md) is already fixed, and the open route53 validation-semantics issue 20261024T160546Z-route53-route-53-validation-semantics-gaps-five-operations-answer.md covers other operations and does not carry these replay keys.

## Steps to Reproduce
Cause 1:
1. CreateHostedZone (public) and UPSERT three A record sets at geo.<zone> with TTL 60 and identical name/type, differing only in SetIdentifier + GeoLocation: ('de', {CountryCode: 'DE'}, 203.0.113.80), ('jp', {CountryCode: 'JP'}, 203.0.113.81), ('default', {CountryCode: '*'}, 203.0.113.82). Wait for GetChange -> INSYNC.
2. Call TestDNSAnswer(HostedZoneId=<zone>, RecordName=geo.<zone>, RecordType='A', EDNS0ClientSubnetIP='62.155.0.0', EDNS0ClientSubnetMask='24'). Observe that RecordData is never ['203.0.113.80'] (the poll never converges and the check is killed at its deadline).
3. Repeat with EDNS0ClientSubnetIP='210.130.0.0' (expect ['203.0.113.81']) and '8.8.8.0' (expect the '*' default ['203.0.113.82']).
4. DELETE the '*' default record set, wait INSYNC, and query again from '8.8.8.0': expect ResponseCode 'NOERROR' with RecordData [], and the 62.155.0.0 query still ['203.0.113.80'].

Cause 2:
1. CreateHostedZone and create an A record at the apex (any zone works).
2. Call TestDNSAnswer(HostedZoneId=<zone>, RecordName=<zone>, RecordType='NOTAREALTYPE') with client-side parameter validation bypassed.
3. Compare the returned Error.Message with AWS's: the bracketed enum value set must read [AAAA, A, SVCB, CAA, NS, SOA, TLSA, SPF, HTTPS, MX, PTR, CNAME, DS, TXT, SSHFP, SRV, NAPTR]; the emulator prints the Smithy declaration order instead.
