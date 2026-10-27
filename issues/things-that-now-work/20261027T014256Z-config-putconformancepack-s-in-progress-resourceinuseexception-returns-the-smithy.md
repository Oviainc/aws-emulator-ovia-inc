Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.deploy-conformance-pack-duplicate-create-rejected
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.deploy-conformance-pack-duplicate-create-rejected|run#13|AssertionError|assert message == (
Issue-Commit: 32af0d92e59e46f2fdf169ea1f28f8cf7b2e0f6a

# PutConformancePack's in-progress ResourceInUseException returns the Smithy documentation sentence instead of AWS's request-derived "A workflow is currently in progress for the Conformance Pack: <name>. Please try later."

> _Note: this issue was identified against the code base at commit `32af0d92e59e46f2fdf169ea1f28f8cf7b2e0f6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutConformancePack

## Description
ONE net-new root cause on an operation that DOES answer. The state-conflict guard now exists (a second PutConformancePack for a pack still being created is refused 400 ResourceInUseException, which is the behavior Cause 1 of the open issue 20261024T114729Z-config-accepts-a-duplicate-conformance-pack-and-an.md asked for), but the refusal carries the wrong message: the handler emits the error shape's *documentation* prose rather than the message AWS's PutConformancePack actually returns for this conflict. This is a different defect from the owned over-acceptance cause in that issue (whose replay key is the "call succeeded" assertion), which is why the check now fails one assertion later, on the message.

Cause 1 of 1: the ResourceInUseException message for a conformance-pack workflow conflict is not AWS's message template.

AWS: Smithy (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json) models PutConformancePack with ConformancePackName required (@length 1..256, pattern ^[a-zA-Z][-a-zA-Z0-9]*$) and TemplateBody @length 1..51200, and its errors are ConformancePackTemplateValidationException / InsufficientPermissionsException / InvalidParameterValueException / MaxNumberOfConformancePacksExceededException / ResourceInUseException, all errorKind client, httpError 400. Recorded behavior: while a pack is still in a non-terminal create state, a second PutConformancePack for the same ConformancePackName is refused HTTP 400, code ResourceInUseException, with the message template "A workflow is currently in progress for the Conformance Pack: <ConformancePackName>. Please try later." — the fixed text is "A workflow is currently in progress for the Conformance Pack: " and ". Please try later.", and the single variable part is the ConformancePackName taken verbatim from the request. The rule is that this refusal names the pack whose workflow is busy.

Emulator: the status (400) and the code (ResourceInUseException) are correct, but the message is "A conformance pack creation, update, and deletion is in progress. Try your request again later." — which is the ResourceInUseException shape's own documentation bullet for PutConformancePack, verbatim and with no request-derived part at all. It names no pack, so it is identical for every pack in the account.

Why: a client cannot tell WHICH conformance pack is busy from the refusal. Concurrent deployers / IaC reconcilers that fan out several PutConformancePack calls and surface or key their retry/backoff on the refused pack name get a constant sentence instead, so the busy pack is unidentifiable from the error alone; log- and alarm-matchers written against AWS's wording never match. It also leaks the model's documentation text as a runtime message, a pattern AWS never uses for this error.

Fix: the ResourceInUseException message construction in the PutConformancePack handler's create/update conflict precondition (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_conformance_pack.rs) — build the message from the requested ConformancePackName instead of returning the error shape's documentation string.

Side effect of the early failure: because the message assertion is the check's second assertion, the check's positive controls never ran — that the original (first) request still reaches CREATE_COMPLETE and that the rejected second TemplateBody never takes effect are unverified in this replay. A fixer should confirm both after correcting the message.

Traces: config.deploy-conformance-pack-duplicate-create-rejected.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false; 70/117 checks passed, config 66/103).

[mismatch] config.deploy-conformance-pack-duplicate-create-rejected
  File "check_config_deploy_conformance_pack_duplicate_create_rejected.py", line 62, in run
    assert message == (
        f"A workflow is currently in progress for the Conformance Pack: {pack_name}. "
        "Please try later."
    ), message
  AssertionError: A conformance pack creation, update, and deletion is in progress. Try your request again later.

So: expected message "A workflow is currently in progress for the Conformance Pack: <pack_name>. Please try later." (fixed text plus the ConformancePackName from the request); actual message "A conformance pack creation, update, and deletion is in progress. Try your request again later." The preceding assertion `code == "ResourceInUseException"` PASSED, so only the message diverges — the error code and the 400 client classification are right.

Model grounding: ResourceInUseException documentation in com-amazonaws-configservice-starlingdoveservice.json contains the bullet "For PutConformancePack and PutOrganizationConformancePack, a conformance pack creation, update, and deletion is in progress. Try your request again later." — the emulator's message is that documentation sentence, which identifies the exception class in the model, not the runtime refusal AWS returns.

Related open issue (do not close with this one): 20261024T114729Z-config-accepts-a-duplicate-conformance-pack-and-an.md, whose Cause 1 covers the earlier state where this same call was accepted with HTTP 200 and a ConformancePackArn. That guard is now present; this issue is only about the message it carries.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline; replay simparity-behavioral-ga-prod-v1 and re-replay config.deploy-conformance-pack-duplicate-create-rejected.
2. Ensure a configuration recorder exists and is started (the check does this first).
3. aws configservice put-conformance-pack --conformance-pack-name P --template-body '<YAML with one AWS::Config::ConfigRule>' — accepted, returns a ConformancePackArn.
4. Immediately (while P is still CREATE_IN_PROGRESS) aws configservice put-conformance-pack --conformance-pack-name P --template-body '<a DIFFERENT YAML with a different ConfigRuleName>'.
5. Observe the refusal: HTTP 400 / ResourceInUseException (correct) with message "A conformance pack creation, update, and deletion is in progress. Try your request again later." instead of "A workflow is currently in progress for the Conformance Pack: P. Please try later."
6. After fixing the message, also re-verify the check's positive controls that this assertion masked: describe-conformance-pack-status for P reaches CREATE_COMPLETE, and the rule from the second (rejected) template was never created.
