Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: partnercentral-account.list-connection-invitations-filter-structure partnercentral-account.list-partners-and-connections-smoke partnercentral-account.partner-profile-reflects-completed-update-task partnercentral-account.profile-update-task-lifecycle-and-terminal-cancel-rejected partnercentral-account.tag-resource-nonexistent-arn-rejected partnercentral-account.training-domain-disassociate-removed-domain-rejected
Issue-Shape-Id: com.amazonaws.partnercentralaccount#PartnerCentralAccount
Issue-Key: partnercentral-account|ListPartners|NotImplemented
Issue-Commit: a36b120d1ec2ecc9379e4b38fe0d35161c55eb57

# Implement partnercentral-account ListPartners: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a36b120d1ec2ecc9379e4b38fe0d35161c55eb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPartners

## Description
`ListPartners` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-partnercentralaccount-partnercentralaccount/src/ops/list_partners.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `ListPartners` call, so each one unblocks only when `ListPartners` works: `partnercentral-account.list-connection-invitations-filter-structure`, `partnercentral-account.list-partners-and-connections-smoke`, `partnercentral-account.partner-profile-reflects-completed-update-task`, `partnercentral-account.profile-update-task-lifecycle-and-terminal-cancel-rejected`, `partnercentral-account.tag-resource-nonexistent-arn-rejected`, `partnercentral-account.training-domain-disassociate-removed-domain-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-partnercentralaccount-partnercentralaccount/src/ops/list_partners.rs`. Replaying the corpus, the emulator refused the `ListPartners` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListPartners operation: operation ListPartners is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the partnercentral-account emulator
Call ListPartners and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
