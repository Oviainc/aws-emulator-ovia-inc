Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-import-invalid-host-key-then-list-unchanged
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-import-invalid-host-key-then-list-unchanged
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|ListHostKeys|2xx->2xx|$.HostKeys[Arn="arn:aws:transfer:us-west-2:000000000000:host-key/s-00000000000000001/hostkey-00000000000000002"].Type
Issue-Commit: 97f4305ce8b2bb7c5ca6e1516bb663233e2cbad1

# Transfer ListHostKeys reports Type ssh-ed25519 for an imported RSA host key after a rejected import

> _Note: this issue was identified against the code base at commit `97f4305ce8b2bb7c5ca6e1516bb663233e2cbad1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListHostKeys

## Description
One net-new divergence. ONE cause must be closed before submitting.

Cause 1 of 1: ListHostKeys misreports the encryption algorithm of a server's imported host key, serving the constant 'ssh-ed25519' instead of the algorithm of the stored key material.

AWS: Smithy: ListHostKeys -> ListHostKeysResponse{ServerId required, HostKeys: ListedHostKeys required}. ListedHostKey.Type targets com.amazonaws.transfer#HostKeyType and its documentation states: "The encryption algorithm that is used for the host key. The Type parameter is specified by using one of the following values: ssh-rsa, ssh-ed25519, ecdsa-sha2-nistp256, ecdsa-sha2-nistp384, ecdsa-sha2-nistp521." The value is therefore a property of the key material held for that host key, not a constant: a host key whose stored material is RSA lists as 'ssh-rsa'. ImportHostKey takes HostKeyBody (shape HostKey, @sensitive, length 0..4096) and returns the HostKeyId it assigned, so the material the caller supplied is what the server-scoped listing must describe. In this scenario a LATER ImportHostKey is refused (invalid key format) and the recording asserts the listing is UNCHANGED by that refusal: the previously imported RSA key is still listed, still with Type 'ssh-rsa'. Recorded behavior for that specific host key: Type 'ssh-rsa' both before and after the rejected import.

Emulator: for the same host key ARN (arn:aws:transfer:<region>:<account>:host-key/<serverId>/<hostKeyId>) the emulator answers HTTP 200 with $.HostKeys[Arn=...].Type = 'ssh-ed25519' where AWS returns 'ssh-rsa'. The same wrong value is served on both reads in the fixture (before and after the rejected import), so the Type emitted does not depend on the key material at all.

Why: Type is how a client identifies which host-key algorithm a Transfer server presents. A caller that reads 'ssh-ed25519' for an RSA key configures its SFTP client with the wrong HostKeyAlgorithms, pins or audits the wrong algorithm, and host-key rotation logic that branches on Type (e.g. "replace the RSA key, keep the ed25519 one") operates on the wrong key. Because the value is constant it also masks whether an import was parsed at all: a compliance check asserting a server has no RSA host key passes against the emulator and fails against AWS.

Fix: the host-key state/projection behind ListHostKeys for the transfer service -- derive ListedHostKey.Type from the stored HostKeyBody material when ImportHostKey persists the key (and project it on read) instead of emitting a fixed 'ssh-ed25519'. This is the same projection DescribeHostKey reads.

Related already-open issue: 20261029T184519Z-transfer-security-policy-catalogue-and-host-key-reads.md describes host-key Type and host-key-projection faults for the transfer service, but the replay report does not mark this cluster as owned by it (its replay key is not carried there), so this divergence is filed separately and must be re-replayed on its own.

Traces: transfer-import-invalid-host-key-then-list-unchanged#task#3 (the same cluster also covers #task#5 of that fixture, the post-refusal re-read).

## Evidence
From replay of the simparity-wire-aws-v1 wire corpus. Expected = recorded AWS response, Actual = emulator. Both sides answered HTTP 200, so this is a response-body divergence.

ListHostKeys / transfer-import-invalid-host-key-then-list-unchanged#task#3 (cluster also covers #task#5):
  $.HostKeys[Arn="arn:aws:transfer:us-west-2:000000000000:host-key/s-00000000000000001/hostkey-00000000000000002"].Type
    expected 'ssh-rsa'
    actual   'ssh-ed25519'

The ARN, region, account id, server id and host key id in that selector are per-environment values from this recording and are not themselves requirements; the requirement is that the Type reported for a host key equals the algorithm of the key material imported for it -- 'ssh-rsa' for RSA material -- drawn from the documented HostKeyType value set (ssh-rsa, ssh-ed25519, ecdsa-sha2-nistp256, ecdsa-sha2-nistp384, ecdsa-sha2-nistp521).

Model grounding: ListedHostKey.Type -> com.amazonaws.transfer#HostKeyType, documented value set as quoted above; ImportHostKeyRequest.HostKeyBody -> HostKey (@sensitive, length 0..4096) is the material the value must be derived from.

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus against it.
3. Re-replay fixture transfer-import-invalid-host-key-then-list-unchanged, step task#3 (and task#5, the same cluster): the scenario imports an RSA host key for the server, then issues an ImportHostKey that the service refuses for invalid key format, and asserts the host-key listing is unchanged.
4. In the ListHostKeys 200 response, read the HostKeys entry for the imported key and compare its Type member: AWS returns 'ssh-rsa' (the algorithm of the imported material); the emulator returns 'ssh-ed25519' on both the pre- and post-refusal read.
