Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.hosted-zone-inventory-and-comment-update
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.hosted-zone-inventory-and-comment-update|run#19|AssertionError|assert err["Message"] == f"No hosted zone found with ID: {unknown_zone}", err
Issue-Commit: e6eec4415380d0861b3cd74514fe77af8743d21c

# UpdateHostedZoneComment's NoSuchHostedZone message is still the shape doc form, not 'No hosted zone found with ID: <id>'

> _Note: this issue was identified against the code base at commit `e6eec4415380d0861b3cd74514fe77af8743d21c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateHostedZoneComment

## Description
Cause 1 of 1: the hosted-zone not-found branch renders the NoSuchHostedZone error shape's own Smithy documentation instead of the request-derived sentence AWS returns. Failing check: route53.hosted-zone-inventory-and-comment-update.

Relationship to an existing issue: open issue 20261025T054608Z-route53-route-53-not-found-and-last-vpc-refusals.md describes this same behavior as its Cause 1 and lists this check among its Issue-Trace-Ids, but its recorded replay key points at a different step of the check (run#21 / the assertion at the then-current check line 141), and the replay report does NOT report this failure as owned by it — the failure now surfaces at check line 165. It is filed here so the still-failing step is not closed unrun; a fixer that closes the other issue by fixing the single shared not-found branch closes this one with it.

AWS: Smithy com.amazonaws.route53#NoSuchHostedZone (errorKind client, httpError 404, member `message`; shape doc 'No hosted zone exists with the ID that you specified.'), bound to UpdateHostedZoneComment (POST /2013-04-01/hostedzone/{Id}) and GetHostedZone (GET /2013-04-01/hostedzone/{Id}). The message AWS returns is request-derived and names the BARE hosted zone id from the request's `Id` label, in the fixed form 'No hosted zone found with ID: <bare zone id>' — fixed text: 'No hosted zone found with ID: '; variable part: the bare zone id taken from the request. The check builds that expectation from its own request in three independent places: an id that never existed, and the id of a zone it just deleted, read back through both UpdateHostedZoneComment and GetHostedZone.
Emulator: the Code NoSuchHostedZone and the 404 classification are correct, and the deleted zone is correctly unreadable and unmutable, but Error.Message is 'no hosted zone exists with the ID <id>' — a lower-cased restatement of the shape's documentation, so both the prefix a client matches on and the capitalisation differ from AWS.
Why: a client that distinguishes a missing hosted zone from other 404s by the documented message prefix, or that extracts the zone id out of the refusal text, classifies or parses the response wrongly; because the divergence lives in one shared not-found branch it affects every operation that mints NoSuchHostedZone, not just this one.
Fix: the hosted-zone not-found branch used by the UpdateHostedZoneComment and GetHostedZone handlers (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/update_hosted_zone_comment.rs, get_hosted_zone.rs, and the shared helper that mints NoSuchHostedZone) must build the message from the request's bare zone id in the AWS form instead of echoing the shape description.
Traces: route53.hosted-zone-inventory-and-comment-update — every earlier assertion in the check (ListHostedZones, the ListHostedZonesByName seek, the GetHostedZoneCount increment and the comment round trip through GetHostedZone) passed, so only the refusal text is at fault.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (route53 34/114 checks passing, so the service is serving; bootstrap_signal false). Verdict: mismatch — Code matched, Message diverged.

  File "check_route53_hosted_zone_inventory_and_comment_update.py", line 165, in run
    assert err["Message"] == f"No hosted zone found with ID: {unknown_zone}", err
  AssertionError: {'Message': 'no hosted zone exists with the ID Z3ACDAFD82C7D49D28B02', 'Code': 'NoSuchHostedZone', 'Type': 'Sender'}

The failing call is UpdateHostedZoneComment(Id=<'Z' + 20 uppercase hex, never created>, Comment='x'); the check first asserted Code == 'NoSuchHostedZone', which held. The same expectation is asserted twice more later in the check against the id of a just-deleted zone, through GetHostedZone and through UpdateHostedZoneComment.

Model evidence (smithy_query shape on services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json): NoSuchHostedZone — errorKind client, httpError 404, member `message`, doc 'No hosted zone exists with the ID that you specified.' The emulator's message is that doc lower-cased with the id appended, rather than the recorded 'No hosted zone found with ID: <id>' form.

## Steps to Reproduce
1. Boot the route53 emulator.
2. Call UpdateHostedZoneComment(Id='Z' + 20 random uppercase hex characters, Comment='x') — an id that was never created.
3. The call is correctly refused with Code NoSuchHostedZone / HTTP 404. Read Error.Message: it is 'no hosted zone exists with the ID <id>' instead of 'No hosted zone found with ID: <id>'.
4. Same form on a deleted zone: CreateHostedZone a public zone, DeleteHostedZone it, then GetHostedZone(Id=<id>) and UpdateHostedZoneComment(Id=<id>, Comment='post-delete') — both are correctly NoSuchHostedZone and both carry the same wrong message form.
5. Replay: route53.hosted-zone-inventory-and-comment-update.
