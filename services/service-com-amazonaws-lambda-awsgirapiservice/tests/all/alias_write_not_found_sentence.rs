// SPDX-License-Identifier: CC0-1.0

//! What a REFUSED write to an absent alias states, and what it leaves behind.
//!
//! The rule under test: a write refused because the alias it addresses is not there states
//! the sentence that WRITE states — the fixed [`WRITE_NOT_FOUND_PREFIX`] followed by the
//! fully-qualified alias ARN the request asked about — and changes nothing: the aliases the
//! function does hold keep their configuration AND their `RevisionId` ("A unique
//! identifier that changes when you update the alias", so a revision that moved is a write
//! that happened), and the refused name stays absent rather than being created by the call
//! that failed. The companion suite [`crate::alias_update_not_found_sentence`] spans the
//! sentence's own variables (the alias-name class, each documented `FunctionName` form);
//! this one pins the refusal's effect on the store, which a message assertion cannot see.
//!
//! The rows differ along what the rule generalises over: which member the refused update
//! stated (none, a description, a version, a routing configuration), the documented
//! `FunctionName` form it addressed the function by (bare name, full ARN, partial ARN), and
//! whether the alias it named never existed or was created and then deleted. Every row
//! runs against the same function, so the surviving alias is checked after each one.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::alias_update::{
    create_alias, create_function, delete_alias, get_alias, router_on, update_alias, Answer,
    ACCOUNT, LATEST, REGION,
};

/// The error code the model declares, at HTTP 404, for an alias these operations cannot
/// find.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The fixed lead-in of the sentence a WRITE to an absent alias is refused with, which the
/// alias ARN follows (`state::alias_update_not_found_message`).
const WRITE_NOT_FOUND_PREFIX: &str = "Alias not found: ";

/// How many versions the function publishes, so a refused update has versions to name.
const PUBLISHED_VERSIONS: usize = 2;

/// The alias that EXISTS throughout, whose configuration and revision no refusal may
/// touch.
const SURVIVOR: &str = "live";

/// The `RevisionId` and `FunctionVersion` an answer describing an alias carries.
fn configuration(case: &str, answer: &Answer) -> (String, String) {
    let revision = answer
        .string("RevisionId")
        .unwrap_or_else(|| panic!("case {case:?}: no RevisionId in {}", answer.text()));
    let version = answer
        .string("FunctionVersion")
        .unwrap_or_else(|| panic!("case {case:?}: no FunctionVersion in {}", answer.text()));
    (revision, version)
}

/// One row: a name the function holds no alias of, the form the request addresses the
/// function by, and the members the refused update states.
struct RefusedWrite {
    name: &'static str,
    /// The alias name the update addresses.
    alias: &'static str,
    /// How the request spells the function, built from its bare name.
    reference: fn(&str) -> String,
    /// The members the refused update states.
    members: Value,
    /// Whether the alias was created and then DELETED, rather than never created.
    vacated: bool,
}

/// The rows.
fn refused_writes() -> Vec<RefusedWrite> {
    vec![
        RefusedWrite {
            name: "a name that never existed, update stating nothing",
            alias: "ghost",
            reference: |name| name.to_string(),
            members: json!({}),
            vacated: false,
        },
        RefusedWrite {
            name: "a vacated name, update stating a description",
            alias: "ghost-alias",
            reference: |name| name.to_string(),
            members: json!({ "Description": "post-delete" }),
            vacated: true,
        },
        RefusedWrite {
            name: "a vacated name addressed by the function ARN, update stating a version",
            alias: "ghost_alias",
            reference: |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
            members: json!({ "FunctionVersion": "1" }),
            vacated: true,
        },
        RefusedWrite {
            name: "a name that never existed, addressed by the partial ARN, update stating a \
                   routing configuration",
            alias: "Ghost2024v2",
            reference: |name| format!("{ACCOUNT}:function:{name}"),
            members: json!({
                "FunctionVersion": "1",
                "RoutingConfig": { "AdditionalVersionWeights": { "2": 0.1 } },
            }),
            vacated: false,
        },
    ]
}

/// RULE: a write refused for an absent alias states the write's own sentence and writes
/// nothing — the alias the function holds keeps its configuration and its revision across
/// every refusal, and the name the refusal named is still absent afterwards.
#[tokio::test]
async fn a_write_refused_for_an_absent_alias_states_its_sentence_and_changes_nothing() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "refused-writes", PUBLISHED_VERSIONS).await;

    let created = create_alias(
        &router,
        "refused-writes",
        &json!({ "Name": SURVIVOR, "FunctionVersion": LATEST }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the surviving alias must be created, got {} {}",
        created.status,
        created.text()
    );
    let before = configuration("the surviving alias", &created);

    for case in refused_writes() {
        if case.vacated {
            // Created, then removed: the id is vacated rather than never used, which is
            // the lifecycle a client hits when two callers race a delete and an update.
            let made = create_alias(
                &router,
                "refused-writes",
                &json!({ "Name": case.alias, "FunctionVersion": LATEST }),
            )
            .await;
            assert_eq!(
                made.status,
                201,
                "case {:?}: the alias to vacate must be created, got {} {}",
                case.name,
                made.status,
                made.text()
            );
            let gone = delete_alias(&router, "refused-writes", case.alias).await;
            assert_eq!(
                gone.status,
                204,
                "case {:?}: the alias must be deleted, got {} {}",
                case.name,
                gone.status,
                gone.text()
            );
        }

        let reference = (case.reference)("refused-writes");
        let refused = update_alias(&router, &reference, case.alias, &case.members).await;
        assert_eq!(
            (refused.status, refused.error_code().contains(RESOURCE_NOT_FOUND)),
            (404, true),
            "case {:?}: expected 404 {RESOURCE_NOT_FOUND}, got {} {}",
            case.name,
            refused.status,
            refused.text()
        );
        let reported = refused.message();
        let expected = format!(
            "{WRITE_NOT_FOUND_PREFIX}arn:aws:lambda:{REGION}:{ACCOUNT}:function:refused-writes:{}",
            case.alias
        );
        assert_eq!(
            reported, expected,
            "case {:?}: expected {expected:?}, got {reported:?}",
            case.name
        );
        // The refused name is still absent: the failed write created nothing.
        let absent = get_alias(&router, "refused-writes", case.alias).await;
        assert_eq!(
            absent.status,
            404,
            "case {:?}: the refused name must still be absent, got {} {}",
            case.name,
            absent.status,
            absent.text()
        );

        // And the alias that exists is untouched, revision included.
        let survivor = get_alias(&router, "refused-writes", SURVIVOR).await;
        assert_eq!(
            survivor.status,
            200,
            "case {:?}: the surviving alias must still be readable, got {} {}",
            case.name,
            survivor.status,
            survivor.text()
        );
        assert_eq!(
            configuration(case.name, &survivor),
            before,
            "case {:?}: expected the surviving alias's revision and version unchanged, got {}",
            case.name,
            survivor.text()
        );
    }
}
