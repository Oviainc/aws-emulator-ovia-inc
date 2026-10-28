Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.type-configuration-nested-json-structure-preserved
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.type-configuration-nested-json-structure-preserved|run#4|AssertionError|raise AssertionError(
Issue-Commit: 66edebc70f4e641d0e6ce34332328f72c099e422

# cloudformation: the PUBLIC extension registry exposes no activatable THIRD_PARTY RESOURCE type carrying a ConfigurationSchema, so the whole third-party extension-configuration surface is undiscoverable

> _Note: this issue was identified against the code base at commit `66edebc70f4e641d0e6ce34332328f72c099e422`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTypes

## Description
One net-new root cause of service `cloudformation`, grounded in one failing behavioral check.

Cause 1 of 1 — the public (Visibility=PUBLIC) type registry served by ListTypes/DescribeType contains no THIRD_PARTY RESOURCE extension that exposes a ConfigurationSchema, so ActivateType -> SetTypeConfiguration -> BatchDescribeTypeConfigurations cannot be reached by registry discovery at all.

AWS: Smithy (com.amazonaws.cloudformation#CloudFormation): `ListTypes` takes Visibility (enum Visibility), Type (enum RegistryType), Filters (TypeFilters, which carries Category), MaxResults @range 1..100, NextToken @length 1..1024; it is paginated (inputToken/outputToken NextToken, items TypeSummaries) and models only CFNRegistryException (client, httpError 400, awsQueryError code `CFNRegistryException`). `DescribeType` returns `ConfigurationSchema` (@length 1..60000, pattern `^[\s\S]+$`) together with Visibility, PublisherId, IsActivated and AutoUpdate, and models CFNRegistryException (400) and TypeNotFoundException (404). `ActivateType` takes PublicTypeArn (pattern `^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}::type/.+/[0-9a-zA-Z]{12,40}/.+$`) plus ExecutionRoleArn, and `SetTypeConfiguration` takes Configuration (required, @length 1..204800) with ConfigurationAlias (pattern `^[a-zA-Z0-9]{1,256}$`). Recorded behavior: paging ListTypes with Visibility=PUBLIC, Type=RESOURCE and Filters={Category: THIRD_PARTY} in a Region yields TypeSummaries whose TypeArns resolve, via DescribeType, to extensions that DO carry a non-empty ConfigurationSchema; such a type can then be activated with an ExecutionRoleArn and given a schema-derived configuration. The public registry catalog is Region-scoped and not account-specific, so this discovery path is available to any caller.

Emulator: paging that same ListTypes query until NextToken is absent (bounded at 8 candidates / 150s of wall clock in the check) never yields a single TypeSummary whose DescribeType response contains a `ConfigurationSchema` member. The check therefore never reaches a configuration call and raises its own precondition assertion: `no publicly-activatable THIRD_PARTY resource type exposing a ConfigurationSchema was found in this account/Region within the search budget`. Either ListTypes returns no PUBLIC/THIRD_PARTY RESOURCE entries, or every entry it does return is described without ConfigurationSchema.

Why: a client that discovers extensions the documented way — enumerate the public registry, filter on Category THIRD_PARTY, read ConfigurationSchema, ActivateType, SetTypeConfiguration — finds an empty or schema-less catalog and cannot drive the third-party extension lifecycle at all. The configuration-payload contract (nested JSON structure preserved on round-trip, each non-ASCII codepoint coerced to a literal `?`) is unverifiable because there is no settable extension configuration to exercise it against. Nothing in the response says the catalog is unpopulated: ListTypes answers 200 with usable-looking pages, so the caller concludes the Region genuinely has no configurable third-party extensions.

Fix: the type-registry state of the cloudformation crate — the Visibility=PUBLIC catalog that ListTypes pages and DescribeType reads must carry THIRD_PARTY RESOURCE entries whose DescribeType response includes ConfigurationSchema, and those entries must be activatable through ActivateType(PublicTypeArn, ExecutionRoleArn) so SetTypeConfiguration/BatchDescribeTypeConfigurations have a subject. Already-open issues touch neighbouring causes but not this one: 20261026T235900Z-cloudformation-7-net-new-root-causes-in-the.md covers ActivateType's refusal taxonomy for a malformed PublicTypeArn and the TypeNotFoundException message, and 20261025T180059Z-cloudformation-custom-resource-providers-and-hook-configuration.md owns the sibling check cloudformation.type-configuration-set-and-batch-describe; neither states that the PUBLIC/THIRD_PARTY catalog is empty of schema-bearing types.

Traces: cloudformation.type-configuration-nested-json-structure-preserved

## Evidence
Cause 1 — cloudformation.type-configuration-nested-json-structure-preserved [mismatch]:

  File "check_cloudformation_type_configuration_nested_json_structure_preserved.py", line 23, in run
    raise AssertionError(
  AssertionError: no publicly-activatable THIRD_PARTY resource type exposing a ConfigurationSchema was found in this account/Region within the search budget; testing configuration-payload fidelity needs a real, settable extension configuration, and the public registry search turned up nothing usable this run.

The check reaches that assertion only when its bounded discovery loop returns an empty candidate list. That loop is, verbatim from the check:
  cfn.list_types(Visibility="PUBLIC", Type="RESOURCE", Filters={"Category": "THIRD_PARTY"}, MaxResults=20)
  -> for each TypeSummary: cfn.describe_type(Arn=summary["TypeArn"]) and keep it only if described.get("ConfigurationSchema") is truthy
  -> paging on NextToken until absent, capped at 8 candidates / 150s.
So the emulator produced zero (TypeArn, ConfigurationSchema) pairs across every page of that query. No call raised — the catalog is simply devoid of schema-bearing public third-party RESOURCE types.

What was never exercised as a result (the contract the check exists to assert): SetTypeConfiguration(Type="RESOURCE", TypeName=<type>, ConfigurationAlias="Unicode", Configuration=<schema-derived JSON containing non-ASCII>) followed by BatchDescribeTypeConfigurations(TypeConfigurationIdentifiers=[{TypeArn, TypeConfigurationAlias:"Unicode"}]) must return exactly one TypeConfiguration, no per-item Errors, and a Configuration that equals the sent payload with each non-ASCII codepoint replaced one-for-one by the ASCII character `?` and nested object/list structure otherwise intact.

Smithy consulted: ListTypes (input Visibility/ProvisioningType/DeprecatedStatus/Type/Filters/MaxResults @range 1..100/NextToken @length 1..1024; output TypeSummaries + NextToken; paginated items TypeSummaries; errors CFNRegistryException client/400); DescribeType (output includes ConfigurationSchema @length 1..60000 pattern ^[\s\S]+$, Visibility, IsActivated, PublisherId; errors CFNRegistryException client/400, TypeNotFoundException client/404); ActivateType (PublicTypeArn pattern ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}::type/.+/[0-9a-zA-Z]{12,40}/.+$, ExecutionRoleArn RoleARN2 pattern ^arn:.+:iam::[0-9]{12}:role/.+$; errors CFNRegistryException/TypeNotFoundException); SetTypeConfiguration (Configuration required @length 1..204800, ConfigurationAlias pattern ^[a-zA-Z0-9]{1,256}$; errors CFNRegistryException/TypeNotFoundException); BatchDescribeTypeConfigurations (TypeConfigurationIdentifiers required @length min 1; output TypeConfigurations/Errors/UnprocessedTypeConfigurations; errors CFNRegistryException, TypeConfigurationNotFoundException client/404).

## Steps to Reproduce
1. Boot the emulator and replay cloudformation.type-configuration-nested-json-structure-preserved.
2. Directly: call ListTypes(Visibility='PUBLIC', Type='RESOURCE', Filters={'Category':'THIRD_PARTY'}, MaxResults=20) and page on NextToken until it is absent. Expected: at least one TypeSummary with a TypeArn.
3. For each TypeArn returned, call DescribeType(Arn=<TypeArn>). Expected: at least one response carries a non-empty `ConfigurationSchema` (a JSON schema document) together with its three-part `TypeName`. Observed: no page/entry yields a ConfigurationSchema, so the candidate set is empty.
4. Consequence to re-check once the catalog is populated: CreateRole with a trust policy for principal `resources.cloudformation.amazonaws.com`, ActivateType(PublicTypeArn=<arn>, AutoUpdate=False, ExecutionRoleArn=<role arn>), then SetTypeConfiguration(Type='RESOURCE', TypeName=<type>, ConfigurationAlias='Unicode', Configuration=<JSON built from the ConfigurationSchema, containing non-ASCII codepoints>), then BatchDescribeTypeConfigurations(TypeConfigurationIdentifiers=[{'TypeArn': DescribeType(...)['Arn'], 'TypeConfigurationAlias': 'Unicode'}]) -> exactly one TypeConfiguration, no Errors, and Configuration equal to the sent payload with every non-ASCII codepoint rendered as the single ASCII character `?`.
