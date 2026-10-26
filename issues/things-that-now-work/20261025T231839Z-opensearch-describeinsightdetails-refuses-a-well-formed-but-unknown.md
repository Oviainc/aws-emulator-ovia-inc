Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.insight-lifecycle-and-feedback opensearch.insights-management
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|DescribeInsightDetails|ResourceNotFoundException
Issue-Commit: 80234be7b8657963e8b406738ba2bc75bea8323e

# opensearch: DescribeInsightDetails refuses a well-formed but unknown InsightId with ResourceNotFoundException instead of answering 200 with the informational Fields list (1 cause)

> _Note: this issue was identified against the code base at commit `80234be7b8657963e8b406738ba2bc75bea8323e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInsightDetails

## Description
Cause 1 of 1: DescribeInsightDetails treats an InsightId the store holds no insight under as a not-found resource and refuses the read, so the entire Insights read path is unreachable for any caller that has not been handed an id the emulator itself minted. Failing checks: opensearch.insight-lifecycle-and-feedback, opensearch.insights-management.

AWS: Smithy (services/service-com-amazonaws-opensearch-amazonopensearchservice/smithy-model/com-amazonaws-opensearch-amazonopensearchservice.json) models DescribeInsightDetails as POST /2021-01-01/opensearch/insight-details with success status 200 and output member Fields (InsightFieldList) @required; input InsightId is a GUID with @length min=36 max=36 and pattern ^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$. The model does declare ResourceNotFoundException (HTTP 409, documented "An exception for accessing or deleting a resource that doesn't exist"), but it does not say an unrecognized insight id is one. Recorded behavior only, and it is the same on both entity types exercised: for ANY syntactically valid InsightId real AWS answers 200 with a non-empty Fields list of informational name/value fields (each entry carrying Name and Value, directing the caller to the console) rather than rejecting the call. That holds for an Entity of {Type: Account, Value: <caller account>} and for {Type: DomainName, Value: <an existing domain>}; the rejections this surface really has are keyed on the ENTITY, not the id (an Entity.Type outside the enum is a ValidationException, and InsightFeedback against a well-formed DomainName that does not exist is a ResourceNotFoundException).

Emulator: the handler resolves the (Entity, InsightId) pair through state::insight_of_entity and maps InsightEntityRefusal::NotFound to ResourceNotFoundException, so a GUID-shaped id with no stored insight is refused. Replay shows, for the DomainName entity against a domain the check had just created and waited Active on: ResourceNotFoundException with message "Insight 00000000-0000-0000-0000-000000000000 not found for domain sim2real-behav-os-c2116f306b", and for the Account entity: ResourceNotFoundException with message "Insight 00000000-0000-0000-0000-000000000000 not found for account 000000000000". In those messages the fixed text is "Insight " ... " not found for domain " / " not found for account " and the variable parts are the request's InsightId and the request's Entity.Value (domain name) or the caller's account id from state — AWS emits no message at all for this call, because it does not refuse it. Expected instead: HTTP 200 with Fields populated.

Why: a client cannot read insight details at all. Because the emulator never raises insights on its own, ListInsights answers an empty list, so there is no id a caller can obtain that this call will accept — every DescribeInsightDetails on this emulator is a 409. Code that reads a domain's insights (or merely probes the surface) sees a hard not-found where AWS returns a well-formed 200, and anything downstream of that read — the InsightFeedback entity-scoping contrast and the ListInsights Entity.Type enum refusal — is never reached, so those behaviors are untested behind this one refusal. No open issue covers the OpenSearch insights surface (the existing *Insight* issues are x-ray, ec2, cloudwatch, dynamodb, eks and devops-guru).

Fix: the DescribeInsightDetails handler and its backing state lookup. An id that resolves to no stored insight must not be a refusal for this operation: answer 200 with the informational Fields list AWS returns for any valid GUID. Keep the Entity-based refusals (InsightEntityRefusal::Validation for an Entity.Type outside the enum) intact; only the NotFound-on-unknown-id branch is wrong here.

Traces: opensearch.insight-lifecycle-and-feedback (Account entity, empty-ListInsights branch, fails at the first describe_insight_details), opensearch.insights-management (DomainName entity on a freshly Active domain, fails at describe_insight_details before it can grade the ListInsights Entity.Type enum refusal). File and line citations: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/describe_insight_details.rs lines 183-192 (the insight_of_entity call and its NotFound -> ResourceNotFoundException mapping) and the doc comment at lines 9-10 that states the intended refusal.

## Evidence
Expected (real AWS, recorded): DescribeInsightDetails(Entity={Type:DomainName,Value:<existing domain>}, InsightId="00000000-0000-0000-0000-000000000000") -> HTTP 200, {"Fields": [{"Name": ..., "Value": ...}, ...]} (at least one informational field); same 200 for Entity={Type:Account,Value:<caller account>}.

Actual (emulator):
- opensearch.insights-management: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeInsightDetails operation: Insight 00000000-0000-0000-0000-000000000000 not found for domain sim2real-behav-os-c2116f306b
- opensearch.insight-lifecycle-and-feedback: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeInsightDetails operation: Insight 00000000-0000-0000-0000-000000000000 not found for account 000000000000

Smithy grounding: DescribeInsightDetails success status 200; output Fields @required (InsightFieldList); InsightId GUID @length 36..36 + GUID pattern; ResourceNotFoundException is modeled at HTTP 409 but only for a resource that does not exist — the recording shows an unknown insight id is not such a case.

Handler grounding: services/.../src/ops/describe_insight_details.rs:183-192 maps InsightEntityRefusal::NotFound to DescribeInsightDetailsErrorModeled::ResourceNotFoundException.

## Steps to Reproduce
1. Boot the emulator and point an opensearch client at it.
2. DomainName entity: CreateDomain a t3.small.search single-node domain with EngineVersion OpenSearch_2.11 and wait for Created=true/Processing=false. Call ListInsights(Entity={Type:"DomainName",Value:<that domain>}) — it answers an empty Insights list, so no real id is obtainable. Then call DescribeInsightDetails(Entity={Type:"DomainName",Value:<that domain>}, InsightId="00000000-0000-0000-0000-000000000000"). Observe 409 ResourceNotFoundException "Insight <id> not found for domain <domain>" instead of 200 with a non-empty Fields list.
3. Account entity: call DescribeInsightDetails(Entity={Type:"Account",Value:<sts GetCallerIdentity Account>}, InsightId="00000000-0000-0000-0000-000000000000"). Observe 409 ResourceNotFoundException "Insight <id> not found for account <account>" instead of 200 with a non-empty Fields list.
4. Replay opensearch.insights-management and opensearch.insight-lifecycle-and-feedback to confirm; both then proceed to grade the ListInsights Entity.Type enum refusal and the InsightFeedback entity-scoping contrast respectively.
