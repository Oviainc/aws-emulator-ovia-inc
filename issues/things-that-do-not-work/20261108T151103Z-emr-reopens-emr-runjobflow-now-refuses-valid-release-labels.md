Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.concurrent-start-session-same-cluster-distinct-ids emr.e2e-studio-session-full-flow emr.e2e-studio-session-start-execute-terminate-flow emr.get-session-endpoint-certificate-blob-preserved emr.get-session-endpoint-for-terminated-cluster-rejected emr.get-session-endpoint-returns-endpoint-for-started-session emr.get-session-endpoint-returns-reachable-endpoint emr.instance-fleet-modify-reflects-target-capacity emr.list-sessions-newest-first emr.list-sessions-ordering-newest-first emr.list-sessions-orders-by-creation-time emr.managed-scaling-policy-numeric-precision-preserved emr.modify-instance-fleet-invalid-capacity-rejected emr.notebook-execution-lifecycle-transitions emr.persistent-app-ui-reaches-attached-state emr.presigned-url-bounded-expiration emr.put-auto-scaling-policy-malformed-rule-rejected emr.session-endpoint-becomes-ready-within-ttl emr.session-lifecycle-start-active-terminate emr.session-lifecycle-start-get-terminate emr.session-tags-empty-vs-absent-preserved emr.session-terminate-then-get-reflects-terminated-state emr.start-session-non-studio-cluster-rejected emr.start-session-on-non-running-cluster-rejected emr.start-session-then-list-sessions-reflects-active-session emr.start-session-transitions-active-then-terminate-then-get-fails emr.terminate-session-already-terminated-rejected emr.terminate-session-twice-rejected emr.three-masters-on-pre-multi-master-release-rejected
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr|RunJobFlow|ValidationException
Issue-Key: emr.session-terminate-then-get-reflects-terminated-state|_run_job_flow_with_retry#3|AssertionError|raise AssertionError(f"run_job_flow never succeeded within {timeout_s}s, last error: {last_exc}")
Issue-Key: emr.three-masters-on-pre-multi-master-release-rejected|run#14|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Commit: 4466ef5882b584b00b87b338b6098c1e65748c9e

# REOPENS: EMR RunJobFlow now refuses valid release labels (emr-7.9.0, emr-spark-8.0.0, emr-5.20.0) with ValidationException 'The supplied release label is invalid'

> _Note: this issue was identified against the code base at commit `4466ef5882b584b00b87b338b6098c1e65748c9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
REOPENS 20261029T175551Z-emr-accepts-two-requests-aws-refuses-unknown-releaselabel.md: its fix left the call refused.

Cause 1 of 1: RunJobFlow validates ReleaseLabel against a stale, partial release-label catalogue, so requests naming release labels that real EMR accepts are refused at submit time.

AWS: Smithy: RunJobFlow.ReleaseLabel is XmlStringMaxLen256 (@length 0..256, pattern ^[\u0020-\uD7FF...]*$) with no enumeration, and RunJobFlow models only InternalServerError (500) — so the label check is recorded behavior only. The recordings show real EMR ACCEPTS RunJobFlow for every currently published release label and returns 200 with {JobFlowId, ClusterArn}. It refuses only a label that does not exist, with HTTP 400, code ValidationException and the message 'The supplied release label is invalid: <ReleaseLabel>.' — fixed text 'The supplied release label is invalid: ' + the label taken verbatim from the request + '.'. The label set AWS publishes is the one ListReleaseLabels enumerates, and it includes the full emr-5.x tail (emr-5.20.0), the current emr-7.x minors (emr-7.9.0) and the non-numeric emr-spark-* family (emr-spark-8.0.0) — see open issue 20261029T212242Z-emr-listreleaselabels-serves-a-stale-and-truncated-release.md, whose own evidence records emr-7.9.0 and emr-spark-8.0.0 as labels AWS returns.

Emulator: RunJobFlow answers HTTP 400 ValidationException 'The supplied release label is invalid: emr-spark-8.0.0.' / '... emr-7.9.0.' / '... emr-5.20.0.' and creates no cluster. The message template itself is right; the set of labels it is applied to is wrong. Hypothesis: the validation the earlier fix added shares the hard-coded, stale catalogue table that ListReleaseLabels reads (frozen at emr-7.5.0, no emr-5.x tail, no emr-spark-* family), so every label outside that snapshot is rejected as nonexistent.

Why: a client that pins a real, current release label cannot create a cluster at all. Every EMR Studio session check, every session-endpoint check, the notebook-execution lifecycle, managed-scaling precision, instance-fleet modify, auto-scaling-policy refusal and presigned-URL checks never get past their first RunJobFlow, so none of those behaviours can be observed. It also masks genuine refusals: emr.three-masters-on-pre-multi-master-release-rejected expects ValidationException 'A master instance group must specify a single instance.' for 3 masters on emr-5.20.0 and instead gets 'The supplied release label is invalid: emr-5.20.0.', i.e. the wrong rule fires. And emr.get-session-endpoint-for-terminated-cluster-rejected spends its whole budget retrying RunJobFlow and is reported ungraded.

Fix: the ReleaseLabel validation in the RunJobFlow handler's input validation and the release-label catalogue it reads, in the EMR crate (the run_job_flow handler and the shared release-label catalogue/state table that DescribeReleaseLabel, ListReleaseLabels and ListSupportedInstanceTypes also read). The catalogue must admit every published label, including the emr-5.x tail, the current emr-7.x minors and non-numeric families such as emr-spark-*; until it is complete the refusal must not be applied to labels that are merely absent from the snapshot.

Traces: the 29 failing checks listed in trace_ids; representative: emr.list-sessions-newest-first (emr-spark-8.0.0 refused), emr.instance-fleet-modify-reflects-target-capacity (emr-7.9.0 refused), emr.three-masters-on-pre-multi-master-release-rejected (emr-5.20.0 refused, wrong rule reported).

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator, service heading `### emr`.

emr.concurrent-start-session-same-cluster-distinct-ids, emr.e2e-studio-session-full-flow, emr.e2e-studio-session-start-execute-terminate-flow, emr.get-session-endpoint-certificate-blob-preserved, emr.get-session-endpoint-returns-endpoint-for-started-session, emr.get-session-endpoint-returns-reachable-endpoint, emr.list-sessions-newest-first, emr.list-sessions-ordering-newest-first, emr.list-sessions-orders-by-creation-time, emr.session-endpoint-becomes-ready-within-ttl, emr.session-lifecycle-start-active-terminate, emr.session-lifecycle-start-get-terminate, emr.session-tags-empty-vs-absent-preserved, emr.start-session-on-non-running-cluster-rejected, emr.start-session-then-list-sessions-reflects-active-session, emr.start-session-transitions-active-then-terminate-then-get-fails, emr.terminate-session-already-terminated-rejected, emr.terminate-session-twice-rejected:
  botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the RunJobFlow operation: The supplied release label is invalid: emr-spark-8.0.0.

emr.instance-fleet-modify-reflects-target-capacity, emr.managed-scaling-policy-numeric-precision-preserved, emr.modify-instance-fleet-invalid-capacity-rejected, emr.notebook-execution-lifecycle-transitions, emr.persistent-app-ui-reaches-attached-state, emr.presigned-url-bounded-expiration, emr.put-auto-scaling-policy-malformed-rule-rejected, emr.start-session-non-studio-cluster-rejected:
  botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the RunJobFlow operation: The supplied release label is invalid: emr-7.9.0.

emr.session-terminate-then-get-reflects-terminated-state:
  AssertionError: run_job_flow never succeeded within 120.0s, last error: An error occurred (ValidationException) when calling the RunJobFlow operation: The supplied release label is invalid: emr-spark-8.0.0.

emr.get-session-endpoint-for-terminated-cluster-rejected:
  TimeoutError: RunJobFlow outlasting IAM propagation: the run budget ended first; not graded (same retry loop on the same refusal).

emr.three-masters-on-pre-multi-master-release-rejected:
  AssertionError: 3 masters on emr-5.20.0 should be rejected ValidationException 'A master instance group must specify a single instance.', got 'ValidationException' 'The supplied release label is invalid: emr-5.20.0.'

Expected in every case: HTTP 200 with {JobFlowId, ClusterArn} (or, for the three-masters check, the master-instance-group refusal).

Smithy grounding: RunJobFlow input ReleaseLabel: XmlStringMaxLen256, @length 0..256, no enum; output {JobFlowId, ClusterArn}; modeled errors: InternalServerError (server, 500). ValidationException for an unknown label is recorded behavior only.

## Steps to Reproduce
Boot the emulator and replay these checks from suite simparity-behavioral-aws-v1 (service token `emr`):
1. emr.list-sessions-newest-first — create an EMR service role and EC2 instance profile in IAM, then RunJobFlow with ReleaseLabel='emr-spark-8.0.0'; expect HTTP 200 with JobFlowId/ClusterArn, observe HTTP 400 ValidationException 'The supplied release label is invalid: emr-spark-8.0.0.'
2. emr.instance-fleet-modify-reflects-target-capacity — same, with ReleaseLabel='emr-7.9.0'; expect acceptance, observe the same refusal naming emr-7.9.0.
3. emr.three-masters-on-pre-multi-master-release-rejected — RunJobFlow with ReleaseLabel='emr-5.20.0' and a MASTER instance group of InstanceCount=3; expect ValidationException 'A master instance group must specify a single instance.', observe 'The supplied release label is invalid: emr-5.20.0.'
4. Cross-check with ListReleaseLabels: the labels refused above are in the set AWS publishes, so RunJobFlow and ListReleaseLabels must agree.
5. Confirm the original fix is not regressed: RunJobFlow with ReleaseLabel='emr-99.99.0' must still be refused ValidationException 'The supplied release label is invalid: emr-99.99.0.' and create no cluster.
