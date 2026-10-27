Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs.metric-extraction-policy-invalid-status-rejected
Issue-Key: cloudwatch__logs.metric-extraction-policy-invalid-status-rejected|_poll_until_stable#26|Timeout|time.sleep(interval_s)
Issue-Commit: b5c349f78c27b93d7bc476c26ede97815cd374b3

# cloudwatch__logs: EMF documents and METRIC_EXTRACTION_POLICY produce no metric data, so a rejected malformed policy cannot be shown to have left extraction running (1 cause)

> _Note: this issue was identified against the code base at commit `b5c349f78c27b93d7bc476c26ede97815cd374b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAccountPolicy

## Description
Cause 1 of 1: the metric plane behind METRIC_EXTRACTION_POLICY is inert - neither an extracted EMF metric nor AWS/Logs EMFDisabledErrors ever materialises - so the no-effect half of a correctly refused malformed policy, which is the claim this check exists to grade, can never be observed and the check hangs to the harness cap.

AWS: Smithy: PutAccountPolicy(policyName required, policyDocument required, policyType enum required, scope enum, selectionCriteria) with errors InvalidParameterException (400), LimitExceededException (400), OperationAbortedException (400), ServiceUnavailableException (500); policyDocument is typed as an opaque string, so the per-policy-type document rules are enforced by the service and not by the model. Recorded behaviour (the check's own measurement, us-west-2, 2026-10-01): for policyType METRIC_EXTRACTION_POLICY, a document whose EmbeddedMetricFormat.Status is any string other than the exact "Disabled" - "Enabled", "ENABLED", "DISABLED", "disabled", "bogus" were all tried - is refused with HTTP 400, error code InvalidParameterException and the message "Invalid json policy document" (entirely fixed text, no part taken from the request), and stores nothing: DescribeAccountPolicies for that policyName returns an empty accountPolicies list. The well-formed Status="Disabled" document is accepted, echoed back with its policyName and selectionCriteria, suppresses EMF extraction for the log groups its selectionCriteria names, and counts one AWS/Logs EMFDisabledErrors datapoint per suppressed document dimensioned by LogGroupName = the suppressed group. A group named only by the REJECTED policy keeps extracting: the EMF document's metric appears in the document's own namespace with Sum equal to the value in the document and SampleCount 1, and that group gets no EMFDisabledErrors datapoint at all.

Emulator: the check hangs and the replay verdict is the bare "Check timed out after 60.0s". The rejection legs would fail fast with an AssertionError had the malformed documents been accepted or refused differently, so the stall is at the metric-plane barrier that follows them: the poll for (the extracted EMF metric from the rejected-policy group, EMFDisabledErrors for the accepted-policy group) has a 200 s budget and never returns a non-empty reading, i.e. PutLogEvents of an EMF document materialises no metric in the document's namespace and the accepted Status="Disabled" policy counts no AWS/Logs EMFDisabledErrors.

Why: a client's EMF pipeline is invisible here. Metrics written through structured log documents never appear, so dashboards and alarms built on them read empty; and because EMFDisabledErrors is never counted either, a client cannot tell a suppressing policy from a broken one, nor confirm that a refused policy changed nothing. The over-permissive failure mode this check targets - storing an opaque policyDocument and then treating "a policy exists" as "extraction disabled" - is also unverifiable while the metric plane produces nothing.

Fix: the cloudwatch-logs EMF ingestion path (the PutLogEvents handler's metric-extraction stage) must parse the _aws/CloudWatchMetrics envelope and publish the declared metrics into the cloudwatch metric store under the document's Namespace and Dimensions, and the METRIC_EXTRACTION_POLICY state must gate that stage per log group, counting one AWS/Logs EMFDisabledErrors datapoint dimensioned by LogGroupName for each document it suppresses. PutAccountPolicy's validation of a METRIC_EXTRACTION_POLICY document must stay strict (only the exact "Disabled") and must store nothing when it refuses.

Related already-open issue, named so this is not closed as a duplicate: 20261025T020208Z-cloudwatch-logs-emf-log-events-and-metric-extraction-policy-produce.md describes the same inert metric plane for the sibling "extraction resumes after policy deleted" check, but it does not carry this check's replay key, and this check additionally pins the refusal contract (InvalidParameterException / "Invalid json policy document" / nothing stored) and the asymmetry between the rejected-policy and accepted-policy groups.

Traces: cloudwatch__logs.metric-extraction-policy-invalid-status-rejected (mismatch, "Check timed out after 60.0s").

## Evidence
Expected (AWS): logs.put_account_policy(policyName=<p1>, policyType="METRIC_EXTRACTION_POLICY", policyDocument='{"EmbeddedMetricFormat":{"Status":"Enabled"}}', scope="ALL", selectionCriteria='LogGroupName IN ["<gA>"]') -> HTTP 400 InvalidParameterException, message "Invalid json policy document" (fixed text), and describe_account_policies(policyType="METRIC_EXTRACTION_POLICY", policyName=<p1>) -> accountPolicies == []. The same for Status="DISABLED". Then put_account_policy with Status="Disabled" for selectionCriteria naming <gB> is accepted and echoed. After one EMF document is written into each group, get_metric_statistics(Namespace=<ns of the gA document>, MetricName="Probe", Dimensions=[{Svc:obs}], Period=60, Statistics=["Sum","SampleCount"]) -> (Sum 11.0, SampleCount 1.0), and get_metric_statistics(Namespace="AWS/Logs", MetricName="EMFDisabledErrors", Dimensions=[{LogGroupName:<gB>}]) -> Sum 1.0; while EMFDisabledErrors for <gA> and the suppressed namespace of <gB> stay absent (None).

Actual (emulator): the metric reads never return any datapoint - the gA EMF metric is absent and EMFDisabledErrors for gB is absent - so the settle poll (200 s budget) never produces a reading and the harness kills the check with "Check timed out after 60.0s" before any of the metric assertions is evaluated.

## Steps to Reproduce
1. logs.create_log_group for two groups <gA>, <gB>; logs.put_retention_policy(retentionInDays=1); logs.create_log_stream(logStreamName="s") in each.
2. logs.put_account_policy(policyName=<p1>, policyType="METRIC_EXTRACTION_POLICY", policyDocument='{"EmbeddedMetricFormat":{"Status":"Enabled"}}', scope="ALL", selectionCriteria='LogGroupName IN ["<gA>"]'). Expected 400 InvalidParameterException / "Invalid json policy document"; repeat with Status="DISABLED".
3. logs.describe_account_policies(policyType="METRIC_EXTRACTION_POLICY", policyName=<p1>) -> expected [].
4. logs.put_account_policy(policyName=<p2>, ... policyDocument='{"EmbeddedMetricFormat":{"Status":"Disabled"}}', selectionCriteria='LogGroupName IN ["<gB>"]') -> accepted.
5. logs.put_log_events into <gA> and <gB>, each one EMF document declaring Namespace=<ns per group>, Dimensions [["Svc"]], Metrics [{Name:"Probe",Unit:"Count"}], Svc="obs", Probe=11.
6. cloudwatch.get_metric_statistics(Namespace=<ns of gA>, MetricName="Probe", Dimensions=[{Name:"Svc",Value:"obs"}], Period=60, Statistics=["Sum","SampleCount"]). Expected (11.0, 1.0). Observed: no datapoints.
7. cloudwatch.get_metric_statistics(Namespace="AWS/Logs", MetricName="EMFDisabledErrors", Dimensions=[{Name:"LogGroupName",Value:"<gB>"}], Period=60, Statistics=["Sum"]). Expected Sum 1.0. Observed: no datapoints.
