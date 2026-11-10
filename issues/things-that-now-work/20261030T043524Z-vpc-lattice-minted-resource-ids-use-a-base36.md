Issue-Source: trace-sweep-divergence
Issue-Trace: vpclattice-service-and-listener-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: vpclattice-service-and-listener-lifecycle vpclattice-service-network-service-association
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice|CreateService|2xx->2xx|$.arn
Issue-Key: vpc-lattice|CreateService|2xx->2xx|$.dnsEntry.domainName
Issue-Key: vpc-lattice|CreateService|2xx->2xx|$.id
Issue-Key: vpc-lattice|GetService|2xx->2xx|$.arn
Issue-Key: vpc-lattice|GetService|2xx->2xx|$.dnsEntry.domainName
Issue-Key: vpc-lattice|CreateListener|2xx->2xx|$.arn
Issue-Key: vpc-lattice|CreateListener|2xx->2xx|$.id
Issue-Key: vpc-lattice|CreateListener|2xx->2xx|$.serviceArn
Issue-Key: vpc-lattice|GetListener|2xx->2xx|$.arn
Issue-Key: vpc-lattice|GetListener|2xx->2xx|$.serviceArn
Issue-Key: vpc-lattice|CreateListener|4xx->4xx|$.resourceId
Issue-Key: vpc-lattice|DeleteService|2xx->2xx|$.arn
Issue-Key: vpc-lattice|CreateServiceNetworkServiceAssociation|2xx->2xx|$.arn
Issue-Key: vpc-lattice|CreateServiceNetworkServiceAssociation|2xx->2xx|$.dnsEntry.domainName
Issue-Key: vpc-lattice|CreateServiceNetworkServiceAssociation|2xx->2xx|$.id
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.arn
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.dnsEntry.domainName
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.id
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.serviceArn
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.serviceId
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.serviceNetworkArn
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.serviceNetworkId
Issue-Key: vpc-lattice|ListServiceNetworkServiceAssociations|2xx->2xx|$.items[*].arn
Issue-Key: vpc-lattice|ListServiceNetworkServiceAssociations|2xx->2xx|$.items[*].dnsEntry.domainName
Issue-Key: vpc-lattice|ListServiceNetworkServiceAssociations|2xx->2xx|$.items[*].id
Issue-Key: vpc-lattice|ListServiceNetworkServiceAssociations|2xx->2xx|$.items[*].serviceArn
Issue-Key: vpc-lattice|ListServiceNetworkServiceAssociations|2xx->2xx|$.items[*].serviceId
Issue-Key: vpc-lattice|ListServiceNetworkServiceAssociations|2xx->2xx|$.items[*].serviceNetworkArn
Issue-Key: vpc-lattice|ListServiceNetworkServiceAssociations|2xx->2xx|$.items[*].serviceNetworkId
Issue-Key: vpc-lattice|DeleteServiceNetworkServiceAssociation|2xx->2xx|$.arn
Issue-Key: vpc-lattice|DeleteServiceNetworkServiceAssociation|2xx->2xx|$.id
Issue-Commit: 29f1e00a7002e451c541eb91ee53a740b213bc0d

# vpc-lattice minted resource ids use a base36 alphabet instead of hex, corrupting ids, ARNs and DNS names

> _Note: this issue was identified against the code base at commit `29f1e00a7002e451c541eb91ee53a740b213bc0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
BUNDLE OF 10 CAUSES, one root fault. VPC Lattice mints the 17-character suffix of every resource id from the LOWERCASE HEXADECIMAL alphabet [0-9a-f]; the emulator mints it from base36 [0-9a-z]. The wrong alphabet then propagates into every value that embeds an id: the resource's own `id`, every ARN (service, listener, association, and the nested listener ARN), the `resourceId` member of ConflictException, and the `dnsEntry.domainName` label, whose service-id suffix is part of the name. Every cause below is one divergence cluster produced by that single minting defect; all ten must be closed before submitting.

Shared AWS reference: the recorded AWS responses mint hex-only suffixes — the recorded listener is `listener-0c26c95e4c2cda1d0`, 17 characters all in [0-9a-f]. Smithy constrains only the looser outer form (ServiceId @pattern ^svc-[0-9a-z]{17}$ @length 21..21; ListenerId @pattern ^listener-[0-9a-z]{17}$ @length 26..26; ServiceNetworkServiceAssociationArn @pattern ^arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:servicenetworkserviceassociation/snsa-[0-9a-z]{17}$; DnsEntry.domainName is an unconstrained String), so the hex restriction is RECORDED BEHAVIOR ONLY, not modeled — which is why the emulator's base36 ids still satisfy the model yet diverge on the wire. Minting hex keeps every modeled pattern satisfied, so the fix is safe.

Shared Fix: the id minter in the service's state module — `mint_id(prefix)` builds `prefix + encode_base36(fresh_entropy(), 17, BASE36_LOWER)`; it must draw the 17-character suffix from the hexadecimal alphabet instead. `arn_for`/`listener_arn`/`rule_arn` and `service_domain_name` compose the minted suffix and need no change once minting is fixed; they are listed per-cause only because that is where the wrong alphabet becomes visible on the wire.

A NOTE FOR THE FIXER on how these render: for the create-path causes the harness asserts the SHAPE of a first-seen value (reported as expected '<shaped-not-asserted>'), and the shape fails on the non-hex suffix. For the read/delete-path causes the harness has aliased the recorded id to the emulator's id for DISPLAY, so the detail prints the expected and actual strings identically while the comparison still fails on the embedded identifier; the substantive difference in those clusters is the same id alphabet, not the region, the account id (000000000000 in both), or any other ARN segment. Do not read the identical-looking strings as a no-op: they clear only once ids are minted in AWS's alphabet.

Cause 1 of 10 — CreateService, fixture vpclattice-service-and-listener-lifecycle step task#1 (HTTP 201 in both).
AWS: returns `id` matching ^svc-<17 hex>$, `arn` .../service/svc-<17 hex>, and `dnsEntry.domainName` of the form <service name from the request>-<the id's 17-hex suffix>.<7-hex account segment>.vpc-lattice-svcs.<region>.on.aws (recorded behavior only; Smithy: CreateService 201, output members ServiceId/ServiceArn/DnsEntry).
Emulator: mints a base36 suffix, so all three fail their shape assertion — `$.id`, `$.arn` and `$.dnsEntry.domainName` are reported value_mismatch against expected '<shaped-not-asserted>'; the actual id suffix carries letters outside [0-9a-f].
Why: a client that validates or pattern-matches the returned service id or ARN, or resolves the returned domain name, rejects a value AWS would never emit.
Fix: the state module's id minter (alphabet), consumed by CreateService's handler via arn_for and service_domain_name.
Traces: vpclattice-service-and-listener-lifecycle#task#1.

Cause 2 of 10 — GetService, fixture vpclattice-service-and-listener-lifecycle step task#2 (HTTP 200 in both).
AWS: echoes the stored service's `arn` and `dnsEntry.domainName` exactly as CreateService returned them, with the hex id suffix.
Emulator: `$.arn` and `$.dnsEntry.domainName` both diverge — the stored values carry the base36 suffix minted in cause 1.
Why: the read path hands back an identifier a caller cannot match against anything AWS produced, so recorded/golden comparisons and any client-side id validation fail on read as well as on create.
Fix: same minting alphabet; GetService's serializer needs no separate change.
Traces: vpclattice-service-and-listener-lifecycle#task#2.

Cause 3 of 10 — CreateListener, fixture vpclattice-service-and-listener-lifecycle step task#5 (HTTP 201 in both).
AWS: `id` matches ^listener-<17 hex>$ and `arn` nests under the service: arn:<partition>:vpc-lattice:<region>:<account>:service/svc-<17 hex>/listener/listener-<17 hex>. The recording shows `listener-0c26c95e4c2cda1d0`.
Emulator: returns `listener-r3y9msjz0uywa2j0x`; `$.id` fails its shape assertion and `$.arn` diverges from the recorded listener ARN, as does the echoed `$.serviceArn`.
Why: the listener ARN is the handle a client stores and passes to GetListener/CreateRule; one built from a non-AWS alphabet breaks any consumer that validates it.
Fix: the state module's id minter (alphabet), consumed through listener_arn.
Traces: vpclattice-service-and-listener-lifecycle#task#5.

Cause 4 of 10 — GetListener, fixture vpclattice-service-and-listener-lifecycle step task#6 (HTTP 200 in both).
AWS: echoes the listener `arn` and its parent `serviceArn`, both with hex id suffixes.
Emulator: `$.arn` and `$.serviceArn` diverge on the embedded base36 suffixes.
Why: same as cause 2, on the listener read path.
Fix: same minting alphabet.
Traces: vpclattice-service-and-listener-lifecycle#task#6.

Cause 5 of 10 — CreateListener duplicate-name conflict, fixture vpclattice-service-and-listener-lifecycle step task#7 (HTTP 409 in both).
AWS: ConflictException (HTTP 409, Smithy: ConflictException with required `message`, `resourceId`, `resourceType`) carries in `resourceId` the id of the CONFLICTING listener — fixed text comes from the error class, while `resourceId` is a value taken from emulator state, namely the existing listener's id, which matches ^listener-<17 hex>$.
Emulator: status, code and message match; `$.resourceId` fails its shape assertion because the stored listener id is base36 (`listener-r3y9msjz0uywa2j0x`).
Why: a client that parses `resourceId` out of a 409 to look the conflicting listener up receives an id of a form AWS never emits.
Fix: same minting alphabet; the conflict-construction path already reports the right member.
Traces: vpclattice-service-and-listener-lifecycle#task#7.

Cause 6 of 10 — DeleteService, fixture vpclattice-service-and-listener-lifecycle step task#13 (HTTP 200 in both).
AWS: the delete response echoes the deleted service's `arn` (Smithy: DeleteService output ServiceArn) with its hex id suffix.
Emulator: `$.arn` diverges on the embedded base36 suffix.
Why: a caller correlating the delete acknowledgement with the ARN it created cannot match them against AWS-shaped values.
Fix: same minting alphabet.
Traces: vpclattice-service-and-listener-lifecycle#task#13.

Cause 7 of 10 — CreateServiceNetworkServiceAssociation, fixture vpclattice-service-network-service-association step task#4 (HTTP 200 in both).
AWS: `id` matches ^snsa-<17 hex>$, `arn` .../servicenetworkserviceassociation/snsa-<17 hex>, and `dnsEntry.domainName` is the associated SERVICE's domain name, i.e. <service name>-<service id's 17-hex suffix>.<7-hex account segment>.vpc-lattice-svcs.<region>.on.aws.
Emulator: `$.id`, `$.arn` and `$.dnsEntry.domainName` all fail their shape assertions; the association id (`snsa-e7h39tvl1nmuvm6w8`) and the service-id suffix inside the domain name are base36.
Why: the association ARN/id is the handle for Get/Delete of the association, and the domain name is what a client actually resolves; both carry a non-AWS alphabet.
Fix: same minting alphabet, via arn_for and service_domain_name.
Traces: vpclattice-service-network-service-association#task#4.

Cause 8 of 10 — GetServiceNetworkServiceAssociation, fixture vpclattice-service-network-service-association step task#5 (HTTP 200 in both).
AWS: echoes `id`, `arn`, `serviceId`, `serviceArn`, `serviceNetworkId`, `serviceNetworkArn` (Smithy: ServiceNetworkId @pattern ^sn-[0-9a-z]{17}$) and `dnsEntry.domainName`, every identifier with a hex suffix.
Emulator: all seven keys diverge — the association, service and service-network ids were all minted base36, and `$.dnsEntry.domainName` additionally fails its shape assertion.
Why: this is the operation a client polls to learn the association's identifiers and DNS name; none of them has an AWS-shaped value.
Fix: same minting alphabet.
Traces: vpclattice-service-network-service-association#task#5.

Cause 9 of 10 — ListServiceNetworkServiceAssociations, fixture vpclattice-service-network-service-association step task#7 (HTTP 200 in both).
AWS: each `items[]` summary repeats the association's `arn`/`id`, the service's `serviceArn`/`serviceId`, the network's `serviceNetworkArn`/`serviceNetworkId` and `dnsEntry.domainName`, all hex-suffixed.
Emulator: the same seven keys diverge on `$.items[0]`, so the list projection inherits the defect for every listed association.
Why: a client enumerating associations and matching them against ids it holds, or against recorded values, fails for every element of the page.
Fix: same minting alphabet; the list serializer itself is correct.
Traces: vpclattice-service-network-service-association#task#7.

Cause 10 of 10 — DeleteServiceNetworkServiceAssociation, fixture vpclattice-service-network-service-association step task#9 (HTTP 200 in both).
AWS: the delete response echoes the association's `id` and `arn` with the hex suffix.
Emulator: `$.id` and `$.arn` diverge on the embedded base36 suffix.
Why: same as cause 6, on the association teardown path.
Fix: same minting alphabet.
Traces: vpclattice-service-network-service-association#task#9.

Related but NOT covered here: the open issue 20261029T192412Z-vpc-lattice-getrule-returns-stale-rule-state-priority.md owns the GetRule stale-state divergence (it carries that replay key), and the vpc-lattice stub issues filed for BatchUpdateRule, CreateTargetGroup, ListRules, PutAuthPolicy, PutResourcePolicy and TagResource own the blocked steps. Neither set covers the identifier-alphabet fault above.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; 110 steps, 45 passed, 39 mismatched, 26 blocked by stubs; score 40.9%). Ten net-new vpc-lattice mismatch clusters, none with a stub_path and none owned by an open issue:

1. CreateService vpclattice-service-and-listener-lifecycle#task#1 — expected 201, actual 201. $.arn: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:vpc-lattice:us-west-2:000000000000:service/svc-l7wf07q0hiwe8vs6g'); $.dnsEntry.domainName: value_mismatch (expected '<shaped-not-asserted>', got 'sim2real-probe-lat-svc-b52d9f07-l7wf07q0hiwe8vs6g.f7b1150.vpc-lattice-svcs.us-west-2.on.aws'); $.id: value_mismatch (expected '<shaped-not-asserted>', got 'svc-l7wf07q0hiwe8vs6g').
2. GetService #task#2 — 200/200. $.arn and $.dnsEntry.domainName value_mismatch.
3. CreateListener #task#5 — 201/201. $.arn: expected 'arn:aws:vpc-lattice:us-west-2:000000000000:service/svc-l7wf07q0hiwe8vs6g/listener/listener-0c26c95e4c2cda1d0', got '.../listener/listener-r3y9msjz0uywa2j0x'; $.id: expected '<shaped-not-asserted>', got 'listener-r3y9msjz0uywa2j0x'; $.serviceArn value_mismatch.
4. GetListener #task#6 — 200/200. $.arn, $.serviceArn value_mismatch.
5. CreateListener #task#7 — 409/409, message and resourceType agree. $.resourceId: expected '<shaped-not-asserted>', got 'listener-r3y9msjz0uywa2j0x'.
6. DeleteService #task#13 — 200/200. $.arn value_mismatch.
7. CreateServiceNetworkServiceAssociation vpclattice-service-network-service-association#task#4 — 200/200. $.arn, $.id ('snsa-e7h39tvl1nmuvm6w8') and $.dnsEntry.domainName ('sim2real-probe-lat-assvc-f73b1d40-nxebjskmksmuk476g.f7b1150.vpc-lattice-svcs.us-west-2.on.aws') all value_mismatch against '<shaped-not-asserted>'.
8. GetServiceNetworkServiceAssociation #task#5 — 200/200. $.arn, $.dnsEntry.domainName, $.id, $.serviceArn, $.serviceId, $.serviceNetworkArn, $.serviceNetworkId value_mismatch.
9. ListServiceNetworkServiceAssociations #task#7 — 200/200. Same seven keys under $.items[0].
10. DeleteServiceNetworkServiceAssociation #task#9 — 200/200. $.arn, $.id value_mismatch.

Expected vs actual, stated generally: AWS mints `<prefix><17 chars from [0-9a-f]>` (recorded: listener-0c26c95e4c2cda1d0), the emulator mints `<prefix><17 chars from [0-9a-z]>` (observed: svc-l7wf07q0hiwe8vs6g, listener-r3y9msjz0uywa2j0x, snsa-e7h39tvl1nmuvm6w8 — each containing letters outside the hex range). The emulator's own code states the alphabet: state.rs `mint_id` -> `encode_base36(fresh_entropy(), ID_SUFFIX_LEN, BASE36_LOWER)` with ID_SUFFIX_LEN = 17, and `service_domain_name` splices that suffix into `<name>-<suffix>.<account segment>.vpc-lattice-svcs.<region>.on.aws`. Every modeled pattern ([0-9a-z]{17}) and length (21/26/20) stays satisfied under hex, so no other member's validation changes. HTTP statuses, error codes and error messages agree in all ten clusters; the divergence is confined to the generated identifiers and the values that embed them. The specific id strings above are one run's values and are not themselves the requirement — the alphabet and the composition template are.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus); or drive the two fixtures by hand:
   a. POST /services with a name such as sim2real-probe-lat-svc-<suffix> (fixture vpclattice-service-and-listener-lifecycle, task#1) and inspect `id`, `arn`, `dnsEntry.domainName`: the 17-character id suffix contains characters outside [0-9a-f], and the suffix is spliced into both the ARN and the domain name.
   b. GET /services/{serviceIdentifier} (task#2) and DELETE /services/{serviceIdentifier} (task#13): the echoed `arn`/`dnsEntry.domainName` carry the same non-hex suffix.
   c. POST /services/{serviceIdentifier}/listeners (task#5), then GET the listener (task#6): `id` is listener-<17 non-hex chars> and the nested ARN embeds it, where the recording has listener-0c26c95e4c2cda1d0.
   d. POST the same listener name again (task#7): the 409 ConflictException's `resourceId` is that non-hex listener id.
   e. POST /servicenetworkserviceassociations (fixture vpclattice-service-network-service-association, task#4), then GET it (task#5), GET /servicenetworkserviceassociations?serviceNetworkIdentifier=... (task#7) and DELETE it (task#9): `id`, `arn`, `serviceId`/`serviceArn`, `serviceNetworkId`/`serviceNetworkArn` and `dnsEntry.domainName` all carry non-hex 17-character suffixes.
3. Confirm the fix by re-replaying the representative step vpclattice-service-and-listener-lifecycle#task#1 and every step listed in trace_ids; all ten clusters must clear, and for the read/delete clusters note that the report renders expected and actual alike, so only a clean verdict proves the fix.
