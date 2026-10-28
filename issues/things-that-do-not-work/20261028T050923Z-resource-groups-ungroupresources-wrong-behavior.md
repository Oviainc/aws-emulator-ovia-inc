Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d9dd770f9e2574e0600226bd25d044e51daa0c38

# UngroupResources reports resources it did not remove in Succeeded

_Filed: 2026-10-28T05:09:23Z_
_Operation: UngroupResources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d9dd770f9e2574e0600226bd25d044e51daa0c38`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler computes `Succeeded` from the group's RESULTING state rather than from what the call actually did: every requested ARN that the group does not hold after the call is reported as succeeded, including an ARN the group never held and every ARN of a repeated call that removed nothing. The handler's own comment states that for such an ARN "no action is recorded against it because none was performed" — yet the same ARN is still placed in a list the model documents as the resources this operation removed, while `Failed` is hard-coded empty. The result is a response that claims a removal the operation did not perform.

Note the sibling tagging path in this same crate decides the symmetric question the other way: `Untag` reports only the keys the stored map actually LOST, so a key the group did not carry is reported in nothing. The two operations answer the same "what did this call actually change?" question with opposite rules even though their output members are documented with the same phrasing.

## Evidence

Smithy model (the sentence that itself states the behaviour), `smithy_query(query="doc",shape="UngroupResourcesOutput",member="Succeeded")`:
  "A list of resources that were successfully removed from the group by this operation."
and `smithy_query(query="doc",shape="UngroupResourcesOutput",member="Failed")`:
  "A list of any resources that failed to be removed from the group by this operation."

Handler, services/service-com-amazonaws-resourcegroups-ardi/src/ops/ungroup_resources.rs:249-259:
```
        // Reported from the memberships the group now holds, not from the request:
        // an ARN appears in `Succeeded` because the group no longer holds it, which
        // makes a removal IDEMPOTENT — an ARN that was never a member is already in
        // the state the request asked for, so it succeeds too, and no action is
        // recorded against it because none was performed.
        let succeeded: Vec<String> = input
            .resource_arns
            .iter()
            .filter(|arn| !group.holds_member(arn))
            .cloned()
            .collect();
```
and ungroup_resources.rs:280-286, where nothing can ever be reported as not-removed:
```
        Ok(UngroupResourcesOutput {
            succeeded: Some(succeeded),
            // Nothing was refused, and a static membership this emulator removes is
            // dropped immediately — so neither list has an entry.
            failed: Some(Vec::new()),
```

Sibling rule in the same crate, services/service-com-amazonaws-resourcegroups-ardi/src/ops/untag.rs:194-197:
```
        // The removal lives on the record: the keys that come back are the ones the
        // stored map actually LOST, so a key the group did not carry is reported in
        // nothing and an empty batch is a no-op success.
        let removed = record.remove_tags(input.keys.iter().map(String::as_str));
```

Test coverage that pins the current behaviour (and would need updating): services/service-com-amazonaws-resourcegroups-ardi/tests/all/ungroup_resources.rs:230-234 asserts "expected every named ARN in Succeeded" for both the "first call" and the "repeated call" round (tests/all/ungroup_resources.rs:218), and tests/all/ungroup_resources.rs:316 sends `NEVER_A_MEMBER` beside a real member. The crate itself records no grouping status for that ARN (tests/all/ungroup_resources.rs:335-341: "the resource the group never held has none"), which is the same admission that nothing was removed.

## Steps to Reproduce

1. CreateGroup Name=static-group with Configuration=[{"Type":"AWS::EC2::HostManagement"}] so the group serves static membership.
2. GroupResources Group=static-group ResourceArns=['arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0'].
3. UngroupResources Group=static-group ResourceArns=['arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0','arn:aws:ec2:us-east-1:000000000000:volume/vol-00000000000000000'] — the volume was never a member.
4. Observe the response: Succeeded contains BOTH ARNs and Failed is empty, i.e. the volume is reported as "successfully removed from the group by this operation" although the handler performed no removal for it (ListGroupingStatuses reports no record for the volume, proving no action was taken).
5. Repeat the same UngroupResources call: Succeeded again lists both ARNs though the call removed nothing at all.
