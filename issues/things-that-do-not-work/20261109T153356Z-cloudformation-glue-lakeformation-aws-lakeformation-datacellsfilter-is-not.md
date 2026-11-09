Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__glue__lakeformation.deploy-cfn-lakeformation-datacellsfilter-round-trip-and-delete cloudformation__glue__lakeformation.deploy-cfn-lakeformation-datacellsfilter-missing-row-filter-rolled-back
Issue-Key: cloudformation__glue__lakeformation|CreateStack|ValidationError
Issue-Commit: e19d7761cdba7dfb41b7452b9b8e275a18501f91

# cloudformation__glue__lakeformation: AWS::LakeFormation::DataCellsFilter is not a registered resource type, so CreateStack refuses every data-cells-filter template

> _Note: this issue was identified against the code base at commit `e19d7761cdba7dfb41b7452b9b8e275a18501f91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's resource-type registry does not know AWS::LakeFormation::DataCellsFilter, so template validation refuses the whole template at CreateStack and no stack is ever created.

AWS: AWS::LakeFormation::DataCellsFilter is a public CloudFormation resource type (https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-lakeformation-datacellsfilter.html). CreateStack accepts a template declaring it and provisions it through Lake Formation CreateDataCellsFilter, so the stack reaches CREATE_COMPLETE and ListDataCellsFilter on the parent Glue table reports the filter with every declared property (TableCatalogId, DatabaseName, TableName, Name, RowFilter, ColumnWildcard, ColumnNames); an AllRowsWildcard row filter reads back as {"FilterExpression": "TRUE", "AllRowsWildcard": {}}. Its Ref is the composite physical id '<TableCatalogId>|<DatabaseName>|<TableName>|<Name>'. An UpdateStack that drops only the filter deletes the filter and leaves its Glue table in place; deleting the stack removes the database. When the RowFilter property is omitted, Lake Formation itself refuses the create and the stack rolls back to ROLLBACK_COMPLETE with the Filter resource's CREATE_FAILED ResourceStatusReason carrying the service's own refusal — error code 'InvalidRequest' and message 'Row filter cannot be null (Service: LakeFormation, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', where 'Row filter cannot be null' and the parenthesised Service/Status Code frame are fixed text and the request id and attempt count are per-call values. recorded behavior only for the exact CreateStack refusal text.

Emulator: CreateStack is refused with HTTP 400, error code 'ValidationError' and message 'Template format error: Unrecognized resource types: [AWS::LakeFormation::DataCellsFilter]'. Fixed text is 'Template format error: Unrecognized resource types: ' followed by the bracketed list of unknown type names taken from the request's Resources block. Both the happy-path round trip and the missing-RowFilter rejection die on this one call, so no filter is ever created, no rollback path is exercised and no Lake Formation refusal text is produced.

Why: a client cannot express Lake Formation cell-level filters as infrastructure at all, and the error it gets back blames its template format rather than naming a missing provider. Row/column filtering declared in CloudFormation is how fine-grained access is normally rolled out, so every such deployment stops here; the rejection path is equally lost, meaning a template AWS would roll back is instead never even accepted.

Fix: CloudFormation's resource-type registry plus a new AWS::LakeFormation::DataCellsFilter resource handler (create -> lakeformation CreateDataCellsFilter, delete -> DeleteDataCellsFilter, update -> replace), its Ref physical-id projection '<catalog>|<database>|<table>|<name>', and propagation of the Lake Formation service refusal into the resource's CREATE_FAILED ResourceStatusReason so a missing RowFilter rolls the stack back.

Traces: the two failing checks listed below.

## Evidence
Expected (round trip): CreateStack returns a StackId, the stack reaches CREATE_COMPLETE, Outputs FilterId == '<account>|<db>|<table>|<name>', and ListDataCellsFilter(Table={CatalogId,DatabaseName,Name}) reports [(account, db, table, name, {"FilterExpression": "TRUE", "AllRowsWildcard": {}}, {"ExcludedColumnNames": []}, [])]; an update dropping the filter leaves ListDataCellsFilter empty while GetTable still answers.
Expected (rejection): the RowFilter-less stack reaches ROLLBACK_COMPLETE with the Filter resource's reason ('Row filter cannot be null (Service: LakeFormation, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest'), no filter listed and the database gone.

Actual (both): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::LakeFormation::DataCellsFilter]

## Steps to Reproduce
1. CreateStack with a template containing an AWS::S3::Bucket, an AWS::Glue::Database, an AWS::Glue::Table and:
   {"Type": "AWS::LakeFormation::DataCellsFilter", "DependsOn": ["Table"], "Properties": {"TableCatalogId": "<account>", "DatabaseName": "<db>", "TableName": "<table>", "Name": "<filter>", "RowFilter": {"AllRowsWildcard": {}}, "ColumnWildcard": {"ExcludedColumnNames": []}}}
2. Observe CreateStack raise ValidationError 'Template format error: Unrecognized resource types: [AWS::LakeFormation::DataCellsFilter]' instead of returning a StackId.
3. Repeat with the RowFilter property omitted: the same refusal is raised, so the expected Lake Formation-driven ROLLBACK_COMPLETE is never reached.
