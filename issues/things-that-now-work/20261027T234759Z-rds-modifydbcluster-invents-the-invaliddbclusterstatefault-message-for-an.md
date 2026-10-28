Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.concurrent-modify-db-cluster-conflicting-state-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.concurrent-modify-db-cluster-conflicting-state-rejected|run#16|AssertionError|assert all(error == expected_refusal for error in refused), (
Issue-Commit: 10facd235b75ceff4fb05feb7e3df512bc30c659

# RDS ModifyDBCluster invents the InvalidDBClusterStateFault message for an overlapping concurrent modify (1 cause)

> _Note: this issue was identified against the code base at commit `10facd235b75ceff4fb05feb7e3df512bc30c659`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBCluster

## Description
One net-new root cause. The already-open issues 'RDS state and validation fidelity: CA-certificate modification, ...' (20261024T030130Z-rds-state-and-validation-fidelity-ca-certificate-modification.md) and 'RDS omits state and existence preconditions and misclassifies the resulting errors' (20261022T162132Z-rds-omits-state-and-existence-preconditions-and-misclassifies.md) cover other RDS operations/members and do NOT cover ModifyDBCluster's concurrency refusal message; this one does.

Cause 1 of 1: when two ModifyDBCluster calls overlap on one DB cluster, the emulator refuses the losing racer with the correct error code but with its own invented message text instead of the message AWS words for that refusal.
AWS: Smithy: ModifyDBCluster models InvalidDBClusterStateFault (errorKind client, httpError 400, awsQueryError code InvalidDBClusterStateFault, doc 'The requested operation can't be performed while the cluster is in this state.'). Recorded behavior: when a ModifyDBCluster arrives while another ModifyDBCluster on the same cluster is still in flight (cluster status 'modifying'), RDS refuses it with HTTP 400, Error.Code 'InvalidDBClusterStateFault' and the message 'This cluster could not be modified. Please try again'. That message is entirely FIXED text for this error class — it has no variable parts: no cluster identifier, no status name and no value from the request or from state is interpolated into it. The model states only the code/status; the message wording above is recorded behavior.
Emulator: returns the right code and status but the message 'DB cluster <DBClusterIdentifier> is in state <status> and cannot be modified' — in the replay, ('InvalidDBClusterStateFault', 'DB cluster sim2real-modrace-64b4d47aa693 is in state modifying and cannot be modified'), where the cluster id and the word 'modifying' are values the emulator substituted from the request and its own state. Neither the fixed text nor the templating matches AWS: AWS emits no identifier and no status in this message at all.
Why: a client (or a test/parity harness) that matches on the refusal text to tell an expected concurrency collision apart from a genuine bad-state refusal (for example 'cluster is being deleted') takes the wrong branch, and a client that surfaces the service message to a user shows text AWS never emits; the retry hint 'Please try again', which is the only part of AWS's message telling the caller the call is retryable, is absent.
Fix: the ModifyDBCluster handler's concurrency/state precondition in the RDS crate — the message string it attaches to InvalidDBClusterStateFault for the overlapping-modify case must be AWS's fixed text, with no identifier or status interpolated.
Traces: rds.concurrent-modify-db-cluster-conflicting-state-rejected.

## Evidence
rds.concurrent-modify-db-cluster-conflicting-state-rejected — AssertionError: "a refused racer must get ('InvalidDBClusterStateFault', 'This cluster could not be modified. Please try again'), got [('InvalidDBClusterStateFault', 'DB cluster sim2real-modrace-64b4d47aa693 is in state modifying and cannot be modified')]". Error.Code and HTTP status (400) agree; only Error.Message diverges. Expected message: the fixed text 'This cluster could not be modified. Please try again' (no variable parts). Actual message: 'DB cluster <id> is in state <status> and cannot be modified', with <id> taken from the request's DBClusterIdentifier and <status> from the emulator's cluster state.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Action=CreateDBCluster with DBClusterIdentifier=<id>, Engine=aurora-postgresql, MasterUsername/MasterUserPassword set and ServerlessV2ScalingConfiguration {MinCapacity 0.5, MaxCapacity 1}; wait until DescribeDBClusters reports Status 'available' with no PendingModifiedValues.
3. Fire two ModifyDBCluster calls on that same DBClusterIdentifier simultaneously (thread barrier), one with BackupRetentionPeriod=3 and one with BackupRetentionPeriod=9, both ApplyImmediately=true.
4. Read the Error.Code and Error.Message of whichever racer is refused: the code is InvalidDBClusterStateFault (correct) but the message is 'DB cluster <id> is in state modifying and cannot be modified' instead of AWS's 'This cluster could not be modified. Please try again'.
