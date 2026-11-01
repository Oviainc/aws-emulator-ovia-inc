// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what the operations that SELECT a schema version do with a
//! `schemaVersion` query the request sent EMPTY.
//!
//! RULE under test: `SchemaVersion` is the optional member "specifying this limits the
//! results to only this schema version", so a request that sends it with no characters
//! in it limits the read to no version at all and is served exactly as the request that
//! omits the member — the latest version the schema still HOLDS answers, whatever that
//! version is, and a schema holding no version at all takes the same refusal, worded for
//! a request that named no version. Only an EMPTY value specifies nothing: a value
//! carrying characters still names a version strictly, so one the schema does not hold
//! is still refused.
//!
//! The rows vary along what the rule generalises over: the operation the member is sent
//! to (and so the HTTP method and served status), how many versions the schema holds,
//! whether the latest version the schema holds is still the highest one ever minted (a
//! delete frees no number, so a deleted newest version leaves an earlier one as the
//! latest), how wide the resolved version's rendering is, whether the schema holds any
//! version at all, and whether the registry is the caller's or the service's own.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The HTTP
//! harness is the crate's one harness, imported from `registry_schema_lifecycle`, and
//! the version URI and the version listing from `schema_version_deletion`, so nothing
//! here reads the state struct directly: what a request resolved to is read back out of
//! the service's own answers.

use std::collections::BTreeSet;

use serde_json::json;
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, AWS_MANAGED_REGISTRY, CODE_BINDING_LANGUAGES,
};

use crate::registry_schema_lifecycle::{create_registry, schema_uri, Answer, Emulator};
use crate::schema_version_deletion::{stored_versions, version_uri};

/// The registry the caller-owned rows are served against.
const REGISTRY: &str = "empty-schema-version";

/// The schema the caller-owned rows are served against.
const SCHEMA: &str = "order.placed";

/// The language the code-binding rows name. Source: `state::CODE_BINDING_LANGUAGES`, the
/// catalog of languages this service generates bindings for, read rather than written out
/// here.
fn language() -> &'static str {
    CODE_BINDING_LANGUAGES
        .first()
        .expect("the service generates bindings for at least one language")
}

/// The export format the `ExportSchema` rows ask for: the one format `ExportSchema`'s
/// required `Type` query accepts (`tests/all/export_reads_the_stored_schema.rs` owns what
/// the exported document itself must be).
const EXPORT_TYPE: &str = "JSONSchemaDraft4";

/// One operation that takes `SchemaVersion` as an optional `schemaVersion` query member.
///
/// Source: the model's four implemented operations binding
/// `SchemaVersion @httpQuery("schemaVersion")` — `DescribeSchema`, `ExportSchema`,
/// `PutCodeBinding` and `DescribeCodeBinding`.
#[derive(Debug, Clone, Copy)]
enum SelectingCall {
    DescribeSchema,
    ExportSchema,
    PutCodeBinding,
    DescribeCodeBinding,
}

impl SelectingCall {
    /// Every row of the table.
    fn all() -> [Self; 4] {
        [
            Self::DescribeSchema,
            Self::ExportSchema,
            Self::PutCodeBinding,
            Self::DescribeCodeBinding,
        ]
    }

    /// The rows that can be SERVED for a schema the caller wrote: all but the export,
    /// which serves only a schema the service itself wrote (its own reading of the
    /// member is driven over the published catalogue below, and over its version
    /// resolution in `tests/all/export_reads_the_stored_schema.rs`).
    fn caller_owned() -> [Self; 3] {
        [
            Self::DescribeSchema,
            Self::PutCodeBinding,
            Self::DescribeCodeBinding,
        ]
    }

    /// The operation name, for the case label every assertion carries.
    fn case(self) -> &'static str {
        match self {
            Self::DescribeSchema => "DescribeSchema",
            Self::ExportSchema => "ExportSchema",
            Self::PutCodeBinding => "PutCodeBinding",
            Self::DescribeCodeBinding => "DescribeCodeBinding",
        }
    }

    /// The HTTP method the operation is bound to.
    fn method(self) -> &'static str {
        match self {
            Self::PutCodeBinding => "POST",
            _ => "GET",
        }
    }

    /// The status this call answers once the version it resolves is there.
    fn served_status(self) -> u16 {
        match self {
            // The put ACCEPTS the generation; it does not finish it.
            Self::PutCodeBinding => 202,
            _ => 200,
        }
    }

    /// Whether two answers of this call made one after the other must be byte-identical:
    /// a pure read of stored content is, while a code-binding answer also reports the
    /// generation status the CLOCK moves, which no two calls need agree on.
    fn answers_are_stable(self) -> bool {
        matches!(self, Self::DescribeSchema | Self::ExportSchema)
    }

    /// This call's route, carrying the members it needs but no `schemaVersion`.
    fn uri(self, registry: &str, schema: &str) -> String {
        let schema_uri = schema_uri(registry, schema);
        match self {
            Self::DescribeSchema => schema_uri,
            Self::ExportSchema => format!("{schema_uri}/export?type={EXPORT_TYPE}"),
            Self::PutCodeBinding | Self::DescribeCodeBinding => {
                format!("{schema_uri}/language/{}", language())
            }
        }
    }

    /// Send this call with the `schemaVersion` query `sent`: absent for `None`, and
    /// present carrying exactly `value` for `Some(value)` — including the empty value,
    /// which is what a `?schemaVersion=` on the wire delivers.
    async fn send(
        self,
        emulator: &Emulator,
        registry: &str,
        schema: &str,
        sent: Option<&str>,
    ) -> Answer {
        let uri = self.uri(registry, schema);
        let uri = match sent {
            None => uri,
            Some(value) => {
                let separator = if uri.contains('?') { '&' } else { '?' };
                format!("{uri}{separator}schemaVersion={value}")
            }
        };
        emulator.call(self.method(), &uri, None).await
    }
}

/// The document the `n`th version of a seeded schema carries: an OpenAPI 3 document with
/// component schemas, so an export has something to render, titled per version so every
/// version's content is its own.
fn content_of(version: u32) -> String {
    json!({
        "openapi": "3.0.0",
        "info": {"title": format!("version-{version}"), "version": "1.0.0"},
        "paths": {},
        "components": {"schemas": {"Event": {"type": "object"}}},
    })
    .to_string()
}

/// Seed [`REGISTRY`] with one schema holding `count` versions, then delete the versions
/// `deleted` names; answers the version labels the schema still holds, in key order.
async fn seed(emulator: &Emulator, count: u32, deleted: &[u32]) -> Vec<String> {
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
    for round in 2..=count {
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
    }
    for version in deleted {
        let removed = emulator
            .call(
                "DELETE",
                &version_uri(REGISTRY, SCHEMA, &version.to_string()),
                None,
            )
            .await;
        assert_eq!(
            removed.status, 204,
            "deleting the seeded version {version} must succeed, got {} {:?}",
            removed.status, removed.body
        );
    }
    by_version_number(stored_versions(emulator, REGISTRY, SCHEMA).await)
}

/// Version labels in the order the service MINTS them: by the number each one renders,
/// so the last is the latest version — which the lexicographic order of a label set is
/// not ("9" sorts after "12").
fn by_version_number(labels: BTreeSet<String>) -> Vec<String> {
    let mut labels: Vec<String> = labels.into_iter().collect();
    labels.sort_by_key(|label| {
        label
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("a stored version label renders in decimal, got {label:?}"))
    });
    labels
}

/// The names of the members an answer carries, so two answers can be compared on their
/// SHAPE as well as on the members a case reads out of them.
fn members(answer: &Answer) -> BTreeSet<String> {
    answer
        .body
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

/// Assert `empty` — the answer to the request that sent `schemaVersion` with nothing in
/// it — is the answer the request that OMITTED the member was served.
fn assert_served_alike(case: &str, call: SelectingCall, omitted: &Answer, empty: &Answer) {
    assert_eq!(
        empty.status, omitted.status,
        "case {case}: an empty schemaVersion must be served the status an absent one is \
         ({}), got {} {:?}",
        omitted.status, empty.status, empty.body
    );
    assert_eq!(
        members(empty),
        members(omitted),
        "case {case}: an empty schemaVersion must be answered the members an absent one \
         is, got {:?}",
        empty.body
    );
    if call.answers_are_stable() {
        assert_eq!(
            empty.body, omitted.body,
            "case {case}: an empty schemaVersion must be served exactly what an absent \
             one is, got {:?}",
            empty.body
        );
    }
}

/// RULE: a `schemaVersion` sent with an empty value limits the read to no version, so
/// every operation that takes the member serves the request as the one that omits it —
/// the latest version the schema still holds answers.
#[tokio::test]
async fn an_empty_schema_version_resolves_the_latest_stored_version() {
    // Rows: how many versions the schema holds, and which of them a delete took first —
    // so the version the rule resolves to is the only one, one of several, one whose
    // rendering is two digits wide, and one that is NOT the highest number ever minted.
    let topologies: [(&str, u32, &[u32]); 5] = [
        ("one version", 1, &[]),
        ("three versions", 3, &[]),
        ("twelve versions, a two-digit latest", 12, &[]),
        ("three versions, the newest deleted", 3, &[3]),
        ("three versions, all but the first deleted", 3, &[2, 3]),
    ];

    for call in SelectingCall::caller_owned() {
        for (topology, count, deleted) in topologies {
            let case = format!("{}/{topology}", call.case());
            let emulator = Emulator::new();
            let held = seed(&emulator, count, deleted).await;
            let latest = held
                .last()
                .unwrap_or_else(|| panic!("case {case}: the seeded schema must hold a version"))
                .clone();

            // A binding is reported only once a put has started one, and it is started
            // for the version the rule must resolve to, named explicitly so this seeding
            // does not itself depend on how an empty value is read.
            if matches!(call, SelectingCall::DescribeCodeBinding) {
                let started = SelectingCall::PutCodeBinding
                    .send(&emulator, REGISTRY, SCHEMA, Some(&latest))
                    .await;
                assert_eq!(
                    started.status,
                    SelectingCall::PutCodeBinding.served_status(),
                    "case {case}: seeding a code binding must succeed, got {} {:?}",
                    started.status,
                    started.body
                );
            }

            let omitted = call.send(&emulator, REGISTRY, SCHEMA, None).await;
            assert_eq!(
                omitted.status,
                call.served_status(),
                "case {case}: expected {} with no schemaVersion member, got {} {:?}",
                call.served_status(),
                omitted.status,
                omitted.body
            );
            let empty = call.send(&emulator, REGISTRY, SCHEMA, Some("")).await;
            assert_served_alike(&case, call, &omitted, &empty);

            // And what it resolved to is the latest version the schema HOLDS, which the
            // answer itself reports.
            assert_eq!(
                empty.str_at("SchemaVersion"),
                latest,
                "case {case}: an empty schemaVersion resolves the latest stored version, \
                 got {:?}",
                empty.body
            );

            // The read left the store alone: the schema still holds exactly the versions
            // it was seeded with.
            let still_held: BTreeSet<String> = held.iter().cloned().collect();
            assert_eq!(
                stored_versions(&emulator, REGISTRY, SCHEMA).await,
                still_held,
                "case {case}: the call must leave the schema's versions alone"
            );
        }
    }
}

/// RULE (where there is nothing more to resolve): a schema always holds a definition —
/// the delete of the only version a schema holds is refused — so an empty `schemaVersion`
/// always has the schema's latest version to resolve, and the refused delete leaves that
/// version to answer with.
#[tokio::test]
async fn a_schema_always_has_a_latest_version_for_an_empty_schema_version_to_resolve() {
    // Rows: how many versions the schema held before every one a delete may take was
    // taken — the case is about the one definition a schema cannot be emptied of.
    for (topology, count) in [("one version", 1_u32), ("three versions", 3)] {
        for call in SelectingCall::caller_owned() {
            let case = format!("{}/{topology}", call.case());
            let emulator = Emulator::new();
            let deleted: Vec<u32> = (1..count).collect();
            let held = seed(&emulator, count, &deleted).await;
            assert_eq!(
                held,
                vec![count.to_string()],
                "case {case}: the schema must be left holding its newest version, got {held:?}"
            );

            // The delete of that last definition is refused, so it is still there.
            let refused = emulator
                .call(
                    "DELETE",
                    &version_uri(REGISTRY, SCHEMA, &count.to_string()),
                    None,
                )
                .await;
            assert_eq!(
                refused.status, 409,
                "case {case}: deleting the last version must be refused, got {} {:?}",
                refused.status, refused.body
            );

            if matches!(call, SelectingCall::DescribeCodeBinding) {
                let started = SelectingCall::PutCodeBinding
                    .send(&emulator, REGISTRY, SCHEMA, Some(&count.to_string()))
                    .await;
                assert_eq!(
                    started.status,
                    SelectingCall::PutCodeBinding.served_status(),
                    "case {case}: seeding a code binding must succeed, got {} {:?}",
                    started.status,
                    started.body
                );
            }

            let omitted = call.send(&emulator, REGISTRY, SCHEMA, None).await;
            let empty = call.send(&emulator, REGISTRY, SCHEMA, Some("")).await;
            assert_eq!(
                omitted.status,
                call.served_status(),
                "case {case}: expected {} with no schemaVersion member, got {} {:?}",
                call.served_status(),
                omitted.status,
                omitted.body
            );
            assert_served_alike(&case, call, &omitted, &empty);
            assert_eq!(
                empty.str_at("SchemaVersion"),
                count.to_string(),
                "case {case}: an empty schemaVersion resolves the version the schema still \
                 holds, got {:?}",
                empty.body
            );
        }
    }
}

/// RULE (the other side of it): only a value with NOTHING in it specifies no version. A
/// value that carries characters still names a version strictly, so one the schema holds
/// is resolved and one it does not hold — whitespace included, which names no version
/// rather than nothing — is still refused as not found.
#[tokio::test]
async fn a_schema_version_that_carries_characters_still_selects_strictly() {
    for call in SelectingCall::caller_owned() {
        let case_prefix = call.case();
        let emulator = Emulator::new();
        let held = seed(&emulator, 3, &[]).await;

        // A version the schema holds and which is NOT the latest: the member still limits
        // the answer to it, so the empty-value reading has not made the member a no-op.
        let selected = held
            .first()
            .unwrap_or_else(|| panic!("case {case_prefix}: the seeded schema must hold a version"))
            .clone();
        if matches!(call, SelectingCall::DescribeCodeBinding) {
            let started = SelectingCall::PutCodeBinding
                .send(&emulator, REGISTRY, SCHEMA, Some(&selected))
                .await;
            assert_eq!(
                started.status,
                SelectingCall::PutCodeBinding.served_status(),
                "case {case_prefix}: seeding a code binding must succeed, got {} {:?}",
                started.status,
                started.body
            );
        }
        let answer = call
            .send(&emulator, REGISTRY, SCHEMA, Some(&selected))
            .await;
        assert_eq!(
            answer.status,
            call.served_status(),
            "case {case_prefix}/{selected:?}: expected {}, got {} {:?}",
            call.served_status(),
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.str_at("SchemaVersion"),
            selected,
            "case {case_prefix}/{selected:?}: the member limits the answer to the version \
             it names, got {:?}",
            answer.body
        );
        assert_ne!(
            answer.str_at("SchemaVersion"),
            *held
                .last()
                .expect("the seeded schema holds a version"),
            "case {case_prefix}/{selected:?}: the named version must not be the latest, or \
             the row proves nothing"
        );

        // Values that carry characters and name no version the schema holds: a number
        // past the newest, one below the first, and values whose only content is
        // whitespace — a space, a tab and a newline, each sent percent-encoded so the
        // handler reads exactly it.
        for (label, sent) in [
            ("a number past the newest", "9"),
            ("a number below the first", "0"),
            ("a single space", "%20"),
            ("a tab", "%09"),
            ("a newline", "%0A"),
            ("spaces only", "%20%20"),
        ] {
            let case = format!("{case_prefix}/{label}");
            let refused = call.send(&emulator, REGISTRY, SCHEMA, Some(sent)).await;
            assert_eq!(
                refused.status, 404,
                "case {case}: a value that names no version is refused, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type(),
                "NotFoundException",
                "case {case}: expected the NotFoundException shape, got {:?}",
                refused.header("x-amzn-errortype")
            );
        }
    }
}

/// RULE (across registry scope): the reading is a property of the MEMBER, not of who owns
/// the registry, so an empty `schemaVersion` resolves the latest version of a schema AWS
/// publishes exactly as it does for one the caller created.
#[tokio::test]
async fn an_empty_schema_version_resolves_a_published_schemas_latest_version() {
    // Source: `state::aws_managed_schemas` — the seeded service-owned catalogue, read
    // rather than written out here; several rows, so no one published schema is pinned.
    let published: Vec<String> = aws_managed_schemas()
        .iter()
        .map(|schema| schema.name.clone())
        .take(3)
        .collect();
    assert!(
        !published.is_empty(),
        "the service-owned catalogue must publish schemas to read"
    );

    for call in SelectingCall::all() {
        for name in &published {
            let case = format!("{}/{name}", call.case());
            let emulator = Emulator::new();
            let held =
                by_version_number(stored_versions(&emulator, AWS_MANAGED_REGISTRY, name).await);
            let latest = held
                .last()
                .unwrap_or_else(|| panic!("case {case}: a published schema holds a version"))
                .clone();

            if matches!(call, SelectingCall::DescribeCodeBinding) {
                let started = SelectingCall::PutCodeBinding
                    .send(&emulator, AWS_MANAGED_REGISTRY, name, Some(&latest))
                    .await;
                assert_eq!(
                    started.status,
                    SelectingCall::PutCodeBinding.served_status(),
                    "case {case}: seeding a code binding must succeed, got {} {:?}",
                    started.status,
                    started.body
                );
            }

            let omitted = call.send(&emulator, AWS_MANAGED_REGISTRY, name, None).await;
            let empty = call
                .send(&emulator, AWS_MANAGED_REGISTRY, name, Some(""))
                .await;
            assert_eq!(
                omitted.status,
                call.served_status(),
                "case {case}: expected {} with no schemaVersion member, got {} {:?}",
                call.served_status(),
                omitted.status,
                omitted.body
            );
            assert_served_alike(&case, call, &omitted, &empty);
            assert_eq!(
                empty.str_at("SchemaVersion"),
                latest,
                "case {case}: an empty schemaVersion resolves the latest published version, \
                 got {:?}",
                empty.body
            );
        }
    }
}

/// The schema's own content is what an empty `schemaVersion` answers FROM: the bytes the
/// latest stored version was written with, read back through the two operations that
/// carry content, so a resolution that answered from the request rather than the store
/// would have to invent them.
#[tokio::test]
async fn the_content_an_empty_schema_version_answers_is_the_latest_versions_own() {
    for (topology, count, deleted) in [
        ("three versions", 3_u32, &[] as &[u32]),
        ("three versions, the newest deleted", 3, &[3]),
    ] {
        let emulator = Emulator::new();
        let held = seed(&emulator, count, deleted).await;
        let latest = held.last().expect("the seeded schema holds a version");
        let expected = content_of(
            latest
                .parse::<u32>()
                .unwrap_or_else(|_| panic!("case {topology}: a version renders in decimal")),
        );

        let described = SelectingCall::DescribeSchema
            .send(&emulator, REGISTRY, SCHEMA, Some(""))
            .await;
        assert_eq!(
            described.str_at("Content"),
            expected,
            "case {topology}: DescribeSchema answers the latest version's own bytes, got {:?}",
            described.body
        );

        // And the version the member resolved to is the one the schema still holds.
        assert_eq!(
            described.str_at("SchemaVersion"),
            *latest,
            "case {topology}: the member resolves the latest stored version, got {:?}",
            described.body
        );
    }
}
