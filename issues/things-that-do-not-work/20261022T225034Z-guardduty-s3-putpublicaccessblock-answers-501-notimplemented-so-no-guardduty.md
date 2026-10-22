Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.ipset-lifecycle-active-then-deleted guardduty.threatintelset-lifecycle guardduty.threatentityset-lifecycle
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|PutPublicAccessBlock|NotImplemented
Issue-Commit: ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c

# S3 PutPublicAccessBlock answers 501 NotImplemented, so no GuardDuty threat-list (IPSet / ThreatIntelSet / ThreatEntitySet) lifecycle is reachable

> _Note: this issue was identified against the code base at commit `ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutPublicAccessBlock

## Description
Cause 1 of 1 — s3:PutPublicAccessBlock is an unimplemented scaffold stub, and it sits on the only path that provisions a GuardDuty threat-list source object.
AWS: Smithy (s3): PutPublicAccessBlock is PUT /{Bucket}?publicAccessBlock with required Bucket and required PublicAccessBlockConfiguration, modeled success status 200 and no modeled error shapes; it creates or replaces the bucket's PublicAccessBlock configuration (the handler's own modeled-contract header in the checkout states this). AWS answers 200 and GetPublicAccessBlock then reflects the stored configuration. Recorded behavior: the checks call it to relax block-public-access on a scratch bucket so the uploaded IP/threat list can be served at the Location that CreateIPSet / CreateThreatIntelSet / CreateThreatEntitySet consume.
Emulator: HTTP 501, error code `NotImplemented`, message "operation PutPublicAccessBlock is scaffolded but not implemented in this emulator" — see services/service-com-amazonaws-s3-amazons3/src/ops/put_public_access_block.rs (the SCAFFOLDED marker at lines 3-6 and the `Err(crate::scaffolded::Scaffolded::not_implemented("PutPublicAccessBlock"))` at line 199).
Why: any client that configures bucket public access — the standard way to publish a GuardDuty threat list, and a near-universal step in S3 bucket setup and security-posture tooling — receives a 501 for a call AWS answers 200. Downstream, the whole GuardDuty threat-list surface is unexercisable: the create -> ACTIVE -> update -> DELETED transitions of IPSet, ThreatIntelSet and ThreatEntitySet (GetIPSet/GetThreatIntelSet/GetThreatEntitySet Status reporting) can never be reached, so those state machines are unverified behind this single stub.
Fix: implement the PutPublicAccessBlock handler in the S3 service crate — services/service-com-amazonaws-s3-amazons3/src/ops/put_public_access_block.rs — storing the PublicAccessBlockConfiguration on the bucket's state, answering the modeled 200, and removing the SCAFFOLDED marker. Note: the finding's service token is taken from the failing check ids, which are all guardduty.*; the defect itself lives in the S3 crate, and no "unimplemented scaffold stub" issue for PutPublicAccessBlock was filed by this suite run.
Traces: guardduty.ipset-lifecycle-active-then-deleted, guardduty.threatintelset-lifecycle, guardduty.threatentityset-lifecycle.

## Evidence
All three checks abort identically, before any GuardDuty assertion runs:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutPublicAccessBlock operation: operation PutPublicAccessBlock is scaffolded but not implemented in this emulator

Call site in each check: _make_public_list_bucket(ctx, <ip/threat list body>) is invoked right after the detector is resolved and before create_ip_set / create_threat_intel_set / create_threat_entity_set.

Stub confirmed in the checkout: services/service-com-amazonaws-s3-amazons3/src/ops/put_public_access_block.rs line 4 ("SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented") and line 199 (returns Scaffolded::not_implemented). Modeled contract in the same file: PUT /{Bucket}?publicAccessBlock, SUCCESS_STATUS 200, required Bucket + PublicAccessBlockConfiguration.

Blocked downstream assertions (from the GuardDuty model, services/service-com-amazonaws-guardduty-guarddutyapiservice/smithy-model/com-amazonaws-guardduty-guarddutyapiservice.json): CreateIPSet -> IpSetId, then GetIPSet must report required Status (IpSetStatus) ACTIVE and, after DeleteIPSet, DELETED; the ThreatIntelSet and ThreatEntitySet lifecycles mirror this.

## Steps to Reproduce
1. Boot the emulator and point a botocore s3 client at it.
2. CreateBucket(Bucket='scratch-gd-list').
3. PutPublicAccessBlock(Bucket='scratch-gd-list', PublicAccessBlockConfiguration={'BlockPublicAcls': False, 'IgnorePublicAcls': False, 'BlockPublicPolicy': False, 'RestrictPublicBuckets': False}).
4. Observe HTTP 501 with error code NotImplemented instead of the modeled 200.
5. Consequently, with a guardduty client: CreateDetector(Enable=true) then CreateIPSet(DetectorId=..., Name=..., Format='TXT', Location=<object url in that bucket>, Activate=true) and the GetIPSet ACTIVE -> DeleteIPSet -> GetIPSet DELETED sequence cannot be exercised at all.
Or replay guardduty.ipset-lifecycle-active-then-deleted, guardduty.threatintelset-lifecycle and guardduty.threatentityset-lifecycle.
