Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.field-level-encryption-config-inconsistent-quantities-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.field-level-encryption-config-inconsistent-quantities-rejected|run#7|AssertionError|assert code == "InconsistentQuantities", f"expected InconsistentQuantities, got {code!r}"
Issue-Commit: cfbdbd19120bc4eb7cd4848a49718d75d65d840b

# CreateFieldLevelEncryptionConfig answers 501 NotImplemented, so the InconsistentQuantities refusal (and config creation) never happens

> _Note: this issue was identified against the code base at commit `cfbdbd19120bc4eb7cd4848a49718d75d65d840b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFieldLevelEncryptionConfig

## Description
Cause 1 of 1: CreateFieldLevelEncryptionConfig is an unimplemented scaffold stub, so neither its modeled quantity/items validation nor its create path exists.

AWS: Smithy: CreateFieldLevelEncryptionConfig, HTTP POST /2020-05-31/field-level-encryption, success code 201, input member FieldLevelEncryptionConfig (httpPayload, required), output FieldLevelEncryption (httpPayload) + Location and ETag headers. Its modeled errors include InconsistentQuantities (errorKind client, httpError 400), documented as "The value of Quantity and the size of Items don't match." The recorded behavior for a FieldLevelEncryptionConfig whose ContentTypeProfileConfig.ContentTypeProfiles declares a Quantity that disagrees with the number of Items is a 400 with error code "InconsistentQuantities" and the message "The specified quantity of ContentTypeProfiles (2) does not match the actual quantity supplied (1)." In that message the fixed text is "The specified quantity of ", " (", ") does not match the actual quantity supplied (", ")."; the variable parts are the name of the offending list member (here ContentTypeProfiles), the Quantity value taken from the request, and the actual number of Items taken from the request. The refusal must happen BEFORE anything is created, so the recording shows no config carrying that CallerReference/Comment in ListFieldLevelEncryptionConfigs afterwards. With a matching Quantity/Items pair the same request succeeds with 201 and echoes ContentTypeProfiles.Quantity == 1 and the Items unchanged.

Emulator: every call answers HTTP 501 with error code "NotImplemented" — the check asserted on the error code and got 'NotImplemented' instead of 'InconsistentQuantities'. The handler is still the generated scaffold: its handle() body ends in Err(crate::scaffolded::Scaffolded::not_implemented("CreateFieldLevelEncryptionConfig")) and the file still carries the "SCAFFOLDED, NOT IMPLEMENTED" marker, so nothing about the operation is asserted and the positive control (create, then read back ContentTypeProfiles) is unreachable too.

Why: a client that builds field-level encryption cannot create a configuration at all, and a client that distinguishes bad input from server faults is misled — 501 NotImplemented is a server-side "not supported" that botocore does not treat as a client validation error, so request-shaping code that catches InconsistentQuantities to correct its Quantity never fires and the caller instead surfaces an unretryable server error for a request AWS would have rejected with a precise, actionable 400.

Fix: the operation's handler and its validation — services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_field_level_encryption_config.rs (the handle() body currently returns the scaffold not_implemented at the end of the file). It needs the Quantity-vs-Items consistency check for ContentTypeProfiles and QueryArgProfiles returning the modeled InconsistentQuantities (400) with AWS's message template above, the state write that makes the created config visible to ListFieldLevelEncryptionConfigs, and the 201 response carrying FieldLevelEncryption, Location and ETag.

Related: the replay report auto-filed a stub issue naming this same operation and handler file (it reports CreateFieldLevelEncryptionConfig blocking 5 checks in this suite); this check was NOT marked as covered by it, and this issue additionally pins the exact error code and message template the implementation owes, which a bare "implement the stub" item does not state.

Traces: cloudfront.field-level-encryption-config-inconsistent-quantities-rejected.

## Evidence
Expected (AWS): create_field_level_encryption_config with ContentTypeProfiles {Quantity: 2, Items: [<one item>]} -> HTTP 400, Error.Code "InconsistentQuantities", Error.Message "The specified quantity of ContentTypeProfiles (2) does not match the actual quantity supplied (1)." (fixed text as quoted; "ContentTypeProfiles", "2" and "1" are the member name, the request's Quantity and the request's Items count), no config created; then the same request with Quantity: 1 -> HTTP 201 echoing Quantity == 1 and the same Items.

Actual (emulator): HTTP 501, Error.Code "NotImplemented". Replay assertion: `AssertionError: expected InconsistentQuantities, got 'NotImplemented'` at check_cloudfront_field_level_encryption_config_inconsistent_quantities_rejected.py line 44. Handler evidence: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_field_level_encryption_config.rs line 4 ("SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`") and line 176 (`Err(crate::scaffolded::Scaffolded::not_implemented("CreateFieldLevelEncryptionConfig"))`).

Model evidence: smithy_query(operation="CreateFieldLevelEncryptionConfig") lists InconsistentQuantities with errorKind client / httpError 400; smithy_query(shape="InconsistentQuantities") doc: "The value of Quantity and the size of Items don't match."

## Steps to Reproduce
1. Create a field-level encryption profile (CreateFieldLevelEncryptionProfile) and note its Id.
2. Call CreateFieldLevelEncryptionConfig with FieldLevelEncryptionConfig = {CallerReference, Comment, ContentTypeProfileConfig: {ForwardWhenContentTypeIsUnknown: true, ContentTypeProfiles: {Quantity: 2, Items: [{Format: "URLEncoded", ProfileId: <profile id>, ContentType: "application/x-www-form-urlencoded"}]}}} — i.e. Quantity 2 with a single Item.
3. Observe the emulator answers HTTP 501 with Error.Code "NotImplemented" instead of HTTP 400 InconsistentQuantities with the message above.
4. Call ListFieldLevelEncryptionConfigs and re-issue step 2 with Quantity: 1: the positive control (201 with ContentTypeProfiles.Quantity == 1 and the Item echoed) also never happens.

Replay: run the behavioral check cloudfront.field-level-encryption-config-inconsistent-quantities-rejected.
