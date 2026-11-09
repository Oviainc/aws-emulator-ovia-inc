Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__translate.deploy-translate-parallel-data-missing-s3-key-rejected-and-rolled-back cloudformation__s3__translate.deploy-translate-parallel-data-round-trip-and-delete cloudformation__translate.deploy-translate-terminology-round-trip-and-delete
Issue-Key: cloudformation__translate|CreateStack|ValidationError
Issue-Commit: 785433b7ada0bbe790b60a1839b6b0233a99f365

# cloudformation__translate: 2 net-new causes — AWS::Translate::ParallelData and AWS::Translate::Terminology are unregistered resource types, so CreateStack refuses every Translate template

> _Note: this issue was identified against the code base at commit `785433b7ada0bbe790b60a1839b6b0233a99f365`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
The emulator's CloudFormation template processor knows no AWS::Translate::* resource type. Every stack that declares one is refused at CreateStack, before any stack is created, so none of the deploy checks for Translate resources can get past their first call. Two resource types are involved; each needs its own registration and resource handler, so they are two causes.

Cause 1 of 2: AWS::Translate::ParallelData is not a registered resource type.
AWS: CloudFormation supports AWS::Translate::ParallelData (https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-translate-paralleldata.html). CreateStack accepts a template declaring it and returns a StackId; the resource handler calls Translate CreateParallelData with the declared Name/Description/ParallelDataConfig{S3Uri,Format} and propagates both the resource Tags and the stack-level Tags. Ref returns the parallel-data ARN and Fn::GetAtt exposes Status, SourceLanguageCode and TargetLanguageCodes; the created parallel data is then visible to Translate ListParallelData/GetParallelData and is removed on stack delete. When the S3Uri names a key that does not exist, the resource goes CREATE_FAILED and the stack reaches ROLLBACK_COMPLETE with the handler reason carrying Translate's own message — fixed text "INPUT_BUCKET_NOT_FOUND: The provided input S3 bucket does not exist." followed by the SDK exception suffix "(Service: Translate; Operation: CreateParallelData; Status Code: 400; Error Code: InvalidRequestException; Request ID: <per-run id>)" and HandlerErrorCode InvalidRequest; after rollback the name is absent from ListParallelData and GetParallelData answers ResourceNotFoundException "INVALID_REQUEST: Parallel Data does not exist.". (The Request ID inside that suffix is a per-run value, not part of the requirement; Smithy models no ValidationError on CreateStack — it is CloudFormation's common 400 ValidationError, so the AWS acceptance here is recorded behavior plus the CFN resource reference.)
Emulator: CreateStack is refused outright with HTTP 400, error code ValidationError and message "Template format error: Unrecognized resource types: [AWS::Translate::ParallelData]". The fixed text "Template format error: Unrecognized resource types: " with the bracketed list of types is CloudFormation's wording for a type it genuinely does not know — the defect is that it is emitted for a type AWS does support, not the wording itself.
Why: a client cannot deploy Translate parallel data through CloudFormation/CDK at all. The failure arrives as a synchronous template-format ValidationError, which a client reads as "my template is wrong", so it has no rollback, no stack events and no resource to inspect; neither the happy path nor the rejection path of a Translate deployment can be exercised.
Fix: CloudFormation's template resource-type registry plus a new AWS::Translate::ParallelData resource handler (create/read/delete, Ref = ARN, GetAtt Status/SourceLanguageCode/TargetLanguageCodes, tag propagation, and mapping Translate's CreateParallelData refusal onto the CREATE_FAILED handler reason).
Traces: cloudformation__s3__translate.deploy-translate-parallel-data-round-trip-and-delete, cloudformation__s3__translate.deploy-translate-parallel-data-missing-s3-key-rejected-and-rolled-back.

Cause 2 of 2: AWS::Translate::Terminology is not a registered resource type.
AWS: CloudFormation supports AWS::Translate::Terminology. CreateStack accepts a template declaring it with Name, Description, TerminologyData{File (base64 CSV), Format} and Tags; the handler calls Translate ImportTerminology, Ref returns the terminology ARN, and Fn::GetAtt exposes SourceLanguageCode, TargetLanguageCodes and Format derived from the imported file. The imported terminology is then visible to ListTerminologies and GetTerminology (Arn, SourceLanguageCode, TargetLanguageCodes, Format, TermCount, Description) with the resource and stack tags attached, and deleting the stack removes it, after which GetTerminology answers ResourceNotFoundException whose message is the fixed text "The Terminology for <account> <name> was not found" with the caller's account id and the requested terminology name filled in. (Recorded behavior plus the CFN resource reference; Smithy models no ValidationError on CreateStack.)
Emulator: CreateStack is refused with HTTP 400, error code ValidationError and message "Template format error: Unrecognized resource types: [AWS::Translate::Terminology]".
Why: custom terminology cannot be managed as infrastructure-as-code. Any stack or CDK app that bundles a terminology with the rest of a translation pipeline fails at submit time, so the whole stack — including unrelated resources in it — never deploys.
Fix: CloudFormation's template resource-type registry plus a new AWS::Translate::Terminology resource handler (create via ImportTerminology, read, delete, Ref = ARN, GetAtt SourceLanguageCode/TargetLanguageCodes/Format, tag propagation).
Traces: cloudformation__translate.deploy-translate-terminology-round-trip-and-delete.

No open issue covers these: the existing Translate issues (e.g. 20261029T192448Z-translate-translatetext-returns-the-source-text-with-a-synthetic.md, 20261109T165048Z-translate-getterminology-presigned-data-location-points-at-amazonaws.md) are data-plane/representation defects in the Translate service itself and say nothing about CloudFormation resource types.

## Evidence
Expected (AWS): CreateStack with a template whose Resources declare Type "AWS::Translate::ParallelData" or "AWS::Translate::Terminology" is accepted and returns a StackId; the stack then reaches CREATE_COMPLETE (or ROLLBACK_COMPLETE with Translate's handler reason when the input S3 key is missing).

Actual (emulator), all three checks fail on the very first CreateStack:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Translate::ParallelData]
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Translate::Terminology]

Status 400, error code ValidationError. Fixed text of the diverging message: "Template format error: Unrecognized resource types: [" + the comma-separated list of resource types from the template's Resources + "]"; the variable part is the type list taken from the request. No stack id is returned, so DescribeStacks/DescribeStackEvents have nothing to report and the downstream Translate assertions (ListParallelData/GetParallelData, ListTerminologies/GetTerminology, Ref/GetAtt outputs, tag propagation, delete behavior) are never reached.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. ParallelData: create an S3 bucket, put a CSV object at parallel-data/input.csv, then call cloudformation CreateStack with TemplateBody = {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Pd":{"Type":"AWS::Translate::ParallelData","Properties":{"Name":"<name>","Description":"...","ParallelDataConfig":{"S3Uri":"s3://<bucket>/parallel-data/input.csv","Format":"CSV"},"Tags":[{"Key":"k","Value":"v"}]}}},"Outputs":{"PdArn":{"Value":{"Ref":"Pd"}},"Status":{"Value":{"Fn::GetAtt":"Pd.Status"}}}}.
   Observe ValidationError "Template format error: Unrecognized resource types: [AWS::Translate::ParallelData]" instead of a StackId.
3. Repeat step 2 with S3Uri naming a key that does not exist; AWS reaches ROLLBACK_COMPLETE with the Translate INPUT_BUCKET_NOT_FOUND handler reason, the emulator refuses CreateStack identically.
4. Terminology: call CreateStack with TemplateBody = {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Term":{"Type":"AWS::Translate::Terminology","Properties":{"Name":"<name>","Description":"...","TerminologyData":{"File":"<base64 of \"en,es\\nconference room,sala de reuniones\\n\">","Format":"CSV"},"Tags":[{"Key":"k","Value":"v"}]}}},"Outputs":{"TermArn":{"Value":{"Ref":"Term"}},"Format":{"Value":{"Fn::GetAtt":"Term.Format"}}}}.
   Observe ValidationError "Template format error: Unrecognized resource types: [AWS::Translate::Terminology]".
