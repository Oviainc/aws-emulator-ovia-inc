Issue-Source: trace-sweep-divergence
Issue-Trace: es-vpc-endpoint-errors
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-vpc-endpoint-errors
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|RevokeVpcEndpointAccess|4xx->4xx|$.message
Issue-Key: es|RevokeVpcEndpointAccess|4xx->4xx|status
Issue-Commit: 67ed68e040d396203b765d4dc2b0964aebb19c3c

# es RevokeVpcEndpointAccess not-found refusal replaces AWS's VPC-endpoint sentence with emulator wording

> _Note: this issue was identified against the code base at commit `67ed68e040d396203b765d4dc2b0964aebb19c3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RevokeVpcEndpointAccess

## Description
1 cause.

Cause 1 of 1: the refusal RevokeVpcEndpointAccess returns for a domain that cannot be resolved carries an emulator-authored message (and the wrong HTTP status).

AWS: RevokeVpcEndpointAccess refuses a request whose DomainName does not resolve to a VPC-endpoint-capable domain the caller may act on. Smithy: com.amazonaws.opensearch#RevokeVpcEndpointAccess / com.amazonaws.elasticsearchservice#RevokeVpcEndpointAccess, POST /2021-01-01/opensearch/domain/{DomainName}/revokeVpcEndpointAccess, DomainName @required @httpLabel @length(3,28) @pattern ^[a-z][a-z0-9\-]+$; modelled errors BaseException(400), DisabledOperationException(409), ResourceNotFoundException(409), ValidationException(400), InternalException(500). The recorded wire for this path answers HTTP 404 with the fixed sentence "Either the domain doesn't exist, it doesn't support creation of VPC endpoints, or you don't have permission to create a VPC endpoint for the domain." That whole message is fixed text: the VPC-endpoint access paths deliberately collapse absence, unsupported-domain and not-authorized into one sentence and name NO request value - there is no variable part to substitute. (The 404, like everywhere else on this service, contradicts the model's @httpError(409) on ResourceNotFoundException; recorded behavior is the requirement.)

Emulator: it answers HTTP 409 with the generic domain-lookup message "Domain not found: <DomainName>", i.e. it routes this operation through the service's ordinary domain-not-found path and interpolates the requested domain name into the message. In the recorded step the body is 'Domain not found: sim2real-probe-absent' - "sim2real-probe-absent" is just the request's DomainName and is not part of any AWS template; the defect is that a request value is interpolated at all, into a message AWS words differently.

Why: a client that matches on the refusal text to distinguish "the domain is gone" from "this domain does not support VPC endpoints" or "you are not authorized" gets a sentence AWS never sends, so that branch never fires; and the deliberate non-disclosure of which of the three conditions applied is lost, because the emulator confirms the exact domain name it failed to find. The 409 instead of 404 additionally pushes status-based clients down a conflict/retry branch for a permanent condition.

Fix: the RevokeVpcEndpointAccess handler's refusal path - it must raise the VPC-endpoint-access-specific not-found/not-permitted refusal with that fixed sentence and no interpolated request value, instead of reusing the generic domain-lookup error. The status half of this divergence is the same service-wide ResourceNotFoundException-to-404 mapping already open as 20261029T190009Z-es-resourcenotfoundexception-answered-as-http-409-instead-of.md; that issue does NOT carry this cluster's replay key and does not touch the message, so fixing it alone leaves this step failing. Also related but distinct: 20261029T230906Z-opensearch-listvpcendpointaccess-refuses-an-unknown-domain-with-resourcenotfoundexception.md covers ListVpcEndpointAccess answering a refusal where AWS answers 200.

Traces: fixture es-vpc-endpoint-errors, step task#6 (RevokeVpcEndpointAccess against an unresolvable domain).

## Evidence
Cluster divergence_key service=es|RevokeVpcEndpointAccess|status=404->409|keys=$.message,status; fixture es-vpc-endpoint-errors step task#6; affected_count 1.

Expected (recorded AWS): HTTP 404, body message = "Either the domain doesn't exist, it doesn't support creation of VPC endpoints, or you don't have permission to create a VPC endpoint for the domain." (entire sentence fixed; no request or state values appear in it).
Actual (emulator): HTTP 409, body message = 'Domain not found: sim2real-probe-absent' - fixed prefix "Domain not found: " authored by the emulator plus the request's DomainName.
Replay detail: "status: expected 404, got 409" and "$.message: value_mismatch".

Model cited: RevokeVpcEndpointAccess errors include ResourceNotFoundException (@httpError 409), ValidationException (400) and DisabledOperationException (409); DomainName @required @httpLabel @length(3,28) @pattern ^[a-z][a-z0-9\-]+$. The model does not state the message text, so the message requirement is recorded behavior.

## Steps to Reproduce
1. Replay fixture es-vpc-endpoint-errors, step task#6: POST /2021-01-01/opensearch/domain/<domain-that-does-not-resolve>/revokeVpcEndpointAccess with a well-formed Account.
2. Observe HTTP 409 with body message 'Domain not found: <DomainName>'.
3. Expected is HTTP 404 with the fixed sentence "Either the domain doesn't exist, it doesn't support creation of VPC endpoints, or you don't have permission to create a VPC endpoint for the domain.", which must not name the requested domain.
4. Re-replay es-vpc-endpoint-errors#task#6 to confirm; note that fixing only the service-wide 409->404 status mapping leaves the $.message mismatch and this step still fails.
