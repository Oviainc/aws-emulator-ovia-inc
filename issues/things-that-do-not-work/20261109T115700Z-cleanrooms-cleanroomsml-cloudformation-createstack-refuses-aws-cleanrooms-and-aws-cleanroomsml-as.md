Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cleanrooms__cleanroomsml__cloudformation.deploy-cfn-cleanroomsml-algorithm-and-association-round-trip-and-delete cleanrooms__cleanroomsml__cloudformation.deploy-cfn-cleanroomsml-association-nonexistent-algorithm-rejected
Issue-Key: cleanrooms__cleanroomsml__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# CreateStack refuses AWS::CleanRooms::* and AWS::CleanRoomsML::* as unrecognized resource types, so no Clean Rooms / Clean Rooms ML estate can be deployed from a template

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's resource-type registry in the emulator does not carry the Clean Rooms and Clean Rooms ML resource types, so template validation refuses every stack that declares one and the whole IaC path into those two services is unreachable. Failing checks: cleanrooms__cleanroomsml__cloudformation.deploy-cfn-cleanroomsml-algorithm-and-association-round-trip-and-delete, cleanrooms__cleanroomsml__cloudformation.deploy-cfn-cleanroomsml-association-nonexistent-algorithm-rejected.

AWS: AWS::CleanRooms::Collaboration, AWS::CleanRooms::Membership, AWS::CleanRoomsML::ConfiguredModelAlgorithm and AWS::CleanRoomsML::ConfiguredModelAlgorithmAssociation are registered CloudFormation resource types. CreateStack therefore accepts a template declaring them (with CAPABILITY_IAM for the stack's IAM role), returns a StackId, and provisions each one through the owning service's control plane: the collaboration and membership through Clean Rooms CreateCollaboration/CreateMembership, the algorithm and its association through Clean Rooms ML CreateConfiguredModelAlgorithm/CreateConfiguredModelAlgorithmAssociation. Ref on each resource yields the identifier/ARN the service read returns, Fn::GetAtt CollaborationIdentifier on the association yields the collaboration id, an update that removes the association deletes only the association, and the stack delete removes the rest (a Retain membership surviving as COLLABORATION_DELETED). When a declared association names a ConfiguredModelAlgorithmArn that does not exist, the resource create is refused by Clean Rooms ML and the stack reaches ROLLBACK_COMPLETE with the handler's reason carrying CleanRoomsML's own NotFound ("Configured Model Algorithm: <arn> not found." with the service/status/request-id suffix CloudFormation appends). Smithy: the type registry is not part of the CloudFormation Smithy model — recorded behavior only for the registry itself; CreateStack's ValidationError (modeled as the request-validation failure) is what AWS uses only for a type it genuinely does not know.

Emulator: CreateStack is refused outright with HTTP 400, error code ValidationError and message "Template format error: Unrecognized resource types: [AWS::CleanRoomsML::ConfiguredModelAlgorithm, AWS::CleanRooms::Collaboration, AWS::CleanRooms::Membership, AWS::CleanRoomsML::ConfiguredModelAlgorithmAssociation]". In that message the fixed text is "Template format error: Unrecognized resource types: [" ... "]"; the variable part is the list of type names, taken verbatim from the Resources block of the submitted template. No stack is created, so nothing downstream of CreateStack in either check is ever exercised. Both checks die on the first CreateStack call, including the positive control stack of the rejection check.

Why: a client that provisions Clean Rooms collaborations/memberships or Clean Rooms ML configured-model algorithms and associations with CloudFormation (or CDK, which emits the same types) cannot deploy at all: it gets a template-format ValidationError for a template AWS accepts. There is no workaround short of abandoning IaC for direct API calls, and the rejection is reported as a malformed template, which sends a user looking for an authoring mistake that does not exist.

Fix: CloudFormation's template resource-type registry/validation (the component that decides whether a declared Type is known) must register these four types, and the resource provisioners that back them must call the Clean Rooms and Clean Rooms ML state the emulator already serves, including Ref/GetAtt attribute mapping, update-time delete of a removed child resource, DeletionPolicy=Retain, and propagation of the owning service's refusal into the resource's CREATE_FAILED reason so the stack rolls back.

Traces: the two checks named above, both failing at their first CreateStack with the same ValidationError.

## Evidence
Expected (AWS): CreateStack returns a StackId and the stack reaches CREATE_COMPLETE; the declared AWS::CleanRooms::Collaboration / ::Membership and AWS::CleanRoomsML::ConfiguredModelAlgorithm / ::ConfiguredModelAlgorithmAssociation resources are provisioned and readable through GetCollaboration, GetMembership, GetConfiguredModelAlgorithm and ListConfiguredModelAlgorithmAssociations; for an association naming a nonexistent algorithm ARN the stack reaches ROLLBACK_COMPLETE with the resource reason "Configured Model Algorithm: <arn> not found. (Service: CleanRoomsMl, Status Code: 404, Request ID: <id>) (SDK Attempt Count: <n>)" and status reason code NotFound.

Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CleanRoomsML::ConfiguredModelAlgorithm, AWS::CleanRooms::Collaboration, AWS::CleanRooms::Membership, AWS::CleanRoomsML::ConfiguredModelAlgorithmAssociation]

Identical message from both checks, raised on the first CreateStack call, so no stack id, no stack events and no resource reads were ever produced.

## Steps to Reproduce
1. Build and boot the emulator; point a botocore client for cloudformation, cleanrooms and cleanroomsml at it.
2. Call CreateStack with Capabilities=["CAPABILITY_IAM"] and a template whose Resources declare, alongside an AWS::IAM::Role: "Algorithm" of Type AWS::CleanRoomsML::ConfiguredModelAlgorithm (Name, RoleArn via Fn::GetAtt, TrainingContainerConfig.ImageUri), "Collab" of Type AWS::CleanRooms::Collaboration (Name, Description, CreatorDisplayName, QueryLogStatus DISABLED, CreatorMemberAbilities [CAN_QUERY, CAN_RECEIVE_RESULTS], CreatorMLMemberAbilities, Members []), "Membership" of Type AWS::CleanRooms::Membership with DeletionPolicy Retain (CollaborationIdentifier via Ref, QueryLogStatus DISABLED) and "Association" of Type AWS::CleanRoomsML::ConfiguredModelAlgorithmAssociation (MembershipIdentifier via Ref, ConfiguredModelAlgorithmArn, Name).
3. Observe the call is refused with HTTP 400 ValidationError "Template format error: Unrecognized resource types: [...]" instead of returning a StackId.
4. Repeat with the Association's ConfiguredModelAlgorithmArn set to an ARN that does not exist: the same CreateStack refusal occurs, so the expected ROLLBACK_COMPLETE path and the CleanRoomsML NotFound resource reason are never reached either.
