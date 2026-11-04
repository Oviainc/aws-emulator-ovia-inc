Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__logs.obs-ecs-2-account-default-insights-inherited-explicit-disabled-publishes-nothing
Issue-Key: cloudwatch__ecs__logs.obs-ecs-2-account-default-insights-inherited-explicit-disabled-publishes-nothing|run#29|AssertionError|assert _cluster_settings(ctx, inherit).get("containerInsights") == "enabled", (
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# cloudwatch__ecs__logs: a cluster's inherited containerInsights setting is not captured at creation, so changing the account default retroactively changes the cluster (1 cause)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Cause 1 of 1: the containerInsights value a cluster inherits from the account setting is resolved on every read instead of being frozen at creation.

AWS: when CreateCluster omits the containerInsights setting, the cluster inherits the account-level default AT CREATION and keeps that value; later PutAccountSetting changes to the default do not alter an existing cluster's settings. DescribeClusters therefore keeps reporting the value the cluster was created with.

Emulator: after the account default is restored/changed, DescribeClusters for the already-created cluster reports the NEW default — the check observes the cluster's settings change to {'containerInsights': 'disabled'} without any UpdateClusterSettings call.

Why: a client that reads a cluster's settings to decide whether detailed metrics are being published gets an answer that silently changes under it, and an account-level change appears to reconfigure existing clusters, which real ECS does not do.

Fix: the CreateCluster handler/cluster state — snapshot the effective containerInsights (and the other inheritable settings) into the cluster record at creation rather than deriving it from the account setting on read.

Traces: cloudwatch__ecs__logs.obs-ecs-2-account-default-insights-inherited-explicit-disabled-publishes-nothing.

## Evidence
Expected (AWS): the cluster's settings keep the value captured at creation regardless of later account-default changes.
Actual (emulator): `AssertionError: a cluster's inherited containerInsights must be captured at creation; restoring the account default changed it to {'containerInsights': 'disabled'}`.

## Steps to Reproduce
1. PutAccountSetting(name=containerInsights, value=enabled).
2. CreateCluster without a containerInsights setting; DescribeClusters -> settings report enabled.
3. PutAccountSetting(name=containerInsights, value=disabled) (or restore the previous default).
4. DescribeClusters for the same cluster -> settings now report disabled, where AWS still reports enabled.
