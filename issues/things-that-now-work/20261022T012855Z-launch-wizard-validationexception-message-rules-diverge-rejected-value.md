Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: launch-wizard.get-workload-invalid-name-pattern-rejected launch-wizard.list-deployments-max-results-out-of-range-rejected launch-wizard.list-deployments-filter-by-invalid-status-rejected launch-wizard.list-deployments-filter-by-workload-name-no-match-returns-empty launch-wizard.representation-list-workloads-nexttoken-opaque-round-trip
Issue-Shape-Id: com.amazonaws.launchwizard#LaunchWizard
Issue-Key: launch-wizard.get-workload-invalid-name-pattern-rejected|run#4|AssertionError|assert message == (
Issue-Key: launch-wizard.list-deployments-max-results-out-of-range-rejected|run#4|AssertionError|assert message == (
Issue-Key: launch-wizard.list-deployments-filter-by-invalid-status-rejected|run#4|AssertionError|assert message == (
Issue-Key: launch-wizard.list-deployments-filter-by-workload-name-no-match-returns-empty|run#5|AssertionError|assert message == (
Issue-Key: launch-wizard.representation-list-workloads-nexttoken-opaque-round-trip|run#16|AssertionError|assert message == "NextToken is not valid.", f"got message={message!r}"
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# Launch Wizard ValidationException message rules diverge: rejected value echoed, pattern anchors leaked, list-member path and composed constraint list wrong, nextToken message invented

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDeployments, GetWorkload, ListWorkloads

## Description
The emulator returns the right error code (ValidationException, HTTP 400) for every constraint refusal below, but builds the message by a different rule than AWS does. These messages are built from the request (they name the member and the constraint it violated), so the rule that produces them is a requirement, not an incidental string. 3 causes, all in launch-wizard.

Cause 1 of 3: scalar member constraint messages echo the rejected value and the anchored pattern.
AWS: for a scalar member that violates a modeled constraint, the message is "1 validation error detected: Value at '<memberPath>' failed to satisfy constraint: Member must <constraint>" — the offending value is NOT rendered, and a @pattern is rendered WITHOUT its ^...$ anchors. Smithy: GetWorkloadInput.workloadName -> WorkloadName @length 1..100, @pattern ^[A-Za-z][a-zA-Z0-9-_]*$; ListDeploymentsInput.maxResults -> MaxDeploymentResults @range min 1 max 100; error ValidationException (errorKind client, httpError 400).
Emulator: renders the value and the anchored pattern — "1 validation error detected: Value '1InvalidLeadingDigit' at 'workloadName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z][a-zA-Z0-9-_]*$" (AWS: "...Value at 'workloadName' ... pattern: [A-Za-z][a-zA-Z0-9-_]*"), and "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100" (AWS: "...Value at 'maxResults' ...").
Why: a client (or a test fixture, or a log scrubber) that parses AWS's constraint-violation message format mis-parses these; echoing the rejected value back also leaks caller-supplied input into the error channel for members AWS deliberately does not render.
Fix: the shared request-validation/message-rendering layer that formats modeled @pattern/@range/@length refusals for this service (the pre-handler validation walk, not the operation handlers — these refusals happen before `handle` runs).
Traces: launch-wizard.get-workload-invalid-name-pattern-rejected, launch-wizard.list-deployments-max-results-out-of-range-rejected.

Cause 2 of 3: constraint violations inside a list member report the per-element path and only one of the composed constraints.
AWS: when a member of a constrained list violates its constraints, the message reports the COLLECTION path and the full composed constraint list: "1 validation error detected: Value at 'filters.1.member.values' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 100, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: .*(^(([A-Za-z])+([0-9A-Za-z\\-\\_])+)$).*]". Smithy: DeploymentFilter.values -> DeploymentFilterValues, member DeploymentFilterValue @length 1..100, @pattern (^(([A-Za-z])+([0-9A-Za-z\\-\\_])+)$).
Emulator: reports the element path and the bare pattern, with the value echoed — "1 validation error detected: Value 'not a valid status!!' at 'filters.1.member.values.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: (^(([A-Za-z])+([0-9A-Za-z\\-\\_])+)$)" (same shape for a WORKLOAD_NAME filter value).
Why: the member path a client reads out of the message to point at the offending input field names a path AWS never emits, and the composed-constraint rendering AWS uses for collection members is missing entirely.
Fix: same validation/message-rendering layer — specifically the path construction and constraint composition for members reached through a list.
Traces: launch-wizard.list-deployments-filter-by-invalid-status-rejected, launch-wizard.list-deployments-filter-by-workload-name-no-match-returns-empty.

Cause 3 of 3: an unusable nextToken is refused with an invented message instead of AWS's fixed pagination message.
AWS: ListWorkloads called with a tampered/unrecognized nextToken fails with ValidationException and the fixed message "NextToken is not valid." (recorded behavior; the model only declares ValidationException for the operation and NextToken @length 1..2048).
Emulator: returns ValidationException with "nextToken '76313a4163746976654469726563746f727A' does not name a position in these results." — a per-request prose message that also echoes the token.
Why: the token is documented as opaque; clients and retry wrappers that key off AWS's fixed pagination-failure text see an unrecognized message, and the rejected token is echoed back.
Fix: the shared pagination helper used by the list operations (`crate::state::paginate`, whose Err(String) is lifted into ValidationException by each list handler).
Traces: launch-wizard.representation-list-workloads-nexttoken-opaque-round-trip.

## Evidence
launch-wizard.get-workload-invalid-name-pattern-rejected — expected message "1 validation error detected: Value at 'workloadName' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z][a-zA-Z0-9-_]*"; got "1 validation error detected: Value '1InvalidLeadingDigit' at 'workloadName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z][a-zA-Z0-9-_]*$". Code ValidationException matched.
launch-wizard.list-deployments-max-results-out-of-range-rejected — expected "...Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"; got "...Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
launch-wizard.list-deployments-filter-by-invalid-status-rejected and launch-wizard.list-deployments-filter-by-workload-name-no-match-returns-empty — expected "1 validation error detected: Value at 'filters.1.member.values' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 100, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: .*(^(([A-Za-z])+([0-9A-Za-z\\-\\_])+)$).*]"; got "1 validation error detected: Value 'not a valid status!!' at 'filters.1.member.values.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: (^(([A-Za-z])+([0-9A-Za-z\\-\\_])+)$)" (and the same with 'not a valid name!!').
launch-wizard.representation-list-workloads-nexttoken-opaque-round-trip — expected message "NextToken is not valid."; got "nextToken '76313a4163746976654469726563746f727A' does not name a position in these results."
Smithy model consulted: services/service-com-amazonaws-launchwizard-launchwizard/smithy-model/com-amazonaws-launchwizard-launchwizard.json (GetWorkloadInput.workloadName, ListDeploymentsInput.maxResults, DeploymentFilterValues/DeploymentFilterValue, ListWorkloadsInput.nextToken, ValidationException).

## Steps to Reproduce
1. Boot the emulator and point a botocore launch-wizard client at it.
2. Cause 1: lw.get_workload(workloadName="1InvalidLeadingDigit") and lw.list_deployments(maxResults=101); read Error.Message.
3. Cause 2: lw.list_deployments(filters=[{"name": "DEPLOYMENT_STATUS", "values": ["not a valid status!!"]}], maxResults=10) and the same with name="WORKLOAD_NAME", values=["not a valid name!!"].
4. Cause 3: page1 = lw.list_workloads(maxResults=1); flip the last character of page1["nextToken"] and call lw.list_workloads(maxResults=1, nextToken=<tampered>).
5. Each call returns ValidationException with a message built by a different rule than AWS's.
