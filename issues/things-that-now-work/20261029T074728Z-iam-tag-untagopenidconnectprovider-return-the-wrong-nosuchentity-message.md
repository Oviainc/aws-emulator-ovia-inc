Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.tag-open-idconnect-provider iam.untag-open-idconnect-provider
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.tag-open-idconnect-provider|run#19|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.untag-open-idconnect-provider|run#21|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 2b09bad8439b0b0ac599f9d638bea059710099b4

# IAM Tag/UntagOpenIDConnectProvider return the wrong NoSuchEntity message text for a missing provider ARN

> _Note: this issue was identified against the code base at commit `2b09bad8439b0b0ac599f9d638bea059710099b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagOpenIDConnectProvider

## Description
One net-new root cause for iam.

Cause 1 of 1: the two OIDC-provider TAGGING operations (TagOpenIDConnectProvider, UntagOpenIDConnectProvider) word their NoSuchEntity refusal with the message template belonging to the provider LOOKUP operations instead of their own. Both diverge identically, so one shared message builder (or one shared not-found helper reused by both tag handlers) is the single cause.

AWS: Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json) models NoSuchEntityException on both TagOpenIDConnectProvider and UntagOpenIDConnectProvider with httpError 404 and awsQueryError code `NoSuchEntity`; the model does not state the message text, so the wording is recorded behavior only. For a well-formed OIDC-provider ARN the account holds no provider under, AWS's tag/untag refusal is worded:

    OpenId connect Provider <OpenIDConnectProviderArn> cannot be found.

The FIXED text is `OpenId connect Provider ` ... ` cannot be found.` (note the "OpenId connect" spelling, the ARN in the MIDDLE, and the trailing period). The single VARIABLE part is the `OpenIDConnectProviderArn` value, echoed verbatim from the request. The account id and the generated provider host inside the ARN in any one replay are per-run values filling that one slot, not part of the requirement.

IMPORTANT: this is NOT the same template the provider-lookup operations use. The open issue 20261029T012435Z-iam-nosuchentity-for-a-missing-oidc-provider-uses.md records that GetOpenIDConnectProvider / AddClientIDToOpenIDConnectProvider / RemoveClientIDFromOpenIDConnectProvider must answer `OpenIDConnect Provider not found for arn <arn>`. AWS words these two families differently, so a single shared helper cannot satisfy both. That open issue does NOT cover the tag/untag operations and these two checks are not owned by it.

Emulator: TagOpenIDConnectProvider and UntagOpenIDConnectProvider on a nonexistent provider ARN return the correct status (404) and the correct code (`NoSuchEntity`), but the message

    OpenIDConnect Provider not found for arn <OpenIDConnectProviderArn>

i.e. the lookup-family fixed text: a different spelling ("OpenIDConnect"), the ARN moved to the END after the literal " for arn ", and no trailing period.

Why: a client that keys off the NoSuchEntity message rather than only the code — IaC/CI assertion harnesses, log scrapers, and user-facing passthroughs of the IAM message — sees text no AWS region ever produces for a tag/untag refusal. The success path of both operations is already correct (tags persist, GetOpenIDConnectProvider and ListOpenIDConnectProviderTags reflect them, untag removes only the named keys), so the message is the whole divergence.

Fix: the not-found message construction used by the two tag handlers — services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/tag_open_id_connect_provider.rs and .../src/ops/untag_open_id_connect_provider.rs (they currently reuse IAM's shared OIDC not-found helper in .../src/state.rs). These two operations need their own template, `OpenId connect Provider {arn} cannot be found.`, formatted from the request's OpenIDConnectProviderArn, kept distinct from the lookup-family template that 20261029T012435Z-iam-nosuchentity-for-a-missing-oidc-provider-uses.md governs. Changing the single shared helper to satisfy one family will break the other.

Traces: iam.tag-open-idconnect-provider, iam.untag-open-idconnect-provider.

## Evidence
Cause 1 — Expected (AWS, per recordings): HTTP 404, Error.Code = NoSuchEntity, Error.Message = "OpenId connect Provider <request's OpenIDConnectProviderArn> cannot be found."
Actual (emulator): HTTP 404, Error.Code = NoSuchEntity, Error.Message = "OpenIDConnect Provider not found for arn <request's OpenIDConnectProviderArn>"

Verbatim replay assertions:
- iam.tag-open-idconnect-provider (check_iam_tag_open_idconnect_provider.py:71): AssertionError: expected message 'OpenId connect Provider arn:aws:iam::000000000000:oidc-provider/missing-2a3a7a00d4c8.example.com cannot be found.', got 'OpenIDConnect Provider not found for arn arn:aws:iam::000000000000:oidc-provider/missing-2a3a7a00d4c8.example.com'
- iam.untag-open-idconnect-provider (check_iam_untag_open_idconnect_provider.py:68): AssertionError: expected message 'OpenId connect Provider arn:aws:iam::000000000000:oidc-provider/missing-593ed294b0a8.example.com cannot be found.', got 'OpenIDConnect Provider not found for arn arn:aws:iam::000000000000:oidc-provider/missing-593ed294b0a8.example.com'

Fixed vs variable text: fixed = "OpenId connect Provider " and " cannot be found."; variable = the OpenIDConnectProviderArn echoed from the request (the account id 000000000000 and the generated host `missing-<hex>.example.com` in the quoted strings are per-run values, not requirements). Status 404 and code NoSuchEntity already match and are not the defect.

## Steps to Reproduce
1. Boot the emulator and point an IAM client at it.
2. Call TagOpenIDConnectProvider with a well-formed but nonexistent provider ARN, e.g. OpenIDConnectProviderArn=arn:aws:iam::<account>:oidc-provider/does-not-exist.example.com, Tags=[{Key: k, Value: v}].
3. Observe HTTP 404 / Error.Code NoSuchEntity with Error.Message "OpenIDConnect Provider not found for arn <arn>" instead of AWS's "OpenId connect Provider <arn> cannot be found."
4. Repeat with UntagOpenIDConnectProvider(OpenIDConnectProviderArn=<same missing arn>, TagKeys=["k"]) — identical wrong message.
5. Confirm the lookup family still needs the OTHER template: GetOpenIDConnectProvider on the same missing ARN must answer "OpenIDConnect Provider not found for arn <arn>" (see 20261029T012435Z-iam-nosuchentity-for-a-missing-oidc-provider-uses.md).
6. Re-replay iam.tag-open-idconnect-provider and iam.untag-open-idconnect-provider to confirm the fix.
