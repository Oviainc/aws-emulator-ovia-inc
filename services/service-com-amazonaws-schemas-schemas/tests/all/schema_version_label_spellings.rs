// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE way a `SchemaVersion` label is read, over every
//! operation that names a version of a schema.
//!
//! RULE under test: a `SchemaVersion` label names a stored version only when it IS that
//! version's own decimal rendering — the form every answer of this service renders a
//! version in — so any other spelling of the same value (a sign, zero padding,
//! surrounding space, a decimal point or exponent, a radix prefix, a non-ASCII digit, a
//! word, a number wider than a version counter can reach) names no version at all and
//! takes the version step's `NotFoundException`, having changed nothing. The rule holds
//! for the delete as for every read, because they resolve the label through the same
//! definition.
//!
//! The rows vary along what the rule generalises over: the SPELLING of the label (the
//! class of difference from the rendering, not one example of it), the operation the
//! label is sent to and so the HTTP method, whether the label travels in the URI path or
//! in a query parameter, and — for the labels that DO name a version — which of the
//! schema's versions it names and how many digits it has. Every label is sent
//! percent-encoded byte by byte, so what the handler reads is the spelling the row names
//! rather than whatever a path segment or a form-decoded query would make of it.
//! (`ExportSchema` reads the member through the same definition; it is driven over these
//! spellings in `tests/all/export_reads_the_stored_schema.rs`, against the schemas it can
//! serve.)
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The HTTP
//! harness is the crate's one harness, imported from `registry_schema_lifecycle`, the
//! label encoder from `resource_tags`, and the version URI and the version listing from
//! `schema_version_deletion`. Nothing here touches the state struct directly, so a
//! refusal that had quietly removed a version would still have to make the listing that
//! follows it agree.

use std::collections::BTreeSet;

use serde_json::json;
use service_com_amazonaws_schemas_schemas::state::CODE_RESOURCE_NOT_FOUND;

use crate::registry_schema_lifecycle::{create_registry, schema_uri, Answer, Emulator};
use crate::resource_tags::percent_encode;
use crate::schema_version_deletion::{stored_versions, version_uri};

/// The registry every row below is served against.
const REGISTRY: &str = "version-label-spellings";

/// The schema every row below is served against.
const SCHEMA: &str = "order.placed";

/// How many versions the seeded schema holds: enough that a row can name the first, a
/// middle one, the newest, and one whose rendering is more than a single digit.
const VERSIONS: u32 = 10;

/// The language the code-binding rows name: one of the languages this service generates
/// bindings for.
const LANGUAGE: &str = "Java8";

/// Spellings that are NOT a version's own decimal rendering, with the class of
/// difference each one stands for. None of them names a version, whichever version the
/// schema holds.
const NAMES_NOTHING: [(&str, &str); 14] = [
    ("+1", "a leading plus, which `u64::from_str` accepts"),
    ("+10", "a leading plus on a multi-digit number"),
    ("-1", "a leading minus"),
    ("01", "one leading zero"),
    ("0010", "leading zeros on a multi-digit number"),
    (" 1", "a leading space"),
    ("1 ", "a trailing space"),
    ("\t1", "a leading tab"),
    ("1\n", "a trailing newline"),
    ("1.0", "a decimal point"),
    ("1e0", "an exponent"),
    ("0x1", "a radix prefix"),
    ("\u{ff11}", "a full-width digit rather than an ASCII one"),
    (
        "18446744073709551616",
        "a number wider than a version counter can reach",
    ),
];

/// The version labels the seeded schema DOES hold, picked to span the first version, a
/// middle one and the newest — whose rendering is two digits wide.
const NAMES_A_VERSION: [&str; 3] = ["1", "5", "10"];

/// One operation of this service that names a version of a schema: the rows the rule is
/// asserted over.
#[derive(Debug, Clone, Copy)]
enum VersionedCall {
    /// `DescribeSchema`: the version in the `schemaVersion` query member.
    DescribeSchema,
    /// `PutCodeBinding`: the version in the `schemaVersion` query member.
    PutCodeBinding,
    /// `DescribeCodeBinding`: the version in the `schemaVersion` query member.
    DescribeCodeBinding,
    /// `DeleteSchemaVersion`: the version in the URI path, and the one call that
    /// REMOVES what the label resolves to.
    DeleteSchemaVersion,
}

impl VersionedCall {
    /// Every row of the table.
    fn all() -> [Self; 4] {
        [
            Self::DescribeSchema,
            Self::PutCodeBinding,
            Self::DescribeCodeBinding,
            Self::DeleteSchemaVersion,
        ]
    }

    /// The operation name, for the case label every assertion carries.
    fn case(self) -> &'static str {
        match self {
            Self::DescribeSchema => "DescribeSchema",
            Self::PutCodeBinding => "PutCodeBinding",
            Self::DescribeCodeBinding => "DescribeCodeBinding",
            Self::DeleteSchemaVersion => "DeleteSchemaVersion",
        }
    }

    /// The status this call answers once the version its label names is there.
    fn served_status(self) -> u16 {
        match self {
            Self::DescribeSchema | Self::DescribeCodeBinding => 200,
            // The put ACCEPTS the generation; it does not finish it.
            Self::PutCodeBinding => 202,
            Self::DeleteSchemaVersion => 204,
        }
    }

    /// Send this call with `label` as its `SchemaVersion`, encoded so the handler reads
    /// exactly that spelling.
    async fn send(self, emulator: &Emulator, label: &str) -> Answer {
        // Encoded through the crate's one label encoder: a `+` in a query value is
        // form-decoded to a space and a raw space would not survive a path segment, so
        // a row that means to send `"+1"` has to spell it `%2B1`.
        let encoded = percent_encode(label);
        let schema = schema_uri(REGISTRY, SCHEMA);
        match self {
            Self::DescribeSchema => {
                emulator
                    .call("GET", &format!("{schema}?schemaVersion={encoded}"), None)
                    .await
            }
            Self::PutCodeBinding => {
                emulator
                    .call(
                        "POST",
                        &format!("{schema}/language/{LANGUAGE}?schemaVersion={encoded}"),
                        None,
                    )
                    .await
            }
            Self::DescribeCodeBinding => {
                emulator
                    .call(
                        "GET",
                        &format!("{schema}/language/{LANGUAGE}?schemaVersion={encoded}"),
                        None,
                    )
                    .await
            }
            Self::DeleteSchemaVersion => {
                emulator
                    .call("DELETE", &version_uri(REGISTRY, SCHEMA, &encoded), None)
                    .await
            }
        }
    }
}

/// The document the `n`th version of the seeded schema is written with: an OpenAPI 3
/// document with component schemas, titled per version so each version's content is its
/// own.
fn content_of(version: u32) -> String {
    json!({
        "openapi": "3.0.0",
        "info": {"title": format!("version-{version}"), "version": "1.0.0"},
        "paths": {},
        "components": {"schemas": {"Event": {"type": "object"}}},
    })
    .to_string()
}

/// Seed [`REGISTRY`] with one schema holding [`VERSIONS`] versions, each with its own
/// content; answers the version labels the service minted, in order.
async fn seed(emulator: &Emulator) -> Vec<String> {
    create_registry(emulator, REGISTRY).await;
    let created = emulator
        .call(
            "POST",
            &schema_uri(REGISTRY, SCHEMA),
            Some(json!({"Content": content_of(1), "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "seeding the schema must succeed, got {} {:?}",
        created.status, created.body
    );
    let mut minted = vec![created.str_at("SchemaVersion")];
    for round in 2..=VERSIONS {
        let updated = emulator
            .call(
                "PUT",
                &schema_uri(REGISTRY, SCHEMA),
                Some(json!({"Content": content_of(round), "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "seeding version {round} must succeed, got {} {:?}",
            updated.status, updated.body
        );
        minted.push(updated.str_at("SchemaVersion"));
    }
    minted
}

/// The service's one sentence for a version the named schema does not hold, worded with
/// the schema the request named and the label it sent.
fn no_such_version_sentence(label: &str) -> String {
    format!("Schema with name {SCHEMA} and version {label} does not exist.")
}

/// Assert one answer is the 404 `NotFoundException` the version step answers for
/// `label`.
fn assert_version_not_found(case: &str, label: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 404,
        "case {case}: expected 404 for the label {label:?}, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type(),
        "NotFoundException",
        "case {case}: expected the NotFoundException shape, got {:?}",
        answer.header("x-amzn-errortype")
    );
    assert_eq!(
        answer.error_code(),
        "NotFoundException",
        "case {case}: expected the NotFoundException wire code, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Code"),
        CODE_RESOURCE_NOT_FOUND,
        "case {case}: expected the {CODE_RESOURCE_NOT_FOUND} condition code, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Message"),
        no_such_version_sentence(label),
        "case {case}: wrong sentence for a version the schema does not hold, got {:?}",
        answer.body
    );
}

/// The labels the seeded schema holds, as `ListSchemaVersions` reports them.
fn seeded_labels() -> BTreeSet<String> {
    (1..=VERSIONS).map(|version| version.to_string()).collect()
}

/// RULE: a `SchemaVersion` label names a stored version only when it is that version's
/// own decimal rendering, so a different spelling of the same value — a sign, zero
/// padding, surrounding space, a decimal point or exponent, a radix prefix, a non-ASCII
/// digit, a word, a number too wide to be a version — names no version, is refused as
/// not found and changes nothing. Asserted over every operation that names a version.
#[tokio::test]
async fn a_label_that_is_not_a_versions_rendering_names_no_version() {
    for call in VersionedCall::all() {
        let emulator = Emulator::new();
        let minted: BTreeSet<String> = seed(&emulator).await.into_iter().collect();
        assert_eq!(
            minted,
            seeded_labels(),
            "case {}: seeding must mint {VERSIONS} versions rendered in decimal, got {minted:?}",
            call.case()
        );

        for (label, why) in NAMES_NOTHING {
            let case = format!("{}/{label:?} ({why})", call.case());
            let answer = call.send(&emulator, label).await;
            assert_version_not_found(&case, label, &answer);
            // The refusal left the store alone: every seeded version is still there,
            // which is what a delete resolving the label would have broken.
            let held = stored_versions(&emulator, REGISTRY, SCHEMA).await;
            assert_eq!(
                held, minted,
                "case {case}: a refused call must leave the schema's versions alone, got \
                 {held:?}"
            );
        }
    }
}

/// RULE (the other side of it): a label that IS a version's own decimal rendering still
/// names that version, so every operation above is SERVED for it — the reading is
/// strict, not refusing. Asserted for the first version, a middle one and the newest,
/// whose rendering is two digits wide.
#[tokio::test]
async fn a_versions_own_decimal_rendering_still_names_it() {
    for call in VersionedCall::all() {
        for label in NAMES_A_VERSION {
            let emulator = Emulator::new();
            let minted: BTreeSet<String> = seed(&emulator).await.into_iter().collect();
            let case = format!("{}/{label:?}", call.case());
            if matches!(call, VersionedCall::DescribeCodeBinding) {
                // A binding is reported only once it has been asked for, and the put is
                // itself one of the rows: it resolves the same label.
                let started = VersionedCall::PutCodeBinding.send(&emulator, label).await;
                assert_eq!(
                    started.status,
                    VersionedCall::PutCodeBinding.served_status(),
                    "case {case}: seeding a code binding must succeed, got {} {:?}",
                    started.status,
                    started.body
                );
            }
            let answer = call.send(&emulator, label).await;
            assert_eq!(
                answer.status,
                call.served_status(),
                "case {case}: expected {}, got {} {:?}",
                call.served_status(),
                answer.status,
                answer.body
            );

            // The delete is the one row that changes the store: exactly the version its
            // label named is gone, and every other one it was seeded with remains.
            if matches!(call, VersionedCall::DeleteSchemaVersion) {
                let held = stored_versions(&emulator, REGISTRY, SCHEMA).await;
                let expected: BTreeSet<String> = minted
                    .iter()
                    .filter(|held| held.as_str() != label)
                    .cloned()
                    .collect();
                assert_eq!(
                    held, expected,
                    "case {case}: the delete must take exactly the version its label \
                     named, got {held:?}"
                );
            }
        }
    }
}
