Issue-Source: trace-sweep-divergence
Issue-Trace: kms-get-key-policy-agent-shape-and-invalid-arn
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kms-get-key-policy-agent-shape-and-invalid-arn
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|GetKeyPolicy|4xx->4xx|$.__type
Issue-Key: kms|GetKeyPolicy|4xx->4xx|$.message
Issue-Commit: 2d0b651e448fe6a44fa56d00833f67c02b248ecb

# KMS GetKeyPolicy given an alias ARN is refused NotFoundException instead of InvalidArnException

> _Note: this issue was identified against the code base at commit `2d0b651e448fe6a44fa56d00833f67c02b248ecb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetKeyPolicy

## Description
One net-new wire divergence (1 cause). The KMS emulator mis-classifies an ALIAS identifier supplied in GetKeyPolicy's KeyId: it resolves it as an alias and then reports the alias absent, instead of refusing the identifier FORM.

Related open issues that do NOT cover this cluster (neither carries this fixture nor this step as a replay key, and both are about other operations/surfaces): 20261103T160539Z-kms-aws-managed-key-lookup-forms-diverge-alias.md (the same mis-classification on ListAliases, plus managed-key Description/AliasCount projections) and 20261103T061309Z-kms-listaliases-refuses-a-backup-vault-s-default.md (a 400 where AWS answers 200 on the backup path). The KeyOrigin clusters of this replay are owned by 20261029T235222Z-kms-crypto-operation-responses-omit-the-keyorigin-projection.md and are not part of this issue.

Cause 1 of 1: GetKeyPolicy called with an alias ARN as KeyId answers the wrong error class and the wrong message (fixture kms-get-key-policy-agent-shape-and-invalid-arn, step task#3).
AWS: Smithy kms#GetKeyPolicy.KeyId (KeyIdType, @required, @length 1-2048) is documented as 'Specify the key ID or key ARN of the KMS key' and lists ONLY the key-ID and key-ARN forms as examples - an alias name or alias ARN is not an accepted form for this parameter. The operation's modelled client errors are InvalidArnException (@httpError 400, awsQueryError InvalidArn), NotFoundException (@httpError 404) and KMSInvalidStateException (409). An alias identifier is therefore a bad ARN/identifier FORM, not a missing resource, and the recorded response is HTTP 400 with __type InvalidArnException and the message 'Key Aliases are not supported for this operation.' That message is entirely fixed text for this error class: it has no variable part interpolated from the request or from state, so it must be reproduced verbatim.
Emulator: HTTP 400 (status matches) but with __type 'NotFoundException' and the message 'Alias <the request's KeyId> is not found.' (recorded run: 'Alias arn:aws:kms:us-west-2:000000000000:alias/sim2real-probe-kms-alias-653e61b9 is not found.' - the alias name/ARN there is a per-run generated value and is not itself a requirement; the fixed parts of the emulator's text are 'Alias ' and ' is not found.', with the request's KeyId interpolated between them). So the emulator accepts the alias form, performs an alias lookup, and reports the lookup result, rather than rejecting the form before any lookup. Note also that NotFoundException is modelled at @httpError 404, so emitting that shape with a 400 status is self-inconsistent with the model regardless of the classification question.
Why: a client that probes a key identifier through several forms (console-style reference chains, inventory/compliance tools handed an opaque identifier, IaC drift checks) branches on the error class: InvalidArnException means 'this identifier form is not accepted here - resolve the alias to a key ARN and retry', while NotFoundException means 'the resource is gone'. The emulator sends such a client down the deleted-resource branch for a request that is merely in the wrong form, so an existing key looks deleted; and an SDK matching on the modelled error shape sees a 400 carrying a shape AWS only emits at 404.
Fix: GetKeyPolicy's input validation in the kms crate (service-com-amazonaws-kms-trentservice) - detect an alias identifier (alias/<name> or an alias ARN) in KeyId and refuse it as InvalidArnException with AWS's fixed message before any alias/key resolution or key-state check. The same classification rule already needed for ListAliases applies here, so it belongs in the shared key-identifier parsing/validation path rather than being special-cased per operation.
Traces: kms-get-key-policy-agent-shape-and-invalid-arn#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, 520 steps, 492 passed, 28 mismatched, 0 errored, 0 blocked, score 94.6%, 15 divergence clusters - 14 of which are already owned by open issues 20261029T235222Z-kms-crypto-operation-responses-omit-the-keyorigin-projection.md and 20261103T160539Z-kms-aws-managed-key-lookup-forms-diverge-alias.md).

The one net-new cluster, verbatim from the replay output:

divergence_key: service=kms|GetKeyPolicy|status=400->400|keys=$.__type,$.message
fixture_id: kms-get-key-policy-agent-shape-and-invalid-arn, step_key: task#3, verdict: mismatch, expected_status 400, actual_status 400, affected_count 1, owned_by: [], stub_path: ''
detail:
  $.__type: value_mismatch (expected 'InvalidArnException', got 'NotFoundException')
  $.message: value_mismatch (expected 'Key Aliases are not supported for this operation.', got 'Alias arn:aws:kms:us-west-2:000000000000:alias/sim2real-probe-kms-alias-653e61b9 is not found.')

Expected (AWS, recorded): 400 / __type InvalidArnException / message 'Key Aliases are not supported for this operation.' (fixed text, no variable part).
Actual (emulator): 400 / __type NotFoundException / message 'Alias <request KeyId> is not found.' (fixed text 'Alias ' + ' is not found.' with the request's KeyId interpolated; the alias value shown is a per-run generated name).

Model checked (services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json): kms#GetKeyPolicy input {KeyId: KeyIdType @required @length 1-2048, doc 'Specify the key ID or key ARN of the KMS key' with only Key ID and Key ARN examples; PolicyName: PolicyNameType @length 1-128 pattern ^[\w]+$}, output {Policy, PolicyName}; errors InvalidArnException @httpError 400 (awsQueryError InvalidArn), NotFoundException @httpError 404, KMSInvalidStateException 409, KMSInternalException 500, DependencyTimeoutException 503.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1).
3. Cause 1 - fixture kms-get-key-policy-agent-shape-and-invalid-arn, step task#3: create a KMS key and an alias for it, then call kms:GetKeyPolicy with KeyId set to that ALIAS ARN (arn:aws:kms:<region>:<account>:alias/<alias-name>) - an alias NAME behaves the same way.
4. Observe the emulator answers 400 with __type 'NotFoundException' and message 'Alias <KeyId> is not found.', where AWS answers 400 with __type 'InvalidArnException' and the fixed message 'Key Aliases are not supported for this operation.'.
5. Re-replay kms-get-key-policy-agent-shape-and-invalid-arn#task#3 to confirm a fix.
