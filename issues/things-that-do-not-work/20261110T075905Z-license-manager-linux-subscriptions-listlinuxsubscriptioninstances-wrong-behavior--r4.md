Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c8fce076113f82e0e3cede3a2aeb857e5906a3df

# ListLinuxSubscriptionInstances reports non-running instances, though the model says it lists the running ones

_Filed: 2026-11-10T07:59:05Z_
_Operation: ListLinuxSubscriptionInstances | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c8fce076113f82e0e3cede3a2aeb857e5906a3df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's own sentence for this operation states WHICH instances it lists: the running ones. The handler reports every record the discovery store holds, whatever `Instance$Status` the EC2 inventory reported for it — the only state discovery drops is `terminated` — so an instance EC2 reports as `stopped` is returned by an unfiltered `ListLinuxSubscriptionInstances` call.

The handler's own module doc states the opposite rule to the model sentence that is quoted, verbatim, 40 lines further down the SAME file: line 15 says "Every discovered record is reported, whatever its `Status`", while line 57 of the same header carries the generated model contract "Lists the running Amazon EC2 instances that were discovered with commercial Linux subscriptions." The stated reason for the deviation — that `Instance$Status` and the documented `Status` filter would otherwise select nothing — is a reason about the filter, not about which instances the list is of; the model sentence contradicts the conclusion drawn from it ("the list itself cannot be restricted to one").

Note the sibling list is NOT affected and must not change: `Subscription$InstanceCount` is modelled as "The total amount of running instances using this subscription", which `crate::state` already computes by counting only `INSTANCE_STATUS_RUNNING` records (state.rs:705), so the store legitimately holds non-running records — it is the INSTANCE report that is wider than its own documentation.

## Evidence

Model (smithy_query(query="doc", shape="ListLinuxSubscriptionInstances")): "Lists the running Amazon EC2 instances that were discovered with commercial Linux subscriptions." — and the shape it returns, `Instance`: "Details discovered information about a running instance using Linux subscriptions."

The same sentence is in the handler file itself:
- `src/ops/list_linux_subscription_instances.rs:57`: "//! Lists the running Amazon EC2 instances that were discovered with commercial Linux" / ":58`: "//! subscriptions."

The handler states the opposite rule and implements it:
- `src/ops/list_linux_subscription_instances.rs:15-17`: "//! Every discovered record is reported, whatever its `Status`: the modelled //! `Instance$Status` member and the documented `Status` filter are what select an //! instance state, so the list itself cannot be restricted to one."
- `src/ops/list_linux_subscription_instances.rs:181-189`: `state.discovered_instances_in_order().into_iter().filter(|record| crate::state::filters_select(&filters, |name| filtered_member(record, name)))` — the only selection applied is the caller's `Filters`; no `Instance$Status` restriction exists.
- `src/state/discovery.rs:93-96`: the only state discovery drops is terminated — "/// The `InstanceState$Name` of an instance that is GONE. … so a pass does not /// record one", `const INSTANCE_STATE_TERMINATED: &str = "terminated";` (checked at `src/state/discovery.rs:493-495`).

Test coverage pins the current behaviour, so the divergence is observable and deliberate:
- `tests/all/discovered_fleet_selection.rs:125-126`: `let stopped = one(fleet.launch(REGION, &rhel_image, 1).await); fleet.stop(REGION, &stopped).await;`
- `tests/all/discovered_fleet_selection.rs:84-92`: `fn discovered(&self) -> Vec<String> { sorted(vec![self.typed.clone(), self.stopped.clone(), …]) }` — "Every instance discovery records, in the instance-id order both lists report" — asserted as the unfiltered/`AccountID`-filtered answer at `:222-228` and as part of `red_hat` at `:207-211`/`:236-240`.

Contrast, which stays correct: `src/state.rs:702-712` counts only running instances for `Subscription$InstanceCount` ("The total amount of running instances using this subscription").

## Steps to Reproduce

1. Enable discovery: UpdateServiceSettings with LinuxSubscriptionsDiscovery=Enabled and SourceRegions=[us-east-1].
2. Through the emulated EC2, launch an instance in us-east-1 from an AMI whose PlatformDetails bills a Linux subscription (e.g. Red Hat Enterprise Linux), then StopInstances it so EC2 reports its state as `stopped`.
3. Call ListLinuxSubscriptionInstances with no Filters.
4. Observe the stopped instance in `Instances` with `Status: "stopped"` (tests/all/discovered_fleet_selection.rs asserts exactly this), while the operation's model documentation says the call "Lists the running Amazon EC2 instances that were discovered with commercial Linux subscriptions".
