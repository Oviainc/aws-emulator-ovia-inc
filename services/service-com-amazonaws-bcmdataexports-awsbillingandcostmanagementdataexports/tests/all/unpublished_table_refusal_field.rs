// SPDX-License-Identifier: CC0-1.0

//! Which MEMBER the refusal of a table name outside the published catalog names, for each
//! member of a submitted export definition a table name can arrive at.
//!
//! The rule under test: `ValidationExceptionField$Name` is "[t]he field name where the
//! invalid entry was detected" (smithy_query shape `ValidationExceptionField`), so a
//! refusal names the member path of the input THE REQUEST CARRIED THE OFFENDING ENTRY AT.
//! A table name reaches a submitted definition at two members — written as a relation of
//! `DataQuery$QueryStatement`, or written as a KEY of `DataQuery$TableConfigurations`
//! (smithy_query shape `DataQuery`; its `TableConfigurations` key targets `TableName`) —
//! so one unpublished name earns one sentence and the member path of whichever member the
//! request wrote it at, never a member that carried nothing invalid.
//!
//! The rule is about the member path, not about any one name, so it is asserted over a
//! TABLE of names the catalog publishes nothing for, generated from the service's own
//! published catalog and varying along what a `TableName` can differ by: a plain
//! unpublished name, every published name in another case, every published name with a
//! suffix, a one-character name, a published name with trailing whitespace (which the
//! model's key pattern `^[\S\s]*$` admits), and the empty name the key's `@length(0,
//! 1024)` admits. Each name is submitted at the statement alone, at the table
//! configurations alone, and at both — through both operations that accept a definition,
//! `CreateExport` and `UpdateExport`, which resolve one through the same pass.
//!
//! What is NOT asserted here: how a message clips a name longer than a message can quote
//! (`refusal_message_bounds` pins that), so every name below is short enough to be quoted
//! whole.

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::TABLE_CATALOG;

use crate::data_query_resolution::export_body;
use crate::export_lifecycle::{error_code, Client};

/// The member path a refusal about the statement names: the member the definition carries
/// its query statement at (smithy_query shape `DataQuery`, member `QueryStatement`,
/// reached through `CreateExportRequest$Export` / `UpdateExportRequest$Export`).
const QUERY_STATEMENT_FIELD: &str = "Export.DataQuery.QueryStatement";

/// The member path a refusal about the table configurations names: the member the
/// definition carries them at (smithy_query shape `DataQuery`, member
/// `TableConfigurations`).
const TABLE_CONFIGURATIONS_FIELD: &str = "Export.DataQuery.TableConfigurations";

/// A published table every placement below reads or configures beside the name under
/// test, so the definition is faultless apart from that name.
const PUBLISHED_TABLE: &str = "COST_AND_USAGE_REPORT";

/// The statement a definition carries when the name under test is NOT in the statement:
/// a published table and a column it publishes.
const BASELINE_STATEMENT: &str = "SELECT line_item_usage_account_id FROM COST_AND_USAGE_REPORT";

/// The `TableConfigurations` entry the queried published table needs: no property named,
/// so every property it publishes is in force at its default. An update owes the entry
/// ("All export parameters must be specified in the UpdateExport request" — smithy_query
/// operation `UpdateExport`), so every definition below restates it.
fn published_entry() -> Value {
    json!({ PUBLISHED_TABLE: {} })
}

/// One name the catalog publishes nothing for.
#[derive(Debug, Clone)]
struct Name {
    label: String,
    name: String,
    /// Whether this name can be WRITTEN as a relation of a statement and read back as
    /// itself. A name the SQL parser does not carry through unchanged (the empty name, a
    /// name with trailing whitespace) can only be submitted as a configuration key, so
    /// the statement placements are skipped for it.
    in_statement: bool,
}

/// The names under test, generated from the published catalog so the rule is asserted
/// over every table it publishes rather than the one a request happens to name.
fn names() -> Vec<Name> {
    let mut names = vec![
        Name {
            label: "a name unlike any published one".to_string(),
            name: "NOT_A_REAL_TABLE".to_string(),
            in_statement: true,
        },
        Name {
            label: "a one-character name".to_string(),
            name: "t".to_string(),
            in_statement: true,
        },
        Name {
            label: format!("{PUBLISHED_TABLE} with trailing whitespace"),
            name: format!("{PUBLISHED_TABLE} "),
            in_statement: false,
        },
        Name {
            // The key's own bound: `TableName` is `@length(0, 1024)` (smithy_query shape
            // `TableConfigurations`), so the empty name is a key a request may send.
            label: "the empty name".to_string(),
            name: String::new(),
            in_statement: false,
        },
    ];
    for table in TABLE_CATALOG {
        let lowercase = table.name.to_lowercase();
        if lowercase != table.name {
            names.push(Name {
                label: format!("{} in another case", table.name),
                name: lowercase,
                in_statement: true,
            });
        }
        names.push(Name {
            label: format!("{} with a suffix", table.name),
            name: format!("{}_NOT_PUBLISHED", table.name),
            in_statement: true,
        });
    }
    names
}

/// Where a definition writes the name under test, and the member path that placement owes
/// the refusal.
#[derive(Debug, Clone, Copy)]
enum Placement {
    /// Written as a relation of the statement only: the configurations carry nothing but
    /// the queried published table's entry.
    Statement,
    /// Written as a configuration key only: the statement reads a published table and
    /// names nothing else.
    Configurations,
    /// Written at both members, which both then carry an invalid entry.
    Both,
}

impl Placement {
    /// The statement a definition carries for this placement.
    fn statement(self, name: &str) -> String {
        match self {
            Self::Configurations => BASELINE_STATEMENT.to_string(),
            Self::Statement | Self::Both => format!("SELECT * FROM {name}"),
        }
    }

    /// The `TableConfigurations` a definition carries for this placement.
    fn configurations(self, name: &str) -> Value {
        let mut configurations = published_entry();
        if matches!(self, Self::Configurations | Self::Both) {
            configurations[name] = json!({});
        }
        configurations
    }

    /// The member paths this placement's request carried the offending name at. One
    /// placement writes it at one member and owes that member; `Both` carries it at
    /// either, so either is the member the invalid entry was detected at.
    fn owed_fields(self) -> &'static [&'static str] {
        match self {
            Self::Statement => &[QUERY_STATEMENT_FIELD],
            Self::Configurations => &[TABLE_CONFIGURATIONS_FIELD],
            Self::Both => &[QUERY_STATEMENT_FIELD, TABLE_CONFIGURATIONS_FIELD],
        }
    }

    /// A short tag for the case label and the generated export names.
    fn tag(self) -> &'static str {
        match self {
            Self::Statement => "statement",
            Self::Configurations => "configurations",
            Self::Both => "both",
        }
    }
}

/// The placements `name` can be submitted at.
fn placements(name: &Name) -> Vec<Placement> {
    if name.in_statement {
        vec![Placement::Statement, Placement::Configurations, Placement::Both]
    } else {
        vec![Placement::Configurations]
    }
}

/// Whether `path` — a dot-separated member path — resolves to a value `request` carries.
fn resolves_in(request: &Value, path: &str) -> bool {
    let mut current = request;
    for segment in path.split('.') {
        match current.get(segment) {
            Some(next) => current = next,
            None => return false,
        }
    }
    true
}

/// Assert that `body` is the modelled 400 for an unpublished table name, naming one of
/// `owed` — a member path the refused `request` really carries — and return the refusal's
/// sentence so the callers can compare the one sentence one fault earns.
fn assert_refusal_names_the_carrier(
    case: &str,
    operation: &str,
    request: &Value,
    (status, body): (u16, &Value),
    owed: &[&str],
    name: &str,
) -> String {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: {operation} must answer HTTP 400 ValidationException, \
         got {status} with {body}"
    );
    let message = body["Message"].as_str().unwrap_or("");
    assert_eq!(
        message,
        format!("Table name {name} is invalid"),
        "case {case:?}: {operation} must state the unpublished table, got {message:?} in {body}"
    );
    let fields = body
        .get("Fields")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: {operation} must name the field at fault, got {body}")
        });
    assert_eq!(
        fields.len(),
        1,
        "case {case:?}: {operation} must name exactly the one member at fault, got {body}"
    );
    let field = fields[0].get("Name").and_then(Value::as_str).unwrap_or("");
    assert!(
        owed.contains(&field),
        "case {case:?}: {operation} must name one of {owed:?} as the field the invalid \
         entry was detected at, got {field:?} in {body}"
    );
    assert!(
        resolves_in(request, field),
        "case {case:?}: {operation} named the member path {field:?}, which the request it \
         refused does not carry: {request}"
    );
    let requirement = fields[0].get("Message").and_then(Value::as_str).unwrap_or("");
    assert!(
        !requirement.is_empty(),
        "case {case:?}: {operation} must state what the named member must carry, got {body}"
    );
    message.to_string()
}

#[tokio::test]
async fn an_unpublished_table_name_is_refused_naming_the_member_that_carried_it() {
    let client = Client::new();

    for (index, name) in names().into_iter().enumerate() {
        for placement in placements(&name) {
            let case = format!("{} at the {}", name.label, placement.tag());
            let statement = placement.statement(&name.name);
            let configurations = placement.configurations(&name.name);

            // THE CREATE PATH.
            let create_name = format!("created_{index}_{}", placement.tag());
            let create_request = json!({
                "Export": export_body(&create_name, &statement, Some(configurations.clone())),
            });
            let (status, body) = client.call("CreateExport", &create_request).await;
            let create_message = assert_refusal_names_the_carrier(
                &case,
                "CreateExport",
                &create_request,
                (status, &body),
                placement.owed_fields(),
                &name.name,
            );
            assert!(
                body.get("ExportArn").is_none(),
                "case {case:?}: a refused create must mint no export, got {body}"
            );

            // THE UPDATE PATH: the same definition submitted as the whole replacement of
            // an export created from the baseline one.
            let update_name = format!("updated_{index}_{}", placement.tag());
            let baseline = client
                .ok(
                    "CreateExport",
                    &json!({
                        "Export": export_body(
                            &update_name,
                            BASELINE_STATEMENT,
                            Some(published_entry()),
                        ),
                    }),
                )
                .await;
            let arn = baseline["ExportArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the baseline create must return an ExportArn, got {baseline}")
                })
                .to_string();
            let update_request = json!({
                "ExportArn": arn.clone(),
                "Export": export_body(&update_name, &statement, Some(configurations)),
            });
            let (status, body) = client.call("UpdateExport", &update_request).await;
            let update_message = assert_refusal_names_the_carrier(
                &case,
                "UpdateExport",
                &update_request,
                (status, &body),
                placement.owed_fields(),
                &name.name,
            );

            // ONE fault, one sentence: what differs between the two operations is nothing,
            // and what differs between the placements is only the member path.
            assert_eq!(
                create_message, update_message,
                "case {case:?}: CreateExport stated {create_message:?} and UpdateExport \
                 {update_message:?}"
            );

            // A refused update replaces nothing.
            let fetched = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
            let stored = fetched["Export"]["DataQuery"]["QueryStatement"].as_str();
            assert_eq!(
                stored,
                Some(BASELINE_STATEMENT),
                "case {case:?}: the refused update must leave the stored definition alone, \
                 got {fetched}"
            );
        }
    }
}

#[tokio::test]
async fn a_configured_only_name_outside_the_catalog_names_the_configurations_member() {
    let client = Client::new();

    // The placement the rule turns on, stated on its own: a statement that is entirely
    // valid against the published catalog, plus one configuration key the catalog does not
    // publish. The member at fault is the one that carried the key, and the statement —
    // which carried no invalid entry — is not named.
    let name = "NOT_A_REAL_TABLE";
    let request = json!({
        "Export": export_body(
            "configured_only_bad_table",
            BASELINE_STATEMENT,
            Some(json!({ PUBLISHED_TABLE: {}, name: {} })),
        ),
    });
    let (status, body) = client.call("CreateExport", &request).await;
    let case = "a configured-only name outside the catalog";
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: CreateExport must answer HTTP 400 ValidationException, \
         got {status} with {body}"
    );
    let field = body["Fields"][0]["Name"].as_str().unwrap_or("");
    assert_eq!(
        field, TABLE_CONFIGURATIONS_FIELD,
        "case {case:?}: the refusal must name {TABLE_CONFIGURATIONS_FIELD:?}, \
         got {field:?} in {body}"
    );
    assert_ne!(
        field, QUERY_STATEMENT_FIELD,
        "case {case:?}: the statement carried no invalid entry, so it must not be named: \
         {body}"
    );
}
