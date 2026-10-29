Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8031858802602324d4d3567159831ce5c3f80459

# AssociateUser stamps the association and materialises the clock from an instant read BEFORE the cross-service EC2 hop

_Filed: 2026-10-29T09:03:49Z_
_Operation: AssociateUser | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8031858802602324d4d3567159831ce5c3f80459`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateUser` reads the wall clock ONCE, before it awaits the instance-authorization gate (which reaches Amazon EC2 over the in-process callback), and then reuses that pre-hop instant for three things after the hop returns: `state.materialise_due(now)`, the record's `association_date`, and the record's `associated_due_at` (the `ASSOCIATING -> ASSOCIATED` transition the clock owes).

Every sibling in this crate reads the clock AFTER its hop, at the instant the store is locked/written, and `UpdateIdentityProviderSettings` states that as the rule in a comment. Consequences of the outlier in `AssociateUser`:

* the sweep of clock-owed transitions runs against a stale instant, so a teardown that fell due while the hop was in flight is not materialised by this request, even though the handler then writes to the store under the lock;
* `AssociationDate` reports an instant earlier than the moment the association was actually stored;
* `associated_due_at` is measured from before the association existed, so with a slow hop the association can be stored already due for `ASSOCIATED` — which defeats the stated purpose of `ASSOCIATION_SPAN` ("so a caller that reads straight back still observes the transient status").

No comment in the handler gives a reason for reading the clock early (the only comment about the gate's placement is about the store lock, not the clock), and no test pins the stale instant.

## Evidence

The outlier, `services/.../src/ops/associate_user.rs:188-208` — the clock is read before the awaited gate and then used after it:
```
188:        let now = std::time::SystemTime::now();
...
200:        crate::state::ec2_fleet::authorize_instance(
201:            &*self.engine,
202:            ctx,
203:            Some(input.instance_id.as_str()),
204:        )
205:        .await?;
206:
207:        let mut state = state.lock();
208:        state.materialise_due(now);
```
and the same stale instant is what the record persists, `associate_user.rs:289-291`:
```
289:            association_date: Some(crate::state::iso8601(now)),
...
291:            associated_due_at: crate::state::association_due_at(now),
```

The rule this crate states for exactly this situation, `services/.../src/ops/update_identity_provider_settings.rs:233-236`:
```
233:        let mut state = state.lock();
234:        // The hop took time, so what the clock owes is materialised against the instant the
235:        // write happens, exactly as the read before the hop did for its own instant.
236:        state.materialise_due(std::time::SystemTime::now());
```

The sibling on the SAME plane behind the SAME gate reads the clock after the hop, `services/.../src/ops/disassociate_user.rs:195-217`:
```
195:        crate::state::ec2_fleet::authorize_instance(
...
200:        .await;
...
215:        let now = std::time::SystemTime::now();
216:        let mut state = state.lock();
217:        state.materialise_due(now);
```
and so do `list_user_associations.rs:171-182` (`state.materialise_due(std::time::SystemTime::now());` after the awaited gate) and `register_identity_provider.rs:212-226` (`let now = std::time::SystemTime::now();` placed after `resolve_settings_network`/`resolve_domain_subnets`).

Why the stale due instant matters, `services/.../src/state.rs:152-158`:
```
152: /// How long after `AssociateUser` the association becomes `ASSOCIATED`.
...
156: /// status, and the span is inside the emulator's 250-750 ms window so a caller that
157: /// reads straight back still observes the transient status.
158: const ASSOCIATION_SPAN: Duration = Duration::from_millis(500);
```
That the hop is a real, time-consuming cross-service call is this crate's own description, `services/.../src/state/ec2_fleet.rs:118-131` (`authorizes_instance` ends in `ec2_holds_instance(ctx, &region, instance_id).await`, a `DescribeInstances` over the in-process callback).

Test coverage: `tests/all/user_associations.rs:794-796` and `tests/all/resource_resolution_refusals.rs:885-888` only assert that `AssociationDate` is the stored ISO-8601 instant / is well formed — no test pins it to a pre-hop reading, so taking the clock at the lock would leave every existing test unchanged.

## Steps to Reproduce

1. Read `services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/src/ops/associate_user.rs:188`: the handler captures `now` before `crate::state::ec2_fleet::authorize_instance(..).await?` on line 205.
2. Observe that the same `now` is used after the await for `state.materialise_due(now)` (line 208), `association_date` (line 289) and `associated_due_at` (line 291).
3. Compare with `src/ops/disassociate_user.rs:215` and `src/ops/list_user_associations.rs:182`, which read the clock AFTER the same gate, and with the rule stated at `src/ops/update_identity_provider_settings.rs:234-236`.
4. Call `AssociateUser` while the EC2 hop is slow (any delay >= `ASSOCIATION_SPAN`, `src/state.rs:158`): the association is persisted with `associated_due_at` already in the past, so the very next read materialises `ASSOCIATED` and the transient `ASSOCIATING` status the span exists for is never observable; the `AssociationDate` reported is also earlier than the instant the record was written.
