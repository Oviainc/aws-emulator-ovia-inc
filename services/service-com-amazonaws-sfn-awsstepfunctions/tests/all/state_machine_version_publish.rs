// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PublishStateMachineVersion`, driven over this service's own
//! wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rules under test:
//!
//! 1. a publish takes an immutable, numbered SNAPSHOT of the state machine's current
//!    configuration: the version carries the configuration the state machine held at
//!    that instant under its own qualified ARN (`<stateMachineArn>:<version>`), the
//!    numbers rise monotonically from 1, and a later update of the state machine
//!    changes nothing a published version reports;
//! 2. publishing is idempotent per REVISION: a repeat for a revision that already has
//!    a version answers that version — same ARN, same creation date — and stores
//!    nothing new, while a revision that has none takes the next number;
//! 3. `revisionId` PINS the revision to publish: the state machine's current revision
//!    (`INITIAL` while it has none) publishes, and any other value is
//!    `ConflictException` (HTTP 409) that stores nothing and reports the revision the
//!    request pinned and then the one the state machine holds;
//! 4. the ARN is judged before the lookup, and a state machine that is absent or
//!    being deleted is the operation's own modelled refusal — never a published
//!    version.
//!
//! The harness (`Emulator`, `create_machine`, `string`, `error_code`) is the one
//! `state_machine_lifecycle` builds; `tests/all.rs` is this binary's crate root, so
//! it is reached through `crate::state_machine_lifecycle`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::INITIAL_REVISION_ID;

use crate::state_machine_lifecycle::{
    create_machine, error_code, string, Emulator, ACCOUNT, HELLO_DEFINITION,
    PAST_THE_DELETE_DELAY, REGION, ROLE_ARN,
};

/// A second definition, so an update before a publish changes the snapshot.
const GOODBYE_DEFINITION: &str =
    r#"{"StartAt":"Bye","States":{"Bye":{"Type":"Pass","Result":{"msg":"bye"},"End":true}}}"#;

/// A third definition, for a row that publishes three distinct snapshots.
const AGAIN_DEFINITION: &str =
    r#"{"StartAt":"Again","States":{"Again":{"Type":"Pass","Result":{"msg":"again"},"End":true}}}"#;

/// `PublishStateMachineVersion` on `arn` with the members `extra` carries merged in.
async fn publish_call(emulator: &Emulator, arn: &str, extra: Value) -> (u16, Value) {
    let Value::Object(extra) = extra else {
        panic!("expected an object of members, got {extra:?}")
    };
    let mut request = json!({ "stateMachineArn": arn });
    for (member, value) in extra {
        request[member] = value;
    }
    emulator
        .call("PublishStateMachineVersion", &request)
        .await
}

/// A publish that must succeed, and the body it answered.
async fn publish(emulator: &Emulator, arn: &str, extra: Value) -> Value {
    let (status, body) = publish_call(emulator, arn, extra).await;
    assert_eq!(
        status, 200,
        "expected a publish of {arn} to succeed, got {status} with body {body:?}"
    );
    body
}

/// The numeric member `name` of `body`, which must be present.
fn number(body: &Value, name: &str) -> f64 {
    body.get(name)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a number '{name}' in {body:?}"))
}

/// `DescribeStateMachine` on `arn`, which must succeed.
async fn describe(emulator: &Emulator, arn: &str) -> Value {
    emulator
        .ok("DescribeStateMachine", &json!({ "stateMachineArn": arn }))
        .await
}

/// Update a state machine's definition WITHOUT publishing, and return the fresh
/// revision ID the update reports.
async fn update_definition(emulator: &Emulator, arn: &str, definition: &str) -> String {
    let updated = emulator
        .ok(
            "UpdateStateMachine",
            &json!({ "stateMachineArn": arn, "definition": definition }),
        )
        .await;
    string(&updated, "revisionId")
}

/// Every version ARN `ListStateMachineVersions` reports for `arn`, in page order.
async fn listed_versions(emulator: &Emulator, arn: &str) -> Vec<String> {
    let body = emulator
        .ok(
            "ListStateMachineVersions",
            &json!({ "stateMachineArn": arn, "maxResults": 1000 }),
        )
        .await;
    body.get("stateMachineVersions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a stateMachineVersions list in {body:?}"))
        .iter()
        .map(|item| string(item, "stateMachineVersionArn"))
        .collect()
}

// ---------------------------------------------------------------------------
// A publish is a numbered snapshot of the current configuration
// ---------------------------------------------------------------------------

/// A publish mints the next version of the state machine and that version reports the
/// configuration the state machine held at the publish — over a table that varies the
/// NUMBER of snapshots and the definition each one carries, which is what the rule
/// generalises over.
#[tokio::test]
async fn a_publish_snapshots_the_current_configuration_under_the_next_number() {
    let emulator = Emulator::new();

    // Rows differ in how many distinct revisions are published, and therefore in how
    // many versions the state machine ends up holding.
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("one snapshot", vec![HELLO_DEFINITION]),
        ("two snapshots", vec![HELLO_DEFINITION, GOODBYE_DEFINITION]),
        (
            "three snapshots",
            vec![HELLO_DEFINITION, GOODBYE_DEFINITION, AGAIN_DEFINITION],
        ),
        (
            "a snapshot of a definition already published before",
            vec![HELLO_DEFINITION, GOODBYE_DEFINITION, HELLO_DEFINITION],
        ),
    ];

    for (index, (case, definitions)) in cases.into_iter().enumerate() {
        let name = format!("snapshot-{index}");
        let arn = create_machine(&emulator, &name, definitions[0]).await;

        let mut published: Vec<(String, String)> = Vec::new();
        for (step, definition) in definitions.iter().enumerate() {
            // Every snapshot after the first follows an update, so each one is taken
            // from a distinct revision of the state machine.
            if step > 0 {
                update_definition(&emulator, &arn, definition).await;
            }
            let body = publish(&emulator, &arn, json!({})).await;
            let version_arn = string(&body, "stateMachineVersionArn");
            let expected = format!("{arn}:{}", step + 1);
            assert_eq!(
                version_arn, expected,
                "case {case:?}: version {} must be named {expected}, got {version_arn:?}",
                step + 1
            );
            assert!(
                number(&body, "creationDate") > 0.0,
                "case {case:?}: the publish must report when the version was created, got {body:?}"
            );

            // Round trip: the version is readable through the implemented reader and
            // reports the configuration the state machine held at the publish.
            let described = describe(&emulator, &version_arn).await;
            assert_eq!(
                string(&described, "definition"),
                *definition,
                "case {case:?}: {version_arn} must snapshot the definition it was published \
                 from, got {described:?}"
            );
            assert_eq!(
                string(&described, "stateMachineArn"),
                version_arn,
                "case {case:?}: the version must report its own ARN, got {described:?}"
            );
            assert_eq!(
                string(&described, "name"),
                name,
                "case {case:?}: the version must report the state machine's name, got \
                 {described:?}"
            );
            assert_eq!(
                string(&described, "roleArn"),
                ROLE_ARN,
                "case {case:?}: the version must snapshot the role, got {described:?}"
            );
            assert_eq!(
                number(&described, "creationDate"),
                number(&body, "creationDate"),
                "case {case:?}: the version's creation date must be the one the publish \
                 reported, got {described:?}"
            );
            published.push((version_arn, (*definition).to_string()));
        }

        // The listing names exactly the versions that were published, newest first.
        let listed = listed_versions(&emulator, &arn).await;
        let expected: Vec<String> = published
            .iter()
            .rev()
            .map(|(version_arn, _)| version_arn.clone())
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: the listing must name every published version, got {listed:?}"
        );

        // A version is IMMUTABLE: updating the state machine afterwards changes
        // nothing any published version reports.
        update_definition(&emulator, &arn, AGAIN_DEFINITION).await;
        for (version_arn, definition) in &published {
            let described = describe(&emulator, version_arn).await;
            assert_eq!(
                string(&described, "definition"),
                *definition,
                "case {case:?}: {version_arn} must still report its own snapshot after a later \
                 update, got {described:?}"
            );
        }
    }
}

/// The version carries the `description` the publish gave it, over the span of the
/// modelled `VersionDescription` length (0..256) — and no description at all leaves
/// the member absent.
#[tokio::test]
async fn the_version_carries_the_description_the_publish_gave_it() {
    let emulator = Emulator::new();

    // Rows span the modelled length bound (0..256, from
    // `smithy_query(query="shape", shape="VersionDescription")`): absent, the empty
    // string at the lower bound, ordinary text, text outside ASCII, and exactly the
    // upper bound.
    let max_length = "d".repeat(256);
    let cases: Vec<(&str, Option<&str>)> = vec![
        ("no description", None),
        ("the empty description", Some("")),
        ("ordinary text", Some("the first cut")),
        ("text outside ASCII", Some("プロダクション — v1 ✅")),
        ("the longest description", Some(max_length.as_str())),
    ];

    for (index, (case, description)) in cases.iter().enumerate() {
        let arn = create_machine(&emulator, &format!("described-{index}"), HELLO_DEFINITION).await;
        let request = match description {
            Some(description) => json!({ "description": description }),
            None => json!({}),
        };
        let body = publish(&emulator, &arn, request).await;
        let version_arn = string(&body, "stateMachineVersionArn");

        let described = describe(&emulator, &version_arn).await;
        let reported = described.get("description").and_then(Value::as_str);
        assert_eq!(
            reported, *description,
            "case {case:?}: expected the description {description:?}, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Idempotence per revision
// ---------------------------------------------------------------------------

/// A repeat for a revision that already has a version answers THAT version and stores
/// nothing new; a revision that has none takes the next number. The table varies how
/// the first version came to exist, how many repeats follow, and what the repeat
/// carries.
#[tokio::test]
async fn publishing_is_idempotent_for_one_revision() {
    let emulator = Emulator::new();

    // Rows vary along where the existing version came from — a publish, a create that
    // published, an update that published — since the idempotency check reads the
    // store rather than which call wrote it.
    enum First {
        Publish,
        CreateWithPublish,
        UpdateWithPublish,
    }
    let cases: Vec<(&str, First, usize)> = vec![
        ("a publish, repeated once", First::Publish, 1),
        ("a publish, repeated four times", First::Publish, 4),
        (
            "a create that published, then a publish",
            First::CreateWithPublish,
            2,
        ),
        (
            "an update that published, then a publish",
            First::UpdateWithPublish,
            3,
        ),
    ];

    for (index, (case, first, repeats)) in cases.into_iter().enumerate() {
        let name = format!("idempotent-{index}");
        // `pin` is how THIS state machine's current revision is named: the sentinel
        // while no update has moved it, the update's own revision ID once one has.
        let (arn, expected_arn, expected_date, pin) = match first {
            First::Publish => {
                let arn = create_machine(&emulator, &name, HELLO_DEFINITION).await;
                let body = publish(&emulator, &arn, json!({})).await;
                (
                    arn,
                    string(&body, "stateMachineVersionArn"),
                    number(&body, "creationDate"),
                    INITIAL_REVISION_ID.to_string(),
                )
            }
            First::CreateWithPublish => {
                let created = emulator
                    .ok(
                        "CreateStateMachine",
                        &json!({
                            "name": name,
                            "definition": HELLO_DEFINITION,
                            "roleArn": ROLE_ARN,
                            "publish": true,
                        }),
                    )
                    .await;
                let arn = string(&created, "stateMachineArn");
                let version_arn = string(&created, "stateMachineVersionArn");
                let date = number(&describe(&emulator, &version_arn).await, "creationDate");
                (arn, version_arn, date, INITIAL_REVISION_ID.to_string())
            }
            First::UpdateWithPublish => {
                let arn = create_machine(&emulator, &name, HELLO_DEFINITION).await;
                let updated = emulator
                    .ok(
                        "UpdateStateMachine",
                        &json!({
                            "stateMachineArn": &arn,
                            "definition": GOODBYE_DEFINITION,
                            "publish": true,
                        }),
                    )
                    .await;
                let version_arn = string(&updated, "stateMachineVersionArn");
                let date = number(&describe(&emulator, &version_arn).await, "creationDate");
                let revision = string(&updated, "revisionId");
                (arn, version_arn, date, revision)
            }
        };

        for round in 0..repeats {
            // The repeats differ in what they carry: a bare repeat, one that carries a
            // different description, and one that pins the current revision
            // explicitly — "Step Functions bases the idempotency check on the
            // stateMachineArn, name and revisionId parameters", so none of them is a
            // different request.
            let extra = match round % 3 {
                0 => json!({}),
                1 => json!({ "description": format!("round {round}") }),
                _ => json!({ "revisionId": pin }),
            };

            let body = publish(&emulator, &arn, extra.clone()).await;
            assert_eq!(
                string(&body, "stateMachineVersionArn"),
                expected_arn,
                "case {case:?} round {round} ({extra:?}): a repeat must answer the existing \
                 version, got {body:?}"
            );
            assert_eq!(
                number(&body, "creationDate"),
                expected_date,
                "case {case:?} round {round} ({extra:?}): a repeat must answer the first \
                 creation date, got {body:?}"
            );
            let listed = listed_versions(&emulator, &arn).await;
            assert_eq!(
                listed,
                vec![expected_arn.clone()],
                "case {case:?} round {round}: a repeat must store no second version, got \
                 {listed:?}"
            );
        }

        // Once the state machine moves to a NEW revision, the next publish mints the
        // next version rather than answering the old one.
        update_definition(&emulator, &arn, AGAIN_DEFINITION).await;
        let body = publish(&emulator, &arn, json!({})).await;
        let next = string(&body, "stateMachineVersionArn");
        assert_eq!(
            next,
            format!("{arn}:2"),
            "case {case:?}: a fresh revision must take the next number, got {next:?}"
        );
        let listed = listed_versions(&emulator, &arn).await;
        assert_eq!(
            listed.len(),
            2,
            "case {case:?}: the state machine must hold exactly two versions, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// revisionId pins the revision to publish
// ---------------------------------------------------------------------------

/// `revisionId` publishes only when it names the state machine's CURRENT revision —
/// `INITIAL` while it has none — and is otherwise `ConflictException` (HTTP 409) that
/// stores nothing.
#[tokio::test]
async fn revision_id_must_name_the_current_revision() {
    let emulator = Emulator::new();

    // A state machine that has never been updated carries no revision ID.
    let fresh = create_machine(&emulator, "pinned-initial", HELLO_DEFINITION).await;
    // And one an update moved on carries the revision that update reported.
    let updated = create_machine(&emulator, "pinned-revision", HELLO_DEFINITION).await;
    let revision = update_definition(&emulator, &updated, GOODBYE_DEFINITION).await;

    // Rows vary along the relationship between the pinned value and the current
    // revision: the stale sentinel, another state machine's revision, text of the
    // right shape that is no revision at all, a different case, and the empty string
    // (`RevisionId` carries no length constraint).
    let mismatched: Vec<(&str, &str, &str)> = vec![
        (
            "a never-updated machine pinned to a revision ID",
            &fresh,
            revision.as_str(),
        ),
        (
            "a never-updated machine pinned to the lowercase sentinel",
            &fresh,
            "initial",
        ),
        ("a never-updated machine pinned to text", &fresh, "no-such-revision"),
        ("a never-updated machine pinned to nothing", &fresh, ""),
        (
            "an updated machine pinned to the initial sentinel",
            &updated,
            INITIAL_REVISION_ID,
        ),
        (
            "an updated machine pinned to another revision",
            &updated,
            "8e9f0a1b-2c3d-4e5f-8a9b-0c1d2e3f4a5b",
        ),
        ("an updated machine pinned to nothing", &updated, ""),
    ];

    for (case, arn, pinned) in &mismatched {
        let (status, body) = publish_call(&emulator, arn, json!({ "revisionId": pinned })).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {status} {code} with body {body:?}"
        );
        let listed = listed_versions(&emulator, arn).await;
        assert_eq!(
            listed,
            Vec::<String>::new(),
            "case {case:?}: a refused publish must store nothing, got {listed:?}"
        );
    }

    // The matching pin publishes, for both forms of "current revision".
    for (case, arn, pinned, expected) in [
        (
            "the initial sentinel on a never-updated machine",
            &fresh,
            INITIAL_REVISION_ID.to_string(),
            format!("{fresh}:1"),
        ),
        (
            "the current revision of an updated machine",
            &updated,
            revision.clone(),
            format!("{updated}:1"),
        ),
    ] {
        let body = publish(&emulator, arn, json!({ "revisionId": pinned })).await;
        let version_arn = string(&body, "stateMachineVersionArn");
        assert_eq!(
            version_arn, expected,
            "case {case:?}: expected {expected}, got {version_arn:?}"
        );
        let described = describe(&emulator, &version_arn).await;
        let reported = described.get("revisionId").and_then(Value::as_str);
        let owed = if pinned == INITIAL_REVISION_ID {
            None
        } else {
            Some(pinned.as_str())
        };
        assert_eq!(
            reported, owed,
            "case {case:?}: the version must snapshot the revision it was published from, got \
             {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a stale pin REPORTS
// ---------------------------------------------------------------------------

/// How many updates have revised the state machine before the publish — what decides
/// which revision the refusal reports as the current one.
enum Revised {
    Never,
    Once,
    Twice,
}

/// What the mismatched pin is built from.
enum Pin {
    /// The revision an EARLIER update reported, which a later update moved on: the
    /// optimistic-concurrency case, where the caller read a revision and then lost
    /// the race.
    Stale,
    /// The current revision of ANOTHER state machine — a real revision of something,
    /// but not of the state machine being published.
    OtherMachine,
    /// Literal text that names no revision of anything.
    Literal(&'static str),
}

/// A publish whose `revisionId` is not the state machine's current revision reports
/// the revision the REQUEST asked to publish and then the revision the state machine
/// holds, so a caller can re-drive the publish from the message body — over a table
/// varying both the pinned value and the current revision, which is what the rule
/// generalises over.
#[tokio::test]
async fn a_stale_revision_pin_reports_the_requested_and_the_current_revision() {
    let emulator = Emulator::new();

    // Another state machine, revised once, so a row can pin a value that IS a
    // revision — of a different state machine.
    let other = create_machine(&emulator, "another-machine", HELLO_DEFINITION).await;
    let other_revision = update_definition(&emulator, &other, GOODBYE_DEFINITION).await;

    let cases: Vec<(&str, Revised, Pin)> = vec![
        (
            "never revised, pinned to another machine's revision",
            Revised::Never,
            Pin::OtherMachine,
        ),
        (
            "never revised, pinned to the sentinel in the wrong case",
            Revised::Never,
            Pin::Literal("initial"),
        ),
        (
            "never revised, pinned to nothing",
            Revised::Never,
            Pin::Literal(""),
        ),
        (
            "revised once, pinned to the initial sentinel",
            Revised::Once,
            Pin::Literal(INITIAL_REVISION_ID),
        ),
        (
            "revised once, pinned to a UUID that is no revision",
            Revised::Once,
            Pin::Literal("8e9f0a1b-2c3d-4e5f-8a9b-0c1d2e3f4a5b"),
        ),
        (
            "revised twice, pinned to the revision the first update reported",
            Revised::Twice,
            Pin::Stale,
        ),
        (
            "revised twice, pinned to another machine's revision",
            Revised::Twice,
            Pin::OtherMachine,
        ),
        (
            "revised twice, pinned to text carrying the refusal's own words",
            Revised::Twice,
            Pin::Literal("The current State Machine revision is"),
        ),
    ];

    for (index, (case, revised, pin)) in cases.into_iter().enumerate() {
        let name = format!("stale-pin-{index}");
        let arn = create_machine(&emulator, &name, HELLO_DEFINITION).await;

        let updates = match revised {
            Revised::Never => 0,
            Revised::Once => 1,
            Revised::Twice => 2,
        };
        let mut revisions: Vec<String> = Vec::new();
        for definition in [GOODBYE_DEFINITION, AGAIN_DEFINITION].iter().take(updates) {
            revisions.push(update_definition(&emulator, &arn, definition).await);
        }
        // What `DescribeStateMachine` reports as the current revision, which is
        // nothing at all while no update has revised the state machine.
        let current = revisions.last().cloned();
        assert_eq!(
            describe(&emulator, &arn)
                .await
                .get("revisionId")
                .and_then(Value::as_str),
            current.as_deref(),
            "case {case:?}: expected the read to report the last update's revision"
        );

        let pinned = match pin {
            Pin::Stale => revisions
                .first()
                .cloned()
                .expect("a stale pin needs an earlier revision"),
            Pin::OtherMachine => other_revision.clone(),
            Pin::Literal(text) => text.to_string(),
        };

        let (status, code, message) = crate::error_messages::failure(
            &emulator,
            case,
            "PublishStateMachineVersion",
            &json!({ "stateMachineArn": &arn, "revisionId": &pinned }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {status} {code} with message \
             {message:?}"
        );

        let reported_current = current
            .clone()
            .unwrap_or_else(|| INITIAL_REVISION_ID.to_string());
        let expected = format!(
            "Failed to publish the State Machine version for revision {pinned}. The current \
             State Machine revision is {reported_current}."
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(&arn) && !message.contains(&name),
            "case {case:?}: the refusal reports the two revisions, not the state machine it is \
             about, got {message:?}"
        );
        assert_eq!(
            listed_versions(&emulator, &arn).await,
            Vec::<String>::new(),
            "case {case:?}: a refused publish must store nothing"
        );

        // The revision the refusal reported as the current one is the one that
        // publishes — what a caller re-driving the publish out of the message body
        // does with it.
        let body = publish(&emulator, &arn, json!({ "revisionId": reported_current })).await;
        let version_arn = string(&body, "stateMachineVersionArn");
        assert_eq!(
            version_arn,
            format!("{arn}:1"),
            "case {case:?}: expected the re-driven publish to mint {arn}:1, got {version_arn:?}"
        );
        assert_eq!(
            listed_versions(&emulator, &arn).await,
            vec![version_arn.clone()],
            "case {case:?}: the re-driven version must be the one the listing reports, got \
             {version_arn:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What cannot be published
// ---------------------------------------------------------------------------

/// A state machine that is absent, or that a started deletion already moved to
/// `DELETING`, is the operation's own modelled refusal — and nothing is stored.
#[tokio::test]
async fn an_absent_or_deleting_state_machine_cannot_publish() {
    let emulator = Emulator::new();

    let never = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");
    let deleting = create_machine(&emulator, "deleting-now", HELLO_DEFINITION).await;
    emulator
        .ok(
            "DeleteStateMachine",
            &json!({ "stateMachineArn": &deleting }),
        )
        .await;

    // A deletion is owed to the clock, so the record is still there, in `DELETING`,
    // for a caller acting straight afterwards.
    for (case, arn, expected) in [
        (
            "a state machine that was never created",
            never.clone(),
            "StateMachineDoesNotExist",
        ),
        (
            "a state machine whose deletion has started",
            deleting.clone(),
            "StateMachineDeleting",
        ),
    ] {
        let (status, body) = publish_call(&emulator, &arn, json!({})).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected 400 {expected}, got {status} {code} with body {body:?}"
        );
        assert_eq!(
            listed_versions(&emulator, &arn).await,
            Vec::<String>::new(),
            "case {case:?}: a refused publish must store nothing"
        );
    }

    // Once the clock has removed it, the same call reports a state machine that is
    // gone rather than one that is being deleted.
    std::thread::sleep(PAST_THE_DELETE_DELAY);
    let (status, body) = publish_call(&emulator, &deleting, json!({})).await;
    let code = error_code(&body);
    assert_eq!(
        (status, code.as_str()),
        (400, "StateMachineDoesNotExist"),
        "a removed state machine is absent, got {status} {code} with body {body:?}"
    );
}

/// The ARN is judged BEFORE the lookup: text that is not a well-formed state machine
/// ARN of this service in the endpoint's region is `InvalidArn`, and a QUALIFIED ARN —
/// a Distributed Map label, a version or an alias — is `ValidationException`, the
/// labeled one carrying the modelled `API_DOES_NOT_SUPPORT_LABELED_ARNS` reason.
#[tokio::test]
async fn a_malformed_or_qualified_arn_is_refused_before_the_lookup() {
    let emulator = Emulator::new();
    let arn = create_machine(&emulator, "arn-rule", HELLO_DEFINITION).await;
    publish(&emulator, &arn, json!({})).await;

    // Rows vary along WHICH component of the ARN is wrong, including an ARN of
    // another region — the check is against the ENDPOINT's region.
    let malformed: Vec<(&str, String)> = vec![
        ("text that is not an ARN", "not-a-valid-arn".to_string()),
        ("a bare state machine name", "arn-rule".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:arn-rule"),
        ),
        (
            "an execution ARN",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:arn-rule:run-1"),
        ),
        (
            "an activity ARN",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:an-activity"),
        ),
        (
            "a state machine ARN with no name",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:"),
        ),
        (
            "an ARN of another region",
            format!("arn:aws:states:eu-west-1:{ACCOUNT}:stateMachine:arn-rule"),
        ),
    ];
    for (case, value) in &malformed {
        let (status, body) = publish_call(&emulator, value, json!({})).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidArn"),
            "case {case:?}: expected 400 InvalidArn, got {status} {code} with body {body:?}"
        );
    }

    // (case, qualified ARN, the modelled reason the refusal must carry).
    let mut qualified: Vec<(String, String, Option<&str>)> = Vec::new();
    for label in ["mapStateLabel", "Map", "inner-map_1"] {
        qualified.push((
            format!("a labeled ARN '{arn}/{label}'"),
            format!("{arn}/{label}"),
            Some("API_DOES_NOT_SUPPORT_LABELED_ARNS"),
        ));
    }
    for qualifier in ["1", "2", "PROD", "live-alias"] {
        qualified.push((
            format!("a qualified ARN '{arn}:{qualifier}'"),
            format!("{arn}:{qualifier}"),
            None,
        ));
    }
    for (case, value, reason) in &qualified {
        let (status, body) = publish_call(&emulator, value, json!({})).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code} with body \
             {body:?}"
        );
        assert_eq!(
            body.get("reason").and_then(Value::as_str),
            *reason,
            "case {case:?}: expected reason {reason:?}, got {body:?}"
        );
    }

    // None of those refusals disturbed the one version that does exist.
    let listed = listed_versions(&emulator, &arn).await;
    assert_eq!(
        listed,
        vec![format!("{arn}:1")],
        "the existing versions must be unchanged, got {listed:?}"
    );
}
