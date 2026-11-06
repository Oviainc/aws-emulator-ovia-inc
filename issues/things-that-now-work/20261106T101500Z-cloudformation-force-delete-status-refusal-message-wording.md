Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.force-delete-stack-rejected-unless-delete-failed
Issue-Commit: 70be64dd9

# RESOLVED: DeleteStack gates `DeletionMode` `FORCE_DELETE_STACK` on `DELETE_FAILED` and words the refusal in the operation's own refusal template

## Operation
DeleteStack

## Description
`DeleteStack` with `DeletionMode` `FORCE_DELETE_STACK` against a stack that is NOT in
`DELETE_FAILED` is refused — HTTP 400, error code `ValidationError`, with nothing of the stack
touched — which is cause 3 of
`20261025T015306Z-cloudformation-refusal-taxonomy-diverges-wrong-not-found-validation.md`.
A deletion already in flight starts nothing, so a repeat of it is the no-op success it is
under the standard mode; the standard mode itself is not gated this way.

The refusal is worded in the template this OPERATION answers a deletion its target does not
admit with — the bracketed stack as the request spelled it, then the condition that refused it
— which is the form of the termination-protection refusal the same operation answers
(`Stack [<name>] cannot be deleted while TerminationProtection is enabled`). Here the
condition is the status the stack is in, read from this store, since the mode acts on the one
status it is documented for ("delete the stack if it's stuck in a DELETE_FAILED state due to
resource deletion failure").

## Evidence
`services/service-com-amazonaws-cloudformation-cloudformation/tests/all/forced_deletion_is_gated_by_status.rs`
holds the rule over 13 rows — every `StackStatus` this service puts a stack in: the accepted
one (asserting the deletion really starts and the stack then stops resolving), the refused ones
(status, code, the exact sentence, and the stack still in that status afterwards), and the two
that start nothing — plus the standard mode's un-gated deletion.

## Steps to Reproduce
1. `CreateStack` any template and wait for `CREATE_COMPLETE`.
2. `DeleteStack(StackName=<the stack>, DeletionMode=FORCE_DELETE_STACK)` — refused 400
   `ValidationError`, the stack untouched.
3. Drive the stack to `DELETE_FAILED` (a resource an owning service refuses to delete) and
   repeat step 2 — accepted, and the deletion completes.
