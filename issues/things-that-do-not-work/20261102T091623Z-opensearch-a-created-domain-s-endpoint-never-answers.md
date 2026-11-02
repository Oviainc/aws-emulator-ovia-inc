Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden
Issue-Key: iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden|_wait_admin#11|TimeoutError|raise TimeoutError("the IAM admin role getting 200 on /_cluster/health: the run budget ended first; not graded")
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# opensearch: a created domain's endpoint never answers /_cluster/health, so no IAM role mapping can be exercised

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Cause 1 of 1: the domain endpoint never serves a request from the IAM master role.
AWS: recorded behavior only — once a domain with fine-grained access control reaches Processing=false, its endpoint answers a SigV4-signed GET /_cluster/health from the configured IAM master user with HTTP 200, which is what lets a client then prove an unmapped role is forbidden (403) on a search.
Emulator: that 200 never arrives within the run budget, so the admin wait fails and the forbidden-search behavior cannot be observed at all.
Why: the whole fine-grained-access-control surface of a domain (master user, role mapping, forbidden search) is unreachable; a client that waits for its domain to answer hangs instead of being served or refused.
Fix: the domain's data-plane endpoint must serve SigV4-signed requests once the domain is active (and refuse an unmapped role with 403). Related open issue: 20261030T062326Z-cognito-identity-cognito-idp-iam-opensea-opensearch-domainstatus-omits-domainendpointoptions-entirely-when-createdomain-did.md, which carries a different check's replay key.
Traces: iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden.

## Evidence
'TimeoutError: the IAM admin role getting 200 on /_cluster/health: the run budget ended first; not graded' — raised from the check's _wait_admin after the domain was created.

## Steps to Reproduce
1. CreateDomain with fine-grained access control and an IAM master user role; wait for Processing=false.
2. Send a SigV4-signed GET /_cluster/health to the domain endpoint as that role and observe no 200 ever arrives.
3. Or replay iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden.
