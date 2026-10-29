Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.deploy-domain-cfn-access-policy-enforced
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.deploy-domain-cfn-access-policy-enforced|_retry_until_status#10|TimeoutError|raise TimeoutError(f"status {expected} (last={last}): the run budget ended first; not graded")
Issue-Commit: ae2ab310e48fc3aa3e7d40afa9230e444be28e7b

# OpenSearch domain REST endpoint serves nothing, so a deployed AccessPolicies document is never enforced for SigV4-signed data-plane requests

> _Note: this issue was identified against the code base at commit `ae2ab310e48fc3aa3e7d40afa9230e444be28e7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
One root cause in the opensearch service.

Cause 1 of 1: the domain endpoint published in DomainStatus.Endpoint is not routed to an OpenSearch engine, and in particular no authorizer evaluates the domain's AccessPolicies resource policy for SigV4-signed requests.

AWS: Smithy (com.amazonaws.opensearch#AmazonOpenSearchService, awsService.endpointPrefix 'es', sigv4Name 'es') documents DomainStatus.Endpoint as the "Domain-specific endpoint used to submit index, search, and data upload requests to the domain." A domain created with an AccessPolicies document therefore answers SigV4-signed ('es') HTTPS requests at that host as an OpenSearch engine, and gates each request on the resource policy: a principal the policy explicitly allows gets the engine's normal verdicts (PUT /<index>/_doc/<id> -> 201 with result 'created', GET of the same document -> 200 with the stored _source, POST /<index>/_refresh -> 200, POST /<index>/_search -> 200 with hits), while a principal the policy explicitly Denies gets HTTP 403 whose JSON body states the principal is not authorized. The 403 body's fixed text is AWS's authorization refusal wording ("User: <assumed-role ARN> is not authorized to perform: <es:ESHttp*action> with an explicit deny in a resource-based policy"); its variable parts are the caller's assumed-role ARN (account id, role name, session name) taken from the signing credentials and the es: action derived from the HTTP method/path of the request. Recorded behavior only for the individual engine response bodies; the requirement under test is that the endpoint the control plane published is served AND that it enforces the deployed resource policy per principal.

Emulator: every SigV4-signed request to the published endpoint is answered by the emulator's generic front door with HTTP 404 and the body {"__type": "UnknownService", "message": "no emulated service is registered for 'es'"}. That is not an OpenSearch response at all: it is neither the engine's 200/201 for the allowed principal nor the 403 explicit-deny refusal for the denied principal. Because every principal gets the same 404, the AccessPolicies document CloudFormation deployed is inert — allow and deny are indistinguishable. The check's poll for the allowed principal's first write never leaves 404 ({200, 201} never reached, last=(404, UnknownService 'es')).

Why: a client that provisions a domain with a resource policy and then talks to the endpoint gets a 404 with an error shape no AWS SDK or OpenSearch client recognises, instead of either its data or an authorization refusal. Indexing, refresh and search are unreachable, and — the specific regression here — resource-policy authorization is unobservable, so a client cannot verify that the policy it deployed actually denies the principal it meant to deny. Security posture that depends on AccessPolicies silently reads as untestable/permissive.

Fix: the emulator's request router must register and serve the 'es' endpoint prefix / the published domain endpoint host, backed by a domain-scoped index+search engine, and that engine's request path needs an authorizer that resolves the signing principal from SigV4 and evaluates the domain's stored AccessPolicies document (explicit Deny wins) before dispatching, emitting AWS's 403 authorization-refusal body when it denies.

Related: the open issue 20261024T055421Z-opensearch-domain-data-plane-is-not-served-and.md describes the unrouted 'es' endpoint as its Cause 1, but its recorded replay keys were the CreateStack/DescribeDomain ResourceNotFoundException and engine-body assertions; this check's replay key is new (the CFN provider now does create the domain, so the failure has moved to the endpoint and to policy enforcement) and the suite did not report it as owned. Resource-policy enforcement for signed data-plane requests is not covered by that issue's Cause 1 text and must be closed here.

Traces: opensearch.deploy-domain-cfn-access-policy-enforced.

## Evidence
Expected: after the AWS::OpenSearchService::Domain stack reaches CREATE_COMPLETE and DescribeDomain reports Created=true/Processing=false with an Endpoint, a SigV4-signed ('es') PUT https://<endpoint>/sim2real-idx-acl/_doc/1 by the role the AccessPolicies Allow statement names returns 200 or 201; POST /<index>/_refresh returns 200; a signed GET /<index>/_doc/1 by the role the Deny statement names returns 403 with a body saying the principal is not authorized; a signed GET by the allowed role returns 200 with _source.title == 'allowed'; and POST /<index>/_search returns 200 with exactly one hit.

Actual: every signed request to the published endpoint returns HTTP 404 with body {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}. The allowed principal's write never reaches 200/201 — the suite reports:
  TimeoutError: status {200, 201} (last=(404, {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}))
raised from _retry_until_status at check_opensearch_deploy_domain_cfn_access_policy_enforced.py line 191, called from run() line 91. The denied principal's read is therefore also never refused with 403, so the deployed AccessPolicies document has no observable effect. (The 404 status/code/body above is the emulator's; the host:port in the URL is per-run and is not part of the requirement.)

## Steps to Reproduce
1. iam.create_role twice (an 'allowed' role and a 'denied' role), each with an inline policy allowing es:* on *, and a trust policy allowing the account root to sts:AssumeRole.
2. cloudformation.create_stack with an AWS::OpenSearchService::Domain resource (t3.small.search, InstanceCount 1, gp3 10 GiB) whose AccessPolicies has an Allow statement for the first role ARN and a Deny statement for the second, both on arn:aws:es:<region>:<account>:domain/<name>/*; wait for stack_create_complete.
3. opensearch.describe_domain(DomainName=<name>) and take DomainStatus['Endpoint'].
4. sts.assume_role for each role and issue a SigV4-signed ('es') PUT https://<Endpoint>/sim2real-idx-acl/_doc/1 with the allowed role's credentials.
   -> Expected 200/201 from the OpenSearch engine; actual HTTP 404 {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}.
5. Issue a signed GET https://<Endpoint>/sim2real-idx-acl/_doc/1 with the denied role's credentials.
   -> Expected HTTP 403 whose body says the principal is not authorized (explicit deny in a resource-based policy); actual the same HTTP 404 UnknownService 'es'.

Replay the check ids listed in trace_ids.
