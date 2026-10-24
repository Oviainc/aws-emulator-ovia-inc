Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.associate-vpc-without-authorization-rejected
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.associate-vpc-without-authorization-rejected|_error_code#5|AssertionError|raise AssertionError(f"expected NotAuthorizedException but the call succeeded and returned {result!r}")
Issue-Commit: 1302d977b6b888f4927f8769f8d72a08ded5722c

# AssociateVPCWithHostedZone applies no authorization/ownership gate: a foreign VPC id is associated with 200 PENDING instead of being refused with 401 NotAuthorizedException

> _Note: this issue was identified against the code base at commit `1302d977b6b888f4927f8769f8d72a08ded5722c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateVPCWithHostedZone

## Description
One net-new root cause in route53.

Cause 1 of 1: AssociateVPCWithHostedZone must refuse a VPC the calling account neither owns nor holds a CreateVPCAssociationAuthorization for, with NotAuthorizedException (HTTP 401), and must not apply the association.

AWS: Smithy (services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json): AssociateVPCWithHostedZone, POST /2013-04-01/hostedzone/{HostedZoneId}/associatevpc, success code 200, input HostedZoneId (httpLabel, required, @length 0-32) + VPC (required) + Comment; output ChangeInfo (required); modeled errors ConflictingDomainExists 400, InvalidInput 400, InvalidVPCId 400, LimitsExceeded 400, NoSuchHostedZone 404, NotAuthorizedException 401, PriorRequestNotComplete 400, PublicZoneVPCAssociation 400. Associating a syntactically well-formed VPC id that the calling account never created, and for which no matching CreateVPCAssociationAuthorization exists, is refused with error code `NotAuthorizedException` / HTTP 401; the association does NOT take effect, so a following GetHostedZone still reports only the VPCs that were legitimately associated. The denial message is built from the request and the caller identity, so the RULE is the requirement — "User: <caller arn> is not authorized to perform: route53:AssociateVPCWithHostedZone on resource: arn:aws:ec2:<region>:<account>:vpc/<vpc-id>" — and the recorded text is only an example of that rule.

Emulator: the call SUCCEEDS. The emulator answers HTTP 200 with a ChangeInfo (recorded example: Id '/change/CYUA4N0F1RMWCLT', Status 'PENDING'), i.e. it performs no ownership or authorization check on the VPC at all and treats any well-formed VPC id as associable. No modeled refusal is produced.

Why: a client relying on AWS's behavior gets a silent privilege escalation in its test environment. Cross-account private-zone flows (CreateVPCAssociationAuthorization -> AssociateVPCWithHostedZone, and the denial path when the authorization is missing or has been revoked) cannot be exercised: code that handles NotAuthorizedException by requesting an authorization, or by failing closed, never takes that branch. Worse, the emulator reports a zone as resolvable from a VPC that AWS would never have attached, so split-horizon DNS assumptions validated against the emulator are wrong against AWS. Because the response is 200 PENDING, nothing downstream can distinguish an authorized association from an unauthorized one.

Fix: the AssociateVPCWithHostedZone handler and its validation (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/associate_vpc_with_hosted_zone.rs), together with the VPC-association-authorization state it must consult (the records written by CreateVPCAssociationAuthorization / removed by DeleteVPCAssociationAuthorization) and the account's own VPC inventory; the refusal must be emitted before any mutation of the hosted zone's VPC list.

Related existing issues (neither covers this failure mode): 20261024T114159Z-route53-associatevpcwithhostedzone-stub.md (now marked fixed) stood the handler up but added no authorization gate; 20261024T160546Z-route53-route-53-validation-semantics-gaps-five-operations-answer.md states this same rule as its Cause 1, but its recorded key for this check is the old `NotImplemented` signature — the emulator no longer answers NotImplemented here, it answers 200, so the surviving failure is the over-permissive acceptance described above and the suite does not report this check as owned by that issue.

Traces: route53.associate-vpc-without-authorization-rejected.

## Evidence
Suite simparity-behavioral-ga-prod-v1, verdict 'mismatch', check route53.associate-vpc-without-authorization-rejected (service token 'route53'; this check is marked neither ALREADY FILED nor OWNED in the replay report).

Expected (real AWS): botocore ClientError with Error.Code = 'NotAuthorizedException', HTTP status 401, no association applied; GetHostedZone(Id=<zone>).VPCs still == {owned vpc}. Message per the rule above.

Actual (emulator): the call returned success. From the replay output:
  File "check_route53_associate_vpc_without_authorization_rejected.py", line 34, in run
    code, message = _error_code(lambda: route53.associate_vpc_with_hosted_zone(HostedZoneId=zone_id, VPC={"VPCRegion": ctx.region, "VPCId": foreign_vpc_id}))
  File "check_route53_associate_vpc_without_authorization_rejected.py", line 132, in _error_code
  AssertionError: expected NotAuthorizedException but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'ChangeInfo': {'Id': '/change/CYUA4N0F1RMWCLT', 'Status': 'PENDING', 'SubmittedAt': ...}}

(The generated change id, timestamps and the random foreign VPC id differ per run and are examples; the requirement is the 401 / NotAuthorizedException refusal and the absent effect.)

Model confirmation (smithy_query query=operation, AssociateVPCWithHostedZone): errors include NotAuthorizedException httpError 401, InvalidVPCId 400, PublicZoneVPCAssociation 400, NoSuchHostedZone 404; http {POST, /2013-04-01/hostedzone/{HostedZoneId}/associatevpc, code 200}.

## Steps to Reproduce
1. Boot the emulator and replay route53.associate-vpc-without-authorization-rejected.
2. Manually: ec2.CreateVpc(CidrBlock='10.x.0.0/16') and wait for State=available -> vpc_owned.
3. route53.CreateHostedZone(Name='<unique>.example.com.', CallerReference=<unique>, HostedZoneConfig={'PrivateZone': True}, VPC={'VPCRegion': <region>, 'VPCId': vpc_owned}); wait for the returned ChangeInfo to reach INSYNC.
4. route53.AssociateVPCWithHostedZone(HostedZoneId=<zone>, VPC={'VPCRegion': <region>, 'VPCId': 'vpc-<17 random hex>'}) — a well-formed id this account never created and holds no CreateVPCAssociationAuthorization for.
   Expected: HTTP 401, Error.Code 'NotAuthorizedException'. Observed: HTTP 200 with ChangeInfo Status PENDING.
5. route53.GetHostedZone(Id=<zone>) and assert VPCs == {vpc_owned} — the rejected VPC must not appear.
6. Positive control (must keep working): create a second VPC in the account, AssociateVPCWithHostedZone with it, wait INSYNC, and confirm GetHostedZone reports both owned VPCs.
