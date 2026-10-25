Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.disassociate-last-vpc-association-rejected route53.overly-permissive-disassociate-last-vpc
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.disassociate-last-vpc-association-rejected|run#5|AssertionError|assert code == "LastVPCAssociation", f"expected LastVPCAssociation, got {code!r}"
Issue-Key: route53.overly-permissive-disassociate-last-vpc|run#6|AssertionError|assert code == "LastVPCAssociation", code
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Implement route53 DisassociateVPCFromHostedZone: unimplemented scaffold stub answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateVPCFromHostedZone

## Description
Cause 1 of 1: DisassociateVPCFromHostedZone is a scaffold stub and answers HTTP 501 `NotImplemented` for every request, so none of its semantics exist.

AWS: Smithy model (services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json) — DisassociateVPCFromHostedZone: POST /2013-04-01/hostedzone/{HostedZoneId}/disassociatevpc, success HTTP 200 with required `ChangeInfo`; modeled errors InvalidInput (400), InvalidVPCId (400), `LastVPCAssociation` (400), NoSuchHostedZone (404), VPCAssociationNotFound (404). The modeled documentation states the rule the two checks exercise: "You can't disassociate the last Amazon VPC from a private hosted zone" — removing the only associated VPC must be refused with `LastVPCAssociation` and leave the association intact, while the identical call succeeds (200 + ChangeInfo) once a second VPC is associated. Recorded message for the last-VPC refusal (example): "Cannot remove last VPC association for private zone".

Emulator: every call returns HTTP 501 with Code `NotImplemented` (Type Receiver), message "operation DisassociateVPCFromHostedZone is scaffolded but not implemented in this emulator". The handler file still carries the SCAFFOLDED marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("DisassociateVPCFromHostedZone"))`.

Why: a client cannot detach a VPC from a private hosted zone at all, and the guard rail that prevents orphaning a private zone is unobservable: an error class the SDK models as a retryable 501 server fault is returned where AWS returns a deterministic 400 client fault, so client-side handling for `LastVPCAssociation` is never reached and VPC-lifecycle teardown paths break.

Fix: implement the handler — services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/disassociate_vpc_from_hosted_zone.rs (the only file for this operation; delete the SCAFFOLDED marker when done). It needs the zone's VPC-association set in the service state so the last-association case can return `LastVPCAssociation`, an unassociated pair `VPCAssociationNotFound`, and the normal case 200 + ChangeInfo.

Traces: route53.disassociate-last-vpc-association-rejected, route53.overly-permissive-disassociate-last-vpc.

## Evidence
Expected (AWS): HTTP 400 with Error.Code `LastVPCAssociation` when disassociating the only VPC of a private hosted zone (recorded message example "Cannot remove last VPC association for private zone"), and HTTP 200 + ChangeInfo once a second VPC is associated.
Actual (emulator): HTTP 501, Error.Code `NotImplemented`, Type `Receiver`, message "operation DisassociateVPCFromHostedZone is scaffolded but not implemented in this emulator".
Check output: "AssertionError: expected LastVPCAssociation, got 'NotImplemented'" (route53.disassociate-last-vpc-association-rejected) and "AssertionError: NotImplemented" at `assert code == "LastVPCAssociation"` (route53.overly-permissive-disassociate-last-vpc).
Source: services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/disassociate_vpc_from_hosted_zone.rs line 4 ("SCAFFOLDED, NOT IMPLEMENTED") and line 183 (`Err(... not_implemented("DisassociateVPCFromHostedZone"))`).

## Steps to Reproduce
1. Create a private hosted zone associated with one VPC.
2. Call route53.disassociate_vpc_from_hosted_zone(HostedZoneId=<zone>, VPC={"VPCId": <that vpc>, "VPCRegion": <region>}).
3. Observe HTTP 501 NotImplemented instead of HTTP 400 LastVPCAssociation.
4. Associate a second VPC and repeat the call for the first VPC: observe 501 instead of HTTP 200 with ChangeInfo.
