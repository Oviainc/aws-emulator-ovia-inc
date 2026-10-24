Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden
Issue-Key: iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden|_wait_master#11|Timeout|time.sleep(10.0)
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# IAM-role-to-FGAC search authorization check never completes: the OpenSearch domain data plane never answers

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
One root cause.

Cause 1 of 1: an assumed IAM role's search request against a fine-grained-access-control domain never resolves, so the 'unmapped role is forbidden' behavior cannot be reached.
AWS: a domain created with AdvancedSecurityOptions (FGAC) publishes a REST endpoint; a SigV4-signed request from an IAM principal that has no FGAC role mapping is answered by the domain with HTTP 403 and a security_exception body, while a mapped principal's identical request succeeds. Recorded behavior only for the body shape; the requirement is that the signed data-plane request is answered at all, and that an unmapped principal is refused rather than served.
Emulator: the check runs to the harness deadline without completing ('Check timed out after 60.0s'). Hypothesis: it is the same root cause this sweep files for the opensearch service — the domain REST data plane is not served at all (signed requests to the published domain endpoint return HTTP 404 {"__type": "UnknownService", "message": "no emulated service is registered for 'es'"}), so the check's wait for a responsive endpoint never terminates.
Why: FGAC is the authorization boundary for OpenSearch reads and writes. A client cannot verify that an unmapped IAM role is denied, and no negative authorization result can be observed, so an emulated estate silently behaves as if authorization did not exist.
Fix: register and serve the OpenSearch domain data plane (the 'es' endpoint prefix / published domain endpoint host) in the emulator's request router, and enforce FGAC role mappings on signed data-plane requests.
Traces: iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden.

## Evidence
Expected: a SigV4-signed search request from an unmapped assumed-role principal against the FGAC domain endpoint -> HTTP 403 (security_exception); a mapped principal -> HTTP 200.
Actual: Error: Check timed out after 60.0s — no data-plane response is ever obtained. Sibling opensearch checks show the underlying response for signed requests to the same endpoint: HTTP 404 {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}.

## Steps to Reproduce
1. Create an IAM role and assume it via STS.
2. opensearch.create_domain with AdvancedSecurityOptions (internal user database master user) and wait for Active.
3. Issue a SigV4-signed GET/POST _search against DomainStatus['Endpoint'] with the assumed-role credentials.
4. No 403 (or 200) is returned; the endpoint does not answer as a domain.
5. Replay iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden.
