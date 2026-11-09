// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the compatibility check a registration is decided by.
//!
//! The rule under test: a `RegisterSchemaVersion` against a schema set whose recorded
//! `Compatibility` asks for a check is compared with the versions the set already holds —
//! the latest one, or every one of them under the `_ALL` modes — in the direction the mode
//! names (`BACKWARD`: a reader using the registered definition reads data written with the
//! version held; `FORWARD`: the reverse; `FULL`: both), and the version is stored with the
//! `SchemaVersionStatus` the answer gives it: `AVAILABLE` when every comparison holds,
//! `FAILURE` when one does not, because a definition that "does not meet the compatibility
//! requirements of the schema set" (`smithy_query operation=RegisterSchemaVersion`) is
//! reported through the version's own status. `NONE` asks for no comparison: "any new
//! version added will be accepted without undergoing a compatibility check"
//! (`smithy_query shape=CreateSchemaInput member=Compatibility`).
//!
//! One comparison — can a reader using schema R read data written with schema W? — is
//! decided by the rules of the `DataFormat` the definitions are written in; the rows below
//! are `AVRO`, whose rules are Avro's own schema resolution.
//!
//! The suites are TABLES whose rows differ along what the rule generalises over: which
//! `Compatibility` mode the set was created with (every value the model admits a check
//! for), how the registered definition differs from the versions held (a field added with a
//! default, one added without, a required field dropped, a field's type changed), how many
//! versions the set holds, and which of them the mode compares against. Every call goes
//! over the wire through the shared [`crate::wire`] harness.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    FIRST_SCHEMA_VERSION, MAX_COMPATIBILITY_DEFINITION_BYTES, SCHEMA_VERSION_STATUS_AVAILABLE,
    SCHEMA_VERSION_STATUS_FAILURE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{emulator, ok};

/// Shorthands for the two statuses a registration can give a version, so a table row reads
/// as the answer it expects.
const AVAILABLE: &str = SCHEMA_VERSION_STATUS_AVAILABLE;
const FAILURE: &str = SCHEMA_VERSION_STATUS_FAILURE;

/// The Avro record the chains below start from: one required field.
const BASE: &str = r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"}]}"#;

/// The base with a field ADDED that carries a default: a reader using it reads data written
/// with the base (the default fills the field in), and a reader of the base ignores the
/// field. Compatible in both directions.
const ADD_OPTIONAL: &str = concat!(
    r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"},"#,
    r#"{"name":"email","type":["null","string"],"default":null}]}"#
);

/// The base with a field ADDED that carries no default: a reader using it cannot read data
/// written with the base, while a reader of the base ignores the field. Backward
/// incompatible, forward compatible.
const ADD_REQUIRED: &str = concat!(
    r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"},"#,
    r#"{"name":"email","type":"string"}]}"#
);

/// The base's required field DROPPED, and another put in its place that carries a default: a
/// reader using it reads data written with the base (it ignores the dropped field), while a
/// reader of the base cannot read data written with it. Forward incompatible, backward
/// compatible.
const DROP_FIELD: &str =
    r#"{"type":"record","name":"User","fields":[{"name":"seq","type":"long","default":0}]}"#;

/// The base's field with its TYPE changed to one Avro promotes neither way: incompatible in
/// both directions.
const CHANGE_TYPE: &str = r#"{"type":"record","name":"User","fields":[{"name":"id","type":"int"}]}"#;

/// How a registered definition can differ from the version held, and what each difference
/// means for a reader of one reading the other's data.
const CANDIDATES: [(&str, &str); 4] = [
    ("a field added with a default", ADD_OPTIONAL),
    ("a field added without a default", ADD_REQUIRED),
    ("the required field dropped", DROP_FIELD),
    ("the field's type changed", CHANGE_TYPE),
];

/// Every `Compatibility` value the model admits beside `DISABLED` (which admits no further
/// version at all, and is covered by [`crate::schema_version_registration`]), with the
/// status it gives each of [`CANDIDATES`] against a schema set holding the base alone.
///
/// Source: `smithy_query shape=Compatibility` (NONE | DISABLED | BACKWARD | BACKWARD_ALL |
/// FORWARD | FORWARD_ALL | FULL | FULL_ALL) and the directions
/// `CreateSchemaInput$Compatibility` documents for each value. A set holding ONE version
/// cannot tell an `_ALL` mode from its latest-version twin — the second suite does that —
/// so both are listed here and must answer alike.
const MODES: [(&str, [&str; 4]); 7] = [
    ("NONE", [AVAILABLE, AVAILABLE, AVAILABLE, AVAILABLE]),
    ("BACKWARD", [AVAILABLE, FAILURE, AVAILABLE, FAILURE]),
    ("BACKWARD_ALL", [AVAILABLE, FAILURE, AVAILABLE, FAILURE]),
    ("FORWARD", [AVAILABLE, AVAILABLE, FAILURE, FAILURE]),
    ("FORWARD_ALL", [AVAILABLE, AVAILABLE, FAILURE, FAILURE]),
    ("FULL", [AVAILABLE, FAILURE, FAILURE, FAILURE]),
    ("FULL_ALL", [AVAILABLE, FAILURE, FAILURE, FAILURE]),
];

/// A string member of a response body.
fn text(case: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: {member} must be a string, got {body}"))
        .to_string()
}

/// A numeric member of a response body.
fn number(case: &str, body: &Value, member: &str) -> i64 {
    body.get(member)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case}: {member} must be a number, got {body}"))
}

/// Create the Avro schema set `schema` with `Compatibility` `mode` and `definition` as its
/// first version.
async fn create_schema(
    router: &MultiServiceRouter,
    case: &str,
    schema: &str,
    mode: &str,
    definition: &str,
) {
    let created = ok(
        router,
        "CreateSchema",
        &json!({
            "SchemaName": schema,
            "DataFormat": "AVRO",
            "Compatibility": mode,
            "SchemaDefinition": definition,
        }),
    )
    .await;
    assert_eq!(
        text(case, &created, "SchemaVersionStatus"),
        AVAILABLE,
        "case {case}: the version a create registers is never compatibility-checked, got \
         {created}"
    );
}

/// Register `definition` against `schema` and return the registration's answer.
async fn register(router: &MultiServiceRouter, schema: &str, definition: &str) -> Value {
    ok(
        router,
        "RegisterSchemaVersion",
        &json!({
            "SchemaId": { "SchemaName": schema },
            "SchemaDefinition": definition,
        }),
    )
    .await
}

/// The version `version_id` as `GetSchemaVersion` reports it.
async fn read_version(router: &MultiServiceRouter, version_id: &str) -> Value {
    ok(
        router,
        "GetSchemaVersion",
        &json!({ "SchemaVersionId": version_id }),
    )
    .await
}

// ---------------------------------------------------------------------------
// What each mode asks of a registration against the version held
// ---------------------------------------------------------------------------

/// The rule over the whole table: the status a registration reports is the one its mode's
/// comparisons give it, and the version is STORED with that status — a later read of it
/// reports the same one.
#[tokio::test]
async fn a_registration_carries_the_status_its_compatibility_check_gives_it() {
    let router = emulator();

    for (mode, expected_statuses) in MODES {
        for (row, ((difference, candidate), expected)) in
            CANDIDATES.iter().zip(expected_statuses).enumerate()
        {
            let case = format!("{mode}, {difference}");
            // One schema set per row, named after the row rather than after the difference:
            // `SchemaRegistryNameString` admits `^[a-zA-Z0-9-_$#.]+$` only.
            let schema = format!("users-{}-{row}", mode.to_lowercase());
            create_schema(&router, &case, &schema, mode, BASE).await;

            let registered = register(&router, &schema, candidate).await;
            let status = text(&case, &registered, "Status");
            assert_eq!(
                status, expected,
                "case {case}: expected the registration to report {expected}, got {registered}"
            );
            // A checked registration is stored either way: the version it minted is the one
            // after the create's, and the status it reported is the status the store holds.
            assert_eq!(
                number(&case, &registered, "VersionNumber"),
                FIRST_SCHEMA_VERSION + 1,
                "case {case}: expected the set's next version number, got {registered}"
            );
            let version_id = text(&case, &registered, "SchemaVersionId");
            let read = read_version(&router, &version_id).await;
            assert_eq!(
                text(&case, &read, "Status"),
                expected,
                "case {case}: expected the stored version to report {expected}, got {read}"
            );
            assert_eq!(
                text(&case, &read, "SchemaDefinition"),
                *candidate,
                "case {case}: the definition registered is the one read back, got {read}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Which versions the mode compares against
// ---------------------------------------------------------------------------

/// An `_ALL` mode checks "against all previous schema versions" where its twin checks
/// against the last one alone: the same chain and the same registration answer differently
/// under the two, because the definition registered is compatible with the latest version
/// held and not with the one before it.
#[tokio::test]
async fn an_all_mode_compares_against_every_version_held() {
    // A field the first version requires as a string, which the second version drops, and
    // which the registration brings back as an `int` carrying a default: a reader using it
    // reads data written with the second version (the default fills the field in) but not
    // data written with the first (Avro promotes neither type to the other).
    const BACKWARD_FIRST: &str = concat!(
        r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"},"#,
        r#"{"name":"f","type":"string"}]}"#
    );
    const BACKWARD_SECOND: &str =
        r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"}]}"#;
    const BACKWARD_CANDIDATE: &str = concat!(
        r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"},"#,
        r#"{"name":"f","type":"int","default":0}]}"#
    );
    // The mirror image for the forward direction: the first version reads the field as an
    // `int` with a default, the second drops it, and the registration writes it as a
    // string — which a reader of the second version ignores and a reader of the first
    // cannot read.
    const FORWARD_FIRST: &str = concat!(
        r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"},"#,
        r#"{"name":"f","type":"int","default":0}]}"#
    );
    const FORWARD_SECOND: &str =
        r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"}]}"#;
    const FORWARD_CANDIDATE: &str = concat!(
        r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"},"#,
        r#"{"name":"f","type":"string"}]}"#
    );

    let router = emulator();
    let rows = [
        (
            "BACKWARD",
            BACKWARD_FIRST,
            BACKWARD_SECOND,
            BACKWARD_CANDIDATE,
            AVAILABLE,
        ),
        (
            "BACKWARD_ALL",
            BACKWARD_FIRST,
            BACKWARD_SECOND,
            BACKWARD_CANDIDATE,
            FAILURE,
        ),
        (
            "FORWARD",
            FORWARD_FIRST,
            FORWARD_SECOND,
            FORWARD_CANDIDATE,
            AVAILABLE,
        ),
        (
            "FORWARD_ALL",
            FORWARD_FIRST,
            FORWARD_SECOND,
            FORWARD_CANDIDATE,
            FAILURE,
        ),
    ];

    for (mode, first, second, candidate, expected) in rows {
        let case = format!("{mode}, compatible with the latest version only");
        let schema = format!("orders-{}", mode.to_lowercase());
        create_schema(&router, &case, &schema, mode, first).await;

        let intermediate = register(&router, &schema, second).await;
        assert_eq!(
            text(&case, &intermediate, "Status"),
            AVAILABLE,
            "case {case}: the chain's second version is compatible under {mode}, got \
             {intermediate}"
        );

        let registered = register(&router, &schema, candidate).await;
        assert_eq!(
            text(&case, &registered, "Status"),
            expected,
            "case {case}: expected {expected} from the registration, got {registered}"
        );
    }
}

/// The bound on how far back an `_ALL` mode compares: one registration parses at most
/// [`MAX_COMPATIBILITY_DEFINITION_BYTES`] of stored schema text, so a chain of definitions
/// near the modelled `SchemaDefinitionString` ceiling (170000 characters) runs the budget
/// out before the oldest version, which is then not compared — where the same chain of small
/// definitions is compared end to end.
#[tokio::test]
async fn the_parse_budget_bounds_how_far_back_an_all_mode_compares() {
    /// The chain's first version: the field the registration below brings back as a type
    /// Avro promotes neither way, so this version is the only one it is incompatible with.
    const FIRST: &str = r#"{"name":"id","type":"string"},{"name":"f","type":"string"}"#;
    /// The second version drops that field, which a reader of the first can ignore.
    const SECOND: &str = r#"{"name":"id","type":"string"}"#;
    /// The registration under test.
    const CANDIDATE: &str =
        r#"{"name":"id","type":"string"},{"name":"f","type":"int","default":0}"#;
    /// How many versions the padded chain needs for its oldest to fall outside the budget:
    /// the budget is a megabyte, so five near-ceiling definitions plus the one being
    /// registered spend it.
    const CHAIN: usize = 5;

    let router = emulator();
    let rows = [
        ("small definitions, compared end to end", None, FAILURE),
        (
            "definitions near the modelled ceiling",
            Some(169_000usize),
            AVAILABLE,
        ),
    ];

    for (case, padded_to, expected) in rows {
        // An Avro `doc` is documentation, so padding with one changes no comparison — it
        // only makes each stored definition cost what a real one of that size costs.
        let definition = |fields: &str| match padded_to {
            None => format!(r#"{{"type":"record","name":"User","fields":[{fields}]}}"#),
            Some(total) => {
                let skeleton =
                    format!(r#"{{"type":"record","name":"User","doc":"","fields":[{fields}]}}"#);
                let pad = "p".repeat(total.saturating_sub(skeleton.len()));
                let padded =
                    format!(r#"{{"type":"record","name":"User","doc":"{pad}","fields":[{fields}]}}"#);
                assert!(
                    padded.len() <= 170_000,
                    "case {case}: a definition must stay within the modelled 170000 \
                     characters, got {}",
                    padded.len()
                );
                // This row must actually span the published bound: the chain's definitions,
                // the first version's and the one being registered add up past it.
                assert!(
                    padded.len() * (CHAIN + 2) > MAX_COMPATIBILITY_DEFINITION_BYTES,
                    "case {case}: expected {} definitions of {} characters to exceed the \
                     parse budget of {MAX_COMPATIBILITY_DEFINITION_BYTES}",
                    CHAIN + 2,
                    padded.len()
                );
                padded
            }
        };

        let schema = format!("ledger-{}", if padded_to.is_some() { "padded" } else { "small" });
        create_schema(&router, case, &schema, "BACKWARD_ALL", &definition(FIRST)).await;
        // Every version between the first and the registration is compatible with all of
        // them: the second drops the field, and each one after it adds a field of its own
        // carrying a default.
        let mut chain = vec![definition(SECOND)];
        for step in 1..CHAIN {
            let mut fields = String::from(r#"{"name":"id","type":"string"}"#);
            for field in 1..=step {
                fields.push_str(&format!(
                    r#",{{"name":"g{field}","type":["null","string"],"default":null}}"#
                ));
            }
            chain.push(definition(&fields));
        }
        for (step, link) in chain.iter().enumerate() {
            let registered = register(&router, &schema, link).await;
            assert_eq!(
                text(case, &registered, "Status"),
                AVAILABLE,
                "case {case}: the chain's version {} is compatible with every version \
                 before it, got {registered}",
                step + 2
            );
        }

        let registered = register(&router, &schema, &definition(CANDIDATE)).await;
        assert_eq!(
            text(case, &registered, "Status"),
            expected,
            "case {case}: expected {expected} from a registration incompatible with the \
             chain's first version only, got {registered}"
        );
    }
}

/// A version whose own check failed is no part of the schema set's evolution, so the next
/// registration is compared with the last version the set holds as AVAILABLE rather than
/// with the failed one.
#[tokio::test]
async fn a_failed_version_is_not_compared_against() {
    let router = emulator();
    let case = "BACKWARD, after a failed version";
    let schema = "payments-after-failure";
    create_schema(&router, case, schema, "BACKWARD", BASE).await;

    let failed = register(&router, schema, CHANGE_TYPE).await;
    assert_eq!(
        text(case, &failed, "Status"),
        FAILURE,
        "case {case}: the type change is backward incompatible with the base, got {failed}"
    );

    // Compatible with the base (its added field carries a default) and NOT with the failed
    // version (whose `id` is an `int` no reader of a `string` can read), so the status below
    // says which of the two it was compared with.
    let registered = register(&router, schema, ADD_OPTIONAL).await;
    assert_eq!(
        text(case, &registered, "Status"),
        AVAILABLE,
        "case {case}: expected the comparison to skip the failed version, got {registered}"
    );
    assert_eq!(
        number(case, &registered, "VersionNumber"),
        FIRST_SCHEMA_VERSION + 2,
        "case {case}: a failed version still occupies its own version number, got \
         {registered}"
    );
}
