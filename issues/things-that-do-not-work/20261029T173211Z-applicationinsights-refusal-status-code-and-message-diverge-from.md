Issue-Source: trace-sweep-divergence
Issue-Trace: application-insights-application-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-insights-application-error-paths application-insights-component-workload-config-errors application-insights-list-applications-reads
Issue-Shape-Id: com.amazonaws.applicationinsights#EC2WindowsBarleyService
Issue-Key: applicationinsights|DescribeApplication|4xx->4xx|$.Message
Issue-Key: applicationinsights|DescribeApplication|4xx->4xx|status
Issue-Key: applicationinsights|ListComponents|4xx->4xx|$.Message
Issue-Key: applicationinsights|ListComponents|4xx->4xx|status
Issue-Key: applicationinsights|DescribeComponent|4xx->4xx|$.Message
Issue-Key: applicationinsights|DescribeComponent|4xx->4xx|status
Issue-Key: applicationinsights|CreateComponent|4xx->4xx|$.message
Issue-Key: applicationinsights|ListApplications|4xx->4xx|$.message
Issue-Commit: 04aee419340dca63f4b07449b9fd0b6ed9270cf9

# applicationinsights refusal status code and message diverge from AWS wording

> _Note: this issue was identified against the code base at commit `04aee419340dca63f4b07449b9fd0b6ed9270cf9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeApplication

## Description
This issue bundles 5 net-new causes that share ONE root fault: where the applicationinsights emulator DOES refuse, it serializes the refusal itself wrongly — it answers the Smithy-modeled @httpError (404) instead of the HTTP 400 Application Insights actually returns, and it substitutes emulator-authored prose for AWS's message templates (in one case emitting no message member at all). All five causes must be closed before this issue is submitted.

Cause 1 of 5: DescribeApplication for an application that does not exist — wrong status and wrong message.
AWS: Smithy: DescribeApplication errors include ResourceNotFoundException (members: Message; the shape carries httpError 404), but Application Insights answers this refusal with HTTP 400 and the body {"__type":"ResourceNotFoundException","Message":"Application does not exist"} — the recorded status is 400 and the message is wholly fixed text with no variable part (it does NOT name the resource group).
Emulator: answers HTTP 404 and $.Message 'no application exists for resource group <resourceGroupName>' — emulator prose that additionally interpolates the request's group name.
Why: a client that branches on the HTTP status (or on an AWS-documented 400-class refusal) misclassifies a missing application as a transport/route-level 404, and a client asserting on the message text never matches.
Fix: the applicationinsights error serializer plus DescribeApplication's handler — map ResourceNotFoundException to status 400 for this service and emit AWS's fixed message.
Traces: application-insights-application-error-paths#task#1.

Cause 2 of 5: ListComponents for an application that does not exist — same wrong status and message.
AWS: Smithy: ListComponents errors include ResourceNotFoundException. Recorded refusal: HTTP 400, {"__type":"ResourceNotFoundException","Message":"Application does not exist"} (fixed text, no variables).
Emulator: HTTP 404 with $.Message 'no application exists for resource group <resourceGroupName>'.
Why: as cause 1 — status and message are both requirements of the error class and both diverge.
Fix: same serializer/handler path, ListComponents' application lookup.
Traces: application-insights-component-workload-config-errors#task#1.

Cause 3 of 5: DescribeComponent when the application itself does not exist — same wrong status and message.
AWS: Smithy: DescribeComponent errors include ResourceNotFoundException. Recorded refusal: HTTP 400, {"__type":"ResourceNotFoundException","Message":"Application does not exist"}. Note this is the application-missing message; the component-missing case uses a different template (see the companion admission-check issue for applicationinsights).
Emulator: HTTP 404 with $.Message 'no application exists for resource group <resourceGroupName>'.
Why: as cause 1.
Fix: same serializer/handler path, DescribeComponent's application lookup.
Traces: application-insights-component-workload-config-errors#task#2.

Cause 4 of 5: CreateComponent constraint-violation message restates the constraint per-value instead of per-member.
AWS: Smithy: CreateComponent.ResourceList member targets ResourceARN with @length min 1 / max 1011 and @pattern ^arn:aws(-\w+)*:[\w\d-]+:([\w\d-]*)?:[\w\d_-]*([:/].+)*$; the refusal is HTTP 400 with lowercase $.message. AWS words it as a member-level constraint report naming the member path without its index and WITHOUT echoing the rejected value, listing the member's full constraint set: "1 validation error detected: Value at 'resourceList' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1011, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: <pattern>]". Fixed text: the count sentence, "Value at '", "' failed to satisfy constraint: Member must satisfy constraint: [" and the bracketed constraint list; the variable parts are the member name taken from the request shape and the limits/pattern taken from the model.
Emulator: HTTP 400 (status matches) but $.message is "1 validation error detected: Value '<rejected value>' at 'resourceList.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — it echoes the rejected value, indexes the member path, and reports only the one constraint that failed instead of the member's constraint set.
Why: clients and tests that parse or assert on AWS's validation sentence (and anything redacting values out of logs) see a different shape of message; the echoed request value is also a disclosure AWS does not make here.
Fix: the applicationinsights request-validation/constraint-message builder — render member-level constraint reports without the value and without the list index, enumerating the member's whole constraint set.
Traces: application-insights-component-workload-config-errors#task#3.

Cause 5 of 5: ListApplications with an undecodable NextToken returns 400 with no message member at all.
AWS: Smithy: ListApplications.NextToken is a PaginationToken, length 1..1024, pattern ^.+$; a syntactically acceptable but undecodable token is refused HTTP 400 with lowercase $.message "Pagination token exception in operation 'ListApplications': <decode failure>" — e.g. 'invalid payload wrapping schema <n>'. Fixed text: "Pagination token exception in operation '" + the operation name + "': " followed by the decoder's reason; the operation name is the invoked operation and the trailing detail is derived deterministically from the supplied token (it is not a per-run value such as a clock reading or request id).
Emulator: answers HTTP 400 (status matches) but the body carries NO message member — $.message is absent entirely.
Why: the client receives a refusal it cannot explain: SDK error objects surface an empty message, so a caller cannot tell a corrupt pagination token from any other 400 and resumption bugs are undiagnosable.
Fix: ListApplications' pagination-token decoding in the applicationinsights service crate — populate the message member when token decode fails.
Traces: application-insights-list-applications-reads#task#3.

## Evidence
Replay of simparity-wire-aws-v1, 5 clusters:

1. application-insights-application-error-paths#task#1 DescribeApplication — status: expected 400, got 404; $.Message value_mismatch (expected 'Application does not exist', got 'no application exists for resource group sim2real-probe-application-insights-app-missing-<suffix>').
2. application-insights-component-workload-config-errors#task#1 ListComponents — status: expected 400, got 404; $.Message value_mismatch (expected 'Application does not exist', got 'no application exists for resource group sim2real-probe-application-insights-comp-cfg-err-<suffix>').
3. application-insights-component-workload-config-errors#task#2 DescribeComponent — status: expected 400, got 404; $.Message value_mismatch, same expected/actual pair as above.
4. application-insights-component-workload-config-errors#task#3 CreateComponent — status 400 on both sides; $.message value_mismatch: expected "1 validation error detected: Value at 'resourceList' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1011, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^arn:aws(-\\w+)*:[\\w\\d-]+:([\\w\\d-]*)?:[\\w\\d_-]*([:/].+)*$]", got "1 validation error detected: Value 'not-an-arn' at 'resourceList.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws(-\\w+)*:[\\w\\d-]+:([\\w\\d-]*)?:[\\w\\d_-]*([:/].+)*$".
5. application-insights-list-applications-reads#task#3 ListApplications — status 400 on both sides; $.message missing (expected "Pagination token exception in operation 'ListApplications': invalid payload wrapping schema -24949", got None).

The generated resource-group names and the token-derived numeric detail quoted above are one run's values; the requirements are the status, the __type, and the message templates described in the issue body.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1).
2. Cause 1: fixture application-insights-application-error-paths, step task#1 — DescribeApplication for a resource group with no monitored application. Expect 400 + 'Application does not exist'; observe 404 + emulator prose.
3. Cause 2: fixture application-insights-component-workload-config-errors, step task#1 — ListComponents for a nonexistent application. Same divergence.
4. Cause 3: same fixture, step task#2 — DescribeComponent for a nonexistent application. Same divergence.
5. Cause 4: same fixture, step task#3 — CreateComponent with a resourceList entry that is not an ARN. Compare $.message against AWS's member-level constraint sentence.
6. Cause 5: fixture application-insights-list-applications-reads, step task#3 — ListApplications with an undecodable NextToken. Observe 400 with no message member.
