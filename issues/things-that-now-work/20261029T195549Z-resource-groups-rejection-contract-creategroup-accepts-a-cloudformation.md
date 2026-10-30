Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resource-groups.create-group-cloudformation-query-missing-stack-identifier-rejected resource-groups.create-group-mutually-exclusive-query-and-configuration-rejected resource-groups.create-group-reserved-name-prefix-rejected resource-groups.group-resources-query-based-group-rejected resource-groups.list-group-resources-type-filter-mismatch-rejected
Issue-Shape-Id: com.amazonaws.resourcegroups#Ardi
Issue-Key: resource-groups.create-group-cloudformation-query-missing-stack-identifier-rejected|_client_error#4|AssertionError|raise AssertionError(f"expected {expect} but the call SUCCEEDED and returned {result!r}")
Issue-Key: resource-groups.create-group-mutually-exclusive-query-and-configuration-rejected|run#7|AssertionError|assert message == "Please use only 'configuration' or 'query' in the request, but not both.", (
Issue-Key: resource-groups.create-group-reserved-name-prefix-rejected|run#8|AssertionError|assert message == "Group name must not start with 'AWS'", (
Issue-Key: resource-groups.group-resources-query-based-group-rejected|run#13|AssertionError|assert message_tag == "This operation does not support the target", (
Issue-Key: resource-groups.list-group-resources-type-filter-mismatch-rejected|run#13|AssertionError|assert message == (
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# resource-groups rejection contract: CreateGroup accepts a CLOUDFORMATION_STACK_1_0 query with no StackIdentifier, and four BadRequestException Messages use non-AWS wording

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
This issue bundles 5 causes, all in the resource-groups rejection path. Cause 1 is a missing validation (the emulator ACCEPTS a request AWS refuses and persists the group); causes 2-5 all answer the right HTTP status and the right error code but substitute hand-written diagnostic prose for the modelled Message, which is part of the error contract. All five must be closed before this issue is submitted.

Cause 1 of 5: CreateGroup does not validate that a CLOOUDFORMATION_STACK_1_0 (CLOUDFORMATION_STACK_1_0) ResourceQuery document names a stack.
AWS: Smithy: CreateGroup (POST /groups, success 200) -> BadRequestException (errorKind client, httpError 400); ResourceQuery.Query is documented as carrying StackIdentifier for a CLOUDFORMATION_STACK_1_0 query ("applicable only if Type = CLOUDFORMATION_STACK_1_0 ... the ARN of the CloudFormation stack whose resources you want included in the group"). A stack query that names no stack defines no membership, so AWS refuses it with HTTP 400 BadRequestException and Message 'Query not valid: Query format not valid: check JSON syntax' — the whole sentence is FIXED text, with no interpolated request value — and no group is created (a following GetGroup answers 404 NotFoundException with the fixed Message 'The specified group does not exist').
Emulator: answers HTTP 200 and CREATES the group, echoing Group.GroupArn/Name plus ResourceQuery {Type: CLOUDFORMATION_STACK_1_0, Query: '{"ResourceTypeFilters": ["AWS::AllSupported"]}'}. The shared validator crate::state::check_resource_query, called from the CreateGroup handler (services/service-com-amazonaws-resourcegroups-ardi/src/ops/create_group.rs:265-267), only proves the document is a JSON object whose Type is a modelled QueryType; it never requires StackIdentifier for a stack query. The same crate already knows the rule elsewhere — ops/search_resources.rs:499-512 (stack_identifier) rejects a stack document that names no stack — so the rule exists but is not applied on create.
Why: a client that relies on AWS to reject a structurally invalid stack query gets a 200 and a persisted group whose query can never resolve membership; the mistake surfaces much later as a permanently empty group instead of at the call site, and negative tests that assert the rejection pass against AWS and fail against the emulator (or worse, leak a group).
Fix: the CreateGroup handler's resource-query validation (the crate::state::check_resource_query path it calls), so a CLOUDFORMATION_STACK_1_0 document missing StackIdentifier is refused with 400 BadRequestException before anything is stored; the same validator is shared by UpdateGroupQuery/SearchResources/tag-sync tasks, so fix it once there.
Traces: resource-groups.create-group-cloudformation-query-missing-stack-identifier-rejected.

Cause 2 of 5: CreateGroup's ResourceQuery+Configuration mutual-exclusion Message is not AWS's sentence.
AWS: Smithy: CreateGroup input carries both ResourceQuery and Configuration (GroupConfigurationList, length 0..2) and models BadRequestException (400); AWS documents that a resource group can contain either a ResourceQuery or a Configuration, but not both. Recorded behavior: HTTP 400, code BadRequestException, Message "Please use only 'configuration' or 'query' in the request, but not both." — entirely fixed text, no request value interpolated.
Emulator: HTTP 400 BadRequestException (correct) with Message 'a resource group can contain either a ResourceQuery or a Configuration, but not both' (ops/create_group.rs:215-220).
Why: the status and code are right, but any client branching on, logging or asserting the error text sees a string AWS never emits; error-text comparison tools report the emulator as a different service.
Fix: the CreateGroup handler's mutual-exclusion branch, where the BadRequestException Message is constructed.
Traces: resource-groups.create-group-mutually-exclusive-query-and-configuration-rejected.

Cause 3 of 5: CreateGroup's reserved-name-prefix Message is not AWS's sentence.
AWS: Smithy: CreateGroupInput$Name (required, length 1..300, pattern ^[a-zA-Z0-9_\.-]+$) documents "The name cannot start with AWS, aws, or any other possible capitalization; these are reserved." Recorded behavior: HTTP 400, code BadRequestException, Message "Group name must not start with 'AWS'" — fixed text; it does NOT include the submitted name, and it is the same sentence for every capitalization of the prefix.
Emulator: HTTP 400 BadRequestException (correct) with Message 'Name <name> is not valid: a group name cannot start with a reserved prefix (any capitalization of \'aws\')' — invented prose that additionally interpolates the request's Name, which AWS never puts in this message (ops/create_group.rs:252-262). Observed in the run as "Name aws-reserved-eabb3060b9a1 is not valid: ..."; the group name there is a per-run value, the requirement is the fixed sentence with no interpolation.
Why: same as cause 2 — the rejection reason is right, the wire text a client reads is not.
Fix: the CreateGroup handler's reserved-prefix branch.
Traces: resource-groups.create-group-reserved-name-prefix-rejected.

Cause 4 of 5: GroupResources against a query-based (TAG_FILTERS_1_0) group reports the emulator's own allow-list prose instead of AWS's sentence.
AWS: Smithy: GroupResources (POST /group-resources) -> BadRequestException (400). Recorded behavior: HTTP 400, code BadRequestException, Message 'This operation does not support the target' — the complete, entirely fixed sentence, with no group name, ARN or type list in it. Membership and the group's own ResourceQuery are unchanged by the refused call.
Emulator: HTTP 400 BadRequestException (correct) with Message 'GroupResources can only be used with a group whose configuration is one of AWS::EC2::CapacityReservationPool, AWS::EC2::HostManagement, AWS::ResourceGroups::ApplicationGroup; other resource group types are not supported by this operation' — invented prose that leaks the emulator's internal allow-list.
Why: same as cause 2.
Fix: the GroupResources handler's target-eligibility branch. NOTE: open issue 20261029T191807Z-resource-groups-groupresources-and-config-query-conflict-errors-reply-with describes the same handler branch from the wire corpus (its Cause 5, reached via a configuration-typed group); this behavioral check reaches it via a TAG_FILTERS_1_0 query-only group and is the behavioral grounding for the same fix. If that issue is fixed first, this cause closes with it — verify with the check id below.
Traces: resource-groups.group-resources-query-based-group-rejected.

Cause 5 of 5: ListGroupResources' incompatible-resource-type-filter Message is not AWS's sentence.
AWS: Smithy: ListGroupResources (POST /list-group-resources) -> BadRequestException (400); a resource-type Filter selects among the types the group's own query admits. Recorded behavior: HTTP 400, code BadRequestException, Message 'Filters not valid: The resource types specified as filters in the request are incompatible with the group query: <type>[, <type>...]'. Fixed text: 'Filters not valid: The resource types specified as filters in the request are incompatible with the group query:'; variable part: the offending resource-type values taken from the request's Filters (in the run, AWS::EC2::Instance against a group whose query admits only AWS::SQS::Queue).
Emulator: HTTP 400 BadRequestException (correct) with Message 'The resource types specified as filters in the request are not valid. AWS::EC2::Instance' — it drops the 'Filters not valid: ' prefix, replaces 'are incompatible with the group query:' with 'are not valid.', and separates the type list with '. ' instead of ': ' (ops/list_group_resources.rs:253-264).
Why: same as cause 2; additionally the emulator's wording says the type is invalid rather than incompatible with this group's query, which misdirects a caller debugging the filter.
Fix: the ListGroupResources handler's filter/query compatibility branch, where the BadRequestException Message is formatted.
Traces: resource-groups.list-group-resources-type-filter-mismatch-rejected.

No open issue covers causes 1, 2, 3 or 5: the existing resource-groups issues concern response shapes, Filters bounds and tagging-API error text (20261029T094111Z-resource-groups-listgroupresources-validation--r2, 20261029T191807Z-resource-groups-unsupported-resource-types-are-grouped-and-listed-instead, 20261029T191807Z-resource-groups-groupconfiguration-round-trip-loses-generic-parameters-and-groupidentifier), not these rejection paths.

## Evidence
Replay of simparity-behavioral-aws-v1 (resource-groups 9/20 passed), all five checks verdict=mismatch:
- create-group-cloudformation-query-missing-stack-identifier-rejected: 'AssertionError: expected BadRequestException but the call SUCCEEDED and returned {ResponseMetadata: {HTTPStatusCode: 200 ...}, Group: {GroupArn: arn:aws:resource-groups:us-west-2:000000000000:group/sim2real-rg-cfnbadquery-..., Name: sim2real-rg-cfnbadquery-...}, ResourceQuery: {Type: CLOUDFORMATION_STACK_1_0, Query: {"ResourceTypeFilters": ["AWS::AllSupported"]}}, Tags: {}}'. Expected: 400 BadRequestException, Message 'Query not valid: Query format not valid: check JSON syntax', and a subsequent GetGroup answering 404 NotFoundException 'The specified group does not exist'. The group name/ARN/account in the capture are per-run values.
- create-group-mutually-exclusive-query-and-configuration-rejected: expected Message "Please use only 'configuration' or 'query' in the request, but not both.", got 'a resource group can contain either a ResourceQuery or a Configuration, but not both'.
- create-group-reserved-name-prefix-rejected: expected Message "Group name must not start with 'AWS'", got "Name aws-reserved-eabb3060b9a1 is not valid: a group name cannot start with a reserved prefix (any capitalization of 'aws')" (name is per-run).
- group-resources-query-based-group-rejected: expected Message 'This operation does not support the target', got 'GroupResources can only be used with a group whose configuration is one of AWS::EC2::CapacityReservationPool, AWS::EC2::HostManagement, AWS::ResourceGroups::ApplicationGroup; other resource group types are not supported by this operation'.
- list-group-resources-type-filter-mismatch-rejected: expected Message 'Filters not valid: The resource types specified as filters in the request are incompatible with the group query: AWS::EC2::Instance', got 'The resource types specified as filters in the request are not valid. AWS::EC2::Instance'.
In every one of causes 2-5 the HTTP status (400) and the error code (BadRequestException) already match; the sole divergence is $.Message. In cause 1 the whole refusal is missing and the side effect (a created group) is present.

## Steps to Reproduce
1. CreateGroup with ResourceQuery={Type: 'CLOUDFORMATION_STACK_1_0', Query: '{"ResourceTypeFilters":["AWS::AllSupported"]}'} (no StackIdentifier). Expect 400 BadRequestException 'Query not valid: Query format not valid: check JSON syntax'; observe 200 and a created group. Then GetGroup on that name: AWS answers 404 NotFoundException 'The specified group does not exist'.
2. CreateGroup with BOTH a well-formed TAG_FILTERS_1_0 ResourceQuery and a Configuration=[{Type: 'AWS::ResourceGroups::Generic', Parameters: [...]}]. Expect 400 with Message "Please use only 'configuration' or 'query' in the request, but not both."
3. CreateGroup with Name='aws-<suffix>' and again with Name='AwS-<suffix>'. Expect 400 with Message "Group name must not start with 'AWS'" in both cases.
4. CreateGroup a TAG_FILTERS_1_0 query group, then GroupResources on it with any ResourceArns. Expect 400 with Message 'This operation does not support the target'; membership and GetGroupQuery must be unchanged.
5. CreateGroup a group whose query has ResourceTypeFilters=['AWS::SQS::Queue'], then ListGroupResources with Filters=[{Name:'resource-type', Values:['AWS::EC2::Instance']}]. Expect 400 with Message 'Filters not valid: The resource types specified as filters in the request are incompatible with the group query: AWS::EC2::Instance'.
Re-replay the five check ids in trace_ids to confirm.
