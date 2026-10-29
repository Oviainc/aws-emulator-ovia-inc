Issue-Source: trace-sweep-divergence
Issue-Trace: es-vpc-endpoint-errors
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-vpc-endpoint-errors opensearch-domain-lifecycle-workflow opensearch-domain-maintenance-and-vpc-endpoint-listing opensearch-vpc-endpoint-directory opensearch-vpc-endpoint-error-paths esdef-list-instance-type-details-max-results-ignored esdef-list-versions-max-results-ignored opensearch-reserved-instance-offering-describe-and-purchase-errors eserr-describe-inbound-connections-invalid-pagination-token eserr-describe-outbound-connections-invalid-pagination-token esval-list-instance-type-details-single-page
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|ListVpcEndpointsForDomain|2xx->2xx|$.NextToken
Issue-Key: es|ListVpcEndpoints|2xx->2xx|$.NextToken
Issue-Key: es|ListInstanceTypeDetails|2xx->2xx|$.InstanceTypeDetails[*]
Issue-Key: es|ListInstanceTypeDetails|2xx->2xx|$.NextToken
Issue-Key: es|ListVersions|2xx->2xx|$.NextToken
Issue-Key: es|ListVersions|2xx->4xx|$.Versions
Issue-Key: es|ListVersions|2xx->4xx|status
Issue-Key: es|DescribeReservedInstanceOfferings|2xx->2xx|$.NextToken
Issue-Key: es|DescribeInboundConnections|4xx->4xx|$.message
Issue-Key: es|DescribeOutboundConnections|4xx->4xx|$.message
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# es: pagination contract is wrong - empty NextToken, ignored MaxResults, invalid-token wording

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceTypeDetails

## Description
8 net-new root causes in the shared pagination layer of this service.

Cause 1 of 8 - ListVpcEndpointsForDomain emits NextToken as the empty string. AWS omits NextToken entirely when the result set is complete; the emulator emits "". A client that tests for the member's presence rather than its truthiness paginates forever. Trace: es-vpc-endpoint-errors#task#3.

Cause 2 of 8 - ListVpcEndpoints has the same empty-string NextToken defect. Trace: opensearch-domain-lifecycle-workflow#task#3 (affected also opensearch-domain-maintenance-and-vpc-endpoint-listing#task#2, opensearch-vpc-endpoint-directory#task#1, opensearch-vpc-endpoint-error-paths#task#1/#task#4).

Cause 3 of 8 - ListInstanceTypeDetails ignores MaxResults and still returns a NextToken. AWS returns the whole catalog in one page and no NextToken for the recorded request; the emulator truncates and hands back a token (the next instance type's name used as an opaque cursor). Traces: esdef-list-instance-type-details-max-results-ignored#task#1, esval-list-instance-type-details-single-page#task#1.

Cause 4 of 8 - ListVersions ignores MaxResults and returns a NextToken (an engine version used as the cursor) where AWS returns the complete list and no token. Trace: esdef-list-versions-max-results-ignored#task#1.

Cause 5 of 8 - ListVersions refuses a follow-up page. AWS answers 200 with the next page of Versions when given the token it issued; the emulator answers 400 with no Versions, so pagination cannot be driven to completion. Trace: esdef-list-versions-max-results-ignored#task#2.

Cause 6 of 8 - DescribeReservedInstanceOfferings omits NextToken when more pages exist. AWS returns an opaque continuation token (an encoded cursor) on the first page of the offerings catalog; the emulator returns none, so a client can never read past page one. Trace: opensearch-reserved-instance-offering-describe-and-purchase-errors#task#1.

Cause 7 of 8 - DescribeInboundConnections invalid-token message. AWS: the fixed text "Pagination token provided is invalid", with no variable part. Emulator: 'Invalid pagination token: <the token from the request>'. Status 400 already matches. Trace: eserr-describe-inbound-connections-invalid-pagination-token#task#2.

Cause 8 of 8 - DescribeOutboundConnections invalid-token message, same defect as cause 7. Trace: eserr-describe-outbound-connections-invalid-pagination-token#task#2.

Why: these are the two failure modes that make a paginating client loop forever or stop early - an always-present NextToken (causes 1, 2, 3, 4) and a missing one (cause 6). Cause 5 makes the token the emulator does issue unusable. Causes 7 and 8 break any client or test that matches the documented refusal text, and leak the caller's token back into the message.

Fix: the service's pagination layer (omit NextToken instead of emitting "", honour MaxResults, issue and accept a usable cursor) and the invalid-token refusal text shared by the connection readers.

## Evidence
Cause 1/2: "$.NextToken: type_mismatch (expected None, got '')".
Cause 3: "$.NextToken: type_mismatch (expected None, got 'c4.4xlarge.search')" alongside the catalog mismatch.
Cause 4: "$.NextToken: type_mismatch (expected None, got 'Elasticsearch_2.3')".
Cause 5: "status: expected 200, got 400" and "$.Versions: missing".
Cause 6: "$.NextToken: missing (expected an opaque continuation token, got None)".
Cause 7/8: expected 'Pagination token provided is invalid'; actual 'Invalid pagination token: <token>'.

## Steps to Reproduce
1. Replay es-vpc-endpoint-errors step task#3 and opensearch-domain-lifecycle-workflow step task#3: NextToken must be absent, not "".
2. Replay esdef-list-instance-type-details-max-results-ignored step task#1 and esdef-list-versions-max-results-ignored steps task#1 then task#2: MaxResults must be honoured and the issued token must be accepted.
3. Replay opensearch-reserved-instance-offering-describe-and-purchase-errors step task#1 for the missing token, and the two eserr-describe-*-connections-invalid-pagination-token step task#2 fixtures for the refusal text.
