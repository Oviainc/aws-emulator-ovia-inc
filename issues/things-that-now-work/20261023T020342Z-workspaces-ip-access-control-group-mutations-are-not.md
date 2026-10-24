Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-ip-group-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-ip-group-lifecycle
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces|DescribeIpGroups|2xx->2xx|$.Result[groupId="wsipg-00000001"].userRules[*]
Issue-Key: workspaces|DescribeIpGroups|2xx->2xx|$.Result[groupId="wsipg-00000001"].userRules[*].ipRule
Issue-Key: workspaces|DescribeIpGroups|2xx->2xx|$.Result[groupId="wsipg-00000001"].userRules[*].ruleDesc
Issue-Key: workspaces|DescribeIpGroups|2xx->2xx|$.Result[groupId="wsipg-00000001"].userRules
Issue-Key: workspaces|DescribeIpGroups|2xx->2xx|$.Result[*]
Issue-Commit: 9aaeda1848c43b0bda5e2e66a82e857bd94691b3

# WorkSpaces IP access control group mutations are not applied to stored state

> _Note: this issue was identified against the code base at commit `9aaeda1848c43b0bda5e2e66a82e857bd94691b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateRulesOfIpGroup

## Description
The emulator accepts WorkSpaces IP-access-control-group mutations (UpdateRulesOfIpGroup, DeleteIpGroup) with a success response but never applies them to the stored group, so every subsequent DescribeIpGroups keeps returning the group exactly as it was at CreateIpGroup time. Four replayed divergences, all on DescribeIpGroups read-backs, reduce to two causes.

Cause 1 of 2: UpdateRulesOfIpGroup is a no-op on state (fixture workspaces-ip-group-lifecycle, steps task#4, task#6 and task#8).
AWS: Smithy - UpdateRulesOfIpGroup(GroupId @required @pattern ^wsipg-[0-9a-z]{8,63}$, UserRules @required IpRuleList) returns an empty output and REPLACES the group's rule set; WorkspacesIpGroup.userRules (IpRuleItem{ipRule, ruleDesc}) is the rule list a following DescribeIpGroups must report. Replacement semantics are what the recording shows: after an update the read-back contains exactly the submitted rules - a superset/added rule (task#4), a wholly different single rule (task#6), and an empty rule set where userRules is then absent from the group (task#8).
Emulator: every one of those read-backs still returns the group's original creation-time rule list. task#4 expected userRules[0] = {ipRule 192.168.1.0/24, ruleDesc 'vpn range'} plus a second entry and got only the original {10.0.0.0/16, 'initial office range'}; task#6 expected {172.16.0.0/12, 'replaced range'} and got the original entry; task#8 expected userRules absent and got the original one-entry list (type_mismatch, expected None).
Why: a client that authorizes a CIDR range, or revokes one, is told it succeeded while the group keeps enforcing the old rules. Code that updates then reads back to confirm sees its change silently discarded, and a revocation looks applied when it is not - a correctness problem a security-group-style workflow depends on.
Fix: the UpdateRulesOfIpGroup handler must write the submitted UserRules into the group's state, replacing (not merging and not ignoring) the previous list, including the empty-list case which must leave userRules unset on the stored group.

Cause 2 of 2: DeleteIpGroup does not remove the group (fixture workspaces-ip-group-lifecycle, step teardown#10).
AWS: Smithy - DeleteIpGroup(GroupId @required) returns an empty output and deletes the group; a subsequent DescribeIpGroups with no GroupIds must not list it (it would return ResourceNotFoundException if asked for it by id). Recorded behavior: the post-delete DescribeIpGroups Result list is empty.
Emulator: the deleted group is still present in Result[0] (groupId wsipg-00000001, with its creation-time groupName/groupDesc/userRules), i.e. an extra element where the recording has none.
Why: deletion is not observable, so cleanup/teardown logic and any 'does this group still exist' check get the wrong answer, and group-count/limit behavior drifts upward over a session.
Fix: the DeleteIpGroup handler must remove the group from the service state so DescribeIpGroups stops listing it.

Traces: workspaces-ip-group-lifecycle#task#4, workspaces-ip-group-lifecycle#task#6, workspaces-ip-group-lifecycle#task#8, workspaces-ip-group-lifecycle#teardown#10 (all DescribeIpGroups, status 200 vs 200, body mismatch).

## Evidence
Replay of trace set simparity-wire-aws-v1 (4 mismatch clusters, service workspaces, all DescribeIpGroups, expected 200 / actual 200):
- workspaces-ip-group-lifecycle#task#4: $.Result[groupId="wsipg-00000001"].userRules[0].ipRule value_mismatch (expected '192.168.1.0/24', got '10.0.0.0/16'); .ruleDesc (expected 'vpn range', got 'initial office range'); userRules[1] missing (expected {'ipRule': '10.0.0.0/16', 'ruleDesc': 'initial office range'}, got None).
- workspaces-ip-group-lifecycle#task#6: userRules[0].ipRule value_mismatch (expected '172.16.0.0/12', got '10.0.0.0/16'); .ruleDesc (expected 'replaced range', got 'initial office range').
- workspaces-ip-group-lifecycle#task#8: userRules type_mismatch (expected None, got [{'ipRule': '10.0.0.0/16', 'ruleDesc': 'initial office range'}]).
- workspaces-ip-group-lifecycle#teardown#10: $.Result[0] extra (expected None, got the group wsipg-00000001 with its creation-time rules).
In all four the emulator returns the group's creation-time snapshot, which is the signature of UpdateRulesOfIpGroup and DeleteIpGroup not mutating state. Model grounding: UpdateRulesOfIpGroup.UserRules and DeleteIpGroup.GroupId are both @required; both operations have empty outputs, so DescribeIpGroups is the only observable effect.

## Steps to Reproduce
1. Replay fixture workspaces-ip-group-lifecycle against the emulator (replay_wire_corpus).
2. CreateIpGroup with one rule (10.0.0.0/16 'initial office range').
3. UpdateRulesOfIpGroup with a different rule set, then DescribeIpGroups -> step task#4/task#6 show the original rules instead of the submitted ones.
4. UpdateRulesOfIpGroup with an empty UserRules list, then DescribeIpGroups -> step task#8 still reports the original rule.
5. DeleteIpGroup, then DescribeIpGroups with no GroupIds -> step teardown#10 still lists the deleted group.
