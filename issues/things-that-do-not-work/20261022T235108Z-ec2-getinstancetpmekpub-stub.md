Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.get-instance-tpm-ek-pub-returns-stable-blob
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.get-instance-tpm-ek-pub-returns-stable-blob|run#5|AssertionError|assert first == ("InvalidInstanceID.Malformed", f"The instance ID '{bogus_instance_id}' is malformed"), first
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement ec2 GetInstanceTpmEkPub: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInstanceTpmEkPub

## Description
`GetInstanceTpmEkPub` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/get_instance_tpm_ek_pub.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetInstanceTpmEkPub` call, so each one unblocks only when `GetInstanceTpmEkPub` works: `ec2.get-instance-tpm-ek-pub-returns-stable-blob`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/get_instance_tpm_ek_pub.rs`. Replaying the corpus, the emulator refused the `GetInstanceTpmEkPub` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ec2_get_instance_tpm_ek_pub_returns_stable_blob.py", line 36, in run
    assert first == ("InvalidInstanceID.Malformed", f"The instance ID '{bogus_instance_id}' is malformed"), first
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: ('NotImplemented', 'operation GetInstanceTpmEkPub is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the ec2 emulator
Call GetInstanceTpmEkPub and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
