Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ccec5ae7d84de7fbffe9aa154f33c32d41f34d03

# The vCPU usage reader treats a partial DescribeInstanceTypes answer as a complete one, reporting 0% for a group that has instances running

_Filed: 2026-10-28T18:41:39Z_
_Operation: StartQuotaUtilizationReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ccec5ae7d84de7fbffe9aa154f33c32d41f34d03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`usage::measure` is the numerator of every `Utilization` a quota utilization report publishes, and `StartQuotaUtilizationReport` is the one handler that calls it (`src/ops/start_quota_utilization_report.rs:180`). The module states an invariant twice: a metric is either COUNTED or absent, and a reading of `0.0` means "counted, and nothing is in use" — and `measure_running_vcpus`'s own doc says outright that nothing is recorded unless BOTH EC2 reads answered, "because a count taken from half the facts would report a usage of zero for a group that has instances running".

The code under that comment breaks the rule for a PARTIAL answer, as opposed to a failed one:

* `instance_type_vcpus` sends the running types in batches of 100 and reads only `described.instance_types()` of the FIRST response of each batch — it never follows the `NextToken` that `DescribeInstanceTypes` returns, although both sibling readers in the same file (`measure_vpcs`, `running_instances`) walk `next_token` up to `MAX_LISTING_PAGES` for exactly that reason. A paged answer therefore yields a `vcpus` map that is missing types, and `instance_type_vcpus` still returns `Some(..)`, i.e. "both reads answered".
* `measure_running_vcpus` has already zero-initialised EVERY vCPU group ("a group with no instance running in it reads zero, which is a measurement and not a placeholder"), and then silently `continue`s past each instance whose type is missing from that map.

The result is the precise outcome the comment says must not happen: the group's reading stays at the 0.0 that was written as a measurement, `utilization_pct` divides it by the value in force, and the report publishes `Utilization: 0.0` for a vCPU quota that has instances running — instead of leaving the metric uncounted (no `Utilization` member), which is the honest answer the module reserves for "this emulator holds no usage source". Only the whole-read FAILURE path is handled this way (`running_instances`/`instance_type_vcpus` return `None` on an error, and the caller bails out); the partial-read path is not.

The same hole swallows any requested type the answer omits for a reason other than paging, because the missing-entry branch cannot tell the two apart.

## Evidence

The rule, stated in this crate — services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/state/usage.rs:219-225:

    /// The vCPUs of the account's running instances, per vCPU quota group and purchase
    /// model.
    ///
    /// Both halves come from EC2: which instances are running and what they are, and how
    /// many vCPUs an instance type has (`DescribeInstanceTypes`, `VCpuInfo$DefaultVCpus`).
    /// Nothing is recorded unless BOTH reads answered — a count taken from half the facts
    /// would report a usage of zero for a group that has instances running.

and usage.rs:29-32:

    //! act on. A reading of `0.0` therefore means "counted, and nothing is in use", which is
    //! a different answer from "not counted".

The code that breaks it — usage.rs:240-257 (every group is first recorded as a measurement of zero, then an instance whose type is missing from the map is dropped):

        // EC2 answered, so every vCPU group IS counted — a group with no instance running
        // in it reads zero, which is a measurement and not a placeholder.
        for class in super::vcpu_group_classes() {
            for purchase_model in [ ... ] {
                usage.set(&super::vcpu_usage_metric_for(class, purchase_model), 0.0);
            }
        }

        for instance in &running {
            let Some(class) = vcpu_group_class(&instance.instance_type) else {
                continue;
            };
            let Some(instance_vcpus) = vcpus.get(&instance.instance_type) else {
                continue;
            };

and the reader that can hand back a short map — usage.rs:324-344, which sends each batch once and never reads a token:

        for batch in types.chunks(MAX_INSTANCE_TYPES_PER_CALL) {
            ...
            let answer = ec2
                .describe_instance_types()
                .set_instance_types(Some(named))
                .send()
                .await;
            ...
            for info in described.instance_types() {

while its two siblings in the same file DO walk pagination, for the reason the const states — usage.rs:38-44:

    /// How many pages of one listing a measurement walks before it stops.
    ///
    /// A measurement is made inside a request, so it is bounded rather than walked to the
    /// end of whatever the account holds: EC2 answers up to 1,000 items per page, so this
    /// covers hundreds of thousands of resources and still terminates if a listing ever
    /// kept handing out tokens.
    const MAX_LISTING_PAGES: usize = 64;

usage.rs:183-205 (`measure_vpcs`): `let mut next_token: Option<String> = None; for _ in 0..MAX_LISTING_PAGES { ... next_token = page.next_token().map(str::to_string); if next_token.is_none() { break; } }` and usage.rs:273-313 (`running_instances`): the same loop.

Where the zero then surfaces — src/state.rs:1966-1973 (`utilization_pct`) returns `Some(usage / applied * 100.0)` for a reading of 0.0, and src/state.rs:1485 puts it on the wire as `utilization: utilization_pct(default, applied, usage)`, which `GetQuotaUtilizationReport` publishes (src/ops/get_quota_utilization_report.rs:255).

Test coverage: tests/all/quota_utilization_usage.rs reads the vCPU figures back from EC2 the same single-call way (lines 295-312, `info.v_cpu_info().and_then(|cpus| cpus.default_v_cpus())`), so no suite pins what the report says when the describe answer does not carry every requested type; usage.rs:457-489 pins only that `add` on a metric nothing `set` is dropped — the counterpart case, not this one.

## Steps to Reproduce

1. Mount the emulator with both EC2 and Service Quotas on one engine.
2. Launch EC2 instances of more than one instance type (e.g. several `m5.large` and `c6g.xlarge`) so the account has running vCPUs in the Standard group.
3. Have the EC2 DescribeInstanceTypes answer carry fewer than the requested types in its first response — i.e. a response bearing a NextToken, which `instance_type_vcpus` never follows (it reads only `described.instance_types()` of the first response, unlike `measure_vpcs`/`running_instances`, which loop on `next_token`).
4. POST / with X-Amz-Target: ServiceQuotasV20190624.StartQuotaUtilizationReport and note the ReportId.
5. Poll X-Amz-Target: ServiceQuotasV20190624.GetQuotaUtilizationReport with that ReportId until Status is COMPLETED.
6. Find the record for 'Running On-Demand Standard (A, C, D, H, I, M, R, T, Z) instances': its `Utilization` is 0.0 — a measurement saying nothing is in use — although the instances from step 2 are running. The honest answer the module reserves for an uncounted metric (no `Utilization` member at all) is never produced, because `instance_type_vcpus` returned `Some(..)` from half the facts.
