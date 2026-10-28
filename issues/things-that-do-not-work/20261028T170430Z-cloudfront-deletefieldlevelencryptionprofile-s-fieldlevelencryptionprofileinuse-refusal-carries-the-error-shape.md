Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.field-level-encryption-profile-delete-in-use-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.field-level-encryption-profile-delete-in-use-rejected|run#19|AssertionError|assert message == in_use_message, f"expected message {in_use_message!r}, got {message!r}"
Issue-Commit: 9f261888e8f01a9cfbfe4fe6472934dc1c3bc859

# DeleteFieldLevelEncryptionProfile's FieldLevelEncryptionProfileInUse refusal carries the error shape's model documentation instead of AWS's runtime message

> _Note: this issue was identified against the code base at commit `9f261888e8f01a9cfbfe4fe6472934dc1c3bc859`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFieldLevelEncryptionProfile

## Description
This issue reports ONE root cause in the cloudfront service.

Cause 1 of 1: the FieldLevelEncryptionProfileInUse refusal returned by DeleteFieldLevelEncryptionProfile is populated with the Smithy error shape's `documentation` prose rather than the service's runtime error message, so the HTTP status and error code are right and the message text is wrong.

AWS: Smithy: DeleteFieldLevelEncryptionProfile -> DELETE /2020-31-05-style path /2020-05-31/field-level-encryption-profile/{Id}, success http code 204 with no output body; `Id` is a required httpLabel, `IfMatch` is sent as the If-Match header. Modeled errors: AccessDenied(403), FieldLevelEncryptionProfileInUse(409), InvalidIfMatchVersion(400), NoSuchFieldLevelEncryptionProfile(404), PreconditionFailed(412). Deleting a field-level-encryption profile that a field-level-encryption config still references through ContentTypeProfileConfig.ContentTypeProfiles[].ProfileId must be refused with code `FieldLevelEncryptionProfileInUse` at HTTP 409. The message AWS returns for that condition is "The field level encryption profile is currently associated with a field level encryption config. Please disassociate the profile before deleting." — the whole string is fixed text for this error class; it has no variable part, so no profile id, config id or count is interpolated into it. The model's own doc string for the FieldLevelEncryptionProfileInUse shape is "The specified profile for field-level encryption is in use."; that is prose describing the error, not the wire message.

Emulator: the status and the code are correct (the call is refused, code `FieldLevelEncryptionProfileInUse`), but the message returned is "The specified profile for field-level encryption is in use." — verbatim the FieldLevelEncryptionProfileInUse shape's `documentation` trait as carried in services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json. The substitution is systematic, not a typo: the emulator is rendering the doc string as the message. The same check's later legs assert the NoSuchFieldLevelEncryptionProfile message "The specified field level encryption profile does not exist." after the profile is deleted (both on a read and on a re-delete of the dead id); the model doc for that shape is "The specified profile for field-level encryption doesn't exist.", so those legs are very likely to show the identical substitution once the in-use message is corrected — they are unreached today because the assertion on the in-use message aborts the check first. Fixing this cause means fixing the message for both error shapes on this operation's paths.

Why: an error message is part of the refusal contract. A client (or a CI golden, or an operator runbook) that matches on CloudFront's text for this condition does not match the doc string, so it cannot recognise the "detach the profile from its config first" condition and falls through to generic error handling. AWS's wording also states the required remediation ("disassociate the profile before deleting"), which the emulator's text omits entirely, so a human reading the emulator's response is not told what to do next.

Fix: the DeleteFieldLevelEncryptionProfile handler's in-use refusal path in the cloudfront crate, plus the shared helper that builds a modeled cloudfront error's Message member — it must emit the service's runtime text for the error class and must never fall back to the error shape's `documentation` trait. Apply the same correction to this operation's NoSuchFieldLevelEncryptionProfile path and audit the other modeled cloudfront errors for the same doc-string-as-message substitution.

Related open issue, which does NOT cover this: 20261027T001538Z-cloudfront-nine-control-plane-divergences-deletepublickey-updatekeygroup-deletefieldlevelencryptionprofile.md. Its Cause 3 carried this same check id but for a different and now-resolved condition (DeleteFieldLevelEncryptionProfile answering 501 NotImplemented, so the 409 never happened at all); that leg now passes — the operation refuses correctly with FieldLevelEncryptionProfileInUse. Its Cause 8 describes the doc-string-as-message substitution generally, but is keyed only to cloudfront.op-contract-createKeyGroup and cloudfront.op-contract-get-public-key-wrong-resource-type-id-rejected and does not carry this operation or this check's current replay key.

## Evidence
Cause 1 — cloudfront.field-level-encryption-profile-delete-in-use-rejected [mismatch], failing at check line 87:
  AssertionError: expected message 'The field level encryption profile is currently associated with a field level encryption config. Please disassociate the profile before deleting.', got 'The specified profile for field-level encryption is in use.'

  Request: DeleteFieldLevelEncryptionProfile(Id=<the profile id>, IfMatch=<the profile's ETag>) while a field-level-encryption config references that profile id in ContentTypeProfileConfig.ContentTypeProfiles[0].ProfileId.
  Expected: HTTP 409, code FieldLevelEncryptionProfileInUse, message "The field level encryption profile is currently associated with a field level encryption config. Please disassociate the profile before deleting." (fixed text in full; no variable part).
  Actual:   HTTP 409, code FieldLevelEncryptionProfileInUse (both correct), message "The specified profile for field-level encryption is in use."

  The actual message is byte-for-byte the FieldLevelEncryptionProfileInUse error shape's documentation trait in the on-disk Smithy model (`"doc": "<p>The specified profile for field-level encryption is in use.</p>"`, tags stripped), which identifies the source of the wrong text.

  Profile ids, config ids, public-key ids and ETags in this check are per-run values and are not part of the requirement; the status, the code and the message text are.

  Unreached legs of the same check, which assert the same operation's not-found message and are blocked by the assertion above:
    get_field_level_encryption_profile(<deleted id>)  -> expect code NoSuchFieldLevelEncryptionProfile, message "The specified field level encryption profile does not exist."
    delete_field_level_encryption_profile(<deleted id>) -> expect the same code and message on a re-delete of a dead id.
  The model doc for NoSuchFieldLevelEncryptionProfile is "The specified profile for field-level encryption doesn't exist.", so the same substitution is expected there.

## Steps to Reproduce
1. CreatePublicKey with a PEM RSA public key; keep the returned PublicKey.Id.
2. CreateFieldLevelEncryptionProfile with EncryptionEntities = {Quantity: 1, Items: [{PublicKeyId: <that id>, ProviderId: <any>, FieldPatterns: {Quantity: 1, Items: ["credit-card"]}}]}; keep the profile Id and its ETag.
3. CreateFieldLevelEncryptionConfig with ContentTypeProfileConfig = {ForwardWhenContentTypeIsUnknown: true, ContentTypeProfiles: {Quantity: 1, Items: [{Format: "URLEncoded", ProfileId: <the profile id>, ContentType: "application/x-www-form-urlencoded"}]}}.
4. Call DeleteFieldLevelEncryptionProfile(Id=<the profile id>, IfMatch=<the profile ETag>).
   Expect: HTTP 409, code FieldLevelEncryptionProfileInUse, message "The field level encryption profile is currently associated with a field level encryption config. Please disassociate the profile before deleting."
   Observe: HTTP 409, code FieldLevelEncryptionProfileInUse, message "The specified profile for field-level encryption is in use." (the error shape's model documentation string).
5. Then exercise the unreached legs: DeleteFieldLevelEncryptionConfig for the config, DeleteFieldLevelEncryptionProfile for the profile (both must succeed), then GetFieldLevelEncryptionProfile(<that id>) and DeleteFieldLevelEncryptionProfile(<that id>) again — each must answer code NoSuchFieldLevelEncryptionProfile with the message "The specified field level encryption profile does not exist.", not the shape's doc string.
6. Re-replay cloudfront.field-level-encryption-profile-delete-in-use-rejected to confirm the fix.
