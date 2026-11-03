Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs.ecsnrt-run-task-count-above-ten-rejected-before-the-empty-cluster-capacity-check ec2__ecs.tgwc-awsvpc-service-with-six-security-groups-rejected
Issue-Key: ec2__ecs.ecsnrt-run-task-count-above-ten-rejected-before-the-empty-cluster-capacity-check|run#10|AssertionError|assert got == _COUNT, f"count 11 should be refused for the count before capacity is considered, got {got!r}"
Issue-Key: ec2__ecs.tgwc-awsvpc-service-with-six-security-groups-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# ECS: RunTask's count-limit refusal is emulator-authored prose, and an awsvpc service accepts more security groups than AWS allows (2 root causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Two net-new root causes replayed under the `ec2__ecs` suite heading.

Cause 1 of 2: RunTask's count-limit refusal message is the emulator's own sentence.
AWS: recorded behavior only. RunTask with count above the per-call maximum of 10 is refused InvalidParameterException, and the count check runs BEFORE any capacity consideration (so an empty cluster still yields the count error, not a capacity error). The code the emulator returns is correct; the message is AWS's own count-limit text with the submitted count and the limit as its variable parts.
Emulator: returns InvalidParameterException with `count must be at most 10` - a terse emulator-authored sentence rather than AWS's wording.
Why: the check asserts the refusal AWS words for this class. A client or operator matching AWS's message - to tell a count violation from the capacity refusal that the same operation also produces - does not match, and the message is the only thing distinguishing the two on this code.
Fix: the count validation message template in the RunTask handler (keep the ordering: count before capacity).
Traces: ec2__ecs.ecsnrt-run-task-count-above-ten-rejected-before-the-empty-cluster-capacity-check

Cause 2 of 2: an awsvpc service is created with more security groups than AWS permits.
AWS: recorded behavior only. awsvpcConfiguration.securityGroups accepts at most five entries; a sixth is refused and no service is created.
Emulator: the call SUCCEEDS with six security groups.
Why: the limit is an ENI constraint, so a service the emulator creates cannot have its task ENIs attached on AWS. The deployment is reported healthy here and fails at first task placement in production - and a client relying on the refusal to prune its group list never prunes it.
Fix: the awsvpcConfiguration.securityGroups length validation in the CreateService (and UpdateService) handler.
Traces: ec2__ecs.tgwc-awsvpc-service-with-six-security-groups-rejected

## Evidence
Replayed failing checks under `### ec2__ecs` (neither marked ALREADY FILED / OWNED by):

1. ec2__ecs.ecsnrt-run-task-count-above-ten-rejected-before-the-empty-cluster-capacity-check -> AssertionError: count 11 should be refused for the count before capacity is considered, got ('InvalidParameterException', 'count must be at most 10')
2. ec2__ecs.tgwc-awsvpc-service-with-six-security-groups-rejected -> AssertionError: the call succeeded; real AWS rejects it, so there is no error code

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; both checks appear under `### ec2__ecs`, unmarked.
2. Standalone repros:
   - CreateCluster (leave it with no registered capacity), RegisterTaskDefinition, then RunTask(cluster=<cluster>, taskDefinition=<td>, count=11): expect InvalidParameterException whose message is AWS's count-limit text, raised ahead of any capacity error.
   - CreateService with networkMode awsvpc and networkConfiguration.awsvpcConfiguration.securityGroups holding six group ids: expect a refusal and no service.
