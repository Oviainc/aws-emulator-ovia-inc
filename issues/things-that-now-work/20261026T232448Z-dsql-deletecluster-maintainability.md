Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# The recorded-delete answer codec is implemented twice, once in DeleteCluster and once in DeleteStream

_Filed: 2026-10-26T23:24:48Z_
_Operation: DeleteCluster | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule for how a delete's recorded result is written to and read back from the idempotency ledger — the three answered members (`arn`, `status`, `creationTime` through `encode_instant`/`decode_instant`), and the decision that a record missing any one of them is served as an InternalServerException rather than as an answer — is implemented twice, once per delete handler. The two copies are structurally identical and differ only in the output type they build and in where the cluster identifier comes from, so a later correction to one (for example to how a missing member is handled, or to the instant encoding) can silently leave the other behind. Everything else of this kind in the crate is already centralised once in `state.rs` (`PageBounds` for the page walk, `Cluster::mutation_conflict` for the status gate, `validate_tag_keys`, `cluster_policy_version_message`), and the member-name constants these two handlers share already live there — their doc comments even name only DeleteCluster, which is the drift starting. A single recorded-delete encode/decode helper in `state.rs` beside those constants would leave every response and every existing test unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-dsql-dsql/src/ops/delete_cluster.rs:340-351: "fn recorded_delete_output(recorded: &crate::state::RecordedCall) -> Option<DeleteClusterOutput> { Some(DeleteClusterOutput { identifier: recorded.resource.clone(), arn: recorded.answered(crate::state::ARN_MEMBER)?.to_string(), status: recorded.answered(crate::state::STATUS_MEMBER)?.to_string(), creation_time: ::aws_smithy_types::DateTime::from(crate::state::decode_instant(recorded.answered(crate::state::CREATION_TIME_MEMBER)?)?), }) }" with the encode side at delete_cluster.rs:301-311 (`.answering(STATUS_MEMBER, ..).answering(ARN_MEMBER, ..).answering(CREATION_TIME_MEMBER, &crate::state::encode_instant(creation_time))`).

Copy 2 — services/service-com-amazonaws-dsql-dsql/src/ops/delete_stream.rs:333-346: "fn recorded_delete_output(cluster_identifier: &str, recorded: &crate::state::RecordedCall) -> Option<DeleteStreamOutput> { Some(DeleteStreamOutput { cluster_identifier: cluster_identifier.to_string(), stream_identifier: recorded.resource.clone(), arn: recorded.answered(crate::state::ARN_MEMBER)?.to_string(), status: recorded.answered(crate::state::STATUS_MEMBER)?.to_string(), creation_time: ::aws_smithy_types::DateTime::from(crate::state::decode_instant(recorded.answered(crate::state::CREATION_TIME_MEMBER)?)?), }) }" with the identical encode side at delete_stream.rs:293-303.

The shared constants already sit in one place but document only one of the two callers: services/service-com-amazonaws-dsql-dsql/src/state.rs:1786-1796 — "The name a cluster's ARN member carries on the wire (`smithy_query(query=\"operation\", operation=\"DeleteCluster\")`: a `@required` `arn`) … The name a cluster's creation-time member carries on the wire (`smithy_query(query=\"operation\", operation=\"DeleteCluster\")` …)" — while delete_stream.rs uses both for a STREAM.

Tests that pin the behaviour the fix must not change: services/service-com-amazonaws-dsql-dsql/tests/all/delete_cluster_idempotency.rs::a_repeated_delete_answers_the_recorded_result and ::every_served_delete_is_recorded_under_its_own_token (both compare the retry's whole four-member answer with the original's), and services/service-com-amazonaws-dsql-dsql/tests/all/stream_deletion.rs::a_repeated_client_token_answers_the_recorded_delete (line 308).

## Steps to Reproduce

1. Read services/service-com-amazonaws-dsql-dsql/src/ops/delete_cluster.rs:293-311 and :333-351.
2. Read services/service-com-amazonaws-dsql-dsql/src/ops/delete_stream.rs:287-303 and :325-346.
3. Observe the same encode/decode rule for a recorded delete (same three member keys, same encode_instant/decode_instant round trip, same 'a record missing a member is an InternalServerException, never an answer' decision) written out twice, so a change to one copy cannot reach the other.
4. Confirm the crate already centralises comparable rules in state.rs (PageBounds, Cluster::mutation_conflict, validate_tag_keys) and already holds the member-name constants these two copies share.
