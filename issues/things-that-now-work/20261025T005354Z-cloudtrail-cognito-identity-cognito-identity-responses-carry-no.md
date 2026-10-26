Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__cognito-identity.obs-cognito-identity-2-identity-pool-data-events-are-delivered-redacted-and-never-in-event-history
Issue-Key: cloudtrail__cognito-identity.obs-cognito-identity-2-identity-pool-data-events-are-delivered-redacted-and-never-in-event-history|_round#50|KeyError|out["GetId"] = (r["ResponseMetadata"]["RequestId"], {})
Issue-Commit: d0dc720ebbf11903c1fa9407d1c82e53eafd4b75

# cloudtrail__cognito-identity: cognito-identity responses carry no x-amzn-RequestId, so ResponseMetadata has no RequestId to correlate a call with its audit record (1 cause)

> _Note: this issue was identified against the code base at commit `d0dc720ebbf11903c1fa9407d1c82e53eafd4b75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetId

## Description
ONE root cause (cause 1 of 1).

AWS: every AWS API response carries a request id header (x-amzn-RequestId for the AWS JSON 1.1 protocol cognito-identity speaks), which the SDK surfaces as ResponseMetadata['RequestId'] and which is the join key between a client-side call and its CloudTrail record (eventID/requestID) or a support case. It is present on success and on error.
Emulator: cognito-identity responses carry only content-type, content-length and date — no request-id header at all — so ResponseMetadata contains {'HTTPStatusCode', 'HTTPHeaders', 'RetryAttempts'} and reading ResponseMetadata['RequestId'] raises KeyError. The check that correlates each call with its delivered CloudTrail event dies on that read before any audit assertion runs.
Why: a client cannot correlate a request with its audit trail, log a request id for support, or deduplicate retries by request id; SDK logging and any tooling that records ResponseMetadata['RequestId'] crashes rather than degrading.
Fix: the response-serialization/HTTP layer for the cognito-identity service must emit the protocol's request-id header (x-amzn-RequestId) on every response, success and error alike.
Traces: cloudtrail__cognito-identity.obs-cognito-identity-2-identity-pool-data-events-are-delivered-redacted-and-never-in-event-history.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1:

cloudtrail__cognito-identity.obs-cognito-identity-2-identity-pool-data-events-are-delivered-redacted-and-never-in-event-history
  File "check_…obs_cognito_identity_2_identity_pool_data_events_….py", line 471, in _round
    out["GetId"] = (r["ResponseMetadata"]["RequestId"], {})
  KeyError: 'RequestId'

Corroborating response dumps from other cognito-identity calls in the same run show the same header set with no request id, e.g.
  'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '63', 'date': 'Sun, 25 Oct 2026 00:41:50 GMT'}, 'RetryAttempts': 0}
(observed for GetId, GetCredentialsForIdentity, GetIdentityPoolRoles, ListTagsForResource and SetPrincipalTagAttributeMap responses).

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-replay the check id in trace_ids.
2. Manual repro: call cognito-identity GetId (or any cognito-identity operation) and read response['ResponseMetadata']['RequestId'] -> expect a request id string echoed from the x-amzn-RequestId response header; observe KeyError because the header is absent.
