Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__rekognition.deploy-rekognition-collection-duplicate-id-rejected cloudformation__rekognition.deploy-rekognition-collection-project-and-dataset-round-trip-and-delete
Issue-Key: cloudformation__rekognition.deploy-rekognition-collection-duplicate-id-rejected|run#10|AssertionError|assert listed == [out["CollId"]] == [shared_id], (
Issue-Key: cloudformation__rekognition|CreateStack|ValidationError
Issue-Commit: ab58585e96332c13ed2b8d30023e29e5d03b1c39

# cloudformation__rekognition: 2 net-new causes — CreateStack refuses AWS::Rekognition::Dataset as an unrecognized resource type, and AWS::Rekognition::Collection is never provisioned (Ref is a synthesized physical id, ListCollections sees nothing)

> _Note: this issue was identified against the code base at commit `ab58585e96332c13ed2b8d30023e29e5d03b1c39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two independent defects in CloudFormation's handling of the AWS::Rekognition::* resource types. Both were reached by behavioral checks in the `cloudformation__rekognition` suite; neither is marked ALREADY FILED or OWNED, and no open issue covers them (the nearest open issues are the per-service "CreateStack refuses <type> as an unrecognized resource type" reports for other namespaces, e.g. 20261109T113214Z-cloudformation-ivs-mediatailor-… and 20261109T113512Z-cloudformation-network-firewall-…, which do not mention Rekognition).

Cause 1 of 2: AWS::Rekognition::Dataset is not a registered resource type.
  AWS: CloudFormation registers AWS::Rekognition::Dataset (CloudFormation Template Reference, aws-resource-rekognition-dataset) and accepts a template declaring one with Properties ProjectArn + DatasetType; the resource is created by calling Rekognition CreateDataset (Smithy: CreateDataset input ProjectArn @required pattern ^arn:[a-z\d-]+:rekognition:[a-z\d-]+:\d{12}:project/[a-zA-Z0-9_.\-]{1,255}/[0-9]+$, DatasetType @required enum; output DatasetArn pattern …/dataset/(train|test)/[0-9]+). The stack then reaches CREATE_COMPLETE and the dataset reads back from DescribeDataset with Status CREATE_COMPLETE and appears in the owning project's DescribeProjects ProjectDescriptions[].Datasets.
  Emulator: CreateStack is refused outright with HTTP 400 / Code `ValidationError` and Message `Template format error: Unrecognized resource types: [AWS::Rekognition::Dataset]`. Fixed text of that message: "Template format error: Unrecognized resource types: [" … "]"; the only variable part is the bracketed list of type names taken from the request's template — here AWS::Rekognition::Dataset. AWS emits that message only for types it genuinely does not know, so emitting it for a registered type is the defect.
  Why: a client cannot deploy any Rekognition Custom Labels dataset from a template. The whole stack is rejected at submit time, so the Collection and Project resources declared alongside it are never created either, and the dataset's own update/delete lifecycle (an UpdateStack that drops the dataset, leaving its project) is unreachable.
  Fix: the CloudFormation resource-type registry plus a provisioner for AWS::Rekognition::Dataset (create -> rekognition CreateDataset, read -> DescribeDataset, delete -> DeleteDataset), including Ref -> DatasetArn.
  Traces: cloudformation__rekognition.deploy-rekognition-collection-project-and-dataset-round-trip-and-delete.

Cause 2 of 2: an AWS::Rekognition::Collection in a CREATE_COMPLETE stack does not exist in Rekognition, and its Ref is a generated physical id rather than the declared CollectionId.
  AWS: provisioning AWS::Rekognition::Collection calls Rekognition CreateCollection with the template's CollectionId (Smithy: CreateCollection input CollectionId @required, @length 1-255, @pattern ^[a-zA-Z0-9_.\-]+$; errors include ResourceAlreadyExistsException). After CREATE_COMPLETE the collection is a real collection: ListCollections CollectionIds contains that id, DescribeCollection answers 200 with CollectionARN arn:<partition>:rekognition:<region>:<account>:collection/<CollectionId>. Per the Template Reference for aws-resource-rekognition-collection, `Ref` on the resource returns the CollectionId — CollectionId is the resource's physical id, so CloudFormation never generates a name for it. Consequently a second stack declaring the same CollectionId fails name-conflict validation and rolls back (ROLLBACK_COMPLETE) with a VALIDATION_ERROR operation event whose ValidationName is NAME_CONFLICT_VALIDATION, ValidationStatus FAILED, ValidationFailureMode FAIL, ValidationPath /Resources/<logical id> and ValidationStatusReason of the fixed form "Resource of type '<resource type>' with identifier '<identifier>' already exists." (the type and the identifier are taken from the request's template; the rest is fixed text), leaving the original collection untouched.
  Emulator: the stack reaches CREATE_COMPLETE, but no collection is created. ListCollections returns [] (no entry matching the declared CollectionId), and the stack output built from {"Ref": "Coll"} is a CloudFormation-synthesized physical name of the form <stack-name>-<logical-id>-<random suffix> instead of the declared CollectionId. Because nothing is ever registered under the declared id, the duplicate-id stack has no conflict to detect, so the NAME_CONFLICT_VALIDATION refusal above can never be produced either.
  Why: a client that deploys a collection by template and then addresses it by its declared CollectionId (DescribeCollection, IndexFaces, SearchFaces) gets ResourceNotFoundException for a resource CloudFormation reported as created; a client that reads the stack output expecting the collection id gets a name that is not a Rekognition collection id; and a template that would be refused on AWS for reusing an existing CollectionId is silently deployed twice.
  Fix: the CloudFormation provisioner for AWS::Rekognition::Collection (create must call Rekognition CreateCollection and use CollectionId as the physical resource id; Ref must resolve to it; GetAtt Arn to the collection ARN; delete to DeleteCollection) and the stack's name-conflict pre-validation for custom-named resources.
  Traces: cloudformation__rekognition.deploy-rekognition-collection-duplicate-id-rejected (ListCollections [], Ref 'sim2real-rekdupgood-<suffix>-Coll-<suffix>'), and the same Collection assertion in cloudformation__rekognition.deploy-rekognition-collection-project-and-dataset-round-trip-and-delete once Cause 1 no longer blocks it.

## Evidence
Cause 1 — check cloudformation__rekognition.deploy-rekognition-collection-project-and-dataset-round-trip-and-delete.
Template submitted (abridged): Resources {Coll: AWS::Rekognition::Collection {CollectionId: <id>}, Proj: AWS::Rekognition::Project {ProjectName: <name>}, Ds: AWS::Rekognition::Dataset {ProjectArn: {Fn::GetAtt: [Proj, Arn]}, DatasetType: TRAIN}}.
Expected (AWS): CreateStack accepted; stack reaches CREATE_COMPLETE; DescribeDataset(DatasetArn=Outputs.DsArn).DatasetDescription.Status == CREATE_COMPLETE; DescribeProjects(ProjectNames=[name]).ProjectDescriptions[0].Datasets == [(DsArn, 'TRAIN', 'CREATE_COMPLETE')].
Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Rekognition::Dataset]  — i.e. HTTP 400, Code ValidationError. Note only Dataset is listed, so Collection and Project are registered types.

Cause 2 — check cloudformation__rekognition.deploy-rekognition-collection-duplicate-id-rejected.
Template submitted: Resources {Coll: AWS::Rekognition::Collection {CollectionId: <shared id>}}; Outputs {CollId: {Ref: Coll}, CollArn: {Fn::GetAtt: [Coll, Arn]}}.
Expected (AWS): stack CREATE_COMPLETE; [c for c in ListCollections.CollectionIds if c == <shared id>] == [Outputs.CollId] == [<shared id>]; DescribeCollection(<shared id>).CollectionARN == Outputs.CollArn == arn:<partition>:rekognition:<region>:<account>:collection/<shared id>; a second stack with the same CollectionId ends ROLLBACK_COMPLETE with the VALIDATION_ERROR row ('Coll', 'NAME_CONFLICT_VALIDATION', 'FAILED', 'FAIL', "Resource of type 'AWS::Rekognition::Collection' with identifier '<shared id>' already exists.", '/Resources/Coll').
Actual (emulator): stack reached CREATE_COMPLETE, then
  AssertionError: ListCollections [], Ref 'sim2real-rekdupgood-0d62160db010-Coll-7F4E64AB675E'
(ListCollections had no entry for the declared id, and Ref returned a generated <stack>-<logical id>-<suffix> physical name instead of the declared CollectionId; the generated-name values shown are per-run and are not themselves a requirement).

## Steps to Reproduce
Cause 1:
1. cfn.create_stack(StackName=<name>, TemplateBody=json of {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Coll":{"Type":"AWS::Rekognition::Collection","Properties":{"CollectionId":"<id>"}},"Proj":{"Type":"AWS::Rekognition::Project","Properties":{"ProjectName":"<name>"}},"Ds":{"Type":"AWS::Rekognition::Dataset","Properties":{"ProjectArn":{"Fn::GetAtt":["Proj","Arn"]},"DatasetType":"TRAIN"}}},"Outputs":{"DsArn":{"Value":{"Ref":"Ds"}}}}).
2. Observe HTTP 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::Rekognition::Dataset]' instead of an accepted CreateStack.

Cause 2:
1. cfn.create_stack with a template declaring only Coll = AWS::Rekognition::Collection with Properties.CollectionId = <id>, and Outputs CollId={Ref: Coll}, CollArn={Fn::GetAtt: [Coll, Arn]}.
2. Poll DescribeStacks until CREATE_COMPLETE.
3. rekognition.list_collections() -> CollectionIds does not contain <id> (expected: contains it).
4. DescribeStacks Outputs CollId -> a generated <stack>-Coll-<suffix> name (expected: <id>).
5. rekognition.describe_collection(CollectionId=<id>) -> ResourceNotFoundException (expected: 200 with CollectionARN arn:<partition>:rekognition:<region>:<account>:collection/<id>).
6. Create a second stack with the same CollectionId -> reaches CREATE_COMPLETE (expected: ROLLBACK_COMPLETE with the NAME_CONFLICT_VALIDATION VALIDATION_ERROR event).
