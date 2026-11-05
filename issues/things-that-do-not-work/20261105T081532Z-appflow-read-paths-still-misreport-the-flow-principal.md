Issue-Source: trace-sweep-divergence
Issue-Trace: appflow-describe-flow-reports-task-and-source-config-fidelity
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appflow-describe-flow-reports-task-and-source-config-fidelity appflow-list-flows-reflects-flow-lifecycle
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: appflow|DescribeFlow|2xx->2xx|$.createdBy
Issue-Key: appflow|DescribeFlow|2xx->2xx|$.lastUpdatedBy
Issue-Key: appflow|ListFlows|2xx->2xx|$.flows[*]
Issue-Commit: 56ff870e65d226f4952f88c474cf52081981448e

# AppFlow read paths still misreport the flow principal and omit the flow from ListFlows

> _Note: this issue was identified against the code base at commit `56ff870e65d226f4952f88c474cf52081981448e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFlow

## Description
AppFlow's two read paths answer 200 but still do not report the flow the write path created: DescribeFlow fills createdBy/lastUpdatedBy with a synthesized principal instead of the calling identity, and ListFlows returns an empty list for an account that holds a flow. The shared fault is that the flow record held in the emulator's appflow state is not projected faithfully into the read responses. 2 causes. (The now-resolved issue 20261029T192701Z-appflow-describeflow-and-listflows-omit-flow-state-fidelity.md reported a superset of these two keys; it is closed, and both of these replay steps still diverge, so this is the residual, unfixed part of that surface. Also related but NOT covering them: open issues 20261029T194452Z-appflow-flow-lifecycle-wrong-initial-flowstatus-executions-always.md (behavioral flowStatus/execution semantics) and 20261029T192701Z-appflow-error-message-text-diverges-from-aws-wording.md (error wording).)

Cause 1 of 2: DescribeFlow reports a synthesized principal name in createdBy/lastUpdatedBy instead of the caller's IAM principal.
AWS: Smithy: DescribeFlow output member createdBy targets CreatedBy (string, @length 0..256, @pattern ^\\S+$, doc "The ARN of the user who created the flow") and lastUpdatedBy targets UpdatedBy (same constraints, doc "Specifies the account user name that most recently updated the flow"); the same two members exist on FlowDefinition. Recorded behavior for the value: both members carry the ARN of the IAM principal that made the request, in the form arn:aws:iam::<account>:user/<principal-name>, where <account> is the account the request authenticated into and <principal-name> is the caller's own user name - i.e. the same principal STS GetCallerIdentity resolves for those credentials. Both members are the identical principal ARN for a flow that has only ever been created (never updated by another identity).
Emulator: 200 and the ARN form, account and path (arn:aws:iam::<account>:user/...) are right, but the user-name segment is a value the emulator invents per flow/session - a fixed "EMULATOR" prefix followed by an uppercase hex suffix - rather than the requesting principal's name. Only that last segment diverges, on both createdBy and lastUpdatedBy of the same response.
Why: createdBy/lastUpdatedBy are the ownership and audit fields of a flow. A client that compares them against its own identity (from STS GetCallerIdentity or its configured principal) to decide whether it owns a flow, that filters a flow list by creator, or that records provenance, never gets a match, because the emulator hands back a principal that no credential in the account resolves to. The ARN being well-formed makes this silent: parsing succeeds and the comparison simply always fails.
Fix: the appflow DescribeFlow handler / the principal-resolution helper that fills createdBy and lastUpdatedBy when a flow record is written and projected - it must derive the principal ARN from the request's caller identity (the same resolution the emulator's STS surface uses) instead of generating a synthetic user name, and ListFlows' FlowDefinition must project that same value.
Traces: appflow-describe-flow-reports-task-and-source-config-fidelity#task#7 (DescribeFlow, 200 vs 200, keys $.createdBy and $.lastUpdatedBy).

Cause 2 of 2: ListFlows does not enumerate a flow that exists in the account.
AWS: Smithy: ListFlows (POST /list-flows) output member flows targets FlowList, a list of FlowDefinition{flowArn, flowName, flowStatus, sourceConnectorType, sourceConnectorLabel, destinationConnectorType, destinationConnectorLabel, triggerType, createdAt, lastUpdatedAt, createdBy, lastUpdatedBy, tags, lastRunExecutionDetails, description}; the operation is @paginated on nextToken/maxResults (maxResults @range 1..100). Recorded behavior: every flow in the account that has not been deleted is enumerated, so a flow created earlier in the same session appears as a FlowDefinition entry carrying its own flowName, its source and destination connector types, its triggerType and its current flowStatus (the recording's first entry is the created flow with sourceConnectorType S3, destinationConnectorType S3, triggerType OnDemand, flowStatus Active).
Emulator: 200 with no entry at index 0 at all - the flows list comes back empty/absent even though the preceding CreateFlow in the same fixture succeeded and DescribeFlow on that flow answers.
Why: list-then-act clients cannot discover a flow they just created. Existence checks implemented over ListFlows report the flow as missing, so idempotent create paths re-create it or fail on the duplicate-name conflict, pagination over the account yields nothing, and console-style enumeration shows an empty account while DescribeFlow proves the flow exists. The emulator's state and its enumeration disagree.
Fix: the appflow ListFlows handler and the shared appflow flow store it reads - ListFlows must project the same flow records CreateFlow wrote into FlowDefinition entries (including flowStatus and the connector/trigger types) and page them with nextToken/maxResults.
Traces: appflow-list-flows-reflects-flow-lifecycle#task#7 (ListFlows, 200 vs 200, key $.flows[0]).

## Evidence
Cause 1 - fixture appflow-describe-flow-reports-task-and-source-config-fidelity, step task#7, operation DescribeFlow: expected status 200, actual 200; body diverges on two keys. $.createdBy: expected the caller's principal ARN arn:aws:iam::<account>:user/<principal-name> (recorded as arn:aws:iam::000000000000:user/emulator for the emulator's own credentials), got arn:aws:iam::000000000000:user/EMULATOR7597A22875C8. $.lastUpdatedBy: same expected value, same actual value. Fixed part of the value = the ARN template arn:aws:iam::<account>:user/<name>; variable parts = the account id the request authenticated into and the caller principal's name - the recorded 'emulator' and the emulator's 'EMULATOR7597A22875C8' are both instances of that name slot, and the divergence is that the emulator synthesizes it instead of taking the caller's. Model basis: DescribeFlow output createdBy: CreatedBy (@length 0..256, @pattern ^\\S+$, "The ARN of the user who created the flow"), lastUpdatedBy: UpdatedBy.

Cause 2 - fixture appflow-list-flows-reflects-flow-lifecycle, step task#7, operation ListFlows: expected status 200, actual 200; $.flows[0] expected a FlowDefinition for the flow created earlier in the fixture ({flowName: <the created flow's name>, sourceConnectorType: 'S3', destinationConnectorType: 'S3', triggerType: 'OnDemand', flowStatus: 'Active'}), got None - there is no element 0, i.e. the list is empty. Model basis: ListFlows output flows: FlowList of FlowDefinition; @paginated(inputToken: nextToken, outputToken: nextToken, pageSize: maxResults).

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus); both steps below are reported as mismatches with expected status 200 and actual status 200.
2. Cause 1: POST /create-flow for an S3-to-S3 flow with a complete sourceFlowConfig/destinationFlowConfigList, then POST /describe-flow with that flowName. Observe createdBy and lastUpdatedBy = arn:aws:iam::<account>:user/EMULATOR<hex> instead of the ARN of the principal whose credentials signed the request (compare with STS GetCallerIdentity against the same credentials). Fixture appflow-describe-flow-reports-task-and-source-config-fidelity, step task#7.
3. Cause 2: after the same successful CreateFlow (and with DescribeFlow on it answering 200), POST /list-flows with no nextToken. Observe {"flows": []} - no FlowDefinition entry for the flow just created, where AWS enumerates it with its flowName, sourceConnectorType S3, destinationConnectorType S3, triggerType OnDemand and its current flowStatus. Fixture appflow-list-flows-reflects-flow-lifecycle, step task#7.
