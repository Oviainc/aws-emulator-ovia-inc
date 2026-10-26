Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.delete-vpc-association-authorization-revokes-future-association route53.e2e-cross-account-vpc-authorization-then-association-consequence route53.vpc-association-authorization-lifecycle route53.vpc-association-authorization-required-before-cross-account-associate route53.vpc-association-requires-authorization-denied route53.associate-vpc-conflicting-domain-rejected route53.list-vpc-association-authorizations-invalid-token-rejected route53.op-malformed-traffic-policy-document-rejected
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.delete-vpc-association-authorization-revokes-future-association|run#20|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: route53.e2e-cross-account-vpc-authorization-then-association-consequence|run#19|AssertionError|assert err["Message"] == (
Issue-Key: route53.vpc-association-authorization-lifecycle|run#14|AssertionError|assert err.get("Message") == (
Issue-Key: route53.vpc-association-authorization-required-before-cross-account-associate|run#8|AssertionError|assert err.get("Message") == not_found, err
Issue-Key: route53.vpc-association-requires-authorization-denied|run#9|AssertionError|assert err.get("Message") == not_found, err
Issue-Key: route53.associate-vpc-conflicting-domain-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53.list-vpc-association-authorizations-invalid-token-rejected|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: route53.op-malformed-traffic-policy-document-rejected|run#5|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: df6081fa6700f84ddafc058f223e27db3aea4084

# Route 53 VPC-authorization, pagination and traffic-policy refusals carry the wrong message rule (4 causes)

> _Note: this issue was identified against the code base at commit `df6081fa6700f84ddafc058f223e27db3aea4084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVPCAssociationAuthorization

## Description
FOUR DISTINCT CAUSES are bundled here; all four must be closed. In every one the HTTP error CODE is right and the effect is right — what diverges is the rule the message is built to, and in each case that rule is known (the message is built from the request, or it states the constraint AWS enforces), so it is the part a client keys on to tell these refusals apart. The recorded strings quoted below are EXAMPLES of each rule, not required text; the ids/regions in them vary per run.

Cause 1 of 4: DeleteVPCAssociationAuthorization renders VPCAssociationAuthorizationNotFound with the AssociateVPCWithHostedZone wording, dropping the authorization-ledger form and the VPC region.
AWS: Smithy com.amazonaws.route53#VPCAssociationAuthorizationNotFound (client error, HTTP 404, member `message`), bound to DeleteVPCAssociationAuthorization (POST /2013-04-01/hostedzone/{HostedZoneId}/deauthorizevpcassociation). The message is request-derived; five independent checks each build the expectation from their own request, giving the rule: name the hosted zone id, the VPC id AND the VPC region from the request, in the ledger form — e.g. 'No authorization exists for hosted zone: ZWMRER8ZOSGCPGN and vpc: vpc-0000000000000002c, us-west-2'.
Emulator: code VPCAssociationAuthorizationNotFound is correct, but the message is 'The VPC <VPCId> is not authorized to be associated with the hosted zone <HostedZoneId>' — the shape's model description (which belongs to the ASSOCIATE path), with the request's VPCRegion absent and no statement that no authorization exists.
Why: an authorization entry is identified by (VPCId, VPCRegion), so a client is told a region-scoped entry is missing without being told which region, and a caller that distinguishes 'this VPC was never authorized for this zone' (the delete/ledger path) from 'this association attempt is not authorized' (the associate path) cannot: both now produce the same sentence.
Fix: the not-found branch of the DeleteVPCAssociationAuthorization handler — services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/delete_vpc_association_authorization.rs — must build its own message from the request's HostedZoneId, VPC.VPCId and VPC.VPCRegion instead of reusing the associate path's text.
Traces: route53.delete-vpc-association-authorization-revokes-future-association, route53.e2e-cross-account-vpc-authorization-then-association-consequence, route53.vpc-association-authorization-lifecycle, route53.vpc-association-authorization-required-before-cross-account-associate, route53.vpc-association-requires-authorization-denied. (Each check's positive control passed — create/list/delete of a real authorization works — so only the refusal text is at fault.)

Cause 2 of 4: AssociateVPCWithHostedZone's ConflictingDomainExists message names the domain instead of the region and the id of the already-associated hosted zone.
AWS: Smithy ConflictingDomainExists (client error, HTTP 400); its documentation for this operation states 'Associate VPCs with a private hosted zone: The VPC that you specified is already associated with another hosted zone that has the same name.' The recorded message is built from the request plus the resolved conflict — rule: name the VPC id, the VPC region from the request, and the ID of the hosted zone already associated with that VPC under the same name, e.g. 'The VPC vpc-0000000000000001e in region us-west-2 has already been associated with the hosted zone ZNS6PCQNTQMQIMR with the same domain name.'
Emulator: code ConflictingDomainExists is correct; the message is 'The VPC <VPCId> is already associated with another hosted zone that has the name <domain>.' — the region is omitted and the conflicting zone's ID is replaced by the domain name.
Why: the conflicting zone id is the actionable content of this refusal (the caller already knows the domain it asked for; what it does not know is WHICH zone holds the VPC). Without it, remediation requires an extra ListHostedZonesByVPC round trip, and message-matching tooling fails.
Fix: the domain-conflict branch of services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/associate_vpc_with_hosted_zone.rs — report the conflicting zone id and the request's VPCRegion.
Traces: route53.associate-vpc-conflicting-domain-rejected.

Cause 3 of 4: ListVPCAssociationAuthorizations describes an invalid NextToken with a rule of its own invention.
AWS: Smithy InvalidPaginationToken (client error, HTTP 400), bound to ListVPCAssociationAuthorizations. The recorded message is built from the request's NextToken alone and makes no claim about VPCs or zones — e.g. 'Invalid pagination token: not-a-real-pagination-token-0123456789'.
Emulator: code InvalidPaginationToken is correct; the message is 'The specified pagination token is not valid: "<token>" names no VPC authorized for hosted zone <zone id>' — it quotes the token and asserts a different rule (that a token names a VPC entry), which is not what AWS reports and leaks how the emulator implements the token.
Why: a client that matches the documented 'Invalid pagination token:' prefix to distinguish a stale/garbled token from a zone-scoped lookup failure classifies the refusal wrongly.
Fix: the NextToken decode path of services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/list_vpc_association_authorizations.rs.
Traces: route53.list-vpc-association-authorizations-invalid-token-rejected.

Cause 4 of 4: CreateTrafficPolicy reports only the first structural violation of a traffic-policy document, not all of them.
AWS: Smithy InvalidTrafficPolicyDocument (client error, HTTP 400), bound to CreateTrafficPolicy (POST /2013-04-01/trafficpolicy). For a document carrying only AWSPolicyFormatVersion and RecordType, the recorded refusal enumerates EVERY violated document constraint, semicolon-joined: 'At least one endpoint must be declared.;Either StartRule or StartEndpoint must be declared, but not both'. Rule: one clause per violated constraint, joined with ';'.
Emulator: code InvalidTrafficPolicyDocument is correct; the message is 'Invalid traffic policy document: the traffic policy document must carry Endpoints as a JSON object' — a single clause, with the missing StartEndpoint/StartRule violation not reported at all.
Why: a client validating a policy document by submitting it is told about one fault at a time, so an authoring loop needs one round trip per fault instead of one, and a document with several faults looks like it has one.
Fix: the document validator behind services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/create_traffic_policy.rs — collect all structural violations and join them, rather than returning on the first.
Traces: route53.op-malformed-traffic-policy-document-rejected. (The positive control passed: a well-formed document is created and round-trips through GetTrafficPolicy.)

Related but NOT covering these: open issue 20261024T160546Z-route53-route-53-validation-semantics-gaps-five-operations-answer.md (operations answering NotImplemented) and the fixed wire issue 20261022T161659Z-route53-route-53-error-responses-omit-requestid-and-return.md, which covered only the CreateHostedZone/DeleteHostedZone/GetHostedZone/ChangeResourceRecordSets message forms — none of the four operations above.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (route53 23/114 checks passing, so the service is serving; bootstrap_signal=false).

Cause 1 — code matched in all five checks, message diverged:
  AssertionError: expected message 'No authorization exists for hosted zone: ZWMRER8ZOSGCPGN and vpc: vpc-0000000000000002c, us-west-2', got 'The VPC vpc-0000000000000002c is not authorized to be associated with the hosted zone ZWMRER8ZOSGCPGN' (route53.delete-vpc-association-authorization-revokes-future-association)
  AssertionError: {'Message': 'The VPC vpc-00000000000000024 is not authorized to be associated with the hosted zone ZNHMHBJOX2UCRVM', 'Code': 'VPCAssociationAuthorizationNotFound', 'Type': 'Sender'} (route53.e2e-cross-account-vpc-authorization-then-association-consequence; same shape in route53.vpc-association-authorization-lifecycle, route53.vpc-association-authorization-required-before-cross-account-associate, route53.vpc-association-requires-authorization-denied)

Cause 2 — AssertionError: expected message 'The VPC vpc-0000000000000001e in region us-west-2 has already been associated with the hosted zone ZNS6PCQNTQMQIMR with the same domain name.', got 'The VPC vpc-0000000000000001e is already associated with another hosted zone that has the name sim2real-behav-cb74c390bb20.example.com..' (code ConflictingDomainExists in both).

Cause 3 — AssertionError: expected message 'Invalid pagination token: not-a-real-pagination-token-0123456789', got 'The specified pagination token is not valid: "not-a-real-pagination-token-0123456789" names no VPC authorized for hosted zone ZXGWH9T30LZ5M6Y' (code InvalidPaginationToken in both).

Cause 4 — AssertionError: expected message 'At least one endpoint must be declared.;Either StartRule or StartEndpoint must be declared, but not both', got 'Invalid traffic policy document: the traffic policy document must carry Endpoints as a JSON object' (code InvalidTrafficPolicyDocument in both).

Model evidence (smithy_query on services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json): VPCAssociationAuthorizationNotFound client/404 bound to DeleteVPCAssociationAuthorization; ConflictingDomainExists client/400 bound to AssociateVPCWithHostedZone (doc: 'The VPC that you specified is already associated with another hosted zone that has the same name'); InvalidPaginationToken client/400 bound to ListVPCAssociationAuthorizations; InvalidTrafficPolicyDocument client/400 bound to CreateTrafficPolicy.

## Steps to Reproduce
1. Cause 1: CreateHostedZone with a VPC (private zone), then DeleteVPCAssociationAuthorization for a VPC/zone pair that was never authorized (or authorize, delete, then delete again). Read Error.Message — it is the associate-path sentence and omits the VPC region.
2. Cause 2: create two private zones with the SAME name, zone A against vpc1 and zone B against vpc2; then AssociateVPCWithHostedZone(zone B, vpc1). Read the ConflictingDomainExists message — it names the domain, not the region or zone A's id.
3. Cause 3: ListVPCAssociationAuthorizations(HostedZoneId=<zone>, NextToken='not-a-real-pagination-token-0123456789'). Read the InvalidPaginationToken message.
4. Cause 4: CreateTrafficPolicy(Document='{"AWSPolicyFormatVersion":"2015-10-01","RecordType":"A"}'). Read the InvalidTrafficPolicyDocument message — only the Endpoints violation is reported.
