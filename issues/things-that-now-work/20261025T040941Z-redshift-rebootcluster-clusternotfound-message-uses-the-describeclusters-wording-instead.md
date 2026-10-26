Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.delete-cluster-lifecycle
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.delete-cluster-lifecycle|run#32|AssertionError|assert reboot_message == expected_message, f"expected the message {expected_message!r}, got {reboot_message!r}"
Issue-Commit: 5b50be90bb844e905a41063b16fb5454d8305c48

# RebootCluster ClusterNotFound message uses the DescribeClusters wording instead of RebootCluster's own template (1 cause)

> _Note: this issue was identified against the code base at commit `5b50be90bb844e905a41063b16fb5454d8305c48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RebootCluster

## Description
Cause 1 of 1: RebootCluster against a cluster identifier that does not exist (here, one that was just deleted) is correctly refused with ClusterNotFound/HTTP 404, but the fault's message is rendered with the DescribeClusters wording rather than RebootCluster's own, so the two operations are indistinguishable in their not-found text.

Failing check ids for this cause: redshift.delete-cluster-lifecycle

AWS: Smithy: RebootCluster errors ClusterNotFoundFault (errorKind client, httpError 404, awsQueryError code `ClusterNotFound`, doc "The ClusterIdentifier parameter does not refer to an existing cluster."), InvalidClusterStateFault. The model fixes the code and status but not the message text; for the text this is recorded behavior only. The recording shows the message is built from the request: real AWS answers RebootCluster on a missing cluster with `Cluster not found: <ClusterIdentifier>` (the identifier taken verbatim from the request), while the SAME account/region answers DescribeClusters on the same missing identifier with `Cluster <ClusterIdentifier> not found.` — i.e. the message template is per-operation, and the identifier interpolation is the rule. The same check asserts and PASSES the DescribeClusters form, so the oracle distinguishes the two templates within one replay.

Emulator: returns ClusterNotFound with HTTP 404 (both correct) but with the message `Cluster sim2real-behav-rs-del-<id> not found.` — the DescribeClusters template — where AWS returns `Cluster not found: sim2real-behav-rs-del-<id>`. The handler hardcodes the one shared wording: services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/reboot_cluster.rs line 178 builds `format!("Cluster {} not found.", input.cluster_identifier)`.

Why: a client or operator tool that matches on the RebootCluster not-found message (log scrapers, CLI error-text assertions, runbooks and test suites pinned to the recorded AWS string, and anything that distinguishes a control-plane read refusal from a mutation refusal by its text) sees the wrong sentence. Everything structural about the refusal is right, so this is cosmetic-but-observable: the identifier placement and the "Cluster not found:" prefix are what the recorded AWS response carries for this operation.

Fix: the RebootCluster handler's ClusterNotFoundFault construction (services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/reboot_cluster.rs) — emit this operation's own message template, `Cluster not found: {ClusterIdentifier}`, built from the request's ClusterIdentifier, and leave the DescribeClusters template (which already matches) alone. If the not-found message is centralised in state/helpers, it must be parameterised per operation rather than shared.

Note: no existing open issue covers this; the nearest neighbours (20261024T024248Z-redshift-cluster-scoped-credential-and-notification-operations-answer.md and 20261025T022516Z-redshift-discontinued-cluster-security-group-family-answers-notimplemented.md) are about operations answering NotImplemented, not about RebootCluster's message, and RebootCluster itself is implemented and answers the right code and status here.

## Evidence
Check redshift.delete-cluster-lifecycle, verdict [mismatch]. The whole lifecycle (CreateCluster -> available -> field/tag reads -> tag-filtered DescribeClusters paginate -> DeleteCluster -> cluster disappears -> DescribeClusters refused with ClusterNotFound/4xx and message `Cluster <id> not found.` -> cluster absent from the tag-filtered listing) succeeds. The LAST assertion fails:

  AssertionError: expected the message 'Cluster not found: sim2real-behav-rs-del-0ea60d00f381', got 'Cluster sim2real-behav-rs-del-0ea60d00f381 not found.'
  (check_redshift_delete_cluster_lifecycle.py line 234: assert reboot_message == expected_message)

Expected (recorded AWS, RebootCluster on a deleted cluster): code `ClusterNotFound`, HTTP 404, message `Cluster not found: <ClusterIdentifier>`.
Actual (emulator): code `ClusterNotFound`, HTTP 404 (both MATCH), message `Cluster <ClusterIdentifier> not found.`.
Contrast, same check, same identifier, asserted and passing: DescribeClusters -> `ClusterNotFound`, 4xx, `Cluster <ClusterIdentifier> not found.`.
Source of the emulator string: reboot_cluster.rs:176-182, `ClusterNotFoundFault { message: Some(format!("Cluster {} not found.", input.cluster_identifier)) }`. The generated name/id in the example (sim2real-behav-rs-del-0ea60d00f381) varies per run and is not a requirement; the template and the identifier interpolation are.

## Steps to Reproduce
1. Call redshift CreateCluster with ClusterIdentifier=<id>, NodeType=ra3.large, ClusterType=single-node, MasterUsername=adminuser, MasterUserPassword=<pw>, DBName=devdb, Encrypted=true, AutomatedSnapshotRetentionPeriod=1; wait for ClusterStatus=available.
2. Call DeleteCluster(ClusterIdentifier=<id>, SkipFinalClusterSnapshot=true) and poll DescribeClusters until the cluster is gone.
3. Call RebootCluster(ClusterIdentifier=<id>).
4. Inspect the error: Code is `ClusterNotFound` and HTTPStatusCode is 404 (correct), but Message is `Cluster <id> not found.` instead of AWS's `Cluster not found: <id>`.
5. For contrast, call DescribeClusters(ClusterIdentifier=<id>) in the same run: its message `Cluster <id> not found.` is the AWS-correct one, showing the single shared template is being used for both.
Replay: run_behavioral_suite and re-run check redshift.delete-cluster-lifecycle.
