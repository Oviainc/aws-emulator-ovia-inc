Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3control.access-point-public-policy-blocked-by-own-block-public-policy-rejected
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control.access-point-public-policy-blocked-by-own-block-public-policy-rejected|run#11|KeyError|default_pab = s3c.get_access_point(AccountId=account, Name=blocked)[
Issue-Commit: 25144a28aa18b694814e19c38663cc989c688e9c

# s3control GetAccessPoint omits PublicAccessBlockConfiguration for an access point created without one

> _Note: this issue was identified against the code base at commit `25144a28aa18b694814e19c38663cc989c688e9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessPoint

## Description
One net-new root cause in the s3control emulator, grounded in a failing behavioral check of suite simparity-behavioral-ga-prod-v1 that the replay report did NOT mark as ALREADY FILED or OWNED. Related open issue: 20261024T005340Z-s3control-public-access-block-not-enforced-getaccesspoint-getaccesspointpolicystatus.md describes the same GetAccessPoint omission as its Cause 1 and even lists this check id in its header, but the suite did not report this check as owned by it (the recorded key's frame index has drifted), so the failure is still live and is filed here; a fixer closing that issue's Cause 1 should re-replay the check below and may close both together.

Cause 1 of 1: GetAccessPoint does not return PublicAccessBlockConfiguration when the access point was created without an explicit one.
AWS: Smithy GetAccessPoint (GET /v20180820/accesspoint/{Name}, 200, host prefix {AccountId}.) -> GetAccessPointResult declares the member PublicAccessBlockConfiguration, targeting structure PublicAccessBlockConfiguration { BlockPublicAcls, IgnorePublicAcls, BlockPublicPolicy, RestrictPublicBuckets } (each target Setting, boolean). Recorded behavior: an access point created by CreateAccessPoint(AccountId, Name, Bucket) with no PublicAccessBlockConfiguration still reports one on read, with all four flags on (access points are secure-by-default), so the member is always present in the 200 answer.
Emulator: the GetAccessPoint 200 response carries no PublicAccessBlockConfiguration member at all for such an access point, so the SDK's dict lookup raises KeyError: 'PublicAccessBlockConfiguration'. The serializer path itself works: when the member IS supplied at CreateAccessPoint it is echoed back on read, so the gap is the defaulted state record, not the wire encoding.
Why: a client that reads an access point's effective block settings before deciding whether a policy will be accepted (or to audit the default-secure posture) crashes on a missing modeled member instead of seeing the service's defaults, and the access point's own BlockPublicPolicy gate becomes unobservable.
Fix: the access-point state record (apply the default all-true PublicAccessBlockConfiguration when CreateAccessPoint supplies none) and the GetAccessPoint handler/serializer in the s3control crate, which must always emit the member.
Traces: s3control.access-point-public-policy-blocked-by-own-block-public-policy-rejected.

## Evidence
Suite simparity-behavioral-ga-prod-v1 — s3control 4/33 passed (12%).

[error] s3control.access-point-public-policy-blocked-by-own-block-public-policy-rejected
KeyError: 'PublicAccessBlockConfiguration' at check_s3control_access_point_public_policy_blocked_by_own_block_public_policy_rejected.py:41:
    default_pab = s3c.get_access_point(AccountId=account, Name=blocked)["PublicAccessBlockConfiguration"]
The access point named there was created immediately before by s3control CreateAccessPoint(AccountId, Name, Bucket) with no PublicAccessBlockConfiguration argument.

Expected (per Smithy GetAccessPointResult + recorded AWS behavior): the response contains PublicAccessBlockConfiguration == {BlockPublicAcls: true, IgnorePublicAcls: true, BlockPublicPolicy: true, RestrictPublicBuckets: true} — the four member names are fixed text from the model; their values are the service defaults for a freshly created access point.
Actual: the member is absent from the response, so the modeled key does not exist in the parsed output.

Because the check dies on this lookup, everything it grades after it (PutAccessPointPolicy of a public document refused with AccessDenied / HTTP 403 and message "Access Denied"; GetAccessPointPolicy then answering NoSuchAccessPointPolicy / HTTP 404 with message "The specified accesspoint policy does not exist"; a scoped policy accepted on the same access point with PolicyStatus.IsPublic=false) was never exercised and is not asserted by this issue.

## Steps to Reproduce
1. Create a general-purpose bucket (CreateBucketConfiguration.LocationConstraint = region when not us-east-1).
2. s3 DeletePublicAccessBlock(Bucket=<bucket>) so only the access point's own block remains.
3. s3control CreateAccessPoint(AccountId=<account>, Name=<ap>, Bucket=<bucket>) — pass NO PublicAccessBlockConfiguration.
4. s3control GetAccessPoint(AccountId=<account>, Name=<ap>) and read the PublicAccessBlockConfiguration member.
Expected: present, with all four flags true. Actual: the member is missing (KeyError in any SDK that indexes it).

Re-replay: s3control.access-point-public-policy-blocked-by-own-block-public-policy-rejected.
