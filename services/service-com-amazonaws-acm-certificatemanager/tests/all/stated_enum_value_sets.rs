// SPDX-License-Identifier: CC0-1.0

//! How ACM states an enum value set, and the position it states it against, over the
//! wire (`POST /`, `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! Three rules are under test, each over a TABLE rather than the one request a trace
//! recorded:
//!
//! 1. A clause about a value outside an enum's value set lists that set in the order
//!    THIS SERVICE states the enum's values in. For an enum the service publishes an
//!    order for that order is stated verbatim, at every member targeting the enum, at
//!    every depth, and whichever seam refused the request — the boundary validator or a
//!    handler. For every other enum the clause states the modelled values in the order
//!    it was handed them, so the set is the model's wherever no order is published.
//! 2. The clause names the input position that failed in the WIRE spelling of its path
//!    — the lowerCamel member names, dot-joined from the input root — and never the
//!    value submitted there, nor the model's own casing of the path.
//! 3. A value INSIDE the set is served, so the refusals above are the value set's doing
//!    rather than a member the emulator cannot take at all. Each row is driven with the
//!    LAST value its enum declares, which is an entry no trace cites.
//!
//! The rows span both filter unions of `SearchCertificates` (every enum member of
//! each), a top-level list member of `ListCertificates`, a list nested inside a
//! structure, a plain structure member six steps deep, both clause forms (a member's own
//! requirement and a collection element's nested constraint set), and the two enums
//! whose stated order is published beside eight that have none.
//!
//! The envelope itself (`N validation error(s) detected: `) and the other requirement
//! classes have their own suite in [`crate::validation_message_rendering`].

use serde_json::{json, Value};

use crate::certificate_lifecycle::{call, error_code, error_message, router};

/// How ACM names the subject of a constraint clause: the input position, never the
/// submitted value.
const WITHHOLDING_SUBJECT: &str = "Value of the input at";

/// Every `CertificateStatus` the model declares, in the order it declares them
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#CertificateStatus")`).
const CERTIFICATE_STATUS: &[&str] = &[
    "PENDING_VALIDATION",
    "ISSUED",
    "INACTIVE",
    "EXPIRED",
    "VALIDATION_TIMED_OUT",
    "REVOKED",
    "FAILED",
];

/// The order ACM STATES `CertificateStatus`'s value set in, recorded on its own refusal
/// of a status outside the set.
const STATED_CERTIFICATE_STATUS: &[&str] = &[
    "VALIDATION_TIMED_OUT",
    "PENDING_VALIDATION",
    "EXPIRED",
    "INACTIVE",
    "ISSUED",
    "FAILED",
    "REVOKED",
];

/// Every `CertificateType` the model declares, in declaration order
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#CertificateType")`).
const CERTIFICATE_TYPE: &[&str] = &["IMPORTED", "AMAZON_ISSUED", "PRIVATE"];

/// The order ACM STATES `CertificateType`'s value set in.
const STATED_CERTIFICATE_TYPE: &[&str] = &["PRIVATE", "IMPORTED", "AMAZON_ISSUED"];

/// `com.amazonaws.acm#RenewalStatus`, in declaration order.
const RENEWAL_STATUS: &[&str] = &["PENDING_AUTO_RENEWAL", "PENDING_VALIDATION", "SUCCESS", "FAILED"];

/// `com.amazonaws.acm#CertificateExport`, in declaration order.
const CERTIFICATE_EXPORT: &[&str] = &["ENABLED", "DISABLED"];

/// `com.amazonaws.acm#CertificateManagedBy`, in declaration order.
const CERTIFICATE_MANAGED_BY: &[&str] = &["CLOUDFRONT"];

/// `com.amazonaws.acm#ValidationMethod`, in declaration order.
const VALIDATION_METHOD: &[&str] = &["EMAIL", "DNS", "HTTP"];

/// `com.amazonaws.acm#CertificateKeyPairOrigin`, in declaration order.
const CERTIFICATE_KEY_PAIR_ORIGIN: &[&str] = &["AWS_MANAGED", "ACME", "CUSTOMER_PROVIDED"];

/// `com.amazonaws.acm#ExtendedKeyUsageName`, in declaration order.
const EXTENDED_KEY_USAGE_NAME: &[&str] = &[
    "TLS_WEB_SERVER_AUTHENTICATION",
    "TLS_WEB_CLIENT_AUTHENTICATION",
    "CODE_SIGNING",
    "EMAIL_PROTECTION",
    "TIME_STAMPING",
    "OCSP_SIGNING",
    "IPSEC_END_SYSTEM",
    "IPSEC_TUNNEL",
    "IPSEC_USER",
    "ANY",
    "NONE",
    "CUSTOM",
];

/// `com.amazonaws.acm#KeyUsageName`, in declaration order.
const KEY_USAGE_NAME: &[&str] = &[
    "DIGITAL_SIGNATURE",
    "NON_REPUDIATION",
    "KEY_ENCIPHERMENT",
    "DATA_ENCIPHERMENT",
    "KEY_AGREEMENT",
    "CERTIFICATE_SIGNING",
    "CRL_SIGNING",
    "ENCIPHER_ONLY",
    "DECIPHER_ONLY",
    "ANY",
    "CUSTOM",
];

/// `com.amazonaws.acm#KeyAlgorithm`, in declaration order.
const KEY_ALGORITHM: &[&str] = &[
    "RSA_1024",
    "RSA_2048",
    "RSA_3072",
    "RSA_4096",
    "EC_prime256v1",
    "EC_secp384r1",
    "EC_secp521r1",
];

/// `com.amazonaws.acm#ComparisonOperator`, in declaration order.
const COMPARISON_OPERATOR: &[&str] = &["CONTAINS", "EQUALS"];

/// Which clause form the requirement is stated in.
#[derive(Clone, Copy)]
enum Form {
    /// The member's own requirement: the value set alone.
    Member,
    /// A collection ELEMENT's requirement, reported at the collection with the element
    /// shape's whole constraint set nested — the value set, then that an element is
    /// never null.
    ElementSet,
    /// A collection element whose element constraint set this service cannot resolve
    /// (a list nested inside a structure): the one violated requirement, nested.
    ElementAlone,
}

/// One row: what it exercises, the operation, a body builder taking the value to put at
/// the position, the member path a clause names, the clause form, the values the model
/// declares for the enum, the order ACM states them in (`None` where it publishes none)
/// and the out-of-set value the row submits.
struct Row {
    what: &'static str,
    operation: &'static str,
    body: fn(&str) -> Value,
    path: &'static str,
    form: Form,
    modelled: &'static [&'static str],
    stated: Option<&'static [&'static str]>,
    submitted: &'static str,
}

/// The rows the three rules are asserted over.
fn rows() -> Vec<Row> {
    vec![
        // The ACM-metadata filter union: every enum member of it, each at the same
        // depth and each refused by the handler, since the validating walk does not
        // descend into a union member.
        Row {
            what: "a Type outside CertificateType, on a union member",
            operation: "SearchCertificates",
            body: |value| {
                json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "Type": value } } } })
            },
            path: "filterStatement.filter.acmCertificateMetadataFilter.type",
            form: Form::Member,
            modelled: CERTIFICATE_TYPE,
            stated: Some(STATED_CERTIFICATE_TYPE),
            submitted: "NOT_A_REAL_TYPE",
        },
        Row {
            what: "a Status outside CertificateStatus, on a union member",
            operation: "SearchCertificates",
            body: |value| {
                json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "Status": value } } } })
            },
            path: "filterStatement.filter.acmCertificateMetadataFilter.status",
            form: Form::Member,
            modelled: CERTIFICATE_STATUS,
            stated: Some(STATED_CERTIFICATE_STATUS),
            // A legal status of the enum, lower-cased: an enum value set is matched
            // exactly.
            submitted: "issued",
        },
        Row {
            what: "a RenewalStatus outside its enum, which publishes no stated order",
            operation: "SearchCertificates",
            body: |value| {
                json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "RenewalStatus": value } } } })
            },
            path: "filterStatement.filter.acmCertificateMetadataFilter.renewalStatus",
            form: Form::Member,
            modelled: RENEWAL_STATUS,
            stated: None,
            // A value of ANOTHER enum of this service.
            submitted: "CLOUDFRONT",
        },
        Row {
            what: "an ExportOption outside CertificateExport, on a union member",
            operation: "SearchCertificates",
            body: |value| {
                json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "ExportOption": value } } } })
            },
            path: "filterStatement.filter.acmCertificateMetadataFilter.exportOption",
            form: Form::Member,
            modelled: CERTIFICATE_EXPORT,
            stated: None,
            submitted: "ENABLED_SOMETIMES",
        },
        Row {
            what: "a ManagedBy outside CertificateManagedBy, a one-value enum",
            operation: "SearchCertificates",
            body: |value| {
                json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "ManagedBy": value } } } })
            },
            path: "filterStatement.filter.acmCertificateMetadataFilter.managedBy",
            form: Form::Member,
            modelled: CERTIFICATE_MANAGED_BY,
            stated: None,
            submitted: "APIGATEWAY",
        },
        Row {
            what: "a ValidationMethod outside its enum, on a union member",
            operation: "SearchCertificates",
            body: |value| {
                json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "ValidationMethod": value } } } })
            },
            path: "filterStatement.filter.acmCertificateMetadataFilter.validationMethod",
            form: Form::Member,
            modelled: VALIDATION_METHOD,
            stated: None,
            submitted: "CARRIER_PIGEON",
        },
        Row {
            what: "a CertificateKeyPairOrigin outside its enum, on a union member",
            operation: "SearchCertificates",
            body: |value| {
                json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "CertificateKeyPairOrigin": value } } } })
            },
            path: "filterStatement.filter.acmCertificateMetadataFilter.certificateKeyPairOrigin",
            form: Form::Member,
            modelled: CERTIFICATE_KEY_PAIR_ORIGIN,
            stated: None,
            // A value whose spelling differs from a legal one by one character.
            submitted: "AWS-MANAGED",
        },
        // The X.509 filter union: the enum members whose value sits directly on the
        // union member.
        Row {
            what: "an ExtendedKeyUsage outside its enum, on the X.509 filter union",
            operation: "SearchCertificates",
            body: |value| {
                json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "ExtendedKeyUsage": value } } } })
            },
            path: "filterStatement.filter.x509AttributeFilter.extendedKeyUsage",
            form: Form::Member,
            modelled: EXTENDED_KEY_USAGE_NAME,
            stated: None,
            submitted: "TLS_WEB_EVERYTHING",
        },
        Row {
            what: "a KeyUsage outside its enum, on the X.509 filter union",
            operation: "SearchCertificates",
            body: |value| {
                json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "KeyUsage": value } } } })
            },
            path: "filterStatement.filter.x509AttributeFilter.keyUsage",
            form: Form::Member,
            modelled: KEY_USAGE_NAME,
            stated: None,
            submitted: "KEY_SHARING",
        },
        Row {
            what: "a KeyAlgorithm outside its enum, on the X.509 filter union",
            operation: "SearchCertificates",
            body: |value| {
                json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "KeyAlgorithm": value } } } })
            },
            path: "filterStatement.filter.x509AttributeFilter.keyAlgorithm",
            form: Form::Member,
            modelled: KEY_ALGORITHM,
            stated: None,
            submitted: "RSA_8192",
        },
        // A plain STRUCTURE member six steps in, refused by the boundary validator
        // rather than the handler: the same subject, the same wire path spelling.
        Row {
            what: "a ComparisonOperator outside its enum, refused by the boundary walk",
            operation: "SearchCertificates",
            body: |value| {
                json!({
                    "FilterStatement": {
                        "Filter": {
                            "X509AttributeFilter": {
                                "Subject": {
                                    "CommonName": {
                                        "Value": "alpha.example.com",
                                        "ComparisonOperator": value,
                                    },
                                },
                            },
                        },
                    },
                })
            },
            path: "filterStatement.filter.x509AttributeFilter.subject.commonName.comparisonOperator",
            form: Form::Member,
            modelled: COMPARISON_OPERATOR,
            stated: None,
            submitted: "STARTS_WITH",
        },
        // A TOP-LEVEL list member, whose element clause is reported at the collection
        // with the element shape's whole constraint set nested.
        Row {
            what: "a CertificateStatuses element outside CertificateStatus",
            operation: "ListCertificates",
            body: |value| json!({ "CertificateStatuses": [value] }),
            path: "certificateStatuses",
            form: Form::ElementSet,
            modelled: CERTIFICATE_STATUS,
            stated: Some(STATED_CERTIFICATE_STATUS),
            submitted: "NOT_A_STATUS",
        },
        // A list NESTED inside a structure, whose element constraint set this service
        // cannot resolve from an input schema: the one violated requirement, nested.
        Row {
            what: "a keyTypes element outside KeyAlgorithm, on a nested list",
            operation: "ListCertificates",
            body: |value| json!({ "Includes": { "keyTypes": [value] } }),
            path: "includes.keyTypes",
            form: Form::ElementAlone,
            modelled: KEY_ALGORITHM,
            stated: None,
            submitted: "RSA_0",
        },
        // An enum member of that same structure, which is not a collection at all.
        Row {
            what: "an exportOption outside CertificateExport, on a nested structure",
            operation: "ListCertificates",
            body: |value| json!({ "Includes": { "exportOption": value } }),
            path: "includes.exportOption",
            form: Form::Member,
            modelled: CERTIFICATE_EXPORT,
            stated: None,
            submitted: "",
        },
    ]
}

/// The whole message a row must answer with, over the value set `set` as the answer
/// states it.
fn expected_message(row: &Row, set: &str) -> String {
    let requirement = match row.form {
        Form::Member => format!("Member must satisfy enum value set: [{set}]"),
        Form::ElementSet => format!(
            "Member must satisfy constraint: [Member must satisfy enum value set: [{set}], \
             Member must not be null]"
        ),
        Form::ElementAlone => {
            format!("Member must satisfy constraint: [Member must satisfy enum value set: [{set}]]")
        }
    };
    format!(
        "1 validation error detected: {WITHHOLDING_SUBJECT} '{}' failed to satisfy constraint: \
         {requirement}",
        row.path
    )
}

/// The value set `message` states, as the comma-separated text between the brackets of
/// its `enum value set: [...]` clause.
fn stated_set(message: &str) -> Option<&str> {
    let (_, tail) = message.split_once("enum value set: [")?;
    let (set, _) = tail.split_once(']')?;
    Some(set)
}

/// `path` in the model's own casing, which is NOT how a clause spells a position: the
/// wire path lower-cases each member's first character.
fn model_casing(path: &str) -> String {
    path.split('.')
        .map(|segment| {
            let mut chars = segment.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(".")
}

/// RULE: a clause about a value outside an enum's value set states that set in the order
/// this service states the enum's values in — the published order verbatim where there
/// is one, and the modelled values as presented where there is not — against the WIRE
/// path of the position that failed, with nothing the request submitted echoed into it.
#[tokio::test]
async fn an_enum_clause_states_the_service_order_against_the_wire_path() {
    let router = router();

    for row in rows() {
        let body = (row.body)(row.submitted);
        let (status, response) = call(&router, row.operation, &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {:?} ({}): expected 400 ValidationException, got {status} {response}",
            row.what,
            row.operation
        );
        let stated = error_message(&response);
        let set = stated_set(&stated).unwrap_or_else(|| {
            panic!(
                "case {:?} ({}): the message states no enum value set, got {stated:?}",
                row.what, row.operation
            )
        });

        match row.stated {
            // A published order is stated verbatim.
            Some(published) => assert_eq!(
                stated,
                expected_message(&row, &published.join(", ")),
                "case {:?} ({}): expected the stated value set order, got {stated:?}",
                row.what,
                row.operation
            ),
            // No published order: the clause states the modelled values, in whatever
            // order it was handed them.
            None => {
                let mut listed: Vec<&str> = set.split(", ").collect();
                let mut declared: Vec<&str> = row.modelled.to_vec();
                listed.sort_unstable();
                declared.sort_unstable();
                assert_eq!(
                    listed, declared,
                    "case {:?} ({}): expected the modelled value set, got {stated:?}",
                    row.what, row.operation
                );
                assert_eq!(
                    stated,
                    expected_message(&row, set),
                    "case {:?} ({}): expected the ACM enum clause, got {stated:?}",
                    row.what,
                    row.operation
                );
            }
        }

        // The position is named in the wire spelling and nothing else: not the model's
        // casing of the path, and not the value submitted there.
        assert!(
            stated.contains(&format!("'{}'", row.path)),
            "case {:?} ({}): expected the wire member path {:?}, got {stated:?}",
            row.what,
            row.operation,
            row.path
        );
        assert!(
            !stated.contains(&format!("'{}'", model_casing(row.path))),
            "case {:?} ({}): the clause must name the wire path, not the model's casing \
             {:?}, got {stated:?}",
            row.what,
            row.operation,
            model_casing(row.path)
        );
        if !row.submitted.is_empty() {
            assert!(
                !stated.contains(row.submitted),
                "case {:?} ({}): the submitted value {:?} must not be stated, got {stated:?}",
                row.what,
                row.operation,
                row.submitted
            );
        }
    }
}

/// RULE: a value INSIDE the enum's value set is served, so every refusal above is the
/// value set's doing rather than a member this emulator cannot take.
///
/// Each row is driven with the LAST value its enum declares, an entry no trace cites.
#[tokio::test]
async fn a_value_inside_the_set_is_served() {
    let router = router();

    for row in rows() {
        let legal = row
            .modelled
            .last()
            .unwrap_or_else(|| panic!("case {:?}: the enum declares no value", row.what));
        let body = (row.body)(legal);
        let (status, response) = call(&router, row.operation, &body).await;
        assert_eq!(
            status, 200,
            "case {:?} ({}) with {legal:?}: expected HTTP 200, got {status} {response}",
            row.what, row.operation
        );
    }
}

/// RULE: a published order is a RE-ORDERING of the enum's modelled values — it holds
/// exactly them, once each — and it is the order the service states rather than one the
/// model or a sort would give.
#[tokio::test]
async fn a_published_order_re_orders_the_modelled_values() {
    // (the enum, the values the model declares, the order ACM states them in)
    let rows: [(&str, &[&str], &[&str]); 2] = [
        ("CertificateStatus", CERTIFICATE_STATUS, STATED_CERTIFICATE_STATUS),
        ("CertificateType", CERTIFICATE_TYPE, STATED_CERTIFICATE_TYPE),
    ];

    for (name, modelled, stated) in rows {
        let mut sorted_stated: Vec<&str> = stated.to_vec();
        let mut sorted_modelled: Vec<&str> = modelled.to_vec();
        sorted_stated.sort_unstable();
        sorted_modelled.sort_unstable();
        assert_eq!(
            sorted_stated, sorted_modelled,
            "case {name}: the stated order must hold exactly the modelled values, got {stated:?}"
        );
        assert_eq!(
            stated.len(),
            modelled.len(),
            "case {name}: the stated order must list each value once, got {stated:?}"
        );
        assert_ne!(
            stated, modelled,
            "case {name}: the stated order is not the model's declaration order"
        );
        assert_ne!(
            stated,
            sorted_modelled.as_slice(),
            "case {name}: the stated order is not a sort of the values"
        );
    }
}
