Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.assume-role-packed-policy-too-large-rejected
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.assume-role-packed-policy-too-large-rejected|run#16|AssertionError|assert control_creds["AccessKeyId"].startswith("ASIA"), (
Issue-Commit: bdd3fb0929eecc75e2cdae766c558b93a33a4087

# sts AssumeRole mints temporary credentials with an EMULATOR-class AccessKeyId instead of the ASIA class

> _Note: this issue was identified against the code base at commit `bdd3fb0929eecc75e2cdae766c558b93a33a4087`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
One net-new root cause, grounded in the failing behavioral check sts.assume-role-packed-policy-too-large-rejected. The check's PackedPolicyTooLarge leg (50 maximal session tags -> PackedPolicyTooLarge with the documented "Serialized token too large for session. Token size of <n> bytes exceeds the maximum allowed limit of 4096 bytes." message) PASSES; the divergence is in the positive control, where the credentials AssumeRole issues are not of the temporary access-key class.

Note on overlap: the open issue 20261027T130805Z-sts-5-net-new-behavioral-root-causes-temporary.md describes the same ASIA-class defect for a different set of sts checks and even names this check in its trace list, but the replay report did NOT mark this check as OWNED by it (its recorded replay key for this check does not match the key the current replay produced), so this check is unowned and is filed here. A fixer closing that issue's Cause 1 closes this one too.

Cause 1 of 1: AssumeRole's minted Credentials.AccessKeyId is outside the ASIA class
AWS: AWS mints temporary security credentials (AssumeRole, AssumeRoleWith*, GetSessionToken, GetFederationToken) with an AccessKeyId whose leading four-character class token is `ASIA`, while long-term IAM user keys use `AKIA` (AWS IAM User Guide, "IAM identifiers" -> unique identifier prefixes). Smithy: `Credentials$AccessKeyId` targets `accessKeyIdType`, @required, @length(min 16, max 128), @pattern "^[\\w]*$", doc "The access key ID that identifies the temporary security credentials." The model constrains only the shape, so the class token is recorded behavior plus AWS's documented identifier prefixes. Only the leading `ASIA` token is the requirement; the remaining characters are a per-mint random id and are NOT part of it.
Emulator: AssumeRole answers an AccessKeyId in an `EMULATOR…` class. The replayed control call returned a key id whose class token is `EMULATOR` (the full value observed in this run is a per-run id and not itself a requirement), so a temporary session key is indistinguishable by prefix from a long-term IAM user key. The accompanying SecretAccessKey/SessionToken are populated and PackedPolicySize is reported, so only the key-id class diverges.
Why: clients branch on the key-id class. Tooling decides from the prefix whether a credential needs an accompanying session token, whether it is a rotatable long-term key and whether it is safe to persist; credential scanners, policy linters and the GetAccessKeyInfo contract all read it. Against the emulator, any code path testing for `ASIA` concludes no STS session was ever issued and treats a short-lived session key as a long-term user key.
Fix: the temporary-credential key-id minting the STS session minter requests from the shared credential store — `crate::state::mint_session` in services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/state.rs, delegating to the emulator's CredentialStore. Session mints must request an `ASIA`-class key id while IAM long-term keys keep `AKIA`.
Traces: sts.assume-role-packed-policy-too-large-rejected.

## Evidence
Failing check sts.assume-role-packed-policy-too-large-rejected [mismatch]:
  check_sts_assume_role_packed_policy_too_large_rejected.py:81
    assert control_creds["AccessKeyId"].startswith("ASIA")
  AssertionError: expected an ASIA-prefixed temporary AccessKeyId, got 'EMULATORASDB6DF3DPID'

Expected: AssumeRole Credentials.AccessKeyId begins with the class token ASIA (the remaining characters are a per-mint id and are not asserted).
Actual: begins with EMULATOR (the observed full value 'EMULATORASDB6DF3DPID' is a per-run id; the diverging part is the class token).

Controls that PASSED in the same check and bound the divergence: the oversized-session-tags AssumeRole was refused with error code PackedPolicyTooLarge and the message "Serialized token too large for session. Token size of <n> bytes exceeds the maximum allowed limit of 4096 bytes." (fixed text as AWS words it, with <n> computed server-side from the session); the small-tags control AssumeRole succeeded and populated SecretAccessKey and SessionToken.

## Steps to Reproduce
1. iam:CreateRole with a trust policy allowing arn:aws:iam::<account>:root the actions sts:AssumeRole and sts:TagSession.
2. sts:AssumeRole on that role with RoleSessionName="small-tags-session" and Tags=[{Key: Team, Value: engineering}].
3. Read Credentials.AccessKeyId from the response.
Expect it to start with `ASIA` (temporary-credential class). Observe an `EMULATOR…`-class key id instead. The same can be checked for GetSessionToken and GetFederationToken.
