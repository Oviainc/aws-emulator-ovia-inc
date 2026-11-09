Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__transcribe.deploy-transcribe-call-analytics-category-duplicate-name-rejected-leaving-original cloudformation__transcribe.deploy-transcribe-call-analytics-category-round-trip-and-delete cloudformation__transcribe.deploy-transcribe-medical-vocabulary-round-trip-and-delete cloudformation__transcribe.deploy-transcribe-vocabulary-filter-duplicate-name-rejected-leaving-original cloudformation__transcribe.deploy-transcribe-vocabulary-filter-round-trip-and-delete cloudformation__transcribe.deploy-transcribe-vocabulary-round-trip-and-delete
Issue-Key: cloudformation__transcribe|CreateStack|ValidationError
Issue-Commit: 67d35df6c1acce919d84587dd79dfbb9f77fdc15

# cloudformation__transcribe: no AWS::Transcribe::* resource type is registered — CreateStack refuses every Vocabulary/VocabularyFilter/MedicalVocabulary/CallAnalyticsCategory template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `67d35df6c1acce919d84587dd79dfbb9f77fdc15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's template resource-type registry knows none of the AWS::Transcribe::* resource types, so CreateStack refuses the whole template at validation time and no Amazon Transcribe resource can ever be deployed, read back, rolled back or deleted through CloudFormation. Failing check ids: cloudformation__transcribe.deploy-transcribe-vocabulary-round-trip-and-delete, cloudformation__transcribe.deploy-transcribe-vocabulary-filter-round-trip-and-delete, cloudformation__transcribe.deploy-transcribe-vocabulary-filter-duplicate-name-rejected-leaving-original, cloudformation__transcribe.deploy-transcribe-medical-vocabulary-round-trip-and-delete, cloudformation__transcribe.deploy-transcribe-call-analytics-category-round-trip-and-delete, cloudformation__transcribe.deploy-transcribe-call-analytics-category-duplicate-name-rejected-leaving-original.

AWS: Smithy (cloudformation): CreateStack takes TemplateBody/TemplateURL and models only AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException and TokenAlreadyExistsException; the generic service-common ValidationError is the refusal used for a template CloudFormation cannot parse, so a template whose resource types are ALL registered is accepted with HTTP 200 and a StackId. The resource-type registry itself is not in the Smithy model, so for the type list this is recorded behavior only: AWS registers AWS::Transcribe::Vocabulary, AWS::Transcribe::VocabularyFilter, AWS::Transcribe::MedicalVocabulary and AWS::Transcribe::CallAnalyticsCategory. CreateStack on a template declaring any of them returns 200 + StackId and the stack then reaches a terminal status — CREATE_COMPLETE for a valid template, with Ref and Fn::GetAtt Arn resolving to the resource's documented Transcribe ARN, Fn::GetAtt VocabularyState resolving to READY, and the resource really existing in Transcribe (listed by ListVocabularies / ListVocabularyFilters / ListMedicalVocabularies / ListCallAnalyticsCategories, its DownloadUri serving the declared Phrases/Words/entry rows); and ROLLBACK_COMPLETE with a per-resource CREATE_FAILED row whose ResourceStatusReason is the registry-handler form 'Resource handler returned message: "<the Transcribe API refusal message>" (RequestToken: <uuid>, HandlerErrorCode: <code>)' when the handler refuses, e.g. a second stack reusing a CategoryName/VocabularyFilterName the account already holds, which must leave the original resource untouched. Deleting the stack must delete the resource from Transcribe.

Emulator: every one of the six CreateStack calls is refused before any stack exists, with HTTP 400, error code ValidationError and the message "Template format error: Unrecognized resource types: [<type list>]". The fixed text is "Template format error: Unrecognized resource types: [" + "]"; the only variable part is the list of unregistered types taken from the submitted template — observed as [AWS::Transcribe::Vocabulary], [AWS::Transcribe::VocabularyFilter], [AWS::Transcribe::MedicalVocabulary] and [AWS::Transcribe::CallAnalyticsCategory]. Because the template is rejected wholesale, nothing downstream is exercised: no StackId, no stack events, no Outputs, no Ref/Fn::GetAtt resolution, no Transcribe-side resource, no duplicate-name handler refusal and no delete propagation.

Why: a client or CDK app that manages Transcribe custom vocabularies, vocabulary filters, medical vocabularies or Call Analytics categories as infrastructure-as-code cannot deploy at all. The stack is never created, so there is nothing to wait on, nothing to roll back and nothing to delete; and a template that mixes Transcribe resources with other services fails as a whole rather than partially. The documented way to learn that a name is already taken (a per-resource CREATE_FAILED row carrying the handler message and HandlerErrorCode, with the original resource preserved) is unreachable.

Fix: the CloudFormation crate's template resource-type registry and the resource handlers it dispatches to — register AWS::Transcribe::Vocabulary, ::VocabularyFilter, ::MedicalVocabulary and ::CallAnalyticsCategory with create/update/delete handlers plus Ref and Fn::GetAtt attribute resolution routed into the transcribe service state, so CreateStack accepts the template and a handler refusal becomes a per-resource CREATE_FAILED event and ROLLBACK_COMPLETE instead of a whole-template parse refusal.

Traces: the six check ids in trace_ids; each fails at its first CreateStack. No open issue carries these checks (list_existing_issues has no cloudformation__transcribe entry); the many open "unrecognized resource type" issues filed for other namespaces — e.g. 20261109T161636Z-cloudformation-route53resolver-no-aws-route53resolver-resource-type-is.md — describe the same class of gap for different type namespaces and do not cover AWS::Transcribe::*.

## Evidence
All six checks fail at the first CreateStack, before a stack id exists.

cloudformation__transcribe.deploy-transcribe-vocabulary-round-trip-and-delete
  expected: CreateStack accepted (200 + StackId); stack reaches CREATE_COMPLETE; Ref and Fn::GetAtt Arn equal the vocabulary's Transcribe ARN, Fn::GetAtt VocabularyState = READY, the name is returned by ListVocabularies and the DownloadUri content holds exactly the declared Phrases; DeleteStack removes the vocabulary from Transcribe.
  actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Transcribe::Vocabulary]

cloudformation__transcribe.deploy-transcribe-vocabulary-filter-round-trip-and-delete
  actual: (ValidationError) CreateStack: Template format error: Unrecognized resource types: [AWS::Transcribe::VocabularyFilter]

cloudformation__transcribe.deploy-transcribe-vocabulary-filter-duplicate-name-rejected-leaving-original
  expected: first stack CREATE_COMPLETE; the second stack reusing the same VocabularyFilterName reaches ROLLBACK_COMPLETE with a CREATE_FAILED row whose reason matches 'Resource handler returned message: "<message>" (RequestToken: <uuid>, HandlerErrorCode: <code>)', the original filter unchanged.
  actual: (ValidationError) CreateStack: Template format error: Unrecognized resource types: [AWS::Transcribe::VocabularyFilter]

cloudformation__transcribe.deploy-transcribe-medical-vocabulary-round-trip-and-delete
  actual: (ValidationError) CreateStack: Template format error: Unrecognized resource types: [AWS::Transcribe::MedicalVocabulary]

cloudformation__transcribe.deploy-transcribe-call-analytics-category-round-trip-and-delete
  actual: (ValidationError) CreateStack: Template format error: Unrecognized resource types: [AWS::Transcribe::CallAnalyticsCategory]

cloudformation__transcribe.deploy-transcribe-call-analytics-category-duplicate-name-rejected-leaving-original
  actual: (ValidationError) CreateStack: Template format error: Unrecognized resource types: [AWS::Transcribe::CallAnalyticsCategory]

In every case AWS answers 200 with a StackId for these templates; the ValidationError / "Unrecognized resource types" refusal is emulator-only.

## Steps to Reproduce
1. cloudformation:CreateStack with TemplateBody {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Voc":{"Type":"AWS::Transcribe::Vocabulary","Properties":{"VocabularyName":"<name>","LanguageCode":"en-US","Phrases":[...]}}},"Outputs":{"Ref":{"Value":{"Ref":"Voc"}},"Arn":{"Value":{"Fn::GetAtt":["Voc","Arn"]}},"State":{"Value":{"Fn::GetAtt":["Voc","VocabularyState"]}}}} -> the call raises ValidationError "Template format error: Unrecognized resource types: [AWS::Transcribe::Vocabulary]" instead of returning a StackId.
2. Repeat with Type AWS::Transcribe::VocabularyFilter (Properties VocabularyFilterName/LanguageCode/Words), AWS::Transcribe::MedicalVocabulary (VocabularyName/LanguageCode/VocabularyFileUri) and AWS::Transcribe::CallAnalyticsCategory (CategoryName/Rules) — each is refused with the same code and message naming the type it does not recognize.
3. Deploy the VocabularyFilter / CallAnalyticsCategory template twice with the same resource name: AWS accepts both CreateStack calls and fails only the second stack's resource with a handler reason; the emulator never gets past step 1.
4. Replay: run_behavioral_suite and re-run the six checks in trace_ids.
