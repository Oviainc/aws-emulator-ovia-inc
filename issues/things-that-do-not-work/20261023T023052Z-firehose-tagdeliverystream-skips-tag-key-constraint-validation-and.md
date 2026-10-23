Issue-Source: trace-sweep-divergence
Issue-Trace: firehose-tagging-input-constraint-validation
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: firehose-tagging-input-constraint-validation
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose|TagDeliveryStream|4xx->4xx|$.__type
Issue-Key: firehose|TagDeliveryStream|4xx->4xx|$.message
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# Firehose TagDeliveryStream skips tag-key constraint validation and answers ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagDeliveryStream

## Description
One net-new root cause in the firehose service, found by replaying the wire fixture corpus (trace set simparity-wire-aws-v1, 14 fixtures / 32 steps, 26 passed, 6 mismatched, score 81.2%, state_reset_failures 0, emulator_up true). firehose is stood up in this build (service-com-amazonaws-firehose-firehose-20150804 is reported bootstrapped and it is absent from the pending set), so this is a fidelity defect, not a bootstrap gap.

Related but NOT covering this divergence (the replay report lists no owner and no stub for this cluster, so neither issue carries its replay key):
- 20261022T064119Z-firehose-tagdeliverystream-stub.md ('Implement firehose TagDeliveryStream: unimplemented scaffold stub') — TagDeliveryStream is no longer a 501 stub in this checkout: it answers a modelled 400 error, so that issue does not describe what is observed here.
- 20261022T194351Z-firehose-control-plane-configuration-echo-drops-members-tag.md, Cause 4 — describes absent tag-key validation from the behavioral suite, where the emulator returned HTTP 200 for a reserved/illegal key. The wire replay shows a DIFFERENT emulator behavior for the same operation (HTTP 400 ResourceNotFoundException, i.e. a resource lookup answered before any tag-key validation ran), and that issue carries only behavioral check ids, no wire replay key.
- 20261022T151017Z-firehose-argument-validation-admits-invalid-input-and-misclassifies.md owns the CreateDeliveryStream / StartDeliveryStreamEncryption clusters of this same replay, not this one.

Cause 1 of 1: TagDeliveryStream performs its delivery-stream/account lookup before (and instead of) validating the supplied tag keys, so a tag key that violates the modelled TagKey constraint is never reported as an input-constraint fault.
AWS: Smithy — TagDeliveryStream input members DeliveryStreamName (@required, length 1-64, pattern ^[a-zA-Z0-9_.-]+$) and Tags (@required, list length 1-50 of Tag); Tag.Key is @required, length 1-128, pattern ^(?!aws:)[\p{L}\p{Z}\p{N}_.:\/=+\-@%]*$ (doc: 'Valid characters: Unicode letters, digits, white space, _ . / = + - % @'); the operation's modelled error set is InvalidArgumentException, LimitExceededException, ResourceInUseException, ResourceNotFoundException (all client / HTTP 400). Recorded behavior: a TagDeliveryStream whose Tags list carries a key outside that character class is refused by constraint pre-validation with HTTP 400 and $.__type = 'ValidationException', and the message is BUILT FROM THE REQUEST — it names the offending member path and the constraint it failed, e.g. "1 validation error detected: Value at 'tags.1.member.key' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!aws:)[\p{L}\p{Z}\p{N}_.:\/=+\-@%]*$". That member-path-plus-constraint rule is the requirement; the exact sentence above is an example of it.
Emulator: HTTP 400 with $.__type = 'ResourceNotFoundException' and a message reporting the account's Firehose as absent (example: 'Firehose under account 000000000000 not found.'). The status matches; the error identity does not, and nothing in the response refers to the tag key at all. Hypothesis: the handler resolves the delivery stream / account scope first and returns not-found, so the tag-key constraint check is unreachable for this request; the stream the fixture addresses is also not being found, which is the same account-scoped lookup gap seen elsewhere in this service.
Why: clients branch on the error code. A caller that submits a malformed or reserved tag key is told its stream (or account) does not exist, so remediation takes the wrong branch — re-create or re-discover the stream rather than sanitize the tag key — and the input-constraint error path that real AWS exercises on this operation is untestable. Tagging code that passes against the emulator fails against AWS, and vice versa: a legitimate not-found and an invalid-tag-key rejection are indistinguishable.
Fix: TagDeliveryStream's handler in the Firehose service crate must run input-constraint validation of the Tags list (Tag.Key length and pattern, including the reserved 'aws:' prefix) BEFORE the delivery-stream/account state lookup, and render the constraint refusal as the service's constraint-violation error (HTTP 400, __type ValidationException) with the failing member path and the violated constraint; the account-scoped delivery-stream lookup used by this handler must also be reconciled with the state CreateDeliveryStream writes, since the fixture's stream is not being found.
Traces: firehose-tagging-input-constraint-validation#task#1.

## Evidence
Cause 1 — fixture firehose-tagging-input-constraint-validation, step task#1, operation TagDeliveryStream, verdict mismatch.
Expected: HTTP 400, $.__type = 'ValidationException', $.message built from the request and naming the failing member and constraint (example: "1 validation error detected: Value at 'tags.1.member.key' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!aws:)[\p{L}\p{Z}\p{N}_.:\/=+\-@%]*$").
Actual: HTTP 400, $.__type = 'ResourceNotFoundException', $.message = 'Firehose under account 000000000000 not found.' (example text).
Replay detail, verbatim: "$.__type: value_mismatch (expected 'ValidationException', got 'ResourceNotFoundException')" / "$.message: value_mismatch (expected \"1 validation error detected: Value at 'tags.1.member.key' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!aws:)[\\p{L}\\p{Z}\\p{N}_.:\\/=+\\-@%]*$\", got 'Firehose under account 000000000000 not found.')". Divergence key: service=firehose|TagDeliveryStream|status=400->400|keys=$.__type,$.message. Cluster size 1 (affected: firehose-tagging-input-constraint-validation#task#1). stub_path empty, owned_by empty.
Both statuses are 400; the error code is the divergence. Replay context: trace set simparity-wire-aws-v1, emulator_up true, state_reset_failures 0, so this is not a leaked-state artifact.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout, then replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture firehose-tagging-input-constraint-validation, step task#1: TagDeliveryStream against the fixture's delivery stream with a Tags list whose second entry has a Key containing a character outside the modelled TagKey class (pattern ^(?!aws:)[\p{L}\p{Z}\p{N}_.:\/=+\-@%]*$).
3. Read $.__type on the 400 response: observe 'ResourceNotFoundException' with an account-not-found message instead of 'ValidationException' with a message naming tags.1.member.key and the violated pattern.
4. Equivalently with botocore: fh = client('firehose'); create a stream, then fh.tag_delivery_stream(DeliveryStreamName=<that stream>, Tags=[{'Key':'ok','Value':'v'},{'Key':'bad*key','Value':'v'}]) — expect the constraint refusal, observe the resource error.
5. Re-replay firehose-tagging-input-constraint-validation#task#1 after the fix to confirm the error identity and the member-naming message rule.
