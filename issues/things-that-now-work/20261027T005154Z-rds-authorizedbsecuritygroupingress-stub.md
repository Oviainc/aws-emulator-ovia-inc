Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.concurrent-authorize-ingress-duplicate-rule-single-winner rds.db-security-group-ingress-lifecycle
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.concurrent-authorize-ingress-duplicate-rule-single-winner|run#16|AssertionError|assert results == [vpc_blocked, vpc_blocked], (
Issue-Key: rds.db-security-group-ingress-lifecycle|run#9|AssertionError|assert code == "InvalidParameterValue", (
Issue-Commit: 4bfacb2b54a1c65638c52b387b9afadee41617eb

# Implement rds AuthorizeDBSecurityGroupIngress: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `4bfacb2b54a1c65638c52b387b9afadee41617eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AuthorizeDBSecurityGroupIngress

## Description
`AuthorizeDBSecurityGroupIngress` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/authorize_db_security_group_ingress.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `AuthorizeDBSecurityGroupIngress` call, so each one unblocks only when `AuthorizeDBSecurityGroupIngress` works: `rds.concurrent-authorize-ingress-duplicate-rule-single-winner`, `rds.db-security-group-ingress-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/authorize_db_security_group_ingress.rs`. Replaying the corpus, the emulator refused the `AuthorizeDBSecurityGroupIngress` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_rds_concurrent_authorize_ingress_duplicate_rule_single_winner.py", line 68, in run
    assert results == [vpc_blocked, vpc_blocked], (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected both concurrent calls to be deterministically rejected identically, got results=[('NotImplemented', 'operation AuthorizeDBSecurityGroupIngress is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation AuthorizeDBSecurityGroupIngress is scaffolded but not implemented in this emulator')]

## Steps to Reproduce
Boot the rds emulator
Call AuthorizeDBSecurityGroupIngress and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
