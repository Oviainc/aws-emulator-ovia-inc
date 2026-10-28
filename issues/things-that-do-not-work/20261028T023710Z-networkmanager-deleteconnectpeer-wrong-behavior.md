Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# DeleteConnectPeer leaves the peer's association and its telemetry registration behind, and the telemetry entry then becomes unremovable

_Filed: 2026-10-28T02:37:10Z_
_Operation: DeleteConnectPeer | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateConnectPeer` stores two records beside the peer: the association in `connect_peer_associations` (keyed by the peer id) and a telemetry registration in `telemetry_resources` (keyed by global network + the PEER's own ARN). `DisassociateConnectPeer` is written as the counterpart that removes both, and reads the peer's ARN out of the stored peer record to do so.

`DeleteConnectPeer` removes only `connect_peers[id]`. It applies no dependency refusal (unlike DeletePeering, DeleteAttachment, DeleteDevice, DeleteLink, DeleteSite, DeleteCoreNetwork, which all go through `dependents_block_delete`) and cascades nothing (unlike DeleteCoreNetwork, which drops the prefix-list ties it holds). After the delete:
* `GetConnectPeerAssociations` keeps reporting an association for a peer id that `GetConnectPeer` answers 404 for (the listing never joins to `connect_peers`);
* `GetNetworkTelemetry` keeps reporting the BGP session of a peer that no longer exists, contradicting the rule state.rs states for that store;
* the telemetry entry can no longer be removed at all, because the only path that removes it looks the ARN up in the peer record that the delete took away — the premise "a peer whose own record is gone registered nothing left to drop" is false, since the registration was made at ASSOCIATE time under that ARN and survives the peer.

## Evidence

The delete removes the peer and nothing else — src/ops/delete_connect_peer.rs:146-164:
```
// An id naming no stored peer — never created, or already deleted — is not found,
let removed = store.connect_peers.remove(&input.connect_peer_id).ok_or_else(...)?;
Ok(DeleteConnectPeerOutput { connect_peer: Some(removed.to_shape_with_state(STATE_DELETING)) })
```
(no `connect_peer_associations` touch, no `deregister_telemetry_resource`, no `dependents_block_delete`).

The rule it breaks — src/state.rs:4681-4694: "Drop the registration `global_network_id` holds for the connection `resource_arn` ... The counterpart of [`Self::register_telemetry_resource`]: the operation that unties a connection from its global network is what deregisters it, so a global network stops reporting the session health of a peering it no longer holds."

The false premise that makes the leak permanent — src/ops/disassociate_connect_peer.rs:171-177: "The telemetry registration the association made is held under the PEER's own ARN (`AssociateConnectPeer`), so that ARN is read from the stored peer before the association goes; a peer whose own record is gone registered nothing left to drop." followed by `let peer_arn = store.connect_peers.get(&input.connect_peer_id).map(|peer| peer.arn.clone());` and `if let Some(arn) = peer_arn { store.deregister_telemetry_resource(...) }` (lines 174-198).

The association that survives is reported with no join — src/ops/get_connect_peer_associations.rs:202-218 iterates `store.connect_peer_associations` filtered only by `record.global_network_id == input.global_network_id`.

The siblings that do refuse/cascade: src/ops/delete_peering.rs:156-170 (`dependents_block_delete`), src/ops/delete_core_network.rs:178-184 ("the ties go with the record the same way its policy versions do, rather than outliving the core network they were made on").

tests/all/connect_peer_delete.rs covers only an UNASSOCIATED peer (its doc block lists the peer's reader, listing, inventory and tags), so nothing pins this.

## Steps to Reproduce

1. Create a global network, core network with a policy, VPC attachment, Connect attachment, device; CreateConnectPeer on the Connect attachment
2. AssociateConnectPeer(GlobalNetworkId, ConnectPeerId, DeviceId) → 200
3. DeleteConnectPeer(ConnectPeerId) → 200 (no refusal, nothing cascaded)
4. GetConnectPeer(ConnectPeerId) → 404, but GetConnectPeerAssociations on the global network still lists an association for that peer id
5. GetNetworkTelemetry on the global network still reports the connect-peer session for the deleted peer's ARN
6. DisassociateConnectPeer(GlobalNetworkId, ConnectPeerId) → 200 removes the association but leaves the telemetry entry forever: the peer record it reads the ARN from is gone
