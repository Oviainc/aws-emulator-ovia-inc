Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.vpc-endpoint-access-ops-on-missing-domain
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|ListVpcEndpointAccess|ResourceNotFoundException
Issue-Commit: c2fc8676d63317349fbaa2ea39a789b7f43b5131

# opensearch ListVpcEndpointAccess refuses an unknown domain with ResourceNotFoundException instead of answering 200 with an empty AuthorizedPrincipalList

> _Note: this issue was identified against the code base at commit `c2fc8676d63317349fbaa2ea39a789b7f43b5131`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListVpcEndpointAccess

## Description
One net-new root cause.

Cause 1 of 1: ListVpcEndpointAccess is permissive about the domain's existence in real AWS, but the emulator makes it a missing-resource refusal.
Failing check ids: opensearch.vpc-endpoint-access-ops-on-missing-domain

AWS: recorded behavior only for the permissiveness. The Smithy model (com-amazonaws-opensearch-amazonopensearchservice) binds ListVpcEndpointAccess to GET /2021-01-01/opensearch/domain/{DomainName}/listVpcEndpointAccess with success code 200; input DomainName @httpLabel @required, @length min 3 max 28, @pattern ^[a-z][a-z0-9\-]+$; output AuthorizedPrincipalList (@required list) and NextToken (@required); modeled errors BaseException (400), DisabledOperationException (409), InternalException (500), ResourceNotFoundException (409). So ResourceNotFoundException is MODELED for this operation, but the model does not say it is raised for an unknown DomainName, and the recording shows real AWS does NOT raise it there: the call succeeds with HTTP 200 and an empty AuthorizedPrincipalList. The recording also pins that this is deliberately NOT uniform across the three VPC-endpoint-access siblings: AuthorizeVpcEndpointAccess and RevokeVpcEndpointAccess DO reject an unknown domain with ResourceNotFoundException (both of those pass against the emulator today), while the read sibling ListVpcEndpointAccess is permissive. The requirement is therefore per-operation, not per-resource: only the two mutating siblings resolve the domain before answering.

Emulator: ListVpcEndpointAccess resolves the domain in the registry first and, when it is absent, returns the modeled ResourceNotFoundException with the shared domain-not-found message. Replay: `botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the ListVpcEndpointAccess operation: Domain not found: <DomainName>`. In that message, 'Domain not found: ' is the fixed text and the trailing value is the DomainName taken from the request — the text itself is the right template for the operations that DO refuse (Authorize/Revoke), so the defect is that this operation refuses at all, not how it words the refusal. The expected response is instead HTTP 200 with AuthorizedPrincipalList == [] and the @required NextToken member present (the empty token the handler already emits at the end of a listing).

Why: a client that enumerates endpoint grants across domains — a reconciler, a drift detector, a 'does this domain expose anything?' audit — sees an exception where AWS hands back an empty list. Against AWS such a client treats the empty list as 'no principals are authorized' and continues; against the emulator the same code aborts or has to swallow ResourceNotFoundException, which would then also swallow the genuine refusals from the Authorize/Revoke siblings. It also silently inverts the probe direction: callers that use this read as a cheap existence test will conclude a domain is absent only on the emulator, and conclude nothing on AWS.

Fix: the operation's handler — the domain lookup must stop being a precondition for this read. Resolve the domain optionally and report the stored vpc_endpoint_access set when it exists, otherwise answer the empty page (empty AuthorizedPrincipalList plus the empty NextToken) with 200. The paging branch (NextToken naming no principal -> ValidationException) stays as it is; for an unknown domain there is no set, so a non-empty token has no place to name and the empty page is the answer. Do not touch AuthorizeVpcEndpointAccess / RevokeVpcEndpointAccess, whose ResourceNotFoundException on an unknown domain is correct and is exercised by the same check.

Traces: opensearch.vpc-endpoint-access-ops-on-missing-domain. The handler that performs the refusal is services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/list_vpc_endpoint_access.rs lines 175-179 (`store.domains.get(&input.domain_name).ok_or_else(|| ...ResourceNotFoundException(crate::state::domain_not_found(...)))`).

## Evidence
Check: opensearch.vpc-endpoint-access-ops-on-missing-domain ([error]).

The check calls, against a syntactically valid but nonexistent domain name (3..28 chars, ^[a-z][a-z0-9\-]+$):
  1. AuthorizeVpcEndpointAccess(DomainName=missing, Account=<caller account>) -> expects ResourceNotFoundException. PASSES on the emulator.
  2. RevokeVpcEndpointAccess(DomainName=missing, Account=<caller account>) -> expects ResourceNotFoundException. PASSES on the emulator.
  3. ListVpcEndpointAccess(DomainName=missing) -> expects HTTP 200 with AuthorizedPrincipalList == [].

Expected (recorded real AWS): step 3 returns 200; resp['AuthorizedPrincipalList'] == [].

Actual (emulator): step 3 raises —
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the ListVpcEndpointAccess operation: Domain not found: sim2real-behav-os-missing-dc

Message decomposition: fixed text 'Domain not found: '; variable part is the DomainName supplied in the request ('sim2real-behav-os-missing-dc' here is this run's generated name and is not part of the requirement). No 2xx response is produced at all, so the required output members AuthorizedPrincipalList and NextToken are never serialized.

The check's own docstring records the asymmetry being pinned: "ListVpcEndpointAccess is genuinely PERMISSIVE: it SUCCEEDS for a domain that does not exist and returns an empty principal list. Pinning this stops an emulator being scored correct for 'helpfully' raising ResourceNotFoundException where real AWS returns 200."

Source: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/list_vpc_endpoint_access.rs:175-179 performs the registry lookup and converts a miss into ResourceNotFoundException before any page is built.

## Steps to Reproduce
1. Boot the emulator and point an opensearch client at it.
2. Pick a DomainName that satisfies the model's constraints (length 3..28, ^[a-z][a-z0-9\-]+$) and that no CreateDomain has ever created, e.g. 'sim2real-behav-os-missing-xx'.
3. Call AuthorizeVpcEndpointAccess(DomainName=<that name>, Account=<the caller's account id>) and observe ResourceNotFoundException — correct, keep it.
4. Call RevokeVpcEndpointAccess(DomainName=<that name>, Account=<the caller's account id>) and observe ResourceNotFoundException — correct, keep it.
5. Call ListVpcEndpointAccess(DomainName=<that name>).
   Observed: HTTP 409 ResourceNotFoundException, message 'Domain not found: <that name>'.
   Expected: HTTP 200 with AuthorizedPrincipalList == [] and the required NextToken member present (empty).
6. Re-replay the behavioral check to confirm: opensearch.vpc-endpoint-access-ops-on-missing-domain.
