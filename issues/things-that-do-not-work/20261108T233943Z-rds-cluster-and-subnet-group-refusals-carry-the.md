Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-cluster-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-cluster-lifecycle rds-refarch-aur-create-cluster-production-settings-refusals rds-global-cluster-lifecycle rds-subnet-group-az-coverage-and-naming
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|ModifyDBCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|RemoveFromGlobalCluster|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|RemoveFromGlobalCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|RemoveFromGlobalCluster|4xx->4xx|status
Issue-Key: rds|ModifyDBSubnetGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Related: 20261102T104537Z-rds-error-envelopes-return-wrong-code-status-and.md
Issue-Commit: 0d0b5d4835067c793fae3be07582601671bbaf82

# RDS cluster and subnet-group refusals carry the wrong error class and message: state faults for missing clusters, not-found for a non-member cluster

> _Note: this issue was identified against the code base at commit `0d0b5d4835067c793fae3be07582601671bbaf82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBCluster

## Description
This bundle carries 4 net-new causes. They share ONE fault: the rds error paths pick the refusal from the handler's own control flow rather than from the condition that actually failed, so the HTTP status, the Error.Code and the Error.Message a client receives are not the ones AWS returns for that condition. Every cause is a wire-visible error contract, which is why this is HIGH even though two of the four match on status.

Cause 1 of 4 — ModifyDBCluster on a cluster that is still creating returns the right class with a placeholder message.
AWS: refusing a modify while the cluster is not available is InvalidDBClusterStateFault, HTTP 400, message "DB cluster isn't available for modification with status creating." — fixed text "DB cluster isn't available for modification with status " plus the cluster's current status from the emulator's state and a trailing period. Smithy: ModifyDBCluster errors include InvalidDBClusterStateFault (httpError 400, awsQueryError code InvalidDBClusterStateFault).
Emulator: 400 and code InvalidDBClusterStateFault (both correct) but message 'This cluster could not be modified. Please try again' — a generic retry string that names neither the status nor the condition.
Why: the message is the only place the status that blocked the call appears; a client that surfaces or matches on it cannot tell a transient failure from 'wait for available', and the suggestion to retry is wrong advice for this condition.
Fix: the ModifyDBCluster handler's state-guard error construction in the rds service crate.
Trace: rds-db-cluster-lifecycle#task#3.

Cause 2 of 4 — ModifyDBCluster on a cluster that does not exist is reported as a state fault instead of not-found.
AWS: 404 with Error.Code DBClusterNotFoundFault and message 'DBCluster not found'. Smithy: ModifyDBCluster errors include DBClusterNotFoundFault with httpError 404 and awsQueryError {code DBClusterNotFoundFault, httpResponseCode 404}; the existence check precedes any state check.
Emulator: 400 with Error.Code InvalidDBClusterStateFault and message 'This cluster could not be modified. Please try again'.
Why: status class and error code are both wrong, so a client's not-found branch never runs — SDK retry/error mapping treats a 400 state fault as a condition that may clear, and idempotent 'modify if present' logic loops or aborts instead of recognising a missing cluster.
Fix: the ModifyDBCluster handler must resolve the cluster first and raise DBClusterNotFoundFault (404) before evaluating state.
Trace: rds-refarch-aur-create-cluster-production-settings-refusals#task#10.

Cause 3 of 4 — RemoveFromGlobalCluster for a cluster ARN that is not a member of the global cluster is reported as not-found instead of an invalid parameter.
AWS: 400 with Error.Code InvalidParameterValue and message 'Cluster <cluster-arn> is not found in global cluster <global-cluster-identifier>' — fixed text 'Cluster ' / ' is not found in global cluster ' with the DbClusterIdentifier from the request and the GlobalClusterIdentifier from the request filling the variable parts. Recorded behavior for the class: RemoveFromGlobalCluster models DBClusterNotFoundFault/GlobalClusterNotFoundFault/InvalidDBClusterStateFault/InvalidGlobalClusterStateFault and the recording shows AWS using the service-common InvalidParameterValue (400) for a well-formed ARN that is not a member.
Emulator: 404 with Error.Code DBClusterNotFoundFault and message 'DBCluster arn:...:cluster:fake not found.'
Why: the emulator reports the cluster as nonexistent when the real condition is 'exists/parses but is not attached to this global cluster'. A client detaching members cannot distinguish a wrong identifier from a non-member, and a 404 pushes it down a resource-missing path AWS never takes here.
Fix: the RemoveFromGlobalCluster handler's membership check — raise the invalid-parameter refusal naming both the cluster and the global cluster, and reserve DBClusterNotFoundFault for an unresolvable cluster.
Trace: rds-global-cluster-lifecycle#task#7.

Cause 4 of 4 — ModifyDBSubnetGroup's invalid-subnet refusal does not carry AWS's message.
AWS: 400 with the modelled InvalidSubnet error (Smithy: ModifyDBSubnetGroup errors include InvalidSubnet, httpError 400, awsQueryError code InvalidSubnet), whose message names the offending subnet id(s) from the request's SubnetIds. The recording shape-asserts the message rather than pinning its text, so the exact wording is recorded behavior only and must be taken from AWS's InvalidSubnet message for this operation.
Emulator: 400 with message 'Some input subnets in :[subnet-00000000000000097] are invalid.' — the replay reports this as not matching the recorded message, i.e. the emulator's template is not AWS's. The subnet id inside it is a value from the request and is not part of the requirement; the fixed text around it is.
Why: callers and tests match on the refusal text to identify which subnet was rejected; a different template breaks that and misreports the condition.
Fix: the ModifyDBSubnetGroup validation path's InvalidSubnet message template in the rds service crate.
Trace: rds-subnet-group-az-coverage-and-naming#task#12.

Related but NOT covering these: open issue 20261029T214632Z-rds-accepts-nine-requests-aws-refuses-missing-request.md owns the CreateDBClusterParameterGroup/ModifyDBClusterParameterGroup 200-instead-of-400 clusters. All four causes above are reported unowned by the replay.

## Evidence
Replay of trace set simparity-wire-aws-v1 (96 fixtures, 977 steps, 483 passed, 494 mismatched).

Cause 1: divergence_key 'service=rds|ModifyDBCluster|status=400->400|keys=$.ErrorResponse.Error.Message'; expected_status 400, actual_status 400; detail '$.ErrorResponse.Error.Message: value_mismatch (expected "DB cluster isn''t available for modification with status creating.", got "This cluster could not be modified. Please try again")'.

Cause 2: divergence_key 'service=rds|ModifyDBCluster|status=404->400|keys=$.ErrorResponse.Error.Code,$.ErrorResponse.Error.Message,status'; expected_status 404, actual_status 400; detail 'status: expected 404, got 400 / Error.Code: value_mismatch (expected DBClusterNotFoundFault, got InvalidDBClusterStateFault) / Error.Message: value_mismatch (expected "DBCluster not found", got "This cluster could not be modified. Please try again")'.

Cause 3: divergence_key 'service=rds|RemoveFromGlobalCluster|status=400->404|keys=$.ErrorResponse.Error.Code,$.ErrorResponse.Error.Message,status'; expected_status 400, actual_status 404; detail 'status: expected 400, got 404 / Error.Code: value_mismatch (expected InvalidParameterValue, got DBClusterNotFoundFault) / Error.Message: value_mismatch (expected "Cluster arn:aws:rds:us-east-1:000000000000:cluster:fake is not found in global cluster <global-cluster-id>", got "DBCluster arn:aws:rds:us-east-1:000000000000:cluster:fake not found.")'.

Cause 4: divergence_key 'service=rds|ModifyDBSubnetGroup|status=400->400|keys=$.ErrorResponse.Error.Message'; expected_status 400, actual_status 400; detail '$.ErrorResponse.Error.Message: value_mismatch (expected <shaped-not-asserted>, got "Some input subnets in :[subnet-00000000000000097] are invalid.")'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay rds-db-cluster-lifecycle step task#3: ModifyDBCluster against a cluster in status creating returns 400 InvalidDBClusterStateFault with message 'This cluster could not be modified. Please try again' instead of "DB cluster isn't available for modification with status creating."
3. Replay rds-refarch-aur-create-cluster-production-settings-refusals step task#10: ModifyDBCluster on a nonexistent cluster returns 400 InvalidDBClusterStateFault instead of 404 DBClusterNotFoundFault / 'DBCluster not found'.
4. Replay rds-global-cluster-lifecycle step task#7: RemoveFromGlobalCluster with a cluster ARN that is not a member returns 404 DBClusterNotFoundFault instead of 400 InvalidParameterValue naming the cluster and the global cluster.
5. Replay rds-subnet-group-az-coverage-and-naming step task#12: ModifyDBSubnetGroup with an invalid subnet returns a message that does not match the recorded InvalidSubnet message.
