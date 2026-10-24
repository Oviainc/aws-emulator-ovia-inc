Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.vpc-association-authorization-required-before-cross-account-associate
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.vpc-association-authorization-required-before-cross-account-associate|run#7|AssertionError|assert err.get("Code") == "VPCAssociationAuthorizationNotFound", (
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Implement route53 DeleteVPCAssociationAuthorization: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVPCAssociationAuthorization

## Description
`DeleteVPCAssociationAuthorization` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/delete_vpc_association_authorization.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteVPCAssociationAuthorization` call, so each one unblocks only when `DeleteVPCAssociationAuthorization` works: `route53.vpc-association-authorization-required-before-cross-account-associate`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/delete_vpc_association_authorization.rs`. Replaying the corpus, the emulator refused the `DeleteVPCAssociationAuthorization` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_route53_vpc_association_authorization_required_before_cross_account_associate.py", line 51, in run
    assert err.get("Code") == "VPCAssociationAuthorizationNotFound", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected VPCAssociationAuthorizationNotFound, got {'Message': 'operation DeleteVPCAssociationAuthorization is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented', 'Type': 'Receiver'}

## Steps to Reproduce
Boot the route53 emulator
Call DeleteVPCAssociationAuthorization and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
