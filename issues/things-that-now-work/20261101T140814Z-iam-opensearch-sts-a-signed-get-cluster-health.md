Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden
Issue-Key: iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden|_wait_master#11|TimeoutError|raise TimeoutError("the IAM master getting 200 on /_cluster/health: the run budget ended first; not graded")
Issue-Commit: 67f5fadccaf81bef3549b8820a70ca0b0ed0533e

# iam__opensearch__sts: a signed GET /_cluster/health as the FGAC master IAM role never answers 200, so the unmapped-role authorization walk never starts (1 cause)

> _Note: this issue was identified against the code base at commit `67f5fadccaf81bef3549b8820a70ca0b0ed0533e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DomainRestDataPlane

## Description
Cause 1 of 1 — the domain's FGAC master IAM role cannot reach the domain's own REST data plane: a SigV4-signed GET /_cluster/health as that role never answers HTTP 200.
AWS: on a domain with fine-grained access control and an IAM master user ARN, the master principal is authorized for every cluster and index action, so a SigV4-signed `GET <endpoint>/_cluster/health` as that role answers HTTP 200 with the cluster-health document within seconds of the domain becoming active. That 200 is the precondition for the actual claim — that a DIFFERENT IAM role which no role mapping binds is refused a search with HTTP 403 and `error.type` `security_exception`. Recorded behavior only for the emulator's behaviour: it produces no answer at all, so there is no status or error code to quote.
Emulator: the bounded poll for the master's 200 on `/_cluster/health` never succeeds and the check ends with `TimeoutError: the IAM master getting 200 on /_cluster/health: the run budget ended first`. Nothing is served for the master principal, so the unmapped-role refusal is never reached.
Why: a client that provisions an FGAC domain with an IAM master role and then uses that role — the documented way to administer and to seed such a domain — hangs instead of being authorized or refused, and the whole IAM-to-FGAC authorization boundary (master allowed, unmapped role `security_exception`) is unobservable. A caller cannot distinguish "not yet ready" from "never".
Note: open issue 20261024T055421Z-iam-opensearch-sts-iam-role-to-fgac-search-authorization-check-never.md describes this family as "the domain data plane never answers"; the data plane DOES now answer other signed requests, so the remaining failure is specific to the FGAC master IAM principal, and that issue does not carry this check's replay key.
Fix: the FGAC principal resolution and the IAM/cluster authorization decision on the domain REST data plane (the fgac and ingest::iam_decision state in services/service-com-amazonaws-opensearch-amazonopensearchservice), plus whatever keeps the domain from reporting itself serviceable to that principal.
Traces: iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden

## Evidence
Expected: a SigV4-signed GET <domain endpoint>/_cluster/health as the domain's AdvancedSecurityOptions MasterUserARN role answers HTTP 200 with the cluster-health document. Actual: the bounded poll never sees a 200 and the check raises TimeoutError('the IAM master getting 200 on /_cluster/health: the run budget ended first'), so the dependent assertion — an unmapped IAM role's search refused HTTP 403 with error.type 'security_exception' — is never evaluated.

## Steps to Reproduce
1. CreateRole in IAM for the FGAC master and a second CreateRole for an unmapped principal; AssumeRole via STS for each.
2. CreateDomain (OpenSearch_2.11, one t3.small.search node, gp3/10) with EncryptionAtRestOptions and NodeToNodeEncryptionOptions enabled, DomainEndpointOptions.EnforceHTTPS true, and AdvancedSecurityOptions {Enabled: true, InternalUserDatabaseEnabled: false, MasterUserOptions: {MasterUserARN: <master role ARN>}}; wait for the domain to be active and read DomainStatus.Endpoint.
3. SigV4-sign GET <endpoint>/_cluster/health with the MASTER role's credentials and poll for HTTP 200 -> never answered.
4. (Blocked by step 3) SigV4-sign a search as the unmapped role and expect HTTP 403 with error.type 'security_exception'.
