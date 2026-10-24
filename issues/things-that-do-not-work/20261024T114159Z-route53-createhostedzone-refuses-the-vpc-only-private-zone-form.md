Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.associate-vpc-conflicting-domain-rejected route53.disassociate-last-vpc-from-private-zone-rejected route53.e2e-cross-account-vpc-authorization-then-association-consequence route53.list-hosted-zones-by-vpc-reflects-associations route53.list-vpc-association-authorizations-invalid-token-rejected route53.vpc-association-requires-authorization-denied
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route-53|CreateHostedZone|InvalidInput
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# CreateHostedZone refuses the VPC-only private-zone form (InvalidInput), blocking every private hosted zone / VPC-association behavior

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHostedZone

## Description
Cause 1 of 1: CreateHostedZone rejects a request that carries `VPC` unless `HostedZoneConfig.PrivateZone` is also explicitly true.

AWS: Smithy model (services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json) — CreateHostedZone input: `Name` (required), `CallerReference` (required), `VPC` (OPTIONAL, target VPC), `HostedZoneConfig` (OPTIONAL). The model documentation for CreateHostedZoneRequest$VPC reads: "(Private hosted zones only) A complex type that contains information about the Amazon VPC that you're associating with this hosted zone. You can specify only one Amazon VPC when you create a private hosted zone. If you are associating a VPC with a hosted zone with this request, the paramaters VPCId and VPCRegion are also required." The only stated requirement when `VPC` is supplied is that `VPCId` and `VPCRegion` be present; supplying `VPC` is itself what makes the new zone private, and `HostedZoneConfig.PrivateZone` is not required. The operation models InvalidInput (400) for genuinely invalid input, so a well-formed `Name`+`CallerReference`+`VPC` request must return 201 with the created private hosted zone (plus the echoed `VPC`), not an error.

Emulator: `create_hosted_zone(Name=..., CallerReference=..., VPC={VPCRegion, VPCId})` is answered with HTTP 400 `InvalidInput`, message (example) "Invalid request: a VPC can be associated only with a private hosted zone, so HostedZoneConfig.PrivateZone must be true when VPC is given". The handler is treating a VPC without an explicit PrivateZone flag as a contradiction rather than as the request that creates a private zone.

Why: a client cannot create a private hosted zone at all in the shape every SDK example and the AWS console use, so the whole private-zone surface downstream of it is unreachable: VPC association/disassociation, cross-account VPC authorization, ListHostedZonesByVPC and split-horizon DNS. Six behavioral checks never get past their very first setup call, so none of the private-zone semantics they exist to verify is exercised. Code that works against AWS gets a hard 400 on zone creation.

Fix: the CreateHostedZone handler and its input validation — services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/create_hosted_zone.rs. `VPC` present with `HostedZoneConfig` absent (or present without `PrivateZone`) must create a PRIVATE zone associated with that VPC; only a request that supplies `VPC` together with an explicit `PrivateZone=false` is contradictory.

Traces: route53.associate-vpc-conflicting-domain-rejected, route53.disassociate-last-vpc-from-private-zone-rejected, route53.e2e-cross-account-vpc-authorization-then-association-consequence, route53.list-hosted-zones-by-vpc-reflects-associations, route53.list-vpc-association-authorizations-invalid-token-rejected, route53.vpc-association-requires-authorization-denied.

## Evidence
Expected (AWS): HTTP 201 with HostedZone (private) + ChangeInfo + echoed VPC for `create_hosted_zone(Name=<domain>, CallerReference=<uuid>, VPC={"VPCRegion": <region>, "VPCId": "vpc-..."})`.
Actual (emulator): botocore.errorfactory.InvalidInput — "An error occurred (InvalidInput) when calling the CreateHostedZone operation: Invalid request: a VPC can be associated only with a private hosted zone, so HostedZoneConfig.PrivateZone must be true when VPC is given" (HTTP 400). Identical failure in all six checks listed, each on the first CreateHostedZone call of the check.

## Steps to Reproduce
1. Boot the emulator and point a botocore route53 client at it.
2. Create a VPC via EC2 CreateVpc (or use any well-formed vpc-id the emulator accepts).
3. Call route53.create_hosted_zone(Name="example-private.com", CallerReference=str(uuid4()), VPC={"VPCRegion": region, "VPCId": vpc_id}) with NO HostedZoneConfig.
4. Observe HTTP 400 InvalidInput instead of a 201 creating a private hosted zone associated with that VPC.
