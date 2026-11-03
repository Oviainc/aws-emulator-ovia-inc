Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.bucket-notification-filter-rule-names-echo-capitalised lambda__s3.invalid-notification-filter-rule-name-rejected
Issue-Key: s3|PutBucketNotificationConfiguration|MalformedXML
Issue-Key: lambda__s3.invalid-notification-filter-rule-name-rejected|run#12|AssertionError|assert err == expected, (
Issue-Commit: a84989a361e8c7c49cb35cc41e86e09526be932e

# lambda__s3: PutBucketNotificationConfiguration refuses a FilterRule Name that is not the exact lowercase literal with 400 MalformedXML — the canonical capitalised spelling is rejected and an unknown name gets the wrong error class (2 causes)

> _Note: this issue was identified against the code base at commit `a84989a361e8c7c49cb35cc41e86e09526be932e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
REOPENS 20261102T093047Z-lambda-s3-sns-sqs-6-net-new-root.md: its fix left the call refused.

This issue bundles TWO net-new root causes of service `lambda__s3`, both in the Filter.Key.FilterRules validation of PutBucketNotificationConfiguration. Both must be closed before this issue is submitted. Neither grounding check was marked ALREADY FILED or OWNED by the replay report.

Context shared by both causes: the re-casing half of the resolved issue 20261102T093047Z-lambda-s3-sns-sqs-6-net-new-root.md (its Cause 1) now works — a configuration sent with the lowercase names is accepted and GetBucketNotificationConfiguration projects `Prefix`/`Suffix`. Both grounding checks confirm that (lambda__s3.invalid-notification-filter-rule-name-rejected asserts exactly that on its control put and PASSES that assertion). What the fix introduced is a name check that admits ONLY the exact lowercase literals and funnels everything else into the generic schema-validation refusal.

=== Cause 1 of 2: a FilterRule Name sent in the canonical CAPITALISED spelling is refused 400 MalformedXML ===
AWS: Smithy `com.amazonaws.s3#FilterRuleName` is an enum whose declared wire values are `prefix` and `suffix` — that is the REQUEST vocabulary, not the stored projection. Recorded behavior: S3 accepts the capitalised spelling of the same rule name as well, the lowercase and the capitalised spellings converge on one and the same stored Filter (a read of either reports the canonical `Prefix`/`Suffix` and the Values exactly as sent — names are normalised, values are not), and because each PutBucketNotificationConfiguration REPLACES the whole configuration, a put whose Id the caller omitted mints a fresh Id each time. So re-applying a configuration that was read back from S3 — the capitalised form — must succeed.
Emulator: re-sending the configuration with `[{"Name": "Prefix", "Value": "raw/"}, {"Name": "Suffix", "Value": ".csv"}]` after the identical lowercase put succeeded is refused HTTP 400 with Error.Code `MalformedXML` and Error.Message `The XML you provided was not well-formed or did not validate against our published schema` (the whole sentence is fixed text — nothing in it comes from the request). Reading of the evidence: the failing frame in lambda__s3.bucket-notification-filter-rule-names-echo-capitalised is inside the check's `_put` helper, which is called twice — once lowercase, once capitalised; the sibling check proves the lowercase call is accepted and read back capitalised, so the refused call is the capitalised one. Hypothesis: the handler compares the `Name` element against the two enum literals byte-for-byte and treats any other string, including the canonical capitalised form it itself serialises on read, as a schema fault.
Why: a client cannot round-trip its own configuration. Every tool that reads the stored document and re-applies it — CloudFormation/CDK drift repair, `terraform apply` after a refresh, `aws s3api get-bucket-notification-configuration | aws s3api put-bucket-notification-configuration` — hands back the capitalised names S3 reported and is told its own XML is malformed, with a message that points the operator at serialisation instead of at the one member that was rejected. There is no way to express the configuration in the spelling the read projects.
Fix: the FilterRule `Name` validation/normalisation in the PutBucketNotificationConfiguration handler of the s3 crate — accept the canonical capitalised spelling as well as the enum's lowercase spelling, normalise both to the stored canonical form, and leave Value untouched; the GetBucketNotificationConfiguration serialiser already projects the capitalised form and must stay consistent with what the Put accepts.
Traces: lambda__s3.bucket-notification-filter-rule-names-echo-capitalised (the lowercase put, the capitalised read-back of the names, the stored Values and the minted Id all passed before this refusal).

=== Cause 2 of 2: a FilterRule Name outside prefix/suffix is refused MalformedXML instead of InvalidArgument with the filter-rule sentence ===
AWS: `com.amazonaws.s3#FilterRuleName` admits only prefix and suffix. Recorded behavior: a filter rule named anything else is refused HTTP 400 with Error.Code `InvalidArgument` and Error.Message `filter rule name must be either prefix or suffix` — the entire sentence is fixed text, no value from the request is interpolated into it — and the refusal is whole-document, so the previously stored configuration is left exactly as it was. S3 reuses the one generic `InvalidArgument` code for every notification-validation failure, so the MESSAGE, not the code, is what tells a caller which rule it broke.
Emulator: the same put is refused HTTP 400 with Error.Code `MalformedXML` and Error.Message `The XML you provided was not well-formed or did not validate against our published schema` — it refuses, but with the wrong error class and a sentence that blames the shape of the document rather than naming the member and the allowed vocabulary.
Why: a client branching on the refusal cannot distinguish an unknown filter-rule name from a genuinely malformed body, so repair logic that would drop or rename the offending rule never runs and the operator is sent to look at serialisation. This is the filter-rule analogue of Cause 5 of the resolved issue 20261102T093047Z-lambda-s3-sns-sqs-6-net-new-root.md, which fixed the same misclassification for an unsupported `Events` entry; the fix did not extend to the FilterRule name, and the open issue 20261102T111455Z-s3-bucket-notification-configuration-validation-and-projection-diverge.md does not carry this check's key either.
Fix: the same FilterRule `Name` validation in the PutBucketNotificationConfiguration handler — classify an unknown name as `InvalidArgument` with the sentence above instead of letting it fall through to the generic schema-validation refusal, and store nothing.
Traces: lambda__s3.invalid-notification-filter-rule-name-rejected (its lowercase control put, the capitalised read-back and the "stored configuration untouched" assertion are all in the same check; only the refusal class and message diverge).

## Evidence
Cause 1 — lambda__s3.bucket-notification-filter-rule-names-echo-capitalised [mismatch]:
  AssertionError: put_bucket_notification_configuration accepted the Lambda destination still failing with MalformedXML after 180.0s: An error occurred (MalformedXML) when calling the PutBucketNotificationConfiguration operation: The XML you provided was not well-formed or did not validate against our published schema
  expected: the second put, carrying LambdaFunctionConfigurations[0].Filter.Key.FilterRules = [{Name:'Prefix',Value:'raw/'},{Name:'Suffix',Value:'.csv'}] for Events ['s3:ObjectCreated:*'], answers 200; GetBucketNotificationConfiguration then reports the SAME Filter as the lowercase put did and a NEW Id.
  actual: HTTP 400, Error.Code 'MalformedXML', Error.Message 'The XML you provided was not well-formed or did not validate against our published schema'.
  The preceding assertions in the same check passed, which is what pins the divergence to the capitalised spelling: the lowercase put was accepted, the names read back as ['Prefix','Suffix'], the values read back as {'Prefix':'raw/','Suffix':'.csv'}, an Id was minted, and the lowercase spelling did not survive alongside the capitalised one.
  Smithy: com.amazonaws.s3#FilterRuleName is an enum with values ['prefix','suffix']; com.amazonaws.s3#S3KeyFilter.FilterRules -> xmlName 'FilterRule', xmlFlattened; com.amazonaws.s3#NotificationConfigurationFilter.Key -> xmlName 'S3Key'.

Cause 2 — lambda__s3.invalid-notification-filter-rule-name-rejected [mismatch]:
  AssertionError: a filter rule named anything other than prefix or suffix must be rejected ('InvalidArgument', 'filter rule name must be either prefix or suffix'), got ('MalformedXML', 'The XML you provided was not well-formed or did not validate against our published schema') — an ignored unknown rule leaves a filter that is broader than the one the caller wrote
  expected: HTTP 400, Error.Code 'InvalidArgument', Error.Message 'filter rule name must be either prefix or suffix', and the previously stored configuration unchanged.
  actual: HTTP 400, Error.Code 'MalformedXML', Error.Message 'The XML you provided was not well-formed or did not validate against our published schema'.
  The control arm of the same check passed: a put whose FilterRules are [{Name:'prefix',Value:'raw/'},{Name:'suffix',Value:'.csv'}] was accepted and read back with names {'Prefix','Suffix'}.

## Steps to Reproduce
Boot the emulator and replay the two check ids in trace_ids from suite simparity-behavioral-aws-v1.

By hand (one bucket, one Lambda function whose resource policy grants s3.amazonaws.com lambda:InvokeFunction for that bucket ARN):
1. Cause 1: PutBucketNotificationConfiguration with LambdaFunctionConfigurations[0] = {LambdaFunctionArn: <fn arn>, Events: ['s3:ObjectCreated:*'], Filter.Key.FilterRules: [{Name:'prefix',Value:'raw/'},{Name:'suffix',Value:'.csv'}]} — it succeeds; GetBucketNotificationConfiguration reports FilterRules names ['Prefix','Suffix']. Now re-send the IDENTICAL configuration with the names spelled 'Prefix' and 'Suffix' (i.e. feed the read back into the put): it is refused 400 MalformedXML instead of answering 200, converging on the same stored Filter and minting a new Id.
2. Cause 2: with that configuration stored, PutBucketNotificationConfiguration with Filter.Key.FilterRules = [{Name:'Prefix2',Value:'raw/'}] — observe 400 MalformedXML / 'The XML you provided was not well-formed or did not validate against our published schema' instead of 400 InvalidArgument / 'filter rule name must be either prefix or suffix'; then confirm GetBucketNotificationConfiguration still reports the previously stored configuration untouched.
