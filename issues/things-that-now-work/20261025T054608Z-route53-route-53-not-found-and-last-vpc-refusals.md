Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.hosted-zone-inventory-and-comment-update route53.disassociate-last-vpc-from-private-zone-rejected route53.overly-permissive-disassociate-last-vpc
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.hosted-zone-inventory-and-comment-update|run#21|AssertionError|assert err["Message"] == f"No hosted zone found with ID: {unknown_zone}", err
Issue-Key: route53.disassociate-last-vpc-from-private-zone-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53.overly-permissive-disassociate-last-vpc|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: c8c507c37f1eb3ce902562e70e02f5e073c85b13

# Route 53 not-found and last-VPC refusals carry the wrong message rule (2 causes)

> _Note: this issue was identified against the code base at commit `c8c507c37f1eb3ce902562e70e02f5e073c85b13`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateHostedZoneComment

## Description
TWO DISTINCT CAUSES are bundled here; both must be closed. In both the HTTP status and the error CODE are correct and the effect is correct — what diverges is the rule the error MESSAGE is built to. In both cases the emulator renders a lower-cased paraphrase of the error shape's own Smithy documentation instead of the sentence AWS returns, so a client that keys on the refusal text (or that reads the id out of it) is misled. The ids quoted below are examples that vary per run; the RULE is the requirement, not the exact recorded ids.

Cause 1 of 2: NoSuchHostedZone is rendered from the shape's model description instead of the request-derived not-found sentence.
AWS: Smithy com.amazonaws.route53#NoSuchHostedZone (errorKind client, httpError 404, member `message`; shape doc 'No hosted zone exists with the ID that you specified.'), bound to UpdateHostedZoneComment (POST /2013-04-01/hostedzone/{Id}) and GetHostedZone (GET /2013-04-01/hostedzone/{Id}). The message is request-derived: it names the BARE hosted zone id taken from the request's `Id` label in the fixed form 'No hosted zone found with ID: <bare zone id>' — the check builds that expectation from its own request in three independent places (an id that never existed, and the id of a zone it just deleted, read back through both GetHostedZone and UpdateHostedZoneComment).
Emulator: code NoSuchHostedZone is correct (and the deleted zone is correctly unreadable and unmutable), but the message is 'no hosted zone exists with the ID <id>' — a lower-cased restatement of the shape's model documentation rather than the 'No hosted zone found with ID: <id>' form, so the prefix a client matches on and the capitalisation both differ.
Why: a client distinguishing a not-found zone from other 404s by the documented message prefix, or extracting the zone id from the text, classifies/parses the refusal wrongly; the divergence is identical for every operation that shares this handler's not-found branch, so it is systematic rather than one-off.
Fix: the hosted-zone not-found branch used by the UpdateHostedZoneComment and GetHostedZone handlers under services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/ (update_hosted_zone_comment.rs, get_hosted_zone.rs, and whatever shared helper mints NoSuchHostedZone) must build the message from the request's bare zone id in the AWS form instead of echoing the shape description.
Traces: route53.hosted-zone-inventory-and-comment-update (every earlier assertion in that check — ListHostedZones, ListHostedZonesByName seek, GetHostedZoneCount increment, and the comment round trip through GetHostedZone — passed, so only the refusal text is at fault).

Cause 2 of 2: DisassociateVPCFromHostedZone's LastVPCAssociation message is the shape's model documentation with the request's ids interpolated, not the constant refusal sentence AWS returns.
AWS: Smithy com.amazonaws.route53#LastVPCAssociation (errorKind client, httpError 400, member `message`), bound to DisassociateVPCFromHostedZone (POST /2013-04-01/hostedzone/{HostedZoneId}/disassociatevpc); its shape documentation is the prose 'The VPC that you're trying to disassociate from the private hosted zone is the last VPC that is associated with the hosted zone. Amazon Route 53 doesn't support disassociating the last VPC from a hosted zone.' Two independent checks, written from different scaffolding and different zones/VPCs, both record the SAME fixed message with no ids in it: 'Cannot remove last VPC association for private zone'. Rule: a constant sentence, not a request-interpolated one.
Emulator: code LastVPCAssociation is correct and the effect is correct (the association survives the refusal, and the identical call succeeds once a second VPC is associated), but the message is the model's prose with the request's ids spliced in — e.g. 'The VPC vpc-0000000000000001e is the last VPC associated with the hosted zone Z7T4PWTQWDE6DTD. Route 53 doesn't support disassociating the last VPC from a hosted zone.'
Why: a client that matches this refusal by text (the usual way to tell the last-VPC boundary apart from VPCAssociationNotFound and InvalidVPCId, which share the 4xx family) does not match it, and tooling that treats the message as a stable constant sees a per-request-varying string instead. Hypothesis: the handler builds its message from the error shape's documentation, the same pattern already filed as Cause 1 of open issue 20261025T023940Z-route53-route-53-vpc-authorization-pagination-and-traffic-policy.md for VPCAssociationAuthorizationNotFound; that issue covers DeleteVPCAssociationAuthorization/AssociateVPCWithHostedZone/ListVPCAssociationAuthorizations/CreateTrafficPolicy and does NOT cover DisassociateVPCFromHostedZone or the NoSuchHostedZone form above.
Fix: the last-VPC branch of services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/disassociate_vpc_from_hosted_zone.rs.
Traces: route53.disassociate-last-vpc-from-private-zone-rejected, route53.overly-permissive-disassociate-last-vpc (both checks' positive controls passed: a non-last VPC disassociates, the rejected call leaves the association intact, and the same call succeeds after a second VPC is associated).

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (route53 32/114 checks passing, so the service is serving; bootstrap_signal false).

Cause 1 — code matched, message diverged:
  AssertionError: {'Message': 'no hosted zone exists with the ID ZEB771A7BFCD44F4983BB', 'Code': 'NoSuchHostedZone', 'Type': 'Sender'} — the check asserted Message == 'No hosted zone found with ID: ZEB771A7BFCD44F4983BB' after asserting Code == 'NoSuchHostedZone' (which held) for UpdateHostedZoneComment on a never-existing zone id (route53.hosted-zone-inventory-and-comment-update, assertion at check line 141). The same expectation is asserted twice more in that check against the id of a just-deleted zone, via GetHostedZone and via UpdateHostedZoneComment.

Cause 2 — code matched (LastVPCAssociation, HTTP 400), message diverged, in two independent checks:
  AssertionError: expected message 'Cannot remove last VPC association for private zone', got "The VPC vpc-0000000000000001e is the last VPC associated with the hosted zone Z7T4PWTQWDE6DTD. Route 53 doesn't support disassociating the last VPC from a hosted zone." (route53.disassociate-last-vpc-from-private-zone-rejected)
  AssertionError: expected message 'Cannot remove last VPC association for private zone', got "The VPC vpc-0000000000000001e is the last VPC associated with the hosted zone ZQMQKGXCA3HRUMZ. Route 53 doesn't support disassociating the last VPC from a hosted zone." (route53.overly-permissive-disassociate-last-vpc)

Model evidence (smithy_query on services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json): NoSuchHostedZone client/404 with member `message`, bound to both UpdateHostedZoneComment and GetHostedZone, doc 'No hosted zone exists with the ID that you specified.'; LastVPCAssociation client/400 with member `message`, bound to DisassociateVPCFromHostedZone, doc 'The VPC that you're trying to disassociate from the private hosted zone is the last VPC that is associated with the hosted zone. Amazon Route 53 doesn't support disassociating the last VPC from a hosted zone.' In each cause the emulator's text tracks the shape doc rather than the recorded refusal sentence.

## Steps to Reproduce
1. Cause 1 (never-existing zone): UpdateHostedZoneComment(Id='Z' + 20 random hex uppercase, Comment='x'). The call is correctly refused with Code NoSuchHostedZone / HTTP 404; read Error.Message — it is 'no hosted zone exists with the ID <id>' instead of 'No hosted zone found with ID: <id>'.
2. Cause 1 (deleted zone): CreateHostedZone a public zone, DeleteHostedZone it, then GetHostedZone(Id=<id>) and UpdateHostedZoneComment(Id=<id>, Comment='post-delete'). Both are correctly NoSuchHostedZone; both messages use the same wrong form.
3. Cause 2: CreateVpc, then CreateHostedZone with HostedZoneConfig.PrivateZone=true and VPC={VPCId:<vpc>,VPCRegion:<region>}; then DisassociateVPCFromHostedZone(HostedZoneId=<zone>, VPC={VPCId:<vpc>,VPCRegion:<region>}). The call is correctly refused with Code LastVPCAssociation / HTTP 400 and the association survives; read Error.Message — it is the model's prose with the request ids interpolated instead of the constant 'Cannot remove last VPC association for private zone'.
