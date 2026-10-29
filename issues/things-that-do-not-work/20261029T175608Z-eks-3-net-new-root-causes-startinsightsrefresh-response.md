Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.start-insights-refresh-already-in-progress-blocked eks.insights-refresh-cooldown-throttle eks.insights-refresh-completes-autonomously eks.insights-refresh-status-lifecycle eks.cancel-update-completed-state-rejected eks.certificate-authority-active-signer-deletion-rejected eks.certificate-authority-delete-in-use-blocked eks.certificate-authority-full-lifecycle-transitions eks.cluster-tags-empty-vs-absent-preserved eks.create-capability-unsupported-type-rejected
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.start-insights-refresh-already-in-progress-blocked|run#10|AssertionError|assert started["message"] == "Initiated cluster insights refresh.", f"StartInsightsRefresh response -> {started!r}"
Issue-Key: eks.insights-refresh-cooldown-throttle|run#12|AssertionError|assert (err.get("Message") or "").startswith(
Issue-Key: eks.insights-refresh-completes-autonomously|run#13|Timeout|time.sleep(min(15.0, max(0.0, deadline - time.monotonic())))
Issue-Key: eks.insights-refresh-status-lifecycle|run#13|Timeout|time.sleep(10)
Issue-Key: eks.cancel-update-completed-state-rejected|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: eks.certificate-authority-active-signer-deletion-rejected|_wait_update_successful#19|Timeout|time.sleep(interval_s)
Issue-Key: eks.certificate-authority-delete-in-use-blocked|_poll_update_terminal#28|Timeout|time.sleep(15.0)
Issue-Key: eks.certificate-authority-full-lifecycle-transitions|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: eks.cluster-tags-empty-vs-absent-preserved|_wait_for#3|Timeout|time.sleep(interval_s)
Issue-Key: eks.create-capability-unsupported-type-rejected|_create_capability_with_retry#5|Timeout|time.sleep(5)
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# eks: 3 net-new root causes — StartInsightsRefresh response wording, the in-progress refusal sentence, and asynchronous EKS state that never settles within a check's budget

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartInsightsRefresh

## Description
Three net-new root causes in the eks emulator (crate services/service-com-amazonaws-eks-awswesleyfrontend) around cluster insights refresh and asynchronous state settlement. All three must be closed.

Cause 1 of 3 — StartInsightsRefresh returns the wrong `message` when it STARTS a refresh.
AWS: Smithy: StartInsightsRefresh returns {message: String, status: InsightsRefreshStatus}. Recorded behavior: the call that starts a refresh answers status IN_PROGRESS with the fixed sentence "Initiated cluster insights refresh." — all fixed text, no variable parts; the distinct 'already running' outcome is a 400 refusal (cause 2), never a 200 with a different message.
Emulator: answers HTTP 200 with {'message': 'The cluster insights refresh is in progress.', 'status': 'IN_PROGRESS'} — the start acknowledgement is reported with the in-progress sentence.
Why: a client cannot distinguish 'I started it' from 'one was already running' off the response message, which is the only textual signal the operation returns.
Fix: the StartInsightsRefresh handler's success response (message selection).
Traces: eks.start-insights-refresh-already-in-progress-blocked.

Cause 2 of 3 — the refusal of a second StartInsightsRefresh while one is in flight uses the wrong sentence.
AWS: Smithy: StartInsightsRefresh models InvalidRequestException (httpError 400). Recorded behavior: a second StartInsightsRefresh fired while the cluster's refresh is IN_PROGRESS is refused 400 InvalidRequestException and the message BEGINS with the fixed sentence "Another refresh operation is already in progress for the cluster." (AWS appends a per-request "Request ID: <uuid>" suffix, which is a per-run value and not part of the requirement), and the in-flight refresh's startedAt is unchanged.
Emulator: code InvalidRequestException and HTTP 400 are correct, but the message is 'An insights refresh is already in progress for cluster <clusterName>; wait for it to complete before starting another.' — a different sentence that also injects the cluster name into what AWS keeps constant.
Why: the fixed sentence is the contract clients and tests match on; the emulator's wording never appears in real EKS.
Fix: the in-progress guard message in the StartInsightsRefresh handler.
Traces: eks.insights-refresh-cooldown-throttle.

Cause 3 of 3 — asynchronous EKS state the API exposes never settles, so every check that polls it burns its whole budget without the transition ever being observed.
AWS: recorded behavior: these state machines settle on their own and are observable while they do — an insights refresh started with StartInsightsRefresh reaches a terminal COMPLETED status through DescribeInsightsRefresh; an Update returned by UpdateClusterConfig / CreateCertificateAuthority / DeleteCertificateAuthority reaches a terminal status (Successful | Failed | Cancelled) via DescribeUpdate; a certificate authority walks NOT_USED -> IN_USE and out again through Activate/Delete; and a RegisterCluster of a valid connected cluster is accepted promptly (its retry loop only tolerates propagation-class refusals).
Emulator: ten checks that poll one of those transitions never see it and are killed by the harness: each reports 'Check timed out after 60.0s' while polling DescribeInsightsRefresh, DescribeUpdate, Describe/ListCertificateAuthorities, RegisterCluster acceptance or a capability create. Hypothesis: the emulator either leaves these resources parked in their initial non-terminal status (no background progression) or keeps answering a retryable refusal to the poll, so the predicate is never satisfied; the suite's output gives the timeout but not the hung call, so the mechanism is a hypothesis while the stall itself is observed in all ten.
Why: any client that waits for an EKS transition — the normal way these APIs are used — hangs forever against the emulator instead of completing, and no flow built on an Update id, a CA lifecycle or an insights refresh can be emulated end to end.
Fix: the asynchronous state progression in the eks crate's state/update machinery (Update status, insights-refresh status, certificate-authority signingStatus) and the RegisterCluster accept path, so each advances to its terminal/accepted state without external prompting.
Traces: eks.insights-refresh-completes-autonomously, eks.insights-refresh-status-lifecycle, eks.cancel-update-completed-state-rejected, eks.certificate-authority-active-signer-deletion-rejected, eks.certificate-authority-delete-in-use-blocked, eks.certificate-authority-full-lifecycle-transitions, eks.cluster-tags-empty-vs-absent-preserved, eks.create-capability-unsupported-type-rejected.

## Evidence
Suite simparity-behavioral-aws-v1: 64/310 passed; eks 60/282. None of these checks is marked ALREADY FILED or OWNED.

Cause 1: eks.start-insights-refresh-already-in-progress-blocked — AssertionError: StartInsightsRefresh response -> {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'message': 'The cluster insights refresh is in progress.', 'status': 'IN_PROGRESS'}; the check asserts started['message'] == 'Initiated cluster insights refresh.'.
Cause 2: eks.insights-refresh-cooldown-throttle — AssertionError: {'Message': 'An insights refresh is already in progress for cluster sim2real-eks-clu-806421ef74c5; wait for it to complete before starting another.', 'Code': 'InvalidRequestException'}; the check asserts the message startswith 'Another refresh operation is already in progress for the cluster.' (code InvalidRequestException and http_status 400 asserted just above both passed).
Cause 3: each of the following reports exactly 'Error: Check timed out after 60.0s' — eks.insights-refresh-completes-autonomously (polls DescribeInsightsRefresh for a terminal status), eks.insights-refresh-status-lifecycle (same), eks.cancel-update-completed-state-rejected (polls DescribeUpdate for a terminal update), eks.certificate-authority-full-lifecycle-transitions, eks.certificate-authority-active-signer-deletion-rejected, eks.certificate-authority-delete-in-use-blocked (all poll CA signingStatus / update terminality), eks.cluster-tags-empty-vs-absent-preserved (bounded retry loop waiting for RegisterCluster to be accepted), eks.create-capability-unsupported-type-rejected. Sibling checks in the same files that do NOT poll a transition reached their assertions inside the same budget, so the budget alone does not explain these ten.

## Steps to Reproduce
1. Boot the emulator and point a boto3 eks client at it; create an ACTIVE cluster.
2. Cause 1: start_insights_refresh(clusterName=<cluster>) -> expect 200 with message exactly 'Initiated cluster insights refresh.' and status IN_PROGRESS.
3. Cause 2: immediately call start_insights_refresh again -> expect 400 InvalidRequestException whose message begins 'Another refresh operation is already in progress for the cluster.', and describe_insights_refresh must show the original startedAt unchanged.
4. Cause 3: poll describe_insights_refresh(clusterName=<cluster>) -> it must reach COMPLETED without further calls. Then update_cluster_config(name=<cluster>, logging={'clusterLogging':[{'types':['api'],'enabled':True}]}) and poll describe_update(name, updateId) -> it must reach a terminal status (Successful | Failed | Cancelled). Then create_certificate_authority / activate_certificate_authority / delete_certificate_authority and poll list_certificate_authorities -> signingStatus must transition NOT_USED <-> IN_USE and the returned update must go terminal. Finally register_cluster with a freshly created connector role -> it must be accepted rather than answering a retryable refusal indefinitely.
