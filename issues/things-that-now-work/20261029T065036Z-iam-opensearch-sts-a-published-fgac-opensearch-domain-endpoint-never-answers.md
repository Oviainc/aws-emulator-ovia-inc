Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden
Issue-Key: iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden|_wait_master#11|TimeoutError|raise TimeoutError("the IAM master getting 200 on /_cluster/health: the run budget ended first; not graded")
Issue-Commit: 6c829b3943454ada5df5dfe2630a7a4c6694ce1c

# A published FGAC OpenSearch domain endpoint never answers 200 to its own IAM master on GET /_cluster/health, so no data-plane authorization verdict is ever reachable

> _Note: this issue was identified against the code base at commit `6c829b3943454ada5df5dfe2630a7a4c6694ce1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
ONE root cause, grounded in 1 failing behavioral check (service token = the composite leading token of the check id, 'iam__opensearch__sts').

Already-open issues describing the same area whose replay keys the report did NOT match for this run (so this check is marked neither OWNED nor ALREADY FILED): 20261024T055421Z-iam-opensearch-sts-iam-role-to-fgac-search-authorization-check-never.md (carries this exact check id, but its Issue-Key fingerprint is `_wait_master#11|Timeout|time.sleep(10.0)` while the current failure raises from _wait_master at a different line with `time.sleep(2.0)`), and the broader 20261024T055421Z-opensearch-domain-data-plane-is-not-served-and.md / 20261026T044500Z-opensearch-package-association-e2e-flows-need-the-domain-data-plane-and.md / 20261029T044025Z-firehose-iam-opensearch-s3-both-firehose-fgac-opensearch.md. Verify together; this issue exists so the check id below is re-replayed.

Cause 1 of 1: the emulator's OpenSearch control plane does create the FGAC domain and does publish an endpoint — DescribeDomain reports Processing=false with an Endpoint, so the check's _wait_domain step passes — but SigV4-signed native REST requests to that published endpoint never return HTTP 200, not even for the domain's own IAM master principal (AdvancedSecurityOptions.Enabled=true, InternalUserDatabaseEnabled=false, MasterUserOptions.MasterUserARN = the master role). The check polls GET /_cluster/health as the master until the run budget is spent and therefore never reaches the behavior under test (a mapped role's _search returning 200 vs an unmapped role's 403 security_exception). Failing check: iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden.

AWS: the OpenSearch domain REST API is not a Smithy API — recorded behavior only. Smithy com.amazonaws.opensearch#CreateDomain models AdvancedSecurityOptions (Enabled, InternalUserDatabaseEnabled, MasterUserOptions.MasterUserARN) and DomainStatus.Endpoint, and the contract AWS honours around them is: once the domain publishes an Endpoint and Processing=false, a SigV4-signed request from the configured master principal to https://<endpoint>/_cluster/health is answered HTTP 200 with the cluster-health document; a signed request from an IAM principal with no FGAC role mapping is answered HTTP 403 with an engine error body of the form {"error": {"type": "security_exception", "reason": ...}, "status": 403} — fixed field names, with the reason naming the refused action/index from the request; and a mapped principal's identical request returns 200. The requirement is that the signed data-plane request to the endpoint the control plane published is answered at all, and that authorization decides between 200 and 403.

Emulator: the published endpoint never answers 200 to the master. The check exhausts its run budget inside _wait_master and reports `TimeoutError: the IAM master getting 200 on /_cluster/health: the run budget ended first; not graded`, so the last observed status code is not printed in this report; sibling opensearch checks in the already-open issues above record signed requests to the same endpoint being answered HTTP 404 {"__type": "UnknownService", "message": "no emulated service is registered for 'es'"} — Hypothesis: the same unrouted data plane is what this check is waiting on. Either way the divergence is the emulator's: it publishes a domain endpoint it then does not serve as a domain.

Why: FGAC is the authorization boundary for every OpenSearch read and write. A client relying on AWS's behavior cannot index or search against an emulated domain at all, and cannot observe any negative authorization result, so an emulated estate behaves as if domain-level authorization did not exist and a 'deny the unmapped role' expectation is never validated.

Fix: the emulator's request router/data-plane for OpenSearch domains — serve the published domain endpoint host (the 'es' native REST surface: /_cluster/health, /<index>/_doc, /<index>/_search) and enforce FGAC role mappings on signed requests so the master gets 200 and an unmapped IAM principal gets 403 security_exception. The endpoint the control plane publishes in DomainStatus.Endpoint must be the host that is served.

Traces: iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden (fails in _wait_master, after _create_iam_master_domain and _wait_domain both succeeded).

## Evidence
Replay report, service iam__opensearch__sts:

  [error] iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden
  File "check_iam__opensearch__sts_xsvc_opensearch_1_unmapped_iam_role_search_forbidden.py", line 330, in run
    _wait_master(ctx, host, master)
  File "...", line 265, in _wait_master
    raise TimeoutError("the IAM master getting 200 on /_cluster/health: the run budget ended first; not graded")
  TimeoutError: the IAM master getting 200 on /_cluster/health: the run budget ended first; not graded

Sequencing that places the defect in the data plane, not the control plane: the check first calls _create_iam_master_domain (CreateDomain with EngineVersion OpenSearch_2.11, ClusterConfig {t3.small.search, 1, DedicatedMasterEnabled false, ZoneAwarenessEnabled false}, EBSOptions {gp3, 10 GiB}, EncryptionAtRestOptions.Enabled true, NodeToNodeEncryptionOptions.Enabled true, DomainEndpointOptions {EnforceHTTPS true, Policy-Min-TLS-1-2-2019-07}, AdvancedSecurityOptions {Enabled true, InternalUserDatabaseEnabled false, MasterUserOptions.MasterUserARN=<master role arn>}, AccessPolicies as a JSON string allowing es:* to the account root) and then _wait_domain, which only returns once DescribeDomain reports Processing=false AND a non-empty Endpoint. Both returned. Only then does _wait_master poll, every 2 s, a SigV4-signed GET /_cluster/health against that endpoint, and no iteration ever saw HTTP 200.

Expected (AWS): HTTP 200 and a cluster-health document for the master principal; subsequently HTTP 200 for the FGAC-mapped reader's _search and HTTP 403 with {"error": {"type": "security_exception", ...}, "status": 403} for the unmapped role's identical _search, with no document content leaked in the refusal.

Actual (emulator): no 200 is ever observed on the published endpoint for the master; the data-plane half of the check (mapped 200 vs unmapped 403 security_exception) is never evaluated. Note on grading: the harness labelled this 'not graded' because the run budget expired, so this report does not carry the last HTTP status; the endpoint's failure to answer as a domain is nevertheless the emulator's, since the emulator both published that endpoint and serves it.

## Steps to Reproduce
1. Boot the emulator; confirm IAM, STS and OpenSearch answer.
2. Create an IAM role (trusted by the account root) with an inline Allow on es:ESHttp* for arn:aws:es:<region>:<account>:domain/<name>/*; this is the master.
3. opensearch.create_domain with AdvancedSecurityOptions={'Enabled': True, 'InternalUserDatabaseEnabled': False, 'MasterUserOptions': {'MasterUserARN': <master role arn>}}, EncryptionAtRestOptions={'Enabled': True}, NodeToNodeEncryptionOptions={'Enabled': True}, DomainEndpointOptions={'EnforceHTTPS': True, 'TLSSecurityPolicy': 'Policy-Min-TLS-1-2-2019-07'}, and AccessPolicies passed as a JSON STRING.
4. Poll describe_domain until DomainStatus.Processing is false and DomainStatus.Endpoint is non-empty; record that Endpoint.
5. Assume the master role and issue a SigV4-signed (service 'es') GET https://<Endpoint>/_cluster/health. On AWS this returns HTTP 200 with the cluster-health document; on the emulator no 200 is returned (sibling checks record HTTP 404 {"__type": "UnknownService", "message": "no emulated service is registered for 'es'"}).
6. After the fix, also verify the authorization half: index a document as the master, map a reader role via FGAC, and confirm the mapped role's POST /<index>/_search returns 200 with the document while an unmapped role's identical request returns HTTP 403 with error.type 'security_exception' and no document content.
7. Re-replay iam__opensearch__sts.xsvc-opensearch-1-unmapped-iam-role-search-forbidden.
