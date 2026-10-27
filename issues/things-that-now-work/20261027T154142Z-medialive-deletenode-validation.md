Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2334cad17af0cf812822af0fde730e954dc43dac

# DeleteNode deregisters a DRAINING node, the one state its own handler says ConflictException refuses

_Filed: 2026-10-27T15:41:42Z_
_Operation: DeleteNode | Class: validation_

> _Note: this issue was identified against the code base at commit `2334cad17af0cf812822af0fde730e954dc43dac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteNode` never looks at the node's state at all: it calls `deregister_node_description`, which calls `begin_deregister` unconditionally, so every node the cluster/node pair resolves to is accepted with 202 `DEREGISTERING`.

The handler is aware of the documented precondition and explicitly reasons about it, concluding that the modelled `ConflictException` is the answer for a node that is `IN_USE` or `DRAINING`, and that the check can be skipped because "No node in this emulator can be in either state". That premise is false in this very crate: `UpdateNodeState`'s `State` member targets `UpdateNodeStateShape`, whose values are `ACTIVE` and `DRAINING`, and `apply_node_state` -> `set_requested_state` stores the requested value verbatim as the node's `NodeState`. So a caller can put a node into `DRAINING` with one call and then delete it with the next, and the refusal the handler says belongs to that state is never reached.

The sibling delete in the same family does enforce its identically-worded precondition: `DeleteCluster` ("Delete a Cluster. The Cluster must be idle.") is refused through `ClusterDeleteRefusal::NotIdle` with the whole check taken before the first write, so the two deletes of the MediaLive Anywhere family disagree about whether a documented idle precondition is enforced at all.

## Evidence

Model (smithy_query query="doc" shape="DeleteNode"):
  "Delete a Node. The Node must be IDLE."
Model (smithy_query query="shape" shape="UpdateNodeStateShape"): values ["ACTIVE", "DRAINING"].

Handler comment, services/service-com-amazonaws-medialive-medialive/src/ops/delete_node.rs:204-212:
  "// \"The Node must be IDLE\" is the documented precondition, and the
   // modelled `ConflictException` is what refuses a node that is not: a node
   // the real service is RUNNING a channel on (`IN_USE`, `DRAINING`). No
   // node in this emulator can be in either state ... so that refusal
   // belongs to the change that makes a node run something, and claiming it
   // now would refuse a delete no stored state justifies."
and the code under it, delete_node.rs:217-225, resolves the pair and nothing else:
  "let node = state\n            .deregister_node_description(&input.cluster_id, &input.node_id, now)" — no state check anywhere in `handle`.

The contradicting writer, services/service-com-amazonaws-medialive-medialive/src/state/nodes.rs:363-366:
  "fn set_requested_state(&mut self, state: &str) {\n        self.state = state.to_string();\n        self.transition = None;\n    }"
with its own doc, nodes.rs:604-607:
  "A value it does carry is one of `UpdateNodeStateShape`'s ... and is stored verbatim because every value that shape accepts is a `NodeState` the node can be in."

Sibling that DOES enforce the same sentence, services/service-com-amazonaws-medialive-medialive/src/state/clusters.rs:796-821:
  "/// \"Delete a Cluster. The Cluster must be idle\": a cluster that still holds
   /// any of the resources that live inside one ... is refused"
  "if !resources.is_empty() { return Err(ClusterDeleteRefusal::NotIdle { resources }); }"

Test coverage: services/service-com-amazonaws-medialive-medialive/tests/all/node_lifecycle.rs:38 declares `const REQUESTABLE_STATES: [&str; 2] = [node_state::ACTIVE, node_state::DRAINING];` and exercises both through `UpdateNodeState`, but no test in the suite deletes a node that has been put into `DRAINING` — `a_deleted_node_deregisters_and_settles_into_deregistered` (node_lifecycle.rs:472) deletes a freshly created node, so nothing pins the current acceptance.

## Steps to Reproduce

1. POST /prod/clusters {"name":"c","clusterType":"ON_PREMISES"} -> note the cluster Id
2. POST /prod/clusters/{ClusterId}/nodes {"name":"n"} -> note the node Id
3. PUT /prod/clusters/{ClusterId}/nodes/{NodeId}/state {"state":"DRAINING"} -> 201, node reports state DRAINING
4. GET /prod/clusters/{ClusterId}/nodes/{NodeId} -> confirms "state":"DRAINING"
5. DELETE /prod/clusters/{ClusterId}/nodes/{NodeId} -> observed: 202 with "state":"DEREGISTERING"; expected per the model ("The Node must be IDLE.") and per delete_node.rs:204-212's own rule: 409 ConflictException naming the DRAINING state, writing nothing
