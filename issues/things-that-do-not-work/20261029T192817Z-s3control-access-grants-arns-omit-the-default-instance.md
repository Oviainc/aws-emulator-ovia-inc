Issue-Source: trace-sweep-divergence
Issue-Trace: s3control-access-grants-grant-lifecycle-and-location-referential-integrity
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3control-access-grants-grant-lifecycle-and-location-referential-integrity s3control-access-grants-location-lifecycle s3control-access-grants-instance-resource-policy-lifecycle
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|GetAccessGrant|2xx->2xx|$.GetAccessGrantResult.AccessGrantArn
Issue-Key: s3control|GetAccessGrant|2xx->2xx|$.GetAccessGrantResult.UsageLimit
Issue-Key: s3control|CreateAccessGrantsLocation|2xx->2xx|$.CreateAccessGrantsLocationResult.AccessGrantsLocationArn
Issue-Key: s3control|GetAccessGrantsLocation|2xx->2xx|$.GetAccessGrantsLocationResult.AccessGrantsLocationArn
Issue-Key: s3control|UpdateAccessGrantsLocation|2xx->2xx|$.UpdateAccessGrantsLocationResult.AccessGrantsLocationArn
Issue-Key: s3control|GetAccessGrantsInstanceResourcePolicy|2xx->2xx|$.GetAccessGrantsInstanceResourcePolicyResult.Policy
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# s3control Access Grants ARNs omit the default instance segment and reformat stored policies

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessGrantsLocation

## Description
5 net-new root causes on the s3control Access Grants surface: the ARNs this service vends for grants and locations are missing the instance (`default`) segment, GetAccessGrant omits UsageLimit, and a stored policy document is re-serialized instead of returned as supplied.

--- Cause 1 of 5: GetAccessGrant returns a grant ARN without the instance segment, and no UsageLimit ---
AWS: recorded behavior only (Smithy: GetAccessGrant output AccessGrantArn; the model's sibling pattern for locations, `^arn:[a-z\-]+:s3:[a-z0-9\-]+:\d{12}:access\-grants\/location/[a-zA-Z0-9\-]+$`, also omits the segment, so the recording is the authority). The recorded ARN is `arn:aws:s3:<region>:<account>:access-grants/default/grant/<grantId>` — the literal segment `default` (the Access Grants instance) sits between `access-grants` and `grant`. The response also carries `UsageLimit`.
Emulator: returns `arn:aws:s3:<region>:<account>:access-grants/grant/<grantId>` (no instance segment) and omits UsageLimit.
Why: the ARN is the resource's identity — a client that stores it, puts it in an IAM policy or feeds it back to another API addresses a resource that does not exist on AWS, and an absent UsageLimit reads as an unset field rather than its value.
Fix: the Access Grants ARN construction in the s3control crate (shared by grant/location state and their serializers) plus the GetAccessGrant projection.
Traces: s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#7.

--- Cause 2 of 5: CreateAccessGrantsLocation returns a location ARN without the instance segment ---
AWS: recorded behavior only — the created location's ARN is `arn:aws:s3:<region>:<account>:access-grants/default/location/<locationId>`; the fixture asserts its shape, not a literal id.
Emulator: returns `arn:aws:s3:<region>:<account>:access-grants/location/<locationId>`, which fails that shape.
Why: as cause 1 — the identity handed to the caller at create time is wrong, so everything derived from it is wrong.
Fix: the Access Grants location ARN construction / CreateAccessGrantsLocation serializer.
Traces: s3control-access-grants-location-lifecycle#task#6.

--- Cause 3 of 5: GetAccessGrantsLocation returns the same malformed ARN ---
AWS: recorded behavior only — `access-grants/default/location/<locationId>`.
Emulator: `access-grants/location/<locationId>`.
Why: as cause 1; the read path confirms the wrong identity, so a client cannot recover from cause 2 by re-reading.
Fix: the GetAccessGrantsLocation projection over the shared ARN builder.
Traces: s3control-access-grants-location-lifecycle#task#7 (2 steps in cluster).

--- Cause 4 of 5: UpdateAccessGrantsLocation returns the same malformed ARN ---
AWS: recorded behavior only — `access-grants/default/location/<locationId>`.
Emulator: `access-grants/location/<locationId>`.
Why: as cause 1.
Fix: the UpdateAccessGrantsLocation projection over the shared ARN builder.
Traces: s3control-access-grants-location-lifecycle#task#9.

--- Cause 5 of 5: GetAccessGrantsInstanceResourcePolicy rewrites the stored policy document ---
AWS: recorded behavior only — the policy document is returned as the byte string that was put: a single-valued `Action` stays a JSON string.
Emulator: returns the document re-serialized from a parsed form, promoting the single `Action` value to a one-element array (`"Action":["s3:ListAccessGrants"]`).
Why: a client that diffs the policy it put against the policy it reads back sees a spurious change, and signature/hash-based change detection on the document always reports drift.
Fix: the Access Grants instance resource-policy state — store and return the policy document verbatim instead of round-tripping it through a JSON model.
Traces: s3control-access-grants-instance-resource-policy-lifecycle#task#4.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator (all at HTTP 200):
- GetAccessGrant: `$.GetAccessGrantResult.AccessGrantArn: value_mismatch (expected 'arn:aws:s3:us-west-2:<acct>:access-grants/default/grant/<id>', got 'arn:aws:s3:us-west-2:<acct>:access-grants/grant/<id>')` and `$.GetAccessGrantResult.UsageLimit: missing (expected '0', got None)` — s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#7.
- CreateAccessGrantsLocation: `$.CreateAccessGrantsLocationResult.AccessGrantsLocationArn: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:s3:us-west-2:<acct>:access-grants/location/<id>')` — s3control-access-grants-location-lifecycle#task#6.
- GetAccessGrantsLocation and UpdateAccessGrantsLocation: `expected '...:access-grants/default/location/<id>', got '...:access-grants/location/<id>'` — same fixture task#7 (2 affected steps) and task#9.
- GetAccessGrantsInstanceResourcePolicy: `$.GetAccessGrantsInstanceResourcePolicyResult.Policy: value_mismatch` — expected `"Action":"s3:ListAccessGrants"`, got `"Action":["s3:ListAccessGrants"]`; everything else in the document is byte-identical — s3control-access-grants-instance-resource-policy-lifecycle#task#4.

## Steps to Reproduce
1. Replay s3control-access-grants-location-lifecycle#task#6/#task#7/#task#9: CreateAccessGrantsInstance, then CreateAccessGrantsLocation, GetAccessGrantsLocation and UpdateAccessGrantsLocation. Each returned AccessGrantsLocationArn lacks the `default` instance segment.
2. Replay s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#7: CreateAccessGrant then GetAccessGrant — AccessGrantArn lacks `default` and UsageLimit is absent.
3. Replay s3control-access-grants-instance-resource-policy-lifecycle#task#4: PutAccessGrantsInstanceResourcePolicy with a statement whose Action is a bare string, then GetAccessGrantsInstanceResourcePolicy — the returned document has Action as a one-element array.
