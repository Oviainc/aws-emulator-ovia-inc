# Skipped operation stubs

The scaffold op-stub generator declined to emit a stub for the operations
below. Every remaining decline class is one the generator cannot TYPE: a
Unit/absent input, a `@streaming` member (event-stream union or streaming
blob), a member whose target is a shape kind that has no Rust type, or a
malformed model. A stub that does not compile would break the tenant build,
so each is left modeled-but-unwired — hand-author it in `ops/<op>.rs`, using
a rendered sibling as the pattern.

Note the difference from a SCAFFOLDED operation. Both answer HTTP 501, but a
scaffolded op has a compile-checked typed skeleton and is in the service's
`operations(...)` allow-list; an op listed here has neither, and is answered
by the framework's unroutable-operation path instead.

An output that is a SCALAR, list or map is declined: only a structure or a
union can be declared as `{Op}Output`. A `@required` union member and a
`@required`-member reference cycle are NOT decline reasons — a scaffolded
handler returns an error, so it never has to construct a value.

All seven operations below were declined for their Unit/absent input and have
since been HAND-AUTHORED, so each now has a handler file in `ops/` (named by its
`//! @op <Name>` header, which is what puts it in the service's `operations(...)`
allow-list) and its shapes in `types/<op>_types.rs`:
`describe_account_limits`, `describe_adjustment_types`,
`describe_auto_scaling_notification_types`, `describe_lifecycle_hook_types`,
`describe_metric_collection_types`, `describe_scaling_process_types`,
`describe_termination_policy_types`. The lines are kept as the record of what the
generator declined.

// SKIPPED: DescribeAccountLimits — unsupported shape construct: operation 'DescribeAccountLimits' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: DescribeAdjustmentTypes — unsupported shape construct: operation 'DescribeAdjustmentTypes' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: DescribeAutoScalingNotificationTypes — unsupported shape construct: operation 'DescribeAutoScalingNotificationTypes' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: DescribeLifecycleHookTypes — unsupported shape construct: operation 'DescribeLifecycleHookTypes' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: DescribeMetricCollectionTypes — unsupported shape construct: operation 'DescribeMetricCollectionTypes' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: DescribeScalingProcessTypes — unsupported shape construct: operation 'DescribeScalingProcessTypes' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: DescribeTerminationPolicyTypes — unsupported shape construct: operation 'DescribeTerminationPolicyTypes' has a Unit/absent input, which the generator does not emit yet
