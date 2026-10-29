Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9f84f44c6767c5c874b7cc6e16fb6469b8a0f678

# A RequestId already spent on another multiplex's program answers 201 with THAT program and creates nothing in the multiplex the path named

_Filed: 2026-10-28T23:45:26Z_
_Operation: CreateMultiplexProgram | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9f84f44c6767c5c874b7cc6e16fb6469b8a0f678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateMultiplexProgram` is bound to `POST /prod/multiplexes/{MultiplexId}/programs`, so the multiplex is part of the endpoint the token is presented to. Its idempotency replay, however, resolves the token GLOBALLY: `state.multiplex_program_for_token(&input.request_id)` keys the token map with the bare `RequestId` and answers with whatever (multiplex, program) pair that token created, never comparing it with `input.multiplex_id` or `input.program_name`. The handler then returns that program with 201 and skips `insert_multiplex_program` entirely.

So a caller that reuses one `RequestId` string for two different multiplexes gets a 201 describing a program of the FIRST multiplex, while the multiplex the request actually addressed stores nothing: `DescribeMultiplexProgram(M2, P2)` then answers 404 and `DescribeMultiplex(M2).programCount` stays 0, although the create reported success. The response reports a resource the request did not ask to exist, and the write the caller asked for is lost.

This is the one sub-resource create in this crate whose token is not scoped to the parent its path names. The node slice, the channel-placement-group slice and the node-registration-script slice all key the token by (parent, token) AND re-check the parent on the record, each stating that rule in its own doc comment, and the partner-input handler — which has to share a token map with `CreateInput` — filters the replay by the parent the request names for exactly the reason stated here. Only the multiplex-program slice omits it.

Note the SAME-multiplex replay (same token, different programName, one multiplex) is deliberate and pinned by a test; this issue is only about a replay against a DIFFERENT multiplex, which no test covers.

## Evidence

The replay that ignores the multiplex the request named — services/service-com-amazonaws-medialive-medialive/src/ops/create_multiplex_program.rs:172-183:
```
        // A retried create (same `@idempotencyToken` `RequestId`) must answer
        // with the program the first one created, not a second program.
        if let Some((multiplex_id, program_name)) =
            state.multiplex_program_for_token(&input.request_id)
        {
            let program = state
                .multiplex_program_description(&multiplex_id, &program_name)
                .ok_or_else(unavailable)?;
            return Ok(CreateMultiplexProgramOutput {
                multiplex_program: Some(program),
            });
        }
```
The lookup behind it, keyed by the BARE token — src/state/multiplexes.rs:1698-1718:
```
    pub fn record_multiplex_program_token(
        &mut self,
        token: String,
        multiplex_id: &str,
        program_name: &str,
    ) {
        self.multiplex_program_tokens.record(
            token,
            &multiplex_program_key(multiplex_id, program_name),
            MAX_MULTIPLEX_PROGRAM_TOKENS,
        );
    }
...
    pub fn multiplex_program_for_token(&self, request_id: &str) -> Option<(String, String)> {
        let key = self.multiplex_program_tokens.created(request_id)?;
        let (multiplex_id, program_name) = key.split_once(MULTIPLEX_PROGRAM_KEY_SEPARATOR)?;
```
(the multiplex id is the stored VALUE, not part of the key — src/state.rs:2360-2365: "`CreateMultiplexProgram`'s `RequestId` (`@idempotencyToken`) -> the program it created, keyed by the multiplex slice's `multiplex_program_key` because a program is named by its multiplex and its own name rather than by an id.")

The rule the crate states and implements for the sibling sub-resource creates — src/state/nodes.rs:514-530:
```
    pub fn record_node_token(&mut self, cluster_id: &str, token: String, node_id: &str) {
        self.node_tokens
            .record(node_token_key(cluster_id, &token), node_id, MAX_NODE_TOKENS);
    }

    /// The node a stored `RequestId` created in `cluster_id`, if that node still
    /// exists in it.
    ///
    /// The cluster is part of the key AND re-checked on the record: a token
    /// presented to one cluster's endpoint says nothing about another's.
```
and src/state/channel_placement_groups.rs:797-811:
```
    /// The group a `RequestId` presented for `cluster_id` created, if that group
    /// still exists in that cluster.
    ///
    /// The cluster is part of the key AND re-checked on the record, so a token
    /// can only ever surface a group of the cluster the request named.
```
and the same guard hand-written where the token map is shared — src/ops/create_partner_input.rs:166-183:
```
        // A retried create (same `@idempotencyToken` `RequestId`) must return the
        // partner input the first one created, not a second one. The token map is
        // the input store's, shared with `CreateInput`, so a replay counts only
        // when the input that token created is a partner OF THE INPUT THIS
        // REQUEST NAMES: a token spent on some other create names an input this
        // call was not asked for, and answering with it would report a resource
        // the request did not ask to exist.
```
Test coverage: the cross-parent row EXISTS for nodes and is asserted to mint two resources — tests/all/node_lifecycle.rs:615-650 (`a_retried_create_returns_the_node_the_first_one_created`, "because a node's id is unique in its cluster and a token is idempotent for the endpoint it was presented to", row `("the same token, two clusters", (0, Some("token-a")), (1, Some("token-a")), false)`). The multiplex-program suite has NO such row: tests/all/multiplex_programs.rs:583-594 runs all three of its rows against the single `multiplex_id` created at line 598, so nothing pins the cross-multiplex replay, while lines 626-642 pin the same-multiplex replay this issue leaves untouched.

## Steps to Reproduce

1. POST /prod/multiplexes with a name, two availabilityZones and valid multiplexSettings; note the id M1. Repeat for a second multiplex M2.
2. POST /prod/multiplexes/M1/programs with {"programName":"P1","requestId":"token-a","multiplexProgramSettings":{"programNumber":1}} -> 201, program P1 created in M1.
3. POST /prod/multiplexes/M2/programs with {"programName":"P2","requestId":"token-a","multiplexProgramSettings":{"programNumber":2}} -> the emulator answers 201 whose body is M1's program P1 (programName "P1", programNumber 1).
4. GET /prod/multiplexes/M2/programs/P2 -> 404: the create that answered 201 stored nothing in M2.
5. GET /prod/multiplexes/M2 -> programCount 0, while GET /prod/multiplexes/M1 -> programCount 1.
6. Compare POST /prod/clusters/C1/nodes and POST /prod/clusters/C2/nodes with one shared requestId: two distinct nodes are created (pinned by tests/all/node_lifecycle.rs:645-650), because that slice keys the token by (cluster, token).
