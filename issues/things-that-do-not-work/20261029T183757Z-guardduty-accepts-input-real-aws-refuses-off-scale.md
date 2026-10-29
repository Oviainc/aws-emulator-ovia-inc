Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.create-filter-out-of-range-criteria-rejected guardduty.create-ipset-malformed-s3-location-rejected guardduty.guardduty-activate-ipset-rejected-by-account-quota
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty.create-filter-out-of-range-criteria-rejected|_error_code#7|AssertionError|raise AssertionError(f"expected rejection but the call SUCCEEDED and returned {result!r}")
Issue-Key: guardduty.create-ipset-malformed-s3-location-rejected|_error_code#5|AssertionError|raise AssertionError(
Issue-Key: guardduty.guardduty-activate-ipset-rejected-by-account-quota|_client_error#10|AssertionError|raise AssertionError(f"expected real AWS to reject this call, but it succeeded: {result!r}")
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# GuardDuty accepts input real AWS refuses: off-scale filter criteria, malformed IPSet Location, and IPSet activation beyond the account limit

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIPSet

## Description
This issue bundles 3 distinct root causes, all in the guardduty crate: three refusals real GuardDuty performs are missing entirely, so the emulator answers 2xx and provisions a resource. These are over-permissive gaps, not message defects. Fix all 3 before submitting.

Cause 1 of 3: CreateFilter accepts a severity value outside GuardDuty's severity scale.
AWS: Smithy CreateFilter.FindingCriteria (required) carries condition values as strings, so the per-field value domain is not expressed in the model — recorded behavior only: a criterion {'severity': {'Equals': ['15']}} is refused HTTP 400 BadRequestException with the fixed text 'The request was rejected because the parameter findingCriteria has invalid values in a condition.', and ListFilters shows no new filter. GuardDuty's severity scale admits 1-10.
Emulator: returns HTTP 200 with {'Name': '<the requested filter name>'} and the filter is created.
Why: a client that relies on GuardDuty validating criterion values builds a filter that can never match a finding, and the parity between a rejected create and an absent filter is lost — the caller must now reap a resource AWS would never have made.
Fix: condition-value validation in the CreateFilter handler/criteria validator in the guardduty crate (src/ops/create_filter.rs).
Traces: guardduty.create-filter-out-of-range-criteria-rejected.

Cause 2 of 3: CreateIPSet accepts a Location that is not an S3 URI or ARN.
AWS: Smithy CreateIPSet.Location (required, length 1..300) carries no pattern, so the syntax rule is recorded behavior only: a Location of free text ('this is not a valid s3 uri or arn') is refused HTTP 400 BadRequestException with the fixed text 'The request is rejected because the parameter ipSetLocation has an invalid value.' (note AWS names ipSetLocation, not the wire member location), and ListIPSets is unchanged; a syntactically valid s3://bucket/key Location is accepted and read back verbatim.
Emulator: returns HTTP 200 with an IpSetId and provisions the IPSet from the garbage Location.
Why: the malformed-input rejection class disappears; a caller gets an IPSet id for a list GuardDuty could never fetch, and its own error handling for bad Locations is never exercised.
Fix: Location syntax validation in the CreateIPSet handler (and the sibling UpdateIPSet/ThreatIntelSet path that shares it) in the guardduty crate (src/ops/create_ip_set.rs).
Traces: guardduty.create-ipset-malformed-s3-location-rejected.

Cause 3 of 3: CreateIPSet with Activate=true is never refused for the account's activated-list limit.
AWS: Smithy CreateIPSet models InternalServerErrorException (httpError 500) alongside BadRequestException; recorded behavior only: with the account's activated-list allowance already consumed, CreateIPSet(Activate=true) is refused — the recording shows error code InternalServerErrorException with the fixed text 'The request is rejected because it\u2019s an attempt to create resources beyond the current AWS account limits.' (AWS's wording, with a U+2019 apostrophe in "it's"), before the Location is ever dereferenced, while the identical call with Activate=false succeeds and is readable via GetIPSet/ListIPSets.
Emulator: returns HTTP 200 and an IpSetId for the Activate=true call; no account-level activated-list accounting exists.
Why: a client exercising its quota-exhaustion path (back off, request a limit increase, fall back to Activate=false) never sees the refusal and silently believes an unlimited number of trusted/threat lists can be activated. Hypothesis: the activated-IPSet count is simply not tracked per account/region, so no limit can be evaluated.
Fix: per-account activated-list accounting and the limit check in the CreateIPSet (Activate=true) handler in the guardduty crate (src/ops/create_ip_set.rs plus the account-level state it reads).
Traces: guardduty.guardduty-activate-ipset-rejected-by-account-quota.

## Evidence
- guardduty.create-filter-out-of-range-criteria-rejected: 'expected rejection but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Name': 'sim2real-gd-filter-bad-...'}'. Expected BadRequestException / 'The request was rejected because the parameter findingCriteria has invalid values in a condition.'
- guardduty.create-ipset-malformed-s3-location-rejected: 'expected BadRequestException for a malformed Location, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'IpSetId': '...'}'. Expected BadRequestException / 'The request is rejected because the parameter ipSetLocation has an invalid value.'
- guardduty.guardduty-activate-ipset-rejected-by-account-quota: 'expected real AWS to reject this call, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'IpSetId': '...'}'. Expected InternalServerErrorException / 'The request is rejected because it\u2019s an attempt to create resources beyond the current AWS account limits.' for CreateIPSet(Activate=True) while the same call with Activate=False succeeds.

## Steps to Reproduce
1. Run the three guardduty checks in trace_ids.
2. Manual repro: CreateDetector, then (a) CreateFilter(Name=..., Action='NOOP', FindingCriteria={'Criterion': {'severity': {'Equals': ['15']}}}) — observe HTTP 200 and the filter in ListFilters; (b) CreateIPSet(Name=..., Format='TXT', Location='this is not a valid s3 uri or arn', Activate=False) — observe HTTP 200 and an IpSetId; (c) CreateIPSet(..., Activate=True) repeatedly — observe every call returns 200 with no account-limit refusal.
