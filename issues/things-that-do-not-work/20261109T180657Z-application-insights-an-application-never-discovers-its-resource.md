Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-insights.application-lifecycle-activates-on-component-monitoring application-insights.custom-component-ec2-only-and-exclusive
Issue-Shape-Id: com.amazonaws.applicationinsights#EC2WindowsBarleyService
Issue-Key: application-insights.application-lifecycle-activates-on-component-monitoring|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: application-insights.custom-component-ec2-only-and-exclusive|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Commit: db165e9afd311b39a9697deff319fb6ff63a6914

# application-insights: an application never discovers its resource group's members as components, and LifeCycle never reports NOT_CONFIGURED

> _Note: this issue was identified against the code base at commit `db165e9afd311b39a9697deff319fb6ff63a6914`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListComponents

## Description
Two net-new root causes in the Application Insights emulator, both reached from the same starting point: CreateApplication over an existing Resource Groups group. Neither check in the suite can get past them, so both are killed by the 180 s check cap while polling. The open issue `20261029T173211Z-applicationinsights-refusal-status-code-and-message-diverge-from.md` covers refusal status/wording only and does NOT cover either cause below.

=== Cause 1 of 2: CreateApplication onboards no components, so ListComponents stays empty forever ===
Failing checks: application-insights.application-lifecycle-activates-on-component-monitoring, application-insights.custom-component-ec2-only-and-exclusive

AWS: ListComponents is documented (Smithy, com.amazonaws.applicationinsights#ListComponents) as "Lists the auto-grouped, standalone, and custom components of the application." Creating an application over a RESOURCE_GROUP_BASED resource group makes Application Insights onboard every resource the group resolves to: each supported resource that is not part of a custom component becomes a default/standalone ApplicationComponent whose ComponentName is the resource's ARN, with Monitor=false until a configuration turns monitoring on. ListComponents then returns those components in ApplicationComponentList (200), and DescribeComponent/DescribeComponentConfiguration answer for them. No part of this is a per-run value: the component set is whatever the named group resolves to, and the name of each is that resource's ARN.

Emulator: CreateApplication writes `components: BTreeMap::new()` and nothing ever adds to it; ListComponents only pages `application.components`. So after CreateApplication over a group that resolves to a tagged S3 bucket (and, in the second check, a tagged EC2 instance), ListComponents answers 200 with an empty ApplicationComponentList indefinitely. The checks poll ListComponents for the bucket ARN `arn:aws:s3:::<bucket>` (and the instance ARN `arn:aws:ec2:<region>:<account>:instance/<id>`) with a 240 s budget; the list never becomes non-empty, so the harness kills each check: "Check timed out after 180.0s".

Why: a client that creates an application and then waits for its components — the normal onboarding sequence, and the only way to reach UpdateComponentConfiguration, DescribeComponent or CreateComponent for a discovered resource — waits forever against the emulator. It does not get a wrong answer it can detect; it hangs, and every downstream Application Insights behavior is unreachable.

Fix: the component-discovery/onboarding step the application's state owes — CreateApplication's handler plus the shared application state that computes an application's component set from its Resource Groups group (the same `crate::state::resource_groups::resolve_group` seam CreateApplication already uses to admit the group), surfaced through the ListComponents / DescribeComponent readers.

=== Cause 2 of 2: LifeCycle moves CREATING -> ACTIVE on a timer, so an application with no monitored component never reports NOT_CONFIGURED ===
Failing check: application-insights.application-lifecycle-activates-on-component-monitoring

AWS: DescribeApplication's ApplicationInfo.LifeCycle (Smithy: ApplicationInfo.LifeCycle, an unconstrained string; the model names no value list, so the recorded behavior is the reference) reports NOT_CONFIGURED for an application that has onboarded but has no component with monitoring enabled, and ACTIVE only once at least one component is monitored. The transition is driven by component configuration — UpdateComponentConfiguration with Monitor=true — not by elapsed time; after that call DescribeApplication reports ACTIVE with no further call.

Emulator: `Application::life_cycle` returns CREATING while `now < onboarded_at` and ACTIVE afterwards, where `onboarded_at = create_time + ONBOARDING_SPAN` (500 ms). NOT_CONFIGURED is not in the emulator's vocabulary at all (state.rs defines only LIFE_CYCLE_CREATING and LIFE_CYCLE_ACTIVE), and monitoring state is never consulted: `Component::monitor` is stored but nothing reads it for LifeCycle. So ~500 ms after CreateApplication the application reports ACTIVE with zero components and zero monitored components, and the check's assertion that an unmonitored application settles on NOT_CONFIGURED cannot hold. This cause is independent of Cause 1: fixing discovery alone still leaves the check failing, because the discovered components start unmonitored and LifeCycle would already read ACTIVE.

Why: LifeCycle is how a client decides whether an application is actually configured for monitoring. An emulator that reports ACTIVE for an application with nothing monitored tells a client its onboarding is complete when no monitoring exists, so configuration logic gated on NOT_CONFIGURED never runs, and logic waiting for ACTIVE-after-configuration succeeds before configuration happened.

Fix: the LifeCycle computation in the application's state (`Application::life_cycle` / `Application::info` in the service's state module) must derive the value from the application's components — CREATING while onboarding is outstanding, NOT_CONFIGURED once onboarded with no monitored component, ACTIVE once any component has Monitor=true — and UpdateComponentConfiguration's handler must set that monitoring flag so the transition is observable.

Traces: both failing checks listed above; see trace_ids.

## Evidence
Suite report (0/4 passed; both application-insights checks mismatched):

  #### [mismatch] application-insights.application-lifecycle-activates-on-component-monitoring
  Error: Check timed out after 180.0s
  #### [mismatch] application-insights.custom-component-ec2-only-and-exclusive
  Error: Check timed out after 180.0s

Cause 1 — expected vs actual:
  Expected: after CreateApplication(ResourceGroupName=<group>, AutoConfigEnabled=False) over a group whose TAG_FILTERS_1_0 query resolves to a tagged S3 bucket (and a tagged EC2 instance), ListComponents(ResourceGroupName=<group>) eventually returns ApplicationComponentList containing an entry with ComponentName == the bucket's ARN (and the instance's ARN), Monitor == false.
  Actual: ListComponents answers 200 with an empty ApplicationComponentList for the life of the application. Both checks' `_poll_until(... timeout_s=240, interval_s=10, description="the bucket to be auto-discovered as a default component" / "the bucket and the instance to be auto-discovered as components")` never satisfies its predicate and the check is killed at 180 s.
  Emulator source: CreateApplication builds the stored record with `components: ::std::collections::BTreeMap::new()`; ListComponents pages only `application.components` via `crate::state::paginate(&application.components, ...)`. No code path inserts a discovered resource.

Cause 2 — expected vs actual:
  Expected: DescribeApplication(ResourceGroupName=<group>)["ApplicationInfo"]["LifeCycle"] == "CREATING" while onboarding, then "NOT_CONFIGURED" while every discovered component is unmonitored, then "ACTIVE" after UpdateComponentConfiguration(ComponentName=<arn>, Monitor=True, Tier="DEFAULT", ComponentConfiguration=...) with no further call.
  Actual: LifeCycle is "CREATING" for the first 500 ms and "ACTIVE" thereafter regardless of components or monitoring. "NOT_CONFIGURED" is never emitted.
  Emulator source: `Application::life_cycle(now)` returns `LIFE_CYCLE_CREATING` if `now < self.onboarded_at` else `LIFE_CYCLE_ACTIVE`; `onboarded_at = now + ONBOARDING_SPAN` (ONBOARDING_SPAN = 500 ms, documented in state.rs as "CHOSEN, not documented"). state.rs declares no NOT_CONFIGURED constant, and `Component::monitor` is documented there as set by "no implemented operation".

Note on the status/code surface: both causes are observed as a client hang, not as an error response — ListComponents and DescribeApplication both answer HTTP 200. There is no diverging error code or message to quote for either cause.

## Steps to Reproduce
Against the emulator, with s3, resource-groups and application-insights mounted:

1. s3.create_bucket(Bucket=<bucket>, CreateBucketConfiguration={"LocationConstraint": <region>}) and s3.put_bucket_tagging(Bucket=<bucket>, Tagging={"TagSet": [{"Key": <tag>, "Value": "1"}]}).
2. resource-groups.create_group(Name=<group>, ResourceQuery={"Type": "TAG_FILTERS_1_0", "Query": json.dumps({"ResourceTypeFilters": ["AWS::AllSupported"], "TagFilters": [{"Key": <tag>, "Values": ["1"]}]})}).
3. application-insights.create_application(ResourceGroupName=<group>, AutoConfigEnabled=False).
4. Cause 1: poll application-insights.list_components(ResourceGroupName=<group>). Expected: ApplicationComponentList eventually contains ComponentName == "arn:aws:s3:::<bucket>" with Monitor == False. Observed: ApplicationComponentList is empty on every call, so a poll with any timeout never completes.
5. Cause 2: call application-insights.describe_application(ResourceGroupName=<group>) immediately, then again after ~1 s. Expected: "CREATING" then "NOT_CONFIGURED" (no component is monitored). Observed: "CREATING" then "ACTIVE". Then (once Cause 1 is fixed) update_component_configuration(ResourceGroupName=<group>, ComponentName="arn:aws:s3:::<bucket>", Monitor=True, Tier="DEFAULT", ComponentConfiguration=json.dumps({"subComponents": []})) and re-read: ACTIVE must be reached only here.

Replay both checks named in trace_ids to confirm.
