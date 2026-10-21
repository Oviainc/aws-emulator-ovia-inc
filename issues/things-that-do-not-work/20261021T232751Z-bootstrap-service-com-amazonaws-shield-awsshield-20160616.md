Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: e44f583f966e818f9abdd86a5b0ab9a8f2f88230

# Bootstrap the Shield emulator

_Filed: 2026-10-21T23:27:51Z_
_Operation: service-com-amazonaws-shield-awsshield-20160616 | Class: missing_

> _Note: this issue was identified against the code base at commit `e44f583f966e818f9abdd86a5b0ab9a8f2f88230`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 36 Shield operations is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST SLICE: the Shield Advanced subscription + protection lifecycle, statefully, end-to-end. Wire protocol is `awsJson1_1` (`POST /`, `X-Amz-Target: AWSShield_20160616.<Operation>`).

Happy path to implement, in order:

1. **CreateSubscription** — no input, no output members. Must record a Subscription in state for the account/region. A second call while a subscription exists must fail with `ResourceAlreadyExistsException` (HTTP 400), not succeed silently. It is the gate for the rest of the slice.
2. **CreateProtection** — input `Name` (required, 1..128, `^[ a-zA-Z0-9_\.\-]*$`), `ResourceArn` (required, 1..2048, `^arn:aws`), optional `Tags`. Must allocate a fresh 36-char ProtectionId (`^[a-zA-Z0-9\-]*$`, i.e. a UUID), persist `Protection{Id, Name, ResourceArn, ProtectionArn, HealthCheckIds}` keyed by that id, and return `ProtectionId`. Protecting the same `ResourceArn` twice must fail with `ResourceAlreadyExistsException`. With no subscription present, fail with `ResourceNotFoundException`.
3. **DescribeProtection** — input `ProtectionId` OR `ResourceArn` (both optional, exactly one must be given: both/neither ⇒ `InvalidParameterException`). Must look the protection up in state and return the stored `Protection` structure (`Id`, `Name`, `ResourceArn`, `ProtectionArn`, `HealthCheckIds`) — the values that were created, not echoes of the request. Unknown id/arn ⇒ `ResourceNotFoundException`.
4. **ListProtections** — `@paginated(inputToken=NextToken, outputToken=NextToken, items=Protections)`; input `NextToken` (1..4096), `MaxResults` (0..10000). Must return every persisted `Protection`, honour `MaxResults` as a page size and emit/consume `NextToken`, omitting `NextToken` on the last page. An unparsable token ⇒ `InvalidPaginationTokenException`.
5. **DeleteProtection** — input `ProtectionId` (required, 36..36). Must remove the protection from state; a subsequent DescribeProtection/ListProtections must no longer see it. Deleting an unknown or already-deleted id ⇒ `ResourceNotFoundException`.
6. **DescribeSubscription** — no input; output `Subscription`. Must report the subscription persisted by step 1 (start/end time, time commitment, auto-renew, `SubscriptionLimits`); with no subscription ⇒ `ResourceNotFoundException`.

NEVER STUBS: each of these six must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (see README "State"). Hardcoded or echoed responses, and generating a ProtectionId without storing it, are failures of this issue. Errors must be the modeled Shield exceptions (awsJson1_1 error shapes, HTTP 400/500 per OPERATIONS.md), not generic 501/500.

## State Model

In `ComAmazonawsShieldAwsshield20160616State` (`src/state.rs`), under one lock:

- `subscription: Option<Subscription>` — `{ start_time, end_time, time_commitment_in_seconds, auto_renew, limits, proactive_engagement_status, subscription_limits, subscription_arn }`; `None` until CreateSubscription, set on create, used by DescribeSubscription and as the gate for CreateProtection.
- `protections: HashMap<String /* ProtectionId (uuid, 36 chars) */, ProtectionEntry>` where `ProtectionEntry { id: String, name: String, resource_arn: String, protection_arn: String /* arn:aws:shield::<acct>:protection/<id> */, health_check_ids: Vec<String>, application_layer_automatic_response: Option<AppLayerConfig>, tags: Vec<Tag>, created_at: SystemTime }`.
- `protections_by_resource_arn: HashMap<String /* ResourceArn */, String /* ProtectionId */>` — the uniqueness index backing `ResourceAlreadyExistsException` and DescribeProtection-by-ResourceArn.
- Insertion order (or `created_at`) must be stable so ListProtections pagination is deterministic; `NextToken` encodes an offset/last-id into that order.

Out of scope for this first slice (stay 501, filed separately later): protection groups, attacks/DescribeAttack(+Statistics), DRT access, emergency contacts/proactive engagement, health-check association, application-layer automatic response, tagging ops, UpdateSubscription, DeleteSubscription.

## Evidence

`services/service-com-amazonaws-shield-awsshield-20160616/OPERATIONS.md` documents all 36 operations with these inputs/outputs/errors (CreateProtection §51-55, DeleteProtection §69-73, CreateSubscription §63-67, DescribeProtection §112-116, DescribeSubscription §124-128, ListProtections §192-197) and the awsJson1_1 wire protocol (§8-15). `smithy_query shape=Protection` confirms the Protection members (Id, Name, ResourceArn, HealthCheckIds, ProtectionArn, ApplicationLayerAutomaticResponseConfiguration). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-shield-awsshield-20160616/src/ops/` matches all 36 handler files, including `create_protection.rs`, `describe_protection.rs`, `list_protections.rs`, `delete_protection.rs`, `create_subscription.rs`, `describe_subscription.rs` — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSShield_20160616.CreateSubscription and body {} → expect 200 with empty body (currently 501)
2. POST DescribeSubscription {} → expect 200 with a Subscription object reflecting the subscription just created
3. POST CreateProtection {"Name":"my-elb","ResourceArn":"arn:aws:elasticloadbalancing:us-east-1:123456789012:loadbalancer/app/my-elb/abc"} → expect 200 with a 36-char ProtectionId; capture it
4. POST DescribeProtection {"ProtectionId":"<id>"} → expect 200 and Protection.Name == "my-elb", Protection.ResourceArn == the ARN sent, Protection.Id == <id>
5. POST DescribeProtection {"ResourceArn":"arn:aws:elasticloadbalancing:...my-elb/abc"} → expect the same protection; POST DescribeProtection {} → expect InvalidParameterException (400)
6. POST CreateProtection with the same ResourceArn again → expect ResourceAlreadyExistsException (400)
7. Create 3 more protections, then POST ListProtections {"MaxResults":2} → expect 2 Protections plus a NextToken; follow the token until NextToken is absent and expect all 4 ids exactly once
8. POST DeleteProtection {"ProtectionId":"<id>"} → expect 200; then DescribeProtection {"ProtectionId":"<id>"} → expect ResourceNotFoundException (400), and ListProtections no longer contains <id>
9. POST DeleteProtection with the same id again → expect ResourceNotFoundException (400)
