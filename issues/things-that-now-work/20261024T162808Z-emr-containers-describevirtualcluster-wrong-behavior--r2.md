Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0831f46a299ca00fdb6555dd0d1d0166298aa2f3

# Virtual clusters, job runs, job templates and security configurations are not Region-scoped: a resource created in one Region is readable from every Region

_Filed: 2026-10-24T16:28:08Z_
_Operation: DescribeVirtualCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0831f46a299ca00fdb6555dd0d1d0166298aa2f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: EMR on EKS resources are regional. A virtual cluster (or job run, job template, security configuration) created against the us-east-1 endpoint does not exist for a request signed for eu-west-1: `DescribeVirtualCluster` / `DescribeJobRun` / `DescribeJobTemplate` / `DescribeSecurityConfiguration` answer `ResourceNotFoundException`, the lists are empty, and a cross-Region `DeleteVirtualCluster` / `CancelJobRun` is refused.

Actual: the store is a single global map keyed by resource id only. The request's Region is read solely to build an ARN at create time (`state::request_region`); every read and mutate path looks the record up by id with no Region check. So a cluster created with us-east-1 credentials is described (200) by a request signed for eu-west-1 — and the record it returns carries the us-east-1 ARN, so the response advertises a resource from another Region. The same applies to `DescribeJobRun`, `DescribeJobTemplate`, `DescribeSecurityConfiguration`, `ListJobRuns`, `ListJobTemplates`, `ListSecurityConfigurations`, `CancelJobRun`, `DeleteVirtualCluster`, `DeleteJobTemplate`, `DeleteSecurityConfiguration`, `UpdateVirtualCluster` and `UntagResource`.

## Evidence

Spec/semantics: every ARN shape in the model embeds a Region (`VirtualClusterArn`, `JobArn`, `JobTemplateArn`, `SecurityConfigurationArn` are all `arn:...:emr-containers:{region}:{account}:/...`), and EMR on EKS is a regional service — a resource id is only meaningful inside the Region that minted it.

Handler/state: src/state.rs — `virtual_clusters`, `job_runs`, `job_templates`, `security_configurations` are `HashMap<String, Record>` keyed by id with no Region field on any record; `request_region(ctx)` is used only by the create handlers to format an ARN (src/ops/create_virtual_cluster.rs:171/204, src/ops/start_job_run.rs:176/306, src/ops/create_job_template.rs:169, src/ops/create_security_configuration.rs:164). `virtual_cluster_view` / `job_template_view` / `security_configuration_view` / `job_run` take only the id. (Note `resource_tags_mut` DOES compare the full ARN, so the tagging path is Region-correct — the describe/list/delete paths are not.)

Tests: the suite drives a single Region constant (`REGION` in tests/all/virtual_cluster_job_run_loop.rs:348), so nothing asserts cross-Region isolation.

## Steps to Reproduce

1. Create a virtual cluster with a SigV4 credential scope for us-east-1: POST /virtualclusters {"name":"vc1","clientToken":"t1","containerProvider":{"id":"eks-1","type":"EKS"}} and note the id (its arn carries us-east-1)
2. Repeat the request walk with a credential scope for eu-west-1 and GET /virtualclusters/{id} — observe 200 with the us-east-1 ARN instead of ResourceNotFoundException
3. With the eu-west-1 scope, DELETE /virtualclusters/{id} — observe the cluster of another Region is torn down (200) instead of being refused
4. Same for a job template: POST /jobtemplates in us-east-1, then GET /jobtemplates and GET /jobtemplates/{id} in eu-west-1 — the template is listed and described
