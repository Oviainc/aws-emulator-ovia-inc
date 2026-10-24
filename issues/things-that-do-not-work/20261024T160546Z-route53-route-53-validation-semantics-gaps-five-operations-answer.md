Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.associate-vpc-without-authorization-rejected route53.create-ksk-malformed-key-argument-rejected route53.list-tags-for-resources-mixed-valid-invalid-fails-whole-batch route53.op-malformed-traffic-policy-document-rejected route53.update-hosted-zone-features-value-round-trip
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.associate-vpc-without-authorization-rejected|run#6|AssertionError|assert code == "NotAuthorizedException", f"expected NotAuthorizedException, got {code!r}"
Issue-Key: route53.create-ksk-malformed-key-argument-rejected|run#12|AssertionError|assert code == "InvalidKMSArn", f"expected InvalidKMSArn, got {code}"
Issue-Key: route53.list-tags-for-resources-mixed-valid-invalid-fails-whole-batch|run#8|AssertionError|assert code == "NoSuchHostedZone", f"expected NoSuchHostedZone, got {code!r}"
Issue-Key: route53.op-malformed-traffic-policy-document-rejected|run#3|AssertionError|assert code == "InvalidTrafficPolicyDocument", (
Issue-Key: route53.update-hosted-zone-features-value-round-trip|run#13|AssertionError|assert code == "InvalidInput", (
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# Route 53 validation-semantics gaps: five operations answer NotImplemented where AWS returns a specific modeled refusal (NotAuthorizedException / InvalidKMSArn / NoSuchHostedZone / InvalidTrafficPolicyDocument / InvalidInput)

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateVPCWithHostedZone

## Description
Five distinct route53 negative-path contracts are unmet: each operation answers the generic `NotImplemented` ("operation X is scaffolded but not implemented in this emulator") instead of the modeled, condition-specific refusal AWS returns. Each operation already has an open "unimplemented scaffold stub" issue that covers merely standing the handler up; these causes additionally pin the VALIDATION contract each handler must satisfy once it exists, which those stub issues do not state. A fixer must close all five before submitting.

Smithy model consulted: services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json.

=== Cause 1 of 5: AssociateVPCWithHostedZone must refuse an unauthorized VPC with NotAuthorizedException ===
AWS: Smithy: AssociateVPCWithHostedZone POST /2013-04-01/hostedzone/{HostedZoneId}/associatevpc models error `NotAuthorizedException` with httpError 401 (also InvalidVPCId 400, PublicZoneVPCAssociation 400, NoSuchHostedZone 404). Associating a syntactically well-formed VPC id the calling account neither owns nor holds a matching CreateVPCAssociationAuthorization for is refused with `NotAuthorizedException` (401) and the association does not take effect; the zone's VPC list is unchanged. The denial message is built from the request and the caller identity, so the RULE is the requirement: "User: <caller arn> is not authorized to perform: route53:AssociateVPCWithHostedZone on resource: arn:aws:ec2:<region>:<account>:vpc/<vpc-id>" (recorded text is an example of that rule).
Emulator: returns error code `NotImplemented` for the call.
Why: a client testing cross-account private-zone authorization sees an unclassifiable NotImplemented instead of a 401 NotAuthorizedException, so authorization-denied handling (retry vs. request-authorization vs. fail) can never be exercised, and an over-permissive implementation that silently associates a foreign VPC would be indistinguishable.
Fix: the AssociateVPCWithHostedZone handler and its validation (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/associate_vpc_with_hosted_zone.rs) plus the VPC-association-authorization state it must consult. Related open issue: 20261024T114159Z-route53-associatevpcwithhostedzone-stub.md (stands the handler up; does not state this rule).
Traces: route53.associate-vpc-without-authorization-rejected.

=== Cause 2 of 5: CreateKeySigningKey must refuse a wrong-key-spec KMS ARN with InvalidKMSArn ===
AWS: Smithy: CreateKeySigningKey POST /2013-04-01/keysigningkey (success 201) models error `InvalidKMSArn` (httpError 400, defaulted), alongside InvalidKeySigningKeyName, InvalidKeySigningKeyStatus, KeySigningKeyAlreadyExists 409, NoSuchHostedZone 404. Route 53 DNSSEC requires an ECC_NIST_P256 SIGN_VERIFY customer-managed key; a well-formed ARN naming an RSA_2048 SIGN_VERIFY key is refused with `InvalidKMSArn` (400) and NO KSK is persisted (GetDNSSEC must not list the rejected name), while an ECC_NIST_P256 key on the same zone succeeds and the created KSK echoes the requested Name and Status. The message is derived from the request's ARN ("The customer managed key with ARN '<arn>' could not be used by Route 53 DNSSEC...") — that derivation is the requirement, the recorded text an example.
Emulator: returns error code `NotImplemented`.
Why: a client cannot distinguish an unusable KMS key from an outage, and cannot verify that a rejected CreateKeySigningKey leaves no partial KSK behind — a DNSSEC setup flow would proceed believing the key was accepted.
Fix: the CreateKeySigningKey handler and its KMS-ARN/key-spec validation (services/.../src/ops/create_key_signing_key.rs), plus the DNSSEC/KSK state read by GetDNSSEC. Related open issue: 20261024T114159Z-route53-createkeysigningkey-stub.md.
Traces: route53.create-ksk-malformed-key-argument-rejected.

=== Cause 3 of 5: ListTagsForResources must fail the WHOLE batch with NoSuchHostedZone when any id is unknown ===
AWS: Smithy: ListTagsForResources POST /2013-04-01/tags/{ResourceType} takes ResourceIds (list, @length min 1 max 10) and models `NoSuchHostedZone` httpError 404 (also NoSuchHealthCheck 404, InvalidInput 400). A batch mixing one valid hosted-zone id with one nonexistent id is refused in full with `NoSuchHostedZone` (404) — it does not partially succeed and drop the bad id. The message names the offending id ("No hosted zone found with ID: <id>"), which is the rule; the exact text is an example. Positive control: the same valid id alone returns exactly one ResourceTagSet carrying its tags.
Emulator: returns error code `NotImplemented`.
Why: a client batching tag reads gets no all-or-nothing guarantee and no 404 signal, so a typo'd or deleted zone id is indistinguishable from an unimplemented API; code that relies on the batch failing atomically may instead act on a silently short result set.
Fix: the ListTagsForResources handler and its per-id existence validation (services/.../src/ops/list_tags_for_resources.rs), which must validate every id before emitting any ResourceTagSet. Related open issue: 20261024T114159Z-route53-listtagsforresources-stub.md.
Traces: route53.list-tags-for-resources-mixed-valid-invalid-fails-whole-batch.

=== Cause 4 of 5: CreateTrafficPolicy must refuse a structurally invalid document with InvalidTrafficPolicyDocument ===
AWS: Smithy: CreateTrafficPolicy POST /2013-04-01/trafficpolicy (success 201) models error `InvalidTrafficPolicyDocument` httpError 400 (also InvalidInput 400, TooManyTrafficPolicies 400, TrafficPolicyAlreadyExists 409). A document with AWSPolicyFormatVersion and RecordType but no Endpoints and no StartEndpoint/StartRule is refused with `InvalidTrafficPolicyDocument` (400); the recorded message enumerates the structural violations ("At least one endpoint must be declared.;Either StartRule or StartEndpoint must be declared, but not both") and is an example of that rule. Positive control: a document declaring Endpoints and StartEndpoint is created and the Document round-trips byte-identical through GetTrafficPolicy.
Emulator: returns error code `NotImplemented`.
Why: traffic-policy authoring tools rely on InvalidTrafficPolicyDocument to surface document defects to the user; NotImplemented gives no actionable classification, and a non-validating implementation would accept broken policies that real AWS rejects.
Fix: the CreateTrafficPolicy handler plus traffic-policy document parsing/validation and the traffic-policy state read back by GetTrafficPolicy (services/.../src/ops/create_traffic_policy.rs). Related open issue: 20261024T114159Z-route53-createtrafficpolicy-stub.md.
Traces: route53.op-malformed-traffic-policy-document-rejected.

=== Cause 5 of 5: UpdateHostedZoneFeatures must refuse EnableAcceleratedRecovery=true on a PRIVATE zone with InvalidInput, and leave Features untouched ===
AWS: Smithy: UpdateHostedZoneFeatures POST /2013-04-01/hostedzone/{HostedZoneId}/features, input EnableAcceleratedRecovery (boolean), empty output, models `InvalidInput` httpError 400 (also NoSuchHostedZone 404, LimitsExceeded, PriorRequestNotComplete). Accelerated recovery is public-zone-only, so EnableAcceleratedRecovery=true against a private hosted zone is refused with `InvalidInput` (400); the recorded message "Private hosted zones cannot enable accelerated recovery" is an example. The representation contract: the HostedZone.Features value must be stable across repeated GetHostedZone reads and byte-identical before and after the REJECTED mutation attempt.
Emulator: returns error code `NotImplemented`.
Why: a client cannot learn that the feature is public-zone-only, and the no-op-on-rejection guarantee for stored Features is unverifiable — a partially-applied feature flag after a refused call would go undetected.
Fix: the UpdateHostedZoneFeatures handler and its private/public-zone validation, and the hosted-zone Features state and its serializer in GetHostedZone (services/.../src/ops/update_hosted_zone_features.rs). Related open issue: 20261024T114159Z-route53-updatehostedzonefeatures-stub.md.
Traces: route53.update-hosted-zone-features-value-round-trip.

## Evidence
All five failures are 'mismatch' verdicts from suite simparity-behavioral-ga-prod-v1 (14/118 passed). In every case the emulator returned botocore Error.Code = 'NotImplemented' with message 'operation <X> is scaffolded but not implemented in this emulator'.

Cause 1 — route53.associate-vpc-without-authorization-rejected:
  Expected: Error.Code 'NotAuthorizedException', HTTP 401, association not applied.
  Actual:   AssertionError: expected NotAuthorizedException, got 'NotImplemented'

Cause 2 — route53.create-ksk-malformed-key-argument-rejected:
  Expected: Error.Code 'InvalidKMSArn', HTTP 400, no KSK persisted.
  Actual:   AssertionError: expected InvalidKMSArn, got NotImplemented

Cause 3 — route53.list-tags-for-resources-mixed-valid-invalid-fails-whole-batch:
  Expected: Error.Code 'NoSuchHostedZone', HTTP 404, whole batch refused.
  Actual:   AssertionError: expected NoSuchHostedZone, got 'NotImplemented'

Cause 4 — route53.op-malformed-traffic-policy-document-rejected:
  Expected: Error.Code 'InvalidTrafficPolicyDocument', HTTP 400.
  Actual:   AssertionError: expected InvalidTrafficPolicyDocument for a document missing Endpoints/StartEndpoint, got NotImplemented

Cause 5 — route53.update-hosted-zone-features-value-round-trip:
  Expected: Error.Code 'InvalidInput', HTTP 400, Features unchanged.
  Actual:   AssertionError: expected InvalidInput rejecting EnableAcceleratedRecovery=True on a private zone (accelerated recovery is public-zone-only), got 'NotImplemented'

Modeled-error confirmation (smithy_query query=operation on com-amazonaws-route53-awsdnsv20130401.json):
  AssociateVPCWithHostedZone errors include NotAuthorizedException (401), InvalidVPCId (400), PublicZoneVPCAssociation (400).
  CreateKeySigningKey errors include InvalidKMSArn (400), InvalidKeySigningKeyName, InvalidKeySigningKeyStatus.
  ListTagsForResources errors include NoSuchHostedZone (404); ResourceIds @length min 1 max 10.
  CreateTrafficPolicy errors include InvalidTrafficPolicyDocument (400).
  UpdateHostedZoneFeatures errors include InvalidInput (400); input member EnableAcceleratedRecovery: boolean.

## Steps to Reproduce
Boot the emulator and replay the five checks listed in trace_ids. Per cause:

Cause 1: create a VPC; CreateHostedZone with HostedZoneConfig.PrivateZone=true and that VPC; then AssociateVPCWithHostedZone with a well-formed but foreign VPC id (e.g. vpc-<17 hex>) for which no CreateVPCAssociationAuthorization exists. Expect 401 NotAuthorizedException; then GetHostedZone and confirm VPCs still contains only the owned VPC; then associate a genuinely owned VPC and confirm it appears.

Cause 2: CreateHostedZone (public); create a KMS SIGN_VERIFY key with KeySpec=RSA_2048 in us-east-1 with the dnssec-route53.amazonaws.com key policy; CreateKeySigningKey with that ARN. Expect 400 InvalidKMSArn; GetDNSSEC must not list the requested KSK name. Repeat with KeySpec=ECC_NIST_P256 and expect 201 with the echoed Name/Status=INACTIVE.

Cause 3: CreateHostedZone, ChangeTagsForResource to add a tag, then ListTagsForResources(ResourceType='hostedzone', ResourceIds=[<valid id>, 'Z00000000000000000000']). Expect 404 NoSuchHostedZone for the whole call. Then ListTagsForResources with only the valid id and expect exactly one ResourceTagSet carrying the tag.

Cause 4: CreateTrafficPolicy with Document='{"AWSPolicyFormatVersion":"2015-10-01","RecordType":"A"}'. Expect 400 InvalidTrafficPolicyDocument. Then CreateTrafficPolicy with a document declaring Endpoints and StartEndpoint, expect 201, and GetTrafficPolicy must return the Document byte-identical.

Cause 5: create a VPC with DNS support/hostnames enabled; CreateHostedZone with PrivateZone=true and that VPC; GetHostedZone twice and record HostedZone.Features; UpdateHostedZoneFeatures(HostedZoneId, EnableAcceleratedRecovery=True). Expect 400 InvalidInput; then GetHostedZone twice more and confirm Features is byte-identical to the baseline.
