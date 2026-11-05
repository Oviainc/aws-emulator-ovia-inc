// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for DataZone custom asset types (`CreateAssetType` ->
//! `GetAssetType`).
//!
//! Every case drives the service the way a client does: a real `restJson1` request
//! through the router, with only the response body as evidence. Nothing reaches into
//! `crate::state`, so a handler answering from a hardcoded value instead of from the
//! store would fail these tests.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `CreateAssetType` persists a named asset type under its domain and owning project
//!    at the first revision, projecting every `formsInput` entry into `formsOutput`
//!    (the form type NAME its `typeIdentifier` resolves to, plus `typeRevision` and
//!    `required`), and `GetAssetType` reports exactly that stored record.
//! 2. A name is unique WITHIN a domain: a duplicate is refused with `ConflictException`
//!    and leaves the stored type alone, while the same name in another domain is fine.
//! 3. The documented prerequisites are enforced: an unknown domain or owning project is
//!    refused, and a domain that has not finished creating cannot take an asset type.
//! 4. An asset type is stored under its domain, so it disappears when the domain does.

use crate::harness::{Answer, Emulator, SETTLED};
use serde_json::{json, Map, Value};

/// The principal every request here signs as: `crate::state::caller_principal` derives
/// it from the SigV4 credential scope's access key id.
const CALLER: &str = "arn:aws:iam::000000000000:user/AKIAEXAMPLETESTKEY";

/// The two asset-type calls this suite drives, on the shared [`Emulator`] harness.
impl Emulator {
    async fn create_asset_type(&self, domain: &str, body: Value) -> Answer {
        self.call(
            "POST",
            &format!("/v2/domains/{domain}/asset-types"),
            Some(body),
        )
        .await
    }

    async fn get_asset_type(&self, domain: &str, identifier: &str, query: &str) -> Answer {
        self.call(
            "GET",
            &format!("/v2/domains/{domain}/asset-types/{identifier}{query}"),
            None,
        )
        .await
    }
}

/// One metadata form a `CreateAssetType` request attaches, plus the `typeName` the
/// stored type must report for it.
///
/// `FormEntryInput` carries `typeIdentifier` (a `FormTypeIdentifier`: len 1..385,
/// pattern `^(?!\.)[\w\.]*\w$`), while `FormEntryOutput` carries `typeName` (a
/// `FormTypeName`: len 1..128, pattern
/// `^(amazon.datazone.)?(?![0-9_])\w+$|^_\w*[a-zA-Z0-9]\w*$`) — both from
/// `smithy_query(query="shape",shape="FormEntryInput,FormEntryOutput")`. So an
/// identifier that is already a well-formed name (including one in the service's own
/// `amazon.datazone.` namespace) is reported unchanged, and a qualified identifier in
/// another namespace reports its unqualified last segment.
#[derive(Clone)]
struct FormRow {
    form_name: String,
    type_identifier: String,
    type_revision: String,
    required: Option<bool>,
    expect_type_name: String,
}

impl FormRow {
    fn new(
        form_name: &str,
        type_identifier: &str,
        type_revision: &str,
        required: Option<bool>,
        expect_type_name: &str,
    ) -> Self {
        Self {
            form_name: form_name.to_string(),
            type_identifier: type_identifier.to_string(),
            type_revision: type_revision.to_string(),
            required,
            expect_type_name: expect_type_name.to_string(),
        }
    }
}

/// One `CreateAssetType` request. The rows differ along what the rule generalises over:
/// the name's length, whether a description is present, and how many forms are attached
/// and in what shape.
struct AssetTypeCase {
    label: String,
    name: String,
    description: Option<String>,
    forms: Vec<FormRow>,
}

impl AssetTypeCase {
    /// The request body: `formsInput` is REQUIRED, and the documentation says it may be
    /// empty (`smithy_query(query="doc",shape="CreateAssetType")`).
    fn body(&self, owning_project: &str) -> Value {
        let mut forms = Map::new();
        for form in &self.forms {
            let mut entry = Map::new();
            entry.insert(
                "typeIdentifier".to_string(),
                Value::String(form.type_identifier.clone()),
            );
            entry.insert(
                "typeRevision".to_string(),
                Value::String(form.type_revision.clone()),
            );
            if let Some(required) = form.required {
                entry.insert("required".to_string(), Value::Bool(required));
            }
            forms.insert(form.form_name.clone(), Value::Object(entry));
        }
        let mut body = Map::new();
        body.insert("name".to_string(), Value::String(self.name.clone()));
        body.insert(
            "owningProjectIdentifier".to_string(),
            Value::String(owning_project.to_string()),
        );
        body.insert("formsInput".to_string(), Value::Object(forms));
        if let Some(description) = &self.description {
            body.insert(
                "description".to_string(),
                Value::String(description.clone()),
            );
        }
        Value::Object(body)
    }

    /// The `formsOutput` the response must carry for this row.
    fn expected_forms_output(&self) -> Value {
        let mut forms = Map::new();
        for form in &self.forms {
            let mut entry = Map::new();
            entry.insert(
                "typeName".to_string(),
                Value::String(form.expect_type_name.clone()),
            );
            entry.insert(
                "typeRevision".to_string(),
                Value::String(form.type_revision.clone()),
            );
            if let Some(required) = form.required {
                entry.insert("required".to_string(), Value::Bool(required));
            }
            forms.insert(form.form_name.clone(), Value::Object(entry));
        }
        Value::Object(forms)
    }
}

fn asset_type_cases() -> Vec<AssetTypeCase> {
    vec![
        AssetTypeCase {
            label: "no description and the empty formsInput the docs allow".to_string(),
            name: "MinimalType".to_string(),
            description: None,
            forms: Vec::new(),
        },
        AssetTypeCase {
            label: "one required form in the service's own namespace".to_string(),
            name: "SingleFormType".to_string(),
            description: Some("a type with one form".to_string()),
            forms: vec![FormRow::new(
                "GlossaryTerms",
                // A built-in form type: `amazon.datazone.` is part of the NAME, so the
                // identifier is reported unchanged.
                "amazon.datazone.GlossaryTermsFormType",
                "1",
                Some(true),
                "amazon.datazone.GlossaryTermsFormType",
            )],
        },
        AssetTypeCase {
            label: "three forms: a plain name, an underscore-led name and a qualified identifier"
                .to_string(),
            // Word characters and digits: `AssetTypeIdentifier` (pattern
            // `^(?!\.)[\w\.]*\w$`) is how a later read addresses the type by name.
            name: "Three_Form_Type3".to_string(),
            description: Some("punctuation: ok! and a longer description".to_string()),
            forms: vec![
                FormRow::new(
                    "TableForm",
                    "CustomTableForm",
                    "2",
                    Some(false),
                    "CustomTableForm",
                ),
                // `required` is optional on input, so it stays absent on output.
                FormRow::new("PrivateForm", "_privateForm1", "1", None, "_privateForm1"),
                FormRow::new(
                    "QualifiedForm",
                    // Qualified in another namespace: the form type's own name is the
                    // unqualified last segment.
                    "mydomain.sub.LeafFormType",
                    "7",
                    Some(true),
                    "LeafFormType",
                ),
            ],
        },
        AssetTypeCase {
            // `FormsInputMap` is len 0..10 (smithy_query shape="FormsInputMap"), so ten
            // forms is the published maximum.
            label: "the maximum ten forms".to_string(),
            name: "TenFormType".to_string(),
            description: None,
            forms: (0..10)
                .map(|i| {
                    FormRow::new(
                        &format!("Form{i}"),
                        &format!("FormType{i}"),
                        &format!("{}", i + 1),
                        Some(i % 2 == 0),
                        &format!("FormType{i}"),
                    )
                })
                .collect(),
        },
        AssetTypeCase {
            // `TypeName` is len 1..256 (smithy_query shape="TypeName"): both bounds.
            label: "a name at the modelled one-character minimum".to_string(),
            name: "A".to_string(),
            description: None,
            forms: Vec::new(),
        },
        AssetTypeCase {
            label: "a name at the modelled 256-character maximum".to_string(),
            name: "n".repeat(256),
            description: Some("x".repeat(2048)),
            forms: Vec::new(),
        },
    ]
}

/// Whether a `Timestamp` member carries an instant from this run's clock: the codec
/// renders one as epoch seconds or as RFC3339, and both forms are checked against the
/// same lower bound, which is what rules out a zero/default stand-in.
fn is_timestamp(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Number(seconds)) => seconds.as_f64().is_some_and(|s| s > 1_600_000_000.0),
        Some(Value::String(text)) => {
            let year = text
                .get(..4)
                .and_then(|year| year.parse::<u32>().ok())
                .unwrap_or(0);
            year >= 2020 && text.contains('T')
        }
        _ => false,
    }
}

#[tokio::test]
async fn created_asset_types_are_persisted_and_reported_by_get() {
    let dz = Emulator::new();
    let domain = dz.available_domain("asset-types-domain").await;
    let project = dz.project(&domain, "Owning Project").await;

    for case in asset_type_cases() {
        let label = &case.label;
        let created = dz
            .create_asset_type(&domain, case.body(&project))
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.s("domainId"),
            domain,
            "case {label:?}: the type must report the domain it was created in, got {:?}",
            created.s("domainId")
        );
        assert_eq!(
            created.s("name"),
            case.name,
            "case {label:?}: expected the requested name, got {:?}",
            created.s("name")
        );
        // DataZone numbers revisions from 1 (`Revision`, len 1..64).
        assert_eq!(
            created.s("revision"),
            "1",
            "case {label:?}: expected a freshly created type at revision 1, got {:?}",
            created.s("revision")
        );
        assert_eq!(
            created.s("owningProjectId"),
            project,
            "case {label:?}: expected the owning project, got {:?}",
            created.s("owningProjectId")
        );
        assert_eq!(
            created.body.get("description").and_then(Value::as_str),
            case.description.as_deref(),
            "case {label:?}: expected the requested description, got {:?}",
            created.body.get("description")
        );
        assert_eq!(
            created.body.get("formsOutput"),
            Some(&case.expected_forms_output()),
            "case {label:?}: expected every formsInput entry projected into formsOutput, got {:?}",
            created.body.get("formsOutput")
        );
        // A type created HERE originated nowhere else.
        for member in ["originDomainId", "originProjectId"] {
            assert!(
                created.body.get(member).is_none_or(Value::is_null),
                "case {label:?}: a locally created type must report no {member}, got {:?}",
                created.body.get(member)
            );
        }
        // `createdBy`/`updatedBy` are the caller's principal, derived from the SigV4
        // credential scope — never an echo of the request body.
        for member in ["createdBy", "updatedBy"] {
            assert_eq!(
                created.s(member),
                CALLER,
                "case {label:?}: expected the signing principal as {member}, got {:?}",
                created.s(member)
            );
        }
        for member in ["createdAt", "updatedAt"] {
            assert!(
                is_timestamp(created.body.get(member)),
                "case {label:?}: expected an instant from this run's clock as {member}, got {:?}",
                created.body.get(member)
            );
        }

        // GetAssetType reports exactly what CreateAssetType stored: both operations
        // carry the same output members, so the whole body must match.
        let got = dz.get_asset_type(&domain, &case.name, "").await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: expected 200 from GetAssetType, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.body, created.body,
            "case {label:?}: GetAssetType must report the record CreateAssetType stored"
        );

        // A read changes nothing a later caller sees.
        let again = dz.get_asset_type(&domain, &case.name, "").await;
        assert_eq!(
            again.body, got.body,
            "case {label:?}: reading an asset type twice must answer identically"
        );

        // `revision` selects which revision to report: the stored one is there, any
        // other names a resource that is not.
        let at_revision = dz.get_asset_type(&domain, &case.name, "?revision=1").await;
        assert_eq!(
            at_revision.body, got.body,
            "case {label:?}: GetAssetType at the stored revision must answer identically"
        );
        let missing_revision = dz.get_asset_type(&domain, &case.name, "?revision=2").await;
        assert_eq!(
            (missing_revision.status, missing_revision.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {label:?}: expected 404 ResourceNotFoundException for an unstored revision, got {} {:?}",
            missing_revision.status,
            missing_revision.body
        );
    }

    // Asset types are stored under their domain, so they go when it goes.
    let deleted = dz.call("DELETE", &format!("/v2/domains/{domain}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "DeleteDomain: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    tokio::time::sleep(SETTLED).await;
    let orphaned = dz.get_asset_type(&domain, "MinimalType", "").await;
    assert_eq!(
        (orphaned.status, orphaned.error_code.as_str()),
        (403, "AccessDeniedException"),
        "an asset type must not outlive its domain, got {} {:?}",
        orphaned.status,
        orphaned.body
    );
    assert_eq!(
        orphaned.s("message"),
        "User is not permitted to perform operation: GetAssetType",
        "the denial must name the operation only, got {:?}",
        orphaned.body
    );
}

#[tokio::test]
async fn an_asset_type_name_is_unique_within_its_domain() {
    let dz = Emulator::new();
    let domain = dz.available_domain("unique-names-domain").await;
    let other_domain = dz.available_domain("other-names-domain").await;
    let project = dz.project(&domain, "Owner").await;
    let other_project = dz.project(&other_domain, "Other Owner").await;

    // Every row of the table is created once, then once again with a DIFFERENT
    // description: the second call is refused and the first record stands.
    for case in asset_type_cases() {
        let label = &case.label;
        let created = dz
            .create_asset_type(&domain, case.body(&project))
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected 201 for the first create, got {} {:?}",
            created.status, created.body
        );

        let mut repeat = case.body(&project);
        repeat["description"] = json!("a second create of the same name");
        let conflict = dz.create_asset_type(&domain, repeat).await;
        assert_eq!(
            (conflict.status, conflict.error_code.as_str()),
            (409, "ConflictException"),
            "case {label:?}: expected 409 ConflictException for a duplicate name, got {} {:?}",
            conflict.status,
            conflict.body
        );

        let got = dz.get_asset_type(&domain, &case.name, "").await;
        assert_eq!(
            got.body, created.body,
            "case {label:?}: a refused duplicate must leave the stored type alone"
        );

        // The name is unique within ONE domain: another domain may carry it too.
        let elsewhere = dz
            .create_asset_type(&other_domain, case.body(&other_project))
            .await;
        assert_eq!(
            elsewhere.status, 201,
            "case {label:?}: the same name in another domain must be accepted, got {} {:?}",
            elsewhere.status, elsewhere.body
        );
        assert_eq!(
            elsewhere.s("domainId"),
            other_domain,
            "case {label:?}: the second type must report its own domain"
        );
        // Each domain reports its own type, and only its own.
        assert_eq!(
            dz.get_asset_type(&domain, &case.name, "").await.s("owningProjectId"),
            project,
            "case {label:?}: the first domain must still report its own owning project"
        );
        assert_eq!(
            dz.get_asset_type(&other_domain, &case.name, "")
                .await
                .s("owningProjectId"),
            other_project,
            "case {label:?}: the second domain must report its own owning project"
        );
    }
}

/// One refused `CreateAssetType`: the request, and the status and error code the
/// prerequisite it breaks must be answered with.
struct RefusalCase {
    label: &'static str,
    /// The domain the request is sent to, resolved per test.
    domain: DomainChoice,
    body: Value,
    status: u16,
    code: &'static str,
}

enum DomainChoice {
    Available,
    Unknown,
    StillCreating,
}

#[tokio::test]
async fn create_asset_type_enforces_its_documented_prerequisites() {
    let dz = Emulator::new();
    let domain = dz.available_domain("prerequisites-domain").await;
    let project = dz.project(&domain, "Owner").await;
    // A domain that has NOT settled yet: the creation the clock owes is still due.
    let creating = dz
        .call("POST", "/v2/domains", Some(json!({ "name": "still-creating" })))
        .await
        .s("id");

    let cases = vec![
        RefusalCase {
            label: "an unknown domain",
            domain: DomainChoice::Unknown,
            body: json!({
                "name": "AnyType",
                "owningProjectIdentifier": project.clone(),
                "formsInput": {},
            }),
            // A permission is held ON a domain, so the caller holds none on an
            // identifier the store serves no domain for: authorization decides the
            // create before any lookup.
            status: 403,
            code: "AccessDeniedException",
        },
        RefusalCase {
            label: "an unknown owning project",
            domain: DomainChoice::Available,
            body: json!({
                "name": "AnyType",
                "owningProjectIdentifier": "nosuchproject",
                "formsInput": {},
            }),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            label: "a domain that has not finished creating",
            domain: DomainChoice::StillCreating,
            body: json!({
                "name": "AnyType",
                "owningProjectIdentifier": project.clone(),
                "formsInput": {},
            }),
            status: 409,
            code: "ConflictException",
        },
        RefusalCase {
            label: "a name one character past the modelled 256-character maximum",
            domain: DomainChoice::Available,
            body: json!({
                "name": "n".repeat(257),
                "owningProjectIdentifier": project.clone(),
                "formsInput": {},
            }),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            label: "eleven forms, one past the modelled ten-entry maximum",
            domain: DomainChoice::Available,
            body: json!({
                "name": "ElevenFormType",
                "owningProjectIdentifier": project.clone(),
                "formsInput": Value::Object(
                    (0..11)
                        .map(|i| (
                            format!("Form{i}"),
                            json!({ "typeIdentifier": "SomeFormType", "typeRevision": "1" }),
                        ))
                        .collect::<Map<String, Value>>(),
                ),
            }),
            status: 400,
            code: "ValidationException",
        },
    ];

    for case in cases {
        let target = match case.domain {
            DomainChoice::Available => domain.clone(),
            // A well-formed `DomainId` (^dzd[-_][a-zA-Z0-9_-]{1,36}$) that was never
            // created, so the refusal comes from the handler and not the router.
            DomainChoice::Unknown => "dzd_nosuchdomain".to_string(),
            DomainChoice::StillCreating => creating.clone(),
        };
        let answer = dz.create_asset_type(&target, case.body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (case.status, case.code),
            "case {:?}: expected {} {}, got {} {:?}",
            case.label,
            case.status,
            case.code,
            answer.status,
            answer.body
        );
    }

    // Nothing above was stored: the type those requests named is not there.
    for name in ["AnyType", "ElevenFormType"] {
        let got = dz.get_asset_type(&domain, name, "").await;
        assert_eq!(
            (got.status, got.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "a refused create must persist nothing, but {name:?} answered {} {:?}",
            got.status,
            got.body
        );
    }
}

#[tokio::test]
async fn get_asset_type_reports_what_is_not_there_as_not_found() {
    let dz = Emulator::new();
    let domain = dz.available_domain("get-refusals-domain").await;
    let project = dz.project(&domain, "Owner").await;
    let created = dz
        .create_asset_type(
            &domain,
            json!({
                "name": "StoredType",
                "owningProjectIdentifier": project,
                "formsInput": {},
            }),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201, got {} {:?}",
        created.status, created.body
    );

    // Each row names something that is not stored; the stored type is the control. An
    // unknown DOMAIN is not an existence answer at all: the read is scoped to a domain
    // the caller holds nothing on, so authorization decides it first.
    let cases = [
        (
            "an unknown domain",
            "dzd_nosuchdomain",
            "StoredType",
            403,
            "AccessDeniedException",
        ),
        (
            "an unknown type",
            domain.as_str(),
            "NoSuchType",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a type stored under another name",
            domain.as_str(),
            "storedtype",
            404,
            "ResourceNotFoundException",
        ),
        ("the stored type", domain.as_str(), "StoredType", 200, ""),
    ];
    for (label, target_domain, identifier, status, code) in cases {
        let got = dz.get_asset_type(target_domain, identifier, "").await;
        assert_eq!(
            got.status, status,
            "case {label:?}: expected {status}, got {} {:?}",
            got.status, got.body
        );
        if !code.is_empty() {
            assert_eq!(
                got.error_code, code,
                "case {label:?}: expected {code}, got {:?}",
                got.error_code
            );
        }
    }
}
