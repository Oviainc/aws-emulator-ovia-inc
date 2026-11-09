Issue-Source: trace-sweep-divergence
Issue-Trace: route53-create-hosted-zone-invalid-domain-error
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-create-hosted-zone-invalid-domain-error route53-public-zone-vpc-association-guardrails route53-traffic-policy-error-paths
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53|CreateHostedZone|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: route53|AssociateVPCWithHostedZone|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: route53|CreateTrafficPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: a9b33a473a9762dcd2804e6444a9b0695dc27129

# Route 53 refusal messages are emulator-authored instead of AWS wording

> _Note: this issue was identified against the code base at commit `a9b33a473a9762dcd2804e6444a9b0695dc27129`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHostedZone

## Description
Three refusals reach the right HTTP status and the right error code, but carry a sentence the emulator composed rather than the one Route 53 words for that error class. 3 causes.

Cause 1 of 4 is not applicable — 3 causes total.

Cause 1 of 3 — CreateHostedZone, name Route 53 reserves.
AWS: Smithy: CreateHostedZone binds InvalidDomainName with httpError 400; Name is @length 0..1024 with no pattern, so the name rule is service-side. For a reserved name the 400 InvalidDomainName message is "<Name> is reserved by AWS!" — the fixed text is the suffix " is reserved by AWS!" and the only variable part is the requested Name echoed verbatim from the request.
Emulator: same status and code, but the message is "<Name> is not a valid hosted zone name: a hosted zone cannot be created for a top-level domain" — a different rule stated in the emulator's own words.
Why: the message is the only place the service explains which rule was violated; a client (or a human reading a log) is told the name is a top-level domain when AWS said it is reserved, so remediation goes the wrong way.
Fix: CreateHostedZone's validation — distinguish the reserved-name rule from the TLD rule and render AWS's sentence for each.
Traces: route53-create-hosted-zone-invalid-domain-error#task#1.

Cause 2 of 3 — AssociateVPCWithHostedZone against a public hosted zone.
AWS: Smithy: AssociateVPCWithHostedZone binds PublicZoneVPCAssociation with httpError 400. The message is "Attempting to associate public zone: <HostedZoneId> with vpc: <VPCId>" — fixed text is "Attempting to associate public zone: " and " with vpc: "; the variable parts are the hosted zone ID from the request path and the VPC ID from the request body. Note it names the zone by ID, not by DNS name.
Emulator: same status and code, but the message is a multi-clause sentence of its own ("You're trying to associate a VPC with a public hosted zone: <zone name> is a public hosted zone, and Route 53 doesn't support associating a VPC with a public hosted zone.") and identifies the zone by its DNS name instead of its ID.
Why: a caller correlating the refusal back to a zone has to parse a name where AWS supplied the ID, and the VPC ID — which AWS includes — is absent entirely.
Fix: the PublicZoneVPCAssociation serializer for AssociateVPCWithHostedZone.
Traces: route53-public-zone-vpc-association-guardrails#task#4.

Cause 3 of 3 — CreateTrafficPolicy with a structurally invalid policy document.
AWS: Smithy: CreateTrafficPolicy binds InvalidTrafficPolicyDocument with httpError 400; Document is @length 0..102400 and is validated service-side. The message is the set of document violations joined with ';' in AWS's fixed order and wording — for a document missing endpoints, mis-declaring the start, and lacking a usable record type: "At least one endpoint must be declared.;Either StartRule or StartEndpoint must be declared, but not both;Policy RecordType is missing or not supported." Each clause is fixed text; the only variability is which clauses apply to the submitted document.
Emulator: same status and code, but emits the record-type clause first and words it "RecordType must be declared." instead of AWS's "Policy RecordType is missing or not supported.", producing "RecordType must be declared.;At least one endpoint must be declared.;Either StartRule or StartEndpoint must be declared, but not both".
Why: clients that split the message on ';' to surface per-violation diagnostics get a clause AWS never emits and lose the one it does, and the ordering difference breaks exact-match assertions.
Fix: CreateTrafficPolicy's document validation — use AWS's clause wording and emit the violations in AWS's order (record-type clause last).
Traces: route53-traffic-policy-error-paths#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 — status and error code matched in all three; only $.ErrorResponse.Error.Message diverged:
- route53-create-hosted-zone-invalid-domain-error#task#1 CreateHostedZone (400): expected '<name> is reserved by AWS!', got '<name> is not a valid hosted zone name: a hosted zone cannot be created for a top-level domain'.
- route53-public-zone-vpc-association-guardrails#task#4 AssociateVPCWithHostedZone (400): expected 'Attempting to associate public zone: <hosted-zone-id> with vpc: <vpc-id>', got "You're trying to associate a VPC with a public hosted zone: <zone-dns-name> is a public hosted zone, and Route 53 doesn't support associating a VPC with a public hosted zone.".
- route53-traffic-policy-error-paths#task#1 CreateTrafficPolicy (400): expected 'At least one endpoint must be declared.;Either StartRule or StartEndpoint must be declared, but not both;Policy RecordType is missing or not supported.', got 'RecordType must be declared.;At least one endpoint must be declared.;Either StartRule or StartEndpoint must be declared, but not both'.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus and inspect the three steps under Traces.
3. Individually: POST /2013-04-01/hostedzone with a name Route 53 reserves; POST /2013-04-01/hostedzone/{public-zone-id}/associatevpc with any VPC; POST /2013-04-01/trafficpolicy with a document that declares no endpoints, no valid start and no record type.
4. Compare each ErrorResponse/Error/Message against the expected strings above.
