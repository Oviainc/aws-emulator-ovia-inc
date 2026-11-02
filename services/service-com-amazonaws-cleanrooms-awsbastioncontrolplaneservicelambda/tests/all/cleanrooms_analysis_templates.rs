// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Clean Rooms analysis templates.
//!
//! The rules under test:
//!
//! 1. `CreateAnalysisTemplate` persists a template under the membership named in the
//!    path, derives its collaboration/membership identity from that STORED membership,
//!    and `GetAnalysisTemplate` reports exactly what was stored.
//! 2. A template's `name` identifies it within its membership: a second create under a
//!    name that membership already uses is a `ConflictException`, while the same name
//!    under a different membership is a different resource.
//! 3. The REQUIRED `schema` is the relation set in force — what the caller declared,
//!    else the relations a SQL analysis itself reads, within the bounds this service
//!    parses (text length, nesting depth and relation count), each of which is tested
//!    at its boundary and past it.
//! 4. `ListAnalysisTemplates` reports its membership's templates, in creation order,
//!    one page at a time, each summary projected from the stored template.
//!
//! Each table varies what those rules generalise over (the analysis format, the source
//! kind, which optional members the request fills in, the shape of the query, which
//! membership the name is taken in), so the recorded duplicate-name case the issue
//! cited is one row among many. Requests go over the wire through the same
//! `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::state::{
    MAX_PARSED_ANALYSIS_CHARS, MAX_PARSED_ANALYSIS_TOKENS, MAX_REFERENCED_TABLES,
};

use super::cleanrooms_collaboration_lifecycle::{
    account_id, error_code, is_model_uuid, minimal_collaboration, Emulator, REGION,
};

/// Every `AnalysisFormat` the model declares (shape `AnalysisFormat`).
const ALL_ANALYSIS_FORMATS: [&str; 2] = ["SQL", "PYSPARK_1_0"];

/// Every `ErrorMessageType` the model declares (shape `ErrorMessageType`).
const ALL_ERROR_MESSAGE_TYPES: [&str; 1] = ["DETAILED"];

/// `ParameterType` values used below; the catalog is the model's `ParameterType` enum
/// (SMALLINT | INTEGER | BIGINT | … | TINYINT), of which these are three entries the
/// issue did not cite.
const PARAMETER_TYPES: [&str; 3] = ["INTEGER", "VARCHAR", "TIMESTAMP"];

/// The published maximum of `CreateAnalysisTemplateInput$name` (model `TableAlias`,
/// `len ..128`) and of `AnalysisTemplate$name` (model `ResourceAlias`, same bound).
pub(crate) const NAME_MAX_LEN: usize = 128;

/// Create a collaboration and join it, answering the new membership's id. Each call
/// uses its own collaboration, because one account holds at most one membership per
/// collaboration.
pub(crate) async fn membership_id(emulator: &Emulator, collaboration_name: &str) -> String {
    let (status, created) = emulator
        .create_collaboration(minimal_collaboration(collaboration_name))
        .await;
    assert_eq!(
        status, 200,
        "setup {collaboration_name:?}: expected CreateCollaboration 200, got {status} with {created}"
    );
    let collaboration_id = created["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {collaboration_name:?}: no collaboration.id in {created}"))
        .to_string();

    let (status, created) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": "ENABLED",
        }))
        .await;
    assert_eq!(
        status, 200,
        "setup {collaboration_name:?}: expected CreateMembership 200, got {status} with {created}"
    );
    created["membership"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {collaboration_name:?}: no membership.id in {created}"))
        .to_string()
}

pub(crate) async fn create_analysis_template(
    emulator: &Emulator,
    membership: &str,
    body: Value,
) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!("/memberships/{membership}/analysistemplates"),
            Some(body),
        )
        .await
}

pub(crate) async fn get_analysis_template(
    emulator: &Emulator,
    membership: &str,
    template: &str,
) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!("/memberships/{membership}/analysistemplates/{template}"),
            None,
        )
        .await
}

/// A PySpark analysis whose code lives in S3. The artifact members follow the model's
/// `AnalysisTemplateArtifacts` (`entryPoint`, `additionalArtifacts` of `len 1..1`,
/// `roleArn` matching `^arn:aws:iam::[\w]+:role/[\w+=./@-]+$`).
pub(crate) fn artifacts_source() -> Value {
    json!({
        "artifacts": {
            "entryPoint": { "location": { "bucket": "analysis-artifacts", "key": "jobs/main.py" } },
            "additionalArtifacts": [
                { "location": { "bucket": "analysis-artifacts", "key": "jobs/helpers.py" } },
            ],
            "roleArn": format!("arn:aws:iam::{}:role/clean-rooms-pyspark", account_id()),
        }
    })
}

/// `SyntheticDataParameters` with the smallest `columnMapping` the model allows
/// (`ColumnClassificationDetails$columnMapping`, `len 5..1000`) and both
/// `SyntheticDataColumnType` values.
pub(crate) fn synthetic_data_parameters() -> Value {
    let columns: Vec<Value> = ["age", "income", "region", "clicks", "converted"]
        .iter()
        .enumerate()
        .map(|(index, name)| {
            json!({
                "columnName": name,
                "columnType": if index < 2 { "NUMERICAL" } else { "CATEGORICAL" },
                "isPredictiveValue": index == 4,
            })
        })
        .collect();
    json!({
        "mlSyntheticDataParameters": {
            // `MLSyntheticDataParameters$epsilon` range 0.0001..10 and
            // `maxMembershipInferenceAttackScore` range 0.5..1.
            "epsilon": 1.5,
            "maxMembershipInferenceAttackScore": 0.75,
            "columnClassification": { "columnMapping": columns },
        }
    })
}

/// One row of the create/read round-trip table.
struct TemplateCase {
    label: &'static str,
    body: Value,
    /// The `schema.referencedTables` the stored template must report, or `None` when
    /// the member must be absent because nothing declared or derived it.
    referenced_tables: Option<Vec<&'static str>>,
}

fn template_cases() -> Vec<TemplateCase> {
    vec![
        TemplateCase {
            label: "minimal SQL text: no description, no parameters",
            body: json!({
                "name": "minimal",
                "format": ALL_ANALYSIS_FORMATS[0],
                "source": { "text": "SELECT id FROM sales" },
            }),
            referenced_tables: Some(vec!["sales"]),
        },
        TemplateCase {
            label: "SQL text with a join, a description and tags",
            body: json!({
                "name": "joined analysis",
                "description": "joins two configured tables",
                "format": ALL_ANALYSIS_FORMATS[0],
                "source": {
                    "text": "SELECT s.id, c.segment FROM sales s JOIN consumers c ON s.id = c.id",
                },
                "tags": { "env": "test", "owner": "fixer" },
            }),
            referenced_tables: Some(vec!["sales", "consumers"]),
        },
        TemplateCase {
            label: "SQL text, every analysis parameter type under test, DETAILED errors",
            body: json!({
                "name": "parameterised",
                "format": ALL_ANALYSIS_FORMATS[0],
                "source": { "text": "SELECT id FROM sales WHERE id = :p0" },
                "analysisParameters": PARAMETER_TYPES
                    .iter()
                    .enumerate()
                    .map(|(index, kind)| json!({
                        "name": format!("p{index}"),
                        "type": kind,
                        "defaultValue": "",
                    }))
                    .collect::<Vec<Value>>(),
                "errorMessageConfiguration": { "type": ALL_ERROR_MESSAGE_TYPES[0] },
            }),
            referenced_tables: Some(vec!["sales"]),
        },
        TemplateCase {
            label: "SQL text with a caller-declared schema, which wins over derivation",
            body: json!({
                "name": "declared schema",
                "format": ALL_ANALYSIS_FORMATS[0],
                "source": { "text": "SELECT id FROM sales" },
                "schema": { "referencedTables": ["sales", "consumers"] },
            }),
            referenced_tables: Some(vec!["sales", "consumers"]),
        },
        TemplateCase {
            label: "PySpark program text: no query to read relations out of",
            body: json!({
                "name": "pyspark_text",
                "format": ALL_ANALYSIS_FORMATS[1],
                "source": { "text": "def entrypoint(spark):\n    return None\n" },
            }),
            referenced_tables: None,
        },
        TemplateCase {
            label: "PySpark artifacts in S3, schema declared by the caller",
            body: json!({
                "name": "pyspark_artifacts",
                "format": ALL_ANALYSIS_FORMATS[1],
                "source": artifacts_source(),
                "schema": { "referencedTables": ["sales"] },
                "syntheticDataParameters": synthetic_data_parameters(),
            }),
            referenced_tables: Some(vec!["sales"]),
        },
        TemplateCase {
            label: "name at the declared maximum length (128)",
            body: json!({
                "name": "n".repeat(NAME_MAX_LEN),
                "format": ALL_ANALYSIS_FORMATS[0],
                "source": { "text": "SELECT 1" },
            }),
            referenced_tables: Some(vec![]),
        },
    ]
}

#[tokio::test]
async fn create_analysis_template_persists_and_get_returns_it_unchanged() {
    let emulator = Emulator::new();
    let membership = membership_id(&emulator, "templates-collab").await;
    let (status, fetched_membership) = emulator.get_membership(&membership).await;
    assert_eq!(status, 200, "setup: GetMembership: {fetched_membership}");
    let membership_view = &fetched_membership["membership"];

    for case in template_cases() {
        let (status, created) = create_analysis_template(&emulator, &membership, case.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateAnalysisTemplate 200, got {status} with {created}",
            case.label
        );

        let template = created
            .get("analysisTemplate")
            .unwrap_or_else(|| panic!("case {:?}: no `analysisTemplate` in {created}", case.label))
            .clone();
        let id = template["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no analysisTemplate.id in {created}", case.label));

        assert!(
            is_model_uuid(id),
            "case {:?}: expected a 36-char lowercase-hex UUID id, got {id:?}",
            case.label
        );
        assert_eq!(
            template["arn"].as_str(),
            Some(
                format!(
                    "arn:aws:cleanrooms:{REGION}:{}:membership/{membership}/analysistemplate/{id}",
                    account_id()
                )
                .as_str()
            ),
            "case {:?}: unexpected arn, got {:?}",
            case.label,
            template["arn"]
        );

        // Collaboration and membership identity come from the STORED membership, not
        // from the request, which never carries them.
        for member in ["membershipId", "membershipArn", "collaborationId", "collaborationArn"] {
            let expected = match member {
                "membershipId" => membership_view["id"].clone(),
                "membershipArn" => membership_view["arn"].clone(),
                "collaborationId" => membership_view["collaborationId"].clone(),
                _ => membership_view["collaborationArn"].clone(),
            };
            assert_eq!(
                template[member], expected,
                "case {:?}: expected {member} {expected:?} from the stored membership, got {:?}",
                case.label, template[member]
            );
        }

        assert_eq!(
            template["name"], case.body["name"],
            "case {:?}: expected the requested name, got {:?}",
            case.label, template["name"]
        );
        assert_eq!(
            template["format"], case.body["format"],
            "case {:?}: expected the requested format, got {:?}",
            case.label, template["format"]
        );
        assert_eq!(
            template["source"], case.body["source"],
            "case {:?}: expected the requested source, got {:?}",
            case.label, template["source"]
        );
        assert_eq!(
            template["description"], case.body["description"],
            "case {:?}: expected the requested description, got {:?}",
            case.label, template["description"]
        );
        assert_eq!(
            template["analysisParameters"], case.body["analysisParameters"],
            "case {:?}: expected the requested analysisParameters, got {:?}",
            case.label, template["analysisParameters"]
        );
        assert_eq!(
            template["errorMessageConfiguration"], case.body["errorMessageConfiguration"],
            "case {:?}: expected the requested errorMessageConfiguration, got {:?}",
            case.label, template["errorMessageConfiguration"]
        );
        assert_eq!(
            template["syntheticDataParameters"], case.body["syntheticDataParameters"],
            "case {:?}: expected the requested syntheticDataParameters, got {:?}",
            case.label, template["syntheticDataParameters"]
        );
        assert_eq!(
            template["createTime"], template["updateTime"],
            "case {:?}: a freshly created template has createTime == updateTime, got {:?} and {:?}",
            case.label, template["createTime"], template["updateTime"]
        );

        let referenced = template["schema"]
            .get("referencedTables")
            .map(|tables| {
                tables
                    .as_array()
                    .unwrap_or_else(|| {
                        panic!("case {:?}: referencedTables is not a list: {tables}", case.label)
                    })
                    .iter()
                    .map(|table| table.as_str().unwrap_or("<not a string>").to_string())
                    .collect::<Vec<String>>()
            });
        let expected = case
            .referenced_tables
            .as_ref()
            .map(|tables| tables.iter().map(|t| t.to_string()).collect::<Vec<String>>());
        assert_eq!(
            referenced, expected,
            "case {:?}: expected schema.referencedTables {expected:?}, got {referenced:?}",
            case.label
        );

        // Read it back: the reader reports the stored resource, byte for byte.
        let (status, fetched) = get_analysis_template(&emulator, &membership, id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetAnalysisTemplate 200, got {status} with {fetched}",
            case.label
        );
        assert_eq!(
            fetched["analysisTemplate"], template,
            "case {:?}: GetAnalysisTemplate must report what was stored, got {:?}",
            case.label, fetched["analysisTemplate"]
        );
    }
}

#[tokio::test]
async fn create_analysis_template_refuses_a_name_its_membership_already_uses() {
    let emulator = Emulator::new();
    let membership = membership_id(&emulator, "conflict-collab").await;
    // A second membership, in its own collaboration: the name index is per membership.
    let other_membership = membership_id(&emulator, "conflict-collab-other").await;

    // Rows differ along the name and the analysis the duplicate carries, so the rule is
    // "the name is taken", not "the request repeated".
    let cases: [(&str, Value, Value); 3] = [
        (
            "plain name, identical analysis",
            json!({ "text": "SELECT id FROM sales" }),
            json!({ "text": "SELECT id FROM sales" }),
        ),
        (
            "name with spaces, different analysis",
            json!({ "text": "SELECT id FROM sales" }),
            json!({ "text": "SELECT segment FROM consumers" }),
        ),
        (
            "name at the maximum length, artifacts analysis",
            json!({ "text": "SELECT 1" }),
            artifacts_source(),
        ),
    ];
    let names = [
        "taken".to_string(),
        "taken name with spaces".to_string(),
        "t".repeat(NAME_MAX_LEN),
    ];

    for (index, (label, first_source, second_source)) in cases.into_iter().enumerate() {
        let name = &names[index];
        let first = json!({
            "name": name,
            "format": "SQL",
            "source": first_source,
        });
        let (status, created) = create_analysis_template(&emulator, &membership, first).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the first CreateAnalysisTemplate 200, got {status} with {created}"
        );
        let existing_id = created["analysisTemplate"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no analysisTemplate.id in {created}"))
            .to_string();

        let duplicate = json!({
            "name": name,
            "format": if second_source.get("artifacts").is_some() { "PYSPARK_1_0" } else { "SQL" },
            "source": second_source,
            "schema": { "referencedTables": ["sales"] },
        });
        let (status, refused) =
            create_analysis_template(&emulator, &membership, duplicate.clone()).await;
        assert_eq!(
            status, 409,
            "case {label:?}: expected HTTP 409 for a name the membership already uses, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {refused}"
        );
        assert_eq!(
            refused["reason"].as_str(),
            Some("ALREADY_EXISTS"),
            "case {label:?}: expected reason ALREADY_EXISTS, got {:?}",
            refused.get("reason")
        );
        assert_eq!(
            refused["resourceId"].as_str(),
            Some(existing_id.as_str()),
            "case {label:?}: expected the existing template's id as resourceId, got {:?}",
            refused.get("resourceId")
        );

        // The refusal changed nothing: the stored template is still the first one.
        let (status, fetched) = get_analysis_template(&emulator, &membership, &existing_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the first template to survive, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["analysisTemplate"]["source"], created["analysisTemplate"]["source"],
            "case {label:?}: the refused create must not have replaced the analysis, got {:?}",
            fetched["analysisTemplate"]["source"]
        );

        // The same name under a DIFFERENT membership is a different resource.
        let (status, accepted) =
            create_analysis_template(&emulator, &other_membership, duplicate).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the name is taken per membership, so the other membership accepts it; got {status} with {accepted}"
        );
        let other_id = accepted["analysisTemplate"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no analysisTemplate.id in {accepted}"));
        assert_ne!(
            other_id, existing_id,
            "case {label:?}: the two memberships' templates must be distinct resources"
        );
    }
}

#[tokio::test]
async fn create_analysis_template_derives_the_relations_a_sql_analysis_reads() {
    let emulator = Emulator::new();
    let membership = membership_id(&emulator, "schema-collab").await;

    // The rule generalises over the SHAPE of the query: where a relation appears (FROM,
    // JOIN, subquery, set operation), how it is qualified, how often it appears, and
    // whether the query defines the name itself.
    let cases: [(&str, &str, Vec<&str>); 7] = [
        ("single relation", "SELECT id FROM sales", vec!["sales"]),
        (
            "three-way join",
            "SELECT s.id FROM sales s JOIN consumers c ON s.id = c.id LEFT JOIN regions r ON r.id = c.region_id",
            vec!["sales", "consumers", "regions"],
        ),
        (
            "relation inside a subquery",
            "SELECT id FROM sales WHERE id IN (SELECT id FROM consumers)",
            vec!["sales", "consumers"],
        ),
        (
            "qualified name reports the relation, not its qualifiers",
            "SELECT id FROM analytics.public.sales",
            vec!["sales"],
        ),
        (
            "the same relation twice is reported once",
            "SELECT id FROM sales UNION ALL SELECT id FROM sales",
            vec!["sales"],
        ),
        (
            "a common table expression is defined by the query, not referenced by it",
            "WITH recent AS (SELECT id FROM sales) SELECT id FROM recent",
            vec!["sales"],
        ),
        ("a query that reads no relation", "SELECT 1", vec![]),
    ];

    for (index, (label, sql, expected)) in cases.into_iter().enumerate() {
        let (status, created) = create_analysis_template(
            &emulator,
            &membership,
            json!({
                "name": format!("derived_{index}"),
                "format": "SQL",
                "source": { "text": sql },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateAnalysisTemplate 200, got {status} with {created}"
        );
        let referenced: Vec<String> = created["analysisTemplate"]["schema"]["referencedTables"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {label:?}: no schema.referencedTables in {created}")
            })
            .iter()
            .map(|table| table.as_str().unwrap_or("<not a string>").to_string())
            .collect();
        assert_eq!(
            referenced, expected,
            "case {label:?}: expected referencedTables {expected:?}, got {referenced:?}"
        );
    }
}

#[tokio::test]
async fn create_analysis_template_refuses_a_sql_analysis_that_is_not_a_query() {
    let emulator = Emulator::new();
    let membership = membership_id(&emulator, "invalid-sql-collab").await;

    // Rows span the ways a SQL analysis can fail to yield a relation set this service
    // can report; the PySpark row is the control: that format carries program text, so
    // nothing is parsed and it is kept.
    let cases: [(&str, &str, &str, u16); 6] = [
        ("prose instead of SQL", "SQL", "this is not a query at all", 400),
        ("truncated statement", "SQL", "SELECT id FROM", 400),
        ("unbalanced parenthesis", "SQL", "SELECT id FROM (SELECT id FROM sales", 400),
        (
            // `QueryTables`'s member is the model's `TableAlias`, whose pattern admits
            // no `!`, so this relation could not appear in the reported relation set:
            // the analysis is refused rather than stored with a set that omits a table
            // it really reads.
            "a relation whose name cannot appear in the modelled relation set",
            "SQL",
            "SELECT id FROM \"sales!raw\"",
            400,
        ),
        (
            // The same name quoted but WITHIN the pattern is reported normally, so the
            // row above is about the name, not about quoting.
            "a quoted relation whose name does fit the modelled relation set",
            "SQL",
            "SELECT id FROM \"sales raw\"",
            200,
        ),
        (
            "PySpark program text is not parsed as SQL",
            "PYSPARK_1_0",
            "def entrypoint(spark):\n    return None\n",
            200,
        ),
    ];

    for (index, (label, format, text, expected_status)) in cases.into_iter().enumerate() {
        let (status, body) = create_analysis_template(
            &emulator,
            &membership,
            json!({
                "name": format!("invalid_{index}"),
                "format": format,
                "source": { "text": text },
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body["reason"].as_str(),
                Some("INVALID_QUERY"),
                "case {label:?}: expected reason INVALID_QUERY, got {:?}",
                body.get("reason")
            );
        }
    }
}

#[tokio::test]
async fn create_analysis_template_refuses_an_analysis_with_more_relations_than_it_reads() {
    let emulator = Emulator::new();
    let membership = membership_id(&emulator, "relation-cap-collab").await;

    // The emulator's own bound (`state::MAX_REFERENCED_TABLES`), tested at the bound and
    // one past it: at the bound the whole relation set is reported, past it the request
    // is refused rather than answered with a truncated set.
    let cases: [(&str, usize, u16); 3] = [
        ("well under the bound", 10, 200),
        ("at the bound", MAX_REFERENCED_TABLES, 200),
        ("one past the bound", MAX_REFERENCED_TABLES + 1, 402),
    ];

    for (index, (label, relation_count, expected_status)) in cases.into_iter().enumerate() {
        let relations: Vec<String> = (0..relation_count).map(|n| format!("t{n:05}")).collect();
        let sql = format!("SELECT 1 FROM {}", relations.join(", "));
        let (status, body) = create_analysis_template(
            &emulator,
            &membership,
            json!({
                "name": format!("cap_{index}"),
                "format": "SQL",
                "source": { "text": sql },
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {label:?} ({relation_count} relations): expected HTTP {expected_status}, got {status}"
        );
        if expected_status == 200 {
            let referenced = body["analysisTemplate"]["schema"]["referencedTables"]
                .as_array()
                .unwrap_or_else(|| panic!("case {label:?}: no schema.referencedTables"))
                .len();
            assert_eq!(
                referenced, relation_count,
                "case {label:?}: expected every relation reported, got {referenced}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ServiceQuotaExceededException",
                "case {label:?}: expected ServiceQuotaExceededException, got {body}"
            );
            assert_eq!(
                body["quotaValue"].as_f64(),
                Some(MAX_REFERENCED_TABLES as f64),
                "case {label:?}: expected the bound as quotaValue, got {:?}",
                body.get("quotaValue")
            );
        }
    }
}

#[tokio::test]
async fn create_analysis_template_refuses_more_analysis_text_than_it_reads() {
    let emulator = Emulator::new();
    let membership = membership_id(&emulator, "text-cap-collab").await;

    // The text the service parses is bounded (`state::MAX_PARSED_ANALYSIS_CHARS`),
    // tested well under the bound, AT the bound and one character past it. Every row is
    // well inside the model's `AnalysisTemplateText` maximum of 500,000 characters, so
    // the boundary under test is this service's, not the modelled one.
    let prefix = "SELECT id FROM sales WHERE note = '";
    let suffix = "'";
    let cases: [(&str, usize, u16); 3] = [
        ("well under the bound", 1_000, 200),
        ("at the bound", MAX_PARSED_ANALYSIS_CHARS, 200),
        ("one past the bound", MAX_PARSED_ANALYSIS_CHARS + 1, 400),
    ];

    for (index, (label, length, expected_status)) in cases.into_iter().enumerate() {
        let padding = length - prefix.len() - suffix.len();
        let sql = format!("{prefix}{}{suffix}", "x".repeat(padding));
        assert_eq!(
            sql.chars().count(),
            length,
            "case {label:?}: the test's own analysis text is the wrong length"
        );
        let (status, body) = create_analysis_template(
            &emulator,
            &membership,
            json!({
                "name": format!("text_cap_{index}"),
                "format": "SQL",
                "source": { "text": sql },
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {label:?} ({length} characters): expected HTTP {expected_status}, got {status}"
        );
        if expected_status == 200 {
            assert_eq!(
                body["analysisTemplate"]["schema"]["referencedTables"],
                json!(["sales"]),
                "case {label:?}: expected the relation set to be read, got {:?}",
                body["analysisTemplate"]["schema"]
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body["fieldList"][0]["name"].as_str(),
                Some("source.text"),
                "case {label:?}: the refusal must name the member it is about, got {:?}",
                body.get("fieldList")
            );
        }
    }
}

#[tokio::test]
async fn create_analysis_template_refuses_an_analysis_too_big_a_tree_to_walk() {
    let emulator = Emulator::new();
    let membership = membership_id(&emulator, "depth-cap-collab").await;

    // The tree the service parses and walks is bounded by its TOKEN count
    // (`state::MAX_PARSED_ANALYSIS_TOKENS`), which is what bounds the depth of the parse,
    // of the relation walk and of the tree's drop. The rows span every shape that grows
    // a tree: bracketed expression nesting, bracketed query nesting, and an UNBRACKETED
    // left-associative operator chain — the shape no bracket count would catch, which
    // the parser builds iteratively into a tree one node deep per operator.
    let parens = |depth: usize| {
        format!(
            "SELECT id FROM sales WHERE {}1 = 1{}",
            "(".repeat(depth),
            ")".repeat(depth)
        )
    };
    let subqueries = |depth: usize| {
        let mut sql = "SELECT id FROM sales".to_string();
        for _ in 0..depth {
            sql = format!("SELECT id FROM ({sql}) AS t");
        }
        sql
    };
    // `SELECT 1 + 1 + …`: two non-whitespace tokens (`SELECT`, `1`) plus two per
    // operator, so the token count is exactly 2 + 2 * operators.
    let chain = |operators: usize| format!("SELECT 1{}", " + 1".repeat(operators));
    let chain_at = |tokens: usize| chain((tokens - 2) / 2);

    let cases: [(&str, String, u16, bool); 8] = [
        ("shallow parentheses", parens(3), 200, true),
        ("parentheses past the parser's recursion limit", parens(300), 400, false),
        ("shallow subqueries", subqueries(3), 200, true),
        ("subqueries past the token cap", subqueries(300), 400, false),
        ("short unbracketed operator chain", chain(10), 200, false),
        (
            "unbracketed operator chain at the token cap",
            chain_at(MAX_PARSED_ANALYSIS_TOKENS),
            200,
            false,
        ),
        (
            "unbracketed operator chain one operator past the token cap",
            chain_at(MAX_PARSED_ANALYSIS_TOKENS + 2),
            400,
            false,
        ),
        (
            "unbracketed operator chain far past the token cap",
            chain(20_000),
            400,
            false,
        ),
    ];

    for (index, (label, sql, expected_status, reads_sales)) in cases.into_iter().enumerate() {
        let (status, body) = create_analysis_template(
            &emulator,
            &membership,
            json!({
                "name": format!("depth_{index}"),
                "format": "SQL",
                "source": { "text": sql },
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        if expected_status == 200 {
            let expected = if reads_sales {
                json!(["sales"])
            } else {
                json!([])
            };
            assert_eq!(
                body["analysisTemplate"]["schema"]["referencedTables"], expected,
                "case {label:?}: expected referencedTables {expected}, got {:?}",
                body["analysisTemplate"]["schema"]
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body["fieldList"][0]["name"].as_str(),
                Some("source.text"),
                "case {label:?}: the refusal must name the member it is about, got {:?}",
                body.get("fieldList")
            );
        }
    }
}

#[tokio::test]
async fn list_analysis_templates_pages_its_membership_s_templates_in_creation_order() {
    let emulator = Emulator::new();
    let membership = membership_id(&emulator, "list-collab").await;
    let other_membership = membership_id(&emulator, "list-collab-other").await;

    // Three templates in creation order, the last one a synthetic-data analysis, so the
    // summary's derived `isSyntheticData` differs across rows.
    let mut created: Vec<Value> = Vec::new();
    for (index, name) in ["first", "second", "third_synthetic"].iter().enumerate() {
        let mut body = json!({
            "name": name,
            "description": format!("template {index}"),
            "format": "SQL",
            "source": { "text": format!("SELECT id FROM sales_{index}") },
        });
        if *name == "third_synthetic" {
            body["format"] = json!("PYSPARK_1_0");
            body["source"] = artifacts_source();
            body["schema"] = json!({ "referencedTables": ["sales"] });
            body["syntheticDataParameters"] = synthetic_data_parameters();
        }
        let (status, response) = create_analysis_template(&emulator, &membership, body).await;
        assert_eq!(status, 200, "setup {name:?}: CreateAnalysisTemplate: {response}");
        created.push(response["analysisTemplate"].clone());
    }
    // One template under another membership, which this listing must not report.
    let (status, elsewhere) = create_analysis_template(
        &emulator,
        &other_membership,
        json!({
            "name": "not_in_the_listing",
            "format": "SQL",
            "source": { "text": "SELECT 1" },
        }),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateAnalysisTemplate elsewhere: {elsewhere}");

    let expected_ids: Vec<String> = created
        .iter()
        .map(|template| template["id"].as_str().expect("id").to_string())
        .collect();

    // Rows: no page size, each published bound of `maxResults` (model `MaxResults`,
    // range 1..100), and a size between them — every row must walk the same sequence.
    for max_results in [None, Some(1), Some(2), Some(100)] {
        let label = format!("maxResults={max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(max) = max_results {
                query.push(format!("maxResults={max}"));
            }
            if let Some(token) = &next_token {
                query.push(format!("nextToken={token}"));
            }
            let path = format!(
                "/memberships/{membership}/analysistemplates{}{}",
                if query.is_empty() { "" } else { "?" },
                query.join("&")
            );
            let (status, page) = emulator.call("GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected ListAnalysisTemplates 200, got {status} with {page}"
            );
            let summaries = page["analysisTemplateSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case {label:?}: no analysisTemplateSummaries in {page}"))
                .clone();
            if let Some(max) = max_results {
                assert!(
                    summaries.len() <= max as usize,
                    "case {label:?}: a page may not exceed the requested size, got {}",
                    summaries.len()
                );
            }
            for summary in &summaries {
                // Each summary is the projection of the stored template.
                let id = summary["id"].as_str().expect("summary id").to_string();
                let template = created
                    .iter()
                    .find(|t| t["id"].as_str() == Some(id.as_str()))
                    .unwrap_or_else(|| panic!("case {label:?}: unknown id {id} in {page}"));
                for member in [
                    "arn",
                    "name",
                    "description",
                    "createTime",
                    "updateTime",
                    "membershipId",
                    "membershipArn",
                    "collaborationId",
                    "collaborationArn",
                ] {
                    assert_eq!(
                        summary[member], template[member],
                        "case {label:?}: summary {member} must match the stored template, got {:?}",
                        summary[member]
                    );
                }
                assert_eq!(
                    summary["isSyntheticData"].as_bool(),
                    Some(template.get("syntheticDataParameters").is_some()),
                    "case {label:?}: isSyntheticData is derived from syntheticDataParameters, got {:?}",
                    summary.get("isSyntheticData")
                );
                seen.push(id);
            }
            next_token = page["nextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected_ids,
            "case {label:?}: expected the membership's templates in creation order {expected_ids:?}, got {seen:?}"
        );
    }

    // Past the published bound, the modelled range refuses the request.
    let (status, refused) = emulator
        .call(
            "GET",
            &format!("/memberships/{membership}/analysistemplates?maxResults=101"),
            None,
        )
        .await;
    assert_eq!(
        status, 400,
        "maxResults past the declared maximum of 100 must be refused, got {status} with {refused}"
    );

    // An unknown membership has no listing.
    let (status, refused) = emulator
        .call(
            "GET",
            "/memberships/00000000-0000-4000-8000-000000000000/analysistemplates",
            None,
        )
        .await;
    assert_eq!(
        status, 404,
        "an unknown membership has no listing, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {refused}"
    );
}

#[tokio::test]
async fn analysis_templates_are_scoped_to_the_membership_in_the_path() {
    let emulator = Emulator::new();
    let membership = membership_id(&emulator, "scope-collab").await;
    let other_membership = membership_id(&emulator, "scope-collab-other").await;
    let unknown_membership = "00000000-0000-4000-8000-000000000000";
    let unknown_template = "11111111-1111-4111-8111-111111111111";

    let (status, created) = create_analysis_template(
        &emulator,
        &membership,
        json!({
            "name": "scoped",
            "format": "SQL",
            "source": { "text": "SELECT id FROM sales" },
        }),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateAnalysisTemplate: {created}");
    let id = created["analysisTemplate"]["id"]
        .as_str()
        .expect("setup: analysisTemplate.id")
        .to_string();

    // Each row is a lookup that must not resolve, with the resource the 404 names.
    let cases: [(&str, String, String, &str, &str); 4] = [
        (
            "create under a membership that does not exist",
            unknown_membership.to_string(),
            String::new(),
            unknown_membership,
            "MEMBERSHIP",
        ),
        (
            "read under a membership that does not exist",
            unknown_membership.to_string(),
            id.clone(),
            unknown_membership,
            "MEMBERSHIP",
        ),
        (
            "read a template id that does not exist",
            membership.clone(),
            unknown_template.to_string(),
            unknown_template,
            "ANALYSIS_TEMPLATE",
        ),
        (
            "read a template that belongs to another membership",
            other_membership.clone(),
            id.clone(),
            &id,
            "ANALYSIS_TEMPLATE",
        ),
    ];

    for (label, membership_path, template_path, expected_resource, expected_type) in cases {
        let (status, body) = if template_path.is_empty() {
            create_analysis_template(
                &emulator,
                &membership_path,
                json!({
                    "name": "unreachable",
                    "format": "SQL",
                    "source": { "text": "SELECT 1" },
                }),
            )
            .await
        } else {
            get_analysis_template(&emulator, &membership_path, &template_path).await
        };

        assert_eq!(
            status, 404,
            "case {label:?}: expected HTTP 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(expected_resource),
            "case {label:?}: expected resourceId {expected_resource:?}, got {:?}",
            body.get("resourceId")
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some(expected_type),
            "case {label:?}: expected resourceType {expected_type:?}, got {:?}",
            body.get("resourceType")
        );
    }

    // None of those refusals disturbed the stored template.
    let (status, fetched) = get_analysis_template(&emulator, &membership, &id).await;
    assert_eq!(
        status, 200,
        "the stored template must still be readable under its own membership, got {status} with {fetched}"
    );
    assert_eq!(
        fetched["analysisTemplate"], created["analysisTemplate"],
        "the stored template must be unchanged, got {:?}",
        fetched["analysisTemplate"]
    );
}
