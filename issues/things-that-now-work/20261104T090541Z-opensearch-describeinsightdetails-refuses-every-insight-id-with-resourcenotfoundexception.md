Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.insight-lifecycle-and-feedback opensearch.insights-management
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|DescribeInsightDetails|ResourceNotFoundException
Issue-Commit: a669b06d661e5ead8877ca850e0fd26c45a4bbfe

# opensearch DescribeInsightDetails refuses every insight id with ResourceNotFoundException instead of answering 200 with its informational Fields list

> _Note: this issue was identified against the code base at commit `a669b06d661e5ead8877ca850e0fd26c45a4bbfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInsightDetails

## Description
Cause 1 of 1: DescribeInsightDetails is implemented as a strict resolve-an-existing-insight read, so any (Entity, InsightId) pair the emulator holds no stored insight under is refused as a missing resource. Because the emulator never raises any insight for a domain or for the account, that is EVERY well-formed request, and the operation can never return its modeled output at all.

Failing checks: opensearch.insight-lifecycle-and-feedback, opensearch.insights-management.

AWS: Smithy (services/service-com-amazonaws-opensearch-amazonopensearchservice/smithy-model/com-amazonaws-opensearch-amazonopensearchservice.json) models DescribeInsightDetails as POST /2021-01-01/opensearch/insight-details with success code 200 and output DescribeInsightDetailsResponse whose single member `Fields` (InsightFieldList) is **required**; input is `Entity` (InsightEntity, required), `InsightId` (GUID, required, @length 36..36, pattern ^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$) and `ShowHtmlContent` (Boolean). ResourceNotFoundException IS a modeled error of the operation (errorKind client, httpError 409), so the divergence is not that the error shape is unknown but that AWS does not take this path for a syntactically valid id. Recorded behavior only for the not-found boundary: the recordings the two checks were cut from show that DescribeInsightDetails is an informational read — a well-formed InsightId the account holds no insight under is answered 200 with a Fields list rather than refused, and that list carries at least one generic informational field (each element having `Name` and `Value`) directing the caller to the console. An InsightId that is NOT a GUID is still a ValidationException on the member constraint; the insight-id-does-not-exist case is simply not a not-found boundary on this operation.

Emulator: both checks receive, from botocore, `ResourceNotFoundException` with Error.Code `ResourceNotFoundException` and Error.Message `Resource not found` (per the model's classification, HTTP 409) for a request whose InsightId is a well-formed GUID. In `opensearch.insights-management` the domain was created and waited to Active first, and the entity is {Type: "DomainName", Value: <that domain>}; in `opensearch.insight-lifecycle-and-feedback` the entity is {Type: "Account", Value: <caller account id>} and ListInsights had already answered with an empty Insights list. In the message, `Resource not found` is fixed text with no variable part — it names neither half of the address — so the whole string is the emulator's own refusal text standing in for a 200 body. No DescribeInsightDetails call can succeed, since nothing in the emulator ever stores an insight to resolve.

Why: a client that reads insight detail for an entity treats this operation as always-answerable and reads `Fields` off a 200. Against the emulator every such read raises ResourceNotFoundException, which a client interprets as "this entity/insight does not exist" — a retry-never, fail-the-flow signal. Dashboards and diagnostic tooling that render insight detail therefore report the domain or the account as missing rather than rendering the informational field, and a client's not-found error path runs on a call AWS answers 200. The refusal also masks the two downstream rules the same checks go on to assert and never reach: the InsightFeedback refusal of an `Account` entity (ValidationException, `1 validation error detected: Value 'Account' at 'entity.type' failed to satisfy constraint: Member must satisfy enum value set: [DomainName]`) and the ListInsights refusal of an out-of-enum Entity.Type (ValidationException, `1 validation error detected: Value '<supplied type>' at 'entity.type' failed to satisfy constraint: Member must satisfy enum value set: [Account, DomainName]`, where the quoted value is taken from the request) — those two sentences are fixed text apart from the supplied Entity.Type value, and a fixer should expect them to come into play once the 200 path exists.

Fix: the DescribeInsightDetails handler, and the insight state it reads. The handler must stop treating an unresolved (entity, insight-id) pair as a refusal for a well-formed GUID: entity resolution and the GUID pattern check stay as refusals, but a valid id with no stored insight is an informational 200 carrying the required Fields list with at least one Name/Value field, not `crate::state::insight_resource_not_found()`. The state component must be able to supply that generic informational field for an entity it holds no raised insight under, so `Fields` is never empty for a resolvable entity.

Traces: opensearch.insight-lifecycle-and-feedback (Account entity, bogus GUID 0000...-0000, after ListInsights returned empty); opensearch.insights-management (DomainName entity on a freshly Active domain, same bogus GUID). Handler: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/describe_insight_details.rs lines 205-220 (the `address_not_found` closure and the `.ok_or_else(address_not_found)?` on the resolved insight) and lines 229-239 (where Fields is built only from `insight.details`).

## Evidence
Expected (AWS, from the recordings the checks were cut from; Smithy makes Fields required on a 200 response with success code 200):
  DescribeInsightDetails(Entity={Type: "DomainName"|"Account", Value: <domain|account id>}, InsightId=<well-formed GUID>) -> HTTP 200, body {"Fields": [ {"Name": <field name>, "Value": <field value>}, ... ]} — at least one generic informational field for an id the account holds no insight under.

Actual (emulator):
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeInsightDetails operation: Resource not found
  Error.Code = "ResourceNotFoundException"; Error.Message = "Resource not found" (entirely fixed text, no request- or state-derived part); HTTP 409 per the model's classification of that error shape.

Both failing checks abort at their first describe_insight_details call:
  opensearch.insight-lifecycle-and-feedback — after list_insights(Entity={Type:"Account",Value:<account>}) returned an empty Insights list, describe_insight_details(Entity=<same>, InsightId="00000000-0000-0000-0000-000000000000") raised ResourceNotFoundException where the check asserts `"Fields" in details and isinstance(details["Fields"], list)`.
  opensearch.insights-management — on a domain created and polled to Processing=False/Created=True, list_insights(Entity={Type:"DomainName",Value:<domain>}) returned a list, then describe_insight_details(Entity=<same>, InsightId="00000000-0000-0000-0000-000000000000") raised ResourceNotFoundException where the check asserts a non-empty Fields whose every element has Name and Value.

The GUID used is well-formed (36 chars, 8-4-4-4-12 hex), so the handler's own ValidationException path for a malformed InsightId is not what fired — the refusal comes from the unresolved-address path.

## Steps to Reproduce
1. Boot the emulator and point an opensearch client at it.
2. Account entity: acct = sts.get_caller_identity()["Account"]; entity = {"Type": "Account", "Value": acct}. Call opensearch.list_insights(Entity=entity) — succeeds, Insights is an empty list.
3. Call opensearch.describe_insight_details(Entity=entity, InsightId="00000000-0000-0000-0000-000000000000").
   Observed: ResourceNotFoundException / "Resource not found".
   Expected: HTTP 200 with a Fields list carrying at least one {Name, Value} informational field.
4. DomainName entity: create_domain(DomainName=<3-28 char name>, EngineVersion="OpenSearch_2.11", ClusterConfig={InstanceType:"t3.small.search", InstanceCount:1, DedicatedMasterEnabled:false, ZoneAwarenessEnabled:false}, EBSOptions={EBSEnabled:true, VolumeType:"gp2", VolumeSize:10}); poll describe_domain until Processing=False and Created=True.
5. Call opensearch.describe_insight_details(Entity={"Type":"DomainName","Value":<domain>}, InsightId="00000000-0000-0000-0000-000000000000") — same ResourceNotFoundException instead of a 200 with Fields.
6. Re-replay the two behavioral checks opensearch.insight-lifecycle-and-feedback and opensearch.insights-management to confirm the fix; both then proceed to their InsightFeedback / ListInsights entity.type enum assertions.
