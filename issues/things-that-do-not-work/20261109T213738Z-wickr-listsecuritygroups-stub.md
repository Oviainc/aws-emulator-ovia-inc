Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wickr.bot-count-and-listing-reflect-accumulated-creates-and-deletes wickr.bots-count-tracks-create-and-delete wickr.create-bot-weak-challenge-rejected-by-password-policy wickr.delete-network-cascades-to-bots-and-security-groups wickr.list-security-groups-pages-all-created-groups
Issue-Shape-Id: com.amazonaws.wickr#WickrAdminApi
Issue-Key: wickr|ListSecurityGroups|NotImplemented
Issue-Commit: 8139116c3695d3bac7d26cbae8c8bb3d87daa4c0

# Implement wickr ListSecurityGroups: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8139116c3695d3bac7d26cbae8c8bb3d87daa4c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSecurityGroups

## Description
`ListSecurityGroups` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-wickr-wickradminapi/src/ops/list_security_groups.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `ListSecurityGroups` call, so each one unblocks only when `ListSecurityGroups` works: `wickr.bot-count-and-listing-reflect-accumulated-creates-and-deletes`, `wickr.bots-count-tracks-create-and-delete`, `wickr.create-bot-weak-challenge-rejected-by-password-policy`, `wickr.delete-network-cascades-to-bots-and-security-groups`, `wickr.list-security-groups-pages-all-created-groups`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-wickr-wickradminapi/src/ops/list_security_groups.rs`. Replaying the corpus, the emulator refused the `ListSecurityGroups` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListSecurityGroups operation: operation ListSecurityGroups is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the wickr emulator
Call ListSecurityGroups and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
