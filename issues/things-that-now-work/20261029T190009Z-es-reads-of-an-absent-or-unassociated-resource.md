Issue-Source: trace-sweep-divergence
Issue-Trace: es-domain-read-not-found-errors
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-domain-read-not-found-errors es-package-management-errors es-vpc-endpoint-errors opensearch-list-insights-domain-scoped-sorted-with-time-range esdef-application-dead-id-probe
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|ListPackagesForDomain|2xx->4xx|$.DomainPackageDetailsList
Issue-Key: es|ListPackagesForDomain|2xx->4xx|status
Issue-Key: es|ListDomainsForPackage|2xx->4xx|$.DomainPackageDetailsList
Issue-Key: es|ListDomainsForPackage|2xx->4xx|status
Issue-Key: es|ListVpcEndpointAccess|2xx->4xx|$.AuthorizedPrincipalList
Issue-Key: es|ListVpcEndpointAccess|2xx->4xx|status
Issue-Key: es|ListInsights|2xx->4xx|$.Insights
Issue-Key: es|ListInsights|2xx->4xx|status
Issue-Key: es|GetApplication|2xx->2xx|$.appConfigs
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# es: reads of an absent or unassociated resource error instead of answering an empty list

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPackagesForDomain

## Description
5 net-new root causes: collection-returning reads that AWS answers 200 with an empty collection are answered with an error, or omit the empty collection from an otherwise-correct 200.

Cause 1 of 5 - ListPackagesForDomain. AWS: 200 with DomainPackageDetailsList: [] even when the named domain has no packages (and in the recorded case does not exist). Emulator: 409 and no body. Trace: es-domain-read-not-found-errors#task#11.

Cause 2 of 5 - ListDomainsForPackage. AWS: 200 with DomainPackageDetailsList: []. Emulator: 400 and no body. Trace: es-package-management-errors#task#6.

Cause 3 of 5 - ListVpcEndpointAccess. AWS: 200 with AuthorizedPrincipalList: []. Emulator: 409. Trace: es-vpc-endpoint-errors#task#4.

Cause 4 of 5 - ListInsights. AWS: 200 with Insights: [] for a domain-scoped, sorted, time-ranged query that matches nothing. Emulator: 409. Trace: opensearch-list-insights-domain-scoped-sorted-with-time-range#task#1.

Cause 5 of 5 - GetApplication omits an empty appConfigs. AWS returns appConfigs: [] for an application with no app configs (Smithy: GetApplication output appConfigs targets AppConfigs, a list with @length 0..200). The emulator omits the member entirely. Trace: esdef-application-dead-id-probe#task#2.

Why: "empty" and "error" are different outcomes to a caller. A client that iterates the list has to special-case an exception per call site instead of looping zero times; a client that treats any 4xx as fatal aborts a perfectly normal "nothing here" read. Cause 5 forces every reader of appConfigs to null-check a member the model says is always present.

Fix: the read handlers for these four list operations (resolve-or-empty rather than resolve-or-error) and GetApplication's response serializer, which must emit the empty list rather than omitting the member.

## Evidence
Cause 1: "status: expected 200, got 409" and "$.DomainPackageDetailsList: missing (expected [], got None)".
Cause 2: "status: expected 200, got 400" and "$.DomainPackageDetailsList: missing (expected [], got None)".
Cause 3: "status: expected 200, got 409" and "$.AuthorizedPrincipalList: missing (expected [], got None)".
Cause 4: "status: expected 200, got 409" and "$.Insights: missing (expected [], got None)".
Cause 5: "$.appConfigs: missing (expected [], got None)" on an otherwise-matching 200.

## Steps to Reproduce
1. Replay es-domain-read-not-found-errors step task#11 (ListPackagesForDomain) - expect 200 with DomainPackageDetailsList: [], observe 409.
2. Replay es-package-management-errors step task#6, es-vpc-endpoint-errors step task#4 and opensearch-list-insights-domain-scoped-sorted-with-time-range step task#1 for the other three.
3. Replay esdef-application-dead-id-probe step task#2 and check that appConfigs is present as [].
