Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.overly-permissive-associate-vpc-with-public-zone route53.rrset-strict-match-delete-requires-exact-values
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.overly-permissive-associate-vpc-with-public-zone|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53.rrset-strict-match-delete-requires-exact-values|run#13|AssertionError|assert msg == mismatch, f"expected message {mismatch!r}, got {msg!r}"
Issue-Commit: 085aca81986f06b9d958c778a1487573b77f6255

# Route 53 refusal payloads diverge: PublicZoneVPCAssociation message is emulator-authored, and InvalidChangeBatch reports every problem twice (message + messages)

> _Note: this issue was identified against the code base at commit `085aca81986f06b9d958c778a1487573b77f6255`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateVPCWithHostedZone

## Description
Two net-new root causes in the route53 emulator's refusal payloads. Both reach the right HTTP status and the right error code, and both diverge in the error body a client reads. 2 causes.

Cause 1 of 2 — AssociateVPCWithHostedZone against a PUBLIC hosted zone returns AWS's error code with a sentence the emulator composed.
AWS: Smithy (services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json): AssociateVPCWithHostedZone binds PublicZoneVPCAssociation, errorKind client, httpError 400, with a single `message` member (ErrorMessage). Recorded behavior for this error class: Message is "Attempting to associate public zone: <HostedZoneId> with vpc: <VPCId>" — the fixed text is "Attempting to associate public zone: " and " with vpc: "; the two variable parts are the hosted zone ID taken from the request path (the bare id, i.e. the segment after the last '/' of the `Id` the API hands back) and the VPC ID taken from VPC.VPCId in the request body. AWS names the zone by its ID, never by its DNS name.
Emulator: status 400 and code PublicZoneVPCAssociation are correct, but the Message is "You're trying to associate a VPC with a public hosted zone: <zone DNS name> is a public hosted zone, and Route 53 doesn't support associating a VPC with a public hosted zone." — the model's doc prose, keyed on the zone's DNS name, with the VPC ID absent entirely.
Why: the Message is the only place the refusal says WHICH zone and WHICH VPC were involved. A client correlating the refusal back to a zone it addressed by ID must now parse a DNS name AWS never sends, and the VPC ID AWS includes is simply not there; exact-match assertions and log scrapers on this error class all break.
Fix: the PublicZoneVPCAssociation construction in AssociateVPCWithHostedZone's handler — render AWS's sentence from the request's hosted zone ID and VPC ID instead of the doc prose and the zone name. (Note: open issue 20261029T220729Z-route53-route-53-refusal-messages-are-emulator-authored-instead.md describes this same wording defect as its Cause 2 of 3 from the wire corpus; this behavioral check is grounded in the same emulator defect and the report did not mark it owned.)
Traces: route53.overly-permissive-associate-vpc-with-public-zone. Handler location: services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/associate_vpc_with_hosted_zone.rs lines 233-245.

Cause 2 of 2 — ChangeResourceRecordSets InvalidChangeBatch duplicates every problem: it fills both `message` and the `messages` list, so a client that reads both sees each problem twice.
AWS: Smithy: InvalidChangeBatch is errorKind client, httpError 400, with members `message` (ErrorMessage) and `messages` (ErrorMessages list). Recorded behavior: on a DELETE whose name and type match an existing record set but whose values (or TTL) do not, AWS answers 400 InvalidChangeBatch with Error.Message carrying the problem list rendered in brackets — "[Tried to delete resource record set [name='<record name>', type='<type>'] but the values provided do not match the current values]" — and the parsed response carries NO `messages` list alongside it. The fixed text is "Tried to delete resource record set [name='", "', type='", "'] but the values provided do not match the current values", wrapped in a single pair of brackets for the whole list; the variable parts are the record set's name and type echoed from the submitted change. The same holds whichever single field (values or TTL) fails to match: one problem, one bracketed rendering.
Emulator: status 400 and code InvalidChangeBatch are correct, and Error.Message is the correct bracketed rendering, but the emulator ALSO populates the modeled `messages` list with the same problem text, so a client reading Error.Message plus `messages` (as the check does, joining with ' | ') observes "[Tried to delete resource record set [name='...', type='A'] but the values provided do not match the current values] | Tried to delete resource record set [name='...', type='A'] but the values provided do not match the current values" — the problem reported twice, once bracketed and once bare.
Why: a client that enumerates per-change problems from `messages` (the member AWS models for exactly that) sees N problems where AWS reports none there, and a client that concatenates both members double-counts every refused change; retry/diagnostic logic keyed on the number of batch problems gets the wrong count.
Fix: the InvalidChangeBatch construction for ChangeResourceRecordSets — emit only the bracketed single `message` AWS sends on the wire and leave `messages` unset, rather than filling both from the same problem list.
Traces: route53.rrset-strict-match-delete-requires-exact-values. Construction site: services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/change_resource_record_sets.rs lines 139-144 (`invalid_change_batch`, which sets message = "[" + join + "]" AND messages = problems).

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout; service heading `### route53` (79/113 passed).

Cause 1 — [mismatch] route53.overly-permissive-associate-vpc-with-public-zone:
  AssociateVPCWithHostedZone on a public hosted zone: code PublicZoneVPCAssociation matched (400); the message did not.
  expected: 'Attempting to associate public zone: <bare hosted zone id> with vpc: <vpc id>'
  got:      "You're trying to associate a VPC with a public hosted zone: <zone dns name>. is a public hosted zone, and Route 53 doesn't support associating a VPC with a public hosted zone."
  (AssertionError at check_route53_overly_permissive_associate_vpc_with_public_zone.py:151 — `assert message == expected`.) The positive control in the same check passes: the same call against a PRIVATE zone succeeds and the VPC appears in GetHostedZone.VPCs, so the divergence is specific to the public-zone refusal body.

Cause 2 — [mismatch] route53.rrset-strict-match-delete-requires-exact-values:
  ChangeResourceRecordSets DELETE with a non-matching value (name/type/TTL correct): code InvalidChangeBatch matched (400); the reported problem text did not.
  expected (Error.Message only; no `messages` list): "[Tried to delete resource record set [name='www.<zone>.', type='A'] but the values provided do not match the current values]"
  got (Error.Message joined with the `messages` entries by ' | '): "[Tried to delete resource record set [name='www.<zone>.', type='A'] but the values provided do not match the current values] | Tried to delete resource record set [name='www.<zone>.', type='A'] but the values provided do not match the current values"
  (AssertionError at check_route53_rrset_strict_match_delete_requires_exact_values.py:214 — `assert msg == mismatch`.) Record names/zone names above are per-run values; the requirement is the template, not the recorded instance.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Cause 1: with the route53 client, CreateHostedZone a PUBLIC zone (HostedZoneConfig.PrivateZone=false), create a VPC via ec2 CreateVpc, then call AssociateVPCWithHostedZone(HostedZoneId=<public zone id>, VPC={VPCId:<vpc>, VPCRegion:<region>}). Observe 400 PublicZoneVPCAssociation and compare Error.Message against 'Attempting to associate public zone: <bare zone id> with vpc: <vpc id>'.
3. Cause 2: CreateHostedZone a public zone, ChangeResourceRecordSets CREATE www.<zone> A TTL 300 value 203.0.113.10, then ChangeResourceRecordSets DELETE the same name/type/TTL with value 203.0.113.99 (and separately with TTL 60 and the correct value). Observe 400 InvalidChangeBatch and inspect BOTH Error.Message and the response's `messages` list: the same problem sentence appears in each.
4. Or replay the two behavioral checks named in trace_ids.
