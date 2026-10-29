Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: ab6490681e1ab26d5567ff83a1d290924eb5dab0

# UntagResource advances DateModified for keys the policy never held, so a read reports a last-modified instant at which nothing changed

_Filed: 2026-10-29T18:08:48Z_
_Operation: UntagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ab6490681e1ab26d5567ff83a1d290924eb5dab0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource::handle` removes each requested key and then calls `policy.mark_modified(now)` unconditionally. The handler itself documents that a key the policy does not hold is simply absent afterwards — no change to the record — yet the stamp is written anyway, so `GetLifecyclePolicy` afterwards reports a `DateModified` that moved while every member of the policy, tags included, is byte-for-byte what it was.

That contradicts the definition the crate gives the member where the stamp lives: `LifecyclePolicyRecord::mark_modified` is documented as carrying "the instant of the last change to ANY of it", and the same doc reasons that "a policy nothing changed keeps the instant it had". A `DELETE /tags/{arn}?tagKeys=never-set` changes nothing and still takes a new instant, so the invariant that comment states about the record is broken by the only caller reached on that path. (`TagResource` re-applying an identical key/value is the same no-op write through the same stamp.)

## Evidence

The unconditional stamp after a removal the handler itself calls a no-op — `services/service-com-amazonaws-dlm-dlm-20180112/src/ops/untag_resource.rs:148-156`:
```
        // Removing a key the policy does not hold is not an error: the request
        // asks for those keys to be absent, and afterwards they are.
        for key in &input.tag_keys {
            policy.tags.remove(key);
        }
        // The tags are a member of the policy a read projects, so this call
        // modified the policy and the instant it reports as last modified is
        // this one.
        policy.mark_modified(now);
```
The rule that stamp is documented to implement — `src/state.rs:639-649`:
```
    /// Stamp the instant this policy was last written, which
    /// `LifecyclePolicy$DateModified` reports as "the local date and time when
    /// the lifecycle policy was last modified".
    ///
    /// EVERY accepted write of this record goes through here, whichever member
    /// it changes ... and the instant it reports is therefore the instant of the last change
    /// to ANY of it, not of the last change to one chosen member. A refused
    /// request never reaches it: every refusal is decided before the first
    /// write, so a policy nothing changed keeps the instant it had.
```
For a request naming only keys the policy never held, `policy.tags.remove(key)` changes nothing — there was no change for the new instant to be "the instant of" — yet the stamp is written before the handler returns `UntagResourceOutput {}`.

Test coverage: `tests/all/tag_write_date_modified.rs` pins that a tag write advances `DateModified`; no case in it or in `tests/all/lifecycle_policy_crud.rs` removes a key the policy does not hold and asserts the instant, so the no-op path is unpinned and the fix leaves the pinned (real) tag writes unchanged.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with a valid custom policy body and note the PolicyId and PolicyArn
2. GET /policies/{PolicyId} and record Policy.DateModified
3. DELETE /tags/{PolicyArn}?tagKeys=never-set (a key the policy never held) — HTTP 200, and the handler's own comment at src/ops/untag_resource.rs:148-150 says nothing changed
4. GET /policies/{PolicyId} again and observe Policy.DateModified has advanced while Policy.Tags and every other member are identical — contradicting src/state.rs:639-649, which defines the member as "the instant of the last change to ANY of it"
