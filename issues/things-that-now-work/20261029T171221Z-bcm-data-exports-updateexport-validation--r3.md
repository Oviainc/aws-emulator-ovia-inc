Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# A column no table publishes passes the column check when the statement aliases it to its own name (SELECT x AS x)

_Filed: 2026-10-29T17:12:21Z_
_Operation: UpdateExport | Class: validation_

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::query::resolve_data_query` — the one pass both `UpdateExport` and `CreateExport` judge a submitted `DataQuery` with — resolves a column reference against the union of (a) the columns in force for the tables the statement reads and (b) EVERY name the statement defines as a select-item alias. The alias set is collected for the whole statement before any reference is resolved, and the aliased expression's OWN identifier is itself collected as a column reference. So when a select item aliases an unpublished name to that same name, the name is in `shape.defined`, the reference resolves against itself, and the definition is accepted.

`SELECT not_a_real_column AS not_a_real_column FROM COST_AND_USAGE_REPORT` is therefore stored, where `SELECT not_a_real_column FROM COST_AND_USAGE_REPORT` (and even `SELECT published_col AS picked FROM COST_AND_USAGE_REPORT WHERE not_a_real_column = 1`) is refused with the modelled `ValidationException`. The same trick works on a property-GATED column: `SELECT line_item_resource_id AS line_item_resource_id FROM COST_AND_USAGE_REPORT` is accepted with `INCLUDE_RESOURCES=FALSE`, i.e. with the column switched out of the schema `GetTable` reports for that configuration.

On `UpdateExport` the consequence is worse than on a create: the replacement is what DISPLACES the stored definition, so a statement the service could never deliver silently overwrites a valid one — the opposite of the rule the crate states for a refused update ("a definition this service could not deliver never displaces the stored one", `src/ops/update_export.rs:213`-`:214`).

This is NOT the gap of the earlier issues on this rule. `20261024T084458Z-bcm-data-exports-createexport-validation` / `20261024T164238Z-bcm-data-exports-updateexport-validation` are about columns not being looked at AT ALL; they now are, and their own examples are correctly refused. What is wrong is the residual bypass in the implemented check: the alias set is an unscoped whitelist, where the module header declares that only "a later clause of the same statement" may name an alias.

## Evidence

The crate's own statement of what the alias exemption covers — `src/state/query.rs:34`-`:38`:

    //! Two kinds of name are NOT column references, and neither is checked:
    //!
    //! * a name the statement ITSELF defines — the alias of a select item, which a later
    //!   clause of the same statement can name;

and the rule it states for every other name, `src/state/query.rs:28`-`:32`:

    //! table it reads publishes that column under the properties this request puts in force:
    //! a name no such table publishes selects a column the export could never deliver (and a
    //! name a table publishes only at another value of a gating property is such a name), so
    //! it is refused with the modelled `ValidationException` like an unpublished table is.

The code that breaks it — the alias set is collected for the whole statement, `src/state/query.rs:426`-`:428` and `:439`-`:444`:

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        collect_defined_names(&query.body, &mut self.shape.defined);

        SetExpr::Select(select) => {
            for item in &select.projection {
                if let SelectItem::ExprWithAlias { alias, .. } = item {
                    into.insert(alias.value.clone());

while the aliased expression's identifier is ALSO collected as a reference, `src/state/query.rs:405`-`:410`:

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        let parts: Vec<&Ident> = match expr {
            Expr::Identifier(ident) => vec![ident],

and the resolution accepts a reference whose part is merely a defined name, `src/state/query.rs:523`-`:532`:

    for reference in &shape.columns {
        // A reference resolves when one of its parts is a published column or a name the
        // statement defines: the other parts qualify a relation or index into the column's
        // value, and the catalog publishes neither.
        let resolved = reference.parts.iter().any(|part| {
            published.contains(part.as_str()) || shape.defined.contains(part.as_str())
        });
        if !resolved {
            return Err(invalid_query_column(&reference.written).into());
        }

The caller this is filed against, `src/ops/update_export.rs:220`-`:230`:

    let table_configurations = crate::state::resolve_data_query(&input.export.data_query, true)
        .map_err(|refusal| match refusal {
            crate::state::QueryRefusal::Invalid(invalid) => {
                UpdateExportErrorModeled::ValidationException(invalid)

(`src/ops/create_export.rs:223` reaches the identical pass with `false`.)

Test coverage states the rule this bypasses but only in the differently-named form — `tests/all/query_column_selection.rs:572`-`:587`:

    // And an alias is not a licence for the rest: a statement that defines one and then
    // names an unpublished column is still refused.
    let statement =
        format!("SELECT {anchor} AS picked FROM {table} WHERE {UNPUBLISHED_COLUMN} = 1");
    ...
    assert_column_refused(
        "an alias beside an unpublished column",

with `const UNPUBLISHED_COLUMN: &str = "not_a_real_column";` (`tests/all/query_column_selection.rs:50`). The UpdateExport half of the same suite pins the stored-definition rule this bypass violates, `tests/all/query_column_selection.rs:479`-`:489`:

    let (status, body) = client
        .call("UpdateExport", &replacement(&statement, configuration))
        .await;
    assert_column_refused(&case, status, &body, &column);
    ...
        "case {case:?}: a refused update must leave the stored definition untouched, got {}",

No row of either table aliases the unpublished column to its own name, which is why this slips through.

## Steps to Reproduce

1. Call CreateExport with a valid definition, e.g. Export.DataQuery.QueryStatement = "SELECT identity_line_item_id FROM COST_AND_USAGE_REPORT" and a destination the crate accepts; record the returned ExportArn.
2. Call GetExport for that ARN and keep the returned Export as the replacement body (it carries the effective TableConfigurations, so UpdateExport's restatement rule is satisfied).
3. Call UpdateExport with that ARN and the same Export, but with DataQuery.QueryStatement = "SELECT not_a_real_column AS not_a_real_column FROM COST_AND_USAGE_REPORT".
4. Observed: HTTP 200 and the replacement is stored — GetExport now reports the statement naming not_a_real_column. Expected: HTTP 400 ValidationException with reason fieldValidationFailed naming Export.DataQuery.QueryStatement and message "Column name not_a_real_column is invalid" (src/state/query.rs:710-:718), with the stored definition left untouched.
5. Contrast: repeat step 3 with "SELECT not_a_real_column FROM COST_AND_USAGE_REPORT" (no alias) — that one IS refused 400, which shows the alias is what bypasses the check.
6. Repeat the same bypass on a gated column: UpdateExport with QueryStatement = "SELECT line_item_resource_id AS line_item_resource_id FROM COST_AND_USAGE_REPORT" and TableConfigurations {"COST_AND_USAGE_REPORT":{"INCLUDE_RESOURCES":"FALSE"}} — accepted, although GetTable reports no line_item_resource_id for that configuration.
7. Repeat step 3's statement against CreateExport — the same pass accepts it there too (src/ops/create_export.rs:223).
