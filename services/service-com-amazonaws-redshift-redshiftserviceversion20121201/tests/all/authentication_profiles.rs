// SPDX-License-Identifier: CC0-1.0

//! The authentication profile lifecycle, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request: a created profile is reported and stored exactly as the request
//! configured it, whatever the shape of its name and the shape of its JSON
//! content; the NAME being what an account's profiles are unique on, so a second
//! create under one name is refused and overwrites nothing; a name that
//! identifies no profile being refused by the read, the modify and the delete; a
//! modify REPLACING the content while leaving the profile where it was; a delete
//! making the profile absent from every later read, leaving the records around it
//! where they were and freeing its name; the modelled name syntax; and content
//! outside the documented JSON form, or larger than this service stores under one
//! profile, being refused with nothing written.
//!
//! Tables rather than a generated corpus, for the reason the crate's
//! `Cargo.toml` records: a property-based generator would add an entry to the
//! workspace `Cargo.lock`, which the squash merge cannot resolve against a
//! mainline that rewrites it.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// One row of the create table: everything a `CreateAuthenticationProfile`
/// request carries.
struct ProfileCase {
    case: &'static str,
    name: String,
    content: String,
}

/// `CreateAuthenticationProfile` with one name and one content.
async fn create(emulator: &Emulator, name: &str, content: &str) -> Answer {
    emulator
        .call(
            "CreateAuthenticationProfile",
            &[
                ("AuthenticationProfileName", name),
                ("AuthenticationProfileContent", content),
            ],
        )
        .await
}

/// `ModifyAuthenticationProfile` with one name and one content.
async fn modify(emulator: &Emulator, name: &str, content: &str) -> Answer {
    emulator
        .call(
            "ModifyAuthenticationProfile",
            &[
                ("AuthenticationProfileName", name),
                ("AuthenticationProfileContent", content),
            ],
        )
        .await
}

/// `DescribeAuthenticationProfiles` for one name.
async fn describe_one(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "DescribeAuthenticationProfiles",
            &[("AuthenticationProfileName", name)],
        )
        .await
}

/// `DescribeAuthenticationProfiles` naming no profile: every profile the account
/// owns.
async fn describe_all(emulator: &Emulator) -> Answer {
    emulator.call("DescribeAuthenticationProfiles", &[]).await
}

/// `DeleteAuthenticationProfile` for one name.
async fn delete_one(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "DeleteAuthenticationProfile",
            &[("AuthenticationProfileName", name)],
        )
        .await
}

/// The `<AuthenticationProfileName>` of every profile a body carries, in the
/// order it lists them.
fn listed_names(body: &str) -> Vec<String> {
    all_text(body, "AuthenticationProfileName")
}

/// The `<AuthenticationProfileContent>` of every profile a body carries, in
/// document order.
fn listed_contents(body: &str) -> Vec<String> {
    all_text(body, "AuthenticationProfileContent")
}

/// The content the store holds for one profile, read straight from this
/// service's state rather than from a response.
fn stored_content(emulator: &Emulator, name: &str) -> Option<String> {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let store = store.lock();
    store
        .authentication_profile(name)
        .and_then(|profile| profile.authentication_profile_content.clone())
}

/// The name of every profile the store holds, in the order it holds them.
fn stored_names(emulator: &Emulator) -> Vec<String> {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let store = store.lock();
    store
        .authentication_profiles
        .iter()
        .filter_map(|profile| profile.authentication_profile_name.clone())
        .collect()
}

/// Assert one refusal row: the answer's status and `<Code>`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected the error code {code}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// A name of exactly `len` characters, for the modelled length bound.
fn name_of_length(len: usize) -> String {
    std::iter::repeat_n('p', len).collect()
}

/// A JSON document whose single value is `filler` characters long, for a content
/// far larger than any other row.
fn large_content(filler: usize) -> String {
    format!(
        r#"{{"setting":"{}"}}"#,
        std::iter::repeat_n('x', filler).collect::<String>()
    )
}

/// A JSON document of exactly `len` characters, for the service's content size
/// bound. `len` must leave room for the wrapper [`large_content`] puts around its
/// filler.
fn content_of_length(len: usize) -> String {
    let wrapper = large_content(0).chars().count();
    let content = large_content(len - wrapper);
    assert_eq!(
        content.chars().count(),
        len,
        "the test's own content builder must produce exactly {len} characters"
    );
    content
}

/// The create table: rows differing along everything the rule generalises over —
/// the shape of the name (letters, mixed case, digits, hyphens, the modelled
/// length bound) and the shape of the profile map the content carries (one
/// setting, several, a nested document, a value that is an array, text needing
/// XML escaping, a large document).
fn create_table() -> Vec<ProfileCase> {
    vec![
        ProfileCase {
            case: "a map carrying a single setting",
            name: "profile".to_string(),
            content: r#"{"App_ID":"example"}"#.to_string(),
        },
        ProfileCase {
            case: "one setting, a mixed-case hyphenated name",
            name: "Analytics-Profile-01".to_string(),
            content: r#"{"AllowDBUserOverride":"1"}"#.to_string(),
        },
        ProfileCase {
            case: "a nested document",
            name: "nested".to_string(),
            content: r#"{"AuthProfile":{"Region":"eu-central-1","Ports":[5439,5440]}}"#.to_string(),
        },
        ProfileCase {
            case: "a setting whose value is an array",
            name: "arrayform".to_string(),
            content: r#"{"Keys":[{"Key":"a"},{"Key":"b"}]}"#.to_string(),
        },
        ProfileCase {
            case: "content whose text needs XML escaping",
            name: "escaped".to_string(),
            content: r#"{"Filter":"a<b & c>d 'q' \"r\""}"#.to_string(),
        },
        ProfileCase {
            case: "a name of digits and hyphens only",
            name: "9-9-9".to_string(),
            content: r#"{"Retries":9}"#.to_string(),
        },
        ProfileCase {
            case: "a name at the modelled length bound (63 characters)",
            name: name_of_length(63),
            content: r#"{"Bound":"63"}"#.to_string(),
        },
        ProfileCase {
            case: "a large document",
            name: "large".to_string(),
            content: large_content(4_000),
        },
    ]
}

/// A create reports and stores exactly what the request configured, and every
/// later read reports that record back: the name as given and the content
/// verbatim.
#[tokio::test]
async fn a_created_profile_is_reported_and_stored_exactly_as_the_request_configured_it() {
    let emulator = Emulator::new();
    let table = create_table();

    for row in &table {
        let answer = create(&emulator, &row.name, &row.content).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected status 200, got {} with body {}",
            row.case, answer.status, answer.body
        );
        assert_eq!(
            answer.text("AuthenticationProfileName").as_deref(),
            Some(row.name.as_str()),
            "case {}: expected the created profile to report the name {:?}, got {:?}",
            row.case,
            row.name,
            answer.text("AuthenticationProfileName")
        );
        assert_eq!(
            answer.text("AuthenticationProfileContent").as_deref(),
            Some(row.content.as_str()),
            "case {}: expected the created profile to report its content verbatim, got {:?}",
            row.case,
            answer.text("AuthenticationProfileContent")
        );
        assert_eq!(
            stored_content(&emulator, &row.name).as_deref(),
            Some(row.content.as_str()),
            "case {}: expected the store to hold the content verbatim, got {:?}",
            row.case,
            stored_content(&emulator, &row.name)
        );

        // The round trip: the create is only done when an implemented read
        // reports the same record back.
        let read = describe_one(&emulator, &row.name).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected the read to answer 200, got {} with body {}",
            row.case, read.status, read.body
        );
        assert_eq!(
            listed_names(&read.body),
            vec![row.name.clone()],
            "case {}: expected the read to report exactly the named profile, got {:?}",
            row.case,
            listed_names(&read.body)
        );
        assert_eq!(
            listed_contents(&read.body),
            vec![row.content.clone()],
            "case {}: expected the read to report the stored content, got {:?}",
            row.case,
            listed_contents(&read.body)
        );
    }

    // A read naming no profile reports every one the account owns, in creation
    // order.
    let all = describe_all(&emulator).await;
    let expected: Vec<String> = table.iter().map(|row| row.name.clone()).collect();
    assert_eq!(
        listed_names(&all.body),
        expected,
        "case every profile the account owns: expected the creation order {expected:?}, got {:?}",
        listed_names(&all.body)
    );
    assert_eq!(
        stored_names(&emulator),
        expected,
        "case the store's own order: expected {expected:?}, got {:?}",
        stored_names(&emulator)
    );
}

/// The NAME is what an account's profiles are unique on: a second create under
/// one name is refused and overwrites nothing, whatever content it carries.
#[tokio::test]
async fn a_second_create_under_one_name_is_refused_and_overwrites_nothing() {
    let emulator = Emulator::new();
    let first = r#"{"Original":"1"}"#;
    create(&emulator, "taken", first).await;

    for (case, content) in [
        ("the same content again", first),
        ("different content", r#"{"Original":"2"}"#),
        ("another setting entirely", r#"{"Replacement":"3"}"#),
    ] {
        let answer = create(&emulator, "taken", content).await;
        assert_refused(
            case,
            &answer,
            400,
            "AuthenticationProfileAlreadyExistsFault",
        );
        assert_eq!(
            stored_content(&emulator, "taken").as_deref(),
            Some(first),
            "case {case}: expected the refused create to overwrite nothing, got {:?}",
            stored_content(&emulator, "taken")
        );
    }

    assert_eq!(
        stored_names(&emulator),
        vec!["taken".to_string()],
        "case the store after three refused creates: expected the one profile, got {:?}",
        stored_names(&emulator)
    );
}

/// A name that identifies no profile this account owns is refused by every
/// operation that looks one up — the read, the modify and the delete — and never
/// served as an empty report. The sentence each one answers is pinned in
/// [`crate::configuration_refusal_wording`].
#[tokio::test]
async fn a_name_that_identifies_no_profile_is_refused_by_every_lookup() {
    let emulator = Emulator::new();
    create(&emulator, "present", r#"{"Present":"1"}"#).await;
    create(&emulator, "removed", r#"{"Who":"removed"}"#).await;
    delete_one(&emulator, "removed").await;

    // Rows differing along WHY the name identifies nothing: never created, one
    // whose profile was deleted, and one differing from a stored name only in
    // case (the modelled syntax admits both cases and states no folding).
    for (case, name) in [
        ("a name nothing created", "absent"),
        ("a name whose profile was deleted", "removed"),
        ("a stored name in another case", "PRESENT"),
    ] {
        for (operation, answer) in [
            ("DescribeAuthenticationProfiles", describe_one(&emulator, name).await),
            (
                "ModifyAuthenticationProfile",
                modify(&emulator, name, r#"{"Who":"nobody"}"#).await,
            ),
            (
                "DeleteAuthenticationProfile",
                delete_one(&emulator, name).await,
            ),
        ] {
            assert_refused(
                &format!("{case} through {operation}"),
                &answer,
                404,
                "AuthenticationProfileNotFoundFault",
            );
        }
    }

    assert_eq!(
        stored_names(&emulator),
        vec!["present".to_string()],
        "case the store after every refused lookup: expected the one profile, got {:?}",
        stored_names(&emulator)
    );
    assert_eq!(
        stored_content(&emulator, "present").as_deref(),
        Some(r#"{"Present":"1"}"#),
        "case the surviving profile: expected its content untouched, got {:?}",
        stored_content(&emulator, "present")
    );
}

/// A modify REPLACES the profile's content — the member is the whole of a
/// profile's configuration, not a patch on it — reports what it stored, and
/// leaves the profile where it was among the records around it.
#[tokio::test]
async fn a_modify_replaces_the_content_and_keeps_the_profile_where_it_was() {
    let emulator = Emulator::new();
    create(&emulator, "first", r#"{"Who":"first"}"#).await;
    create(&emulator, "target", r#"{"Step":"0"}"#).await;
    create(&emulator, "last", r#"{"Who":"last"}"#).await;

    // Rows differing along the shape of the replacement: fewer settings, more
    // settings, a nested document, a value that is an array and one setting
    // again.
    for (case, content) in [
        ("one setting", r#"{"Step":"1"}"#),
        ("several settings", r#"{"Step":"2","Extra":true,"Count":7}"#),
        ("a nested document", r#"{"Step":{"Phase":"3"}}"#),
        ("a setting whose value is an array", r#"{"Steps":["4"]}"#),
        ("one setting again", r#"{"Step":"5"}"#),
    ] {
        let answer = modify(&emulator, "target", content).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected status 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.text("AuthenticationProfileName").as_deref(),
            Some("target"),
            "case {case}: expected the modify to report the profile's name, got {:?}",
            answer.text("AuthenticationProfileName")
        );
        assert_eq!(
            answer.text("AuthenticationProfileContent").as_deref(),
            Some(content),
            "case {case}: expected the modify to report the new content, got {:?}",
            answer.text("AuthenticationProfileContent")
        );

        let read = describe_one(&emulator, "target").await;
        assert_eq!(
            listed_contents(&read.body),
            vec![content.to_string()],
            "case {case}: expected the read to report the replacement, got {:?}",
            listed_contents(&read.body)
        );
        assert_eq!(
            stored_names(&emulator),
            vec![
                "first".to_string(),
                "target".to_string(),
                "last".to_string()
            ],
            "case {case}: expected the modify to keep the creation order, got {:?}",
            stored_names(&emulator)
        );
        assert_eq!(
            stored_content(&emulator, "first").as_deref(),
            Some(r#"{"Who":"first"}"#),
            "case {case}: expected the profile before it to be untouched, got {:?}",
            stored_content(&emulator, "first")
        );
        assert_eq!(
            stored_content(&emulator, "last").as_deref(),
            Some(r#"{"Who":"last"}"#),
            "case {case}: expected the profile after it to be untouched, got {:?}",
            stored_content(&emulator, "last")
        );
    }
}

/// A delete reports the name it removed, makes the profile absent from every
/// later read, leaves every other record where it was, and frees the name for a
/// new profile.
#[tokio::test]
async fn a_delete_frees_the_name_and_leaves_every_other_record_where_it_was() {
    let emulator = Emulator::new();
    let names = ["alpha", "beta", "gamma", "delta"];
    for name in names {
        create(&emulator, name, &format!(r#"{{"Who":"{name}"}}"#)).await;
    }

    // Rows differing along WHERE in the collection the removed record sat: the
    // middle, the head, the tail, and the last record of all.
    let mut remaining: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    for case in ["beta", "alpha", "delta", "gamma"] {
        let answer = delete_one(&emulator, case).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected status 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.text("AuthenticationProfileName").as_deref(),
            Some(case),
            "case {case}: expected the delete to report the name it removed, got {:?}",
            answer.text("AuthenticationProfileName")
        );

        remaining.retain(|name| name != case);
        assert_eq!(
            stored_names(&emulator),
            remaining,
            "case {case}: expected the records around it to keep their order, got {:?}",
            stored_names(&emulator)
        );
        let all = describe_all(&emulator).await;
        assert_eq!(
            listed_names(&all.body),
            remaining,
            "case {case}: expected the read to report only the remaining profiles, got {:?}",
            listed_names(&all.body)
        );
        assert_refused(
            case,
            &describe_one(&emulator, case).await,
            404,
            "AuthenticationProfileNotFoundFault",
        );
    }

    // The name is free again: a create under it succeeds and is the only record
    // the account holds.
    let reborn = create(&emulator, "beta", r#"{"Who":"beta again"}"#).await;
    assert_eq!(
        reborn.status, 200,
        "case a name freed by a delete: expected status 200, got {} with body {}",
        reborn.status, reborn.body
    );
    assert_eq!(
        stored_names(&emulator),
        vec!["beta".to_string()],
        "case a name freed by a delete: expected the one new profile, got {:?}",
        stored_names(&emulator)
    );
}

/// Content outside the documented profile-map form is refused by the create and
/// by the modify, and nothing is written: the create stores no profile and the
/// modify leaves the content the profile already held.
#[tokio::test]
async fn content_outside_the_documented_json_form_is_refused_with_nothing_written() {
    let emulator = Emulator::new();
    let kept = r#"{"Kept":"1"}"#;
    create(&emulator, "existing", kept).await;

    // Rows differing along HOW the content fails to be one profile map: not
    // JSON at all, truncated, missing a value, empty, two documents,
    // single-quoted (which JSON does not admit), a document that is no map, and
    // a map carrying no setting.
    for (case, content) in [
        ("plain text", "not json at all"),
        ("a truncated object", "{"),
        ("a member with no value", r#"{"a":}"#),
        ("an empty string", ""),
        ("two documents", "{} {}"),
        ("single-quoted text", "'quoted'"),
        ("a JSON array rather than a map", r#"[{"Key":"a"}]"#),
        ("a map carrying no setting", "{}"),
    ] {
        let created = create(&emulator, "rejected", content).await;
        assert_refused(
            case,
            &created,
            400,
            "InvalidAuthenticationProfileRequestFault",
        );
        assert!(
            stored_content(&emulator, "rejected").is_none(),
            "case {case}: expected the refused create to store nothing, got {:?}",
            stored_content(&emulator, "rejected")
        );

        let modified = modify(&emulator, "existing", content).await;
        assert_refused(
            case,
            &modified,
            400,
            "InvalidAuthenticationProfileRequestFault",
        );
        assert_eq!(
            stored_content(&emulator, "existing").as_deref(),
            Some(kept),
            "case {case}: expected the refused modify to write nothing, got {:?}",
            stored_content(&emulator, "existing")
        );
    }

    assert_eq!(
        stored_names(&emulator),
        vec!["existing".to_string()],
        "case the store after every refused write: expected the one profile, got {:?}",
        stored_names(&emulator)
    );
}

/// A name outside the modelled syntax is refused before any record is written:
/// the model bounds `AuthenticationProfileName` at 63 characters and admits only
/// letters, digits and hyphens
/// (`smithy_query(query="operation", operation="CreateAuthenticationProfile")` —
/// `^[a-zA-Z0-9\-]+$`).
#[tokio::test]
async fn a_name_outside_the_modelled_syntax_is_refused_before_anything_is_written() {
    let emulator = Emulator::new();

    // Each published bound, and one past it: the longest name the model admits
    // is accepted by `a_created_profile_is_reported_and_stored_exactly_as_the_request_configured_it`,
    // so this table carries the refusals — one character past the length bound
    // and one character outside the admitted set. A modelled constraint is
    // refused at the request boundary, before any handler runs, which is why the
    // code is the query protocols' own `ValidationError` rather than a fault this
    // operation models.
    for (case, name) in [
        (
            "one character past the length bound (64 characters)",
            name_of_length(64),
        ),
        ("an underscore", "bad_name".to_string()),
        ("a space", "bad name".to_string()),
        ("a dot", "bad.name".to_string()),
    ] {
        let answer = create(&emulator, &name, r#"{"App_ID":"example"}"#).await;
        assert_refused(case, &answer, 400, "ValidationError");
        assert!(
            stored_names(&emulator).is_empty(),
            "case {case}: expected the refused create to store nothing, got {:?}",
            stored_names(&emulator)
        );
    }
}

/// Content is accepted exactly up to the size this service stores under one
/// profile and refused one character past it, by the create and by the modify
/// alike — with the fault the model declares for a profile's SIZE rather than for
/// its form.
#[tokio::test]
async fn content_is_accepted_exactly_up_to_the_size_bound() {
    let emulator = Emulator::new();
    let bound = state::MAX_AUTHENTICATION_PROFILE_CONTENT_LEN;
    let at_bound = content_of_length(bound);
    let past_bound = content_of_length(bound + 1);

    let created = create(&emulator, "atbound", &at_bound).await;
    assert_eq!(
        created.status, 200,
        "case content of exactly {bound} characters: expected status 200, got {} with body {}",
        created.status, created.body
    );
    assert_eq!(
        stored_content(&emulator, "atbound").as_deref(),
        Some(at_bound.as_str()),
        "case content of exactly {bound} characters: expected the store to hold it verbatim, got {:?} characters",
        stored_content(&emulator, "atbound").map(|held| held.chars().count())
    );

    let refused = create(&emulator, "pastbound", &past_bound).await;
    assert_refused(
        "a create one character past the size bound",
        &refused,
        400,
        "AuthenticationProfileQuotaExceededFault",
    );
    assert!(
        stored_content(&emulator, "pastbound").is_none(),
        "case a create one character past the size bound: expected nothing stored, got {:?}",
        stored_content(&emulator, "pastbound").map(|held| held.chars().count())
    );

    let over = modify(&emulator, "atbound", &past_bound).await;
    assert_refused(
        "a modify one character past the size bound",
        &over,
        400,
        "AuthenticationProfileQuotaExceededFault",
    );
    assert_eq!(
        stored_content(&emulator, "atbound").as_deref(),
        Some(at_bound.as_str()),
        "case a modify one character past the size bound: expected the content untouched, got {:?} characters",
        stored_content(&emulator, "atbound").map(|held| held.chars().count())
    );

    let replaced = content_of_length(bound);
    let modified = modify(&emulator, "atbound", &replaced).await;
    assert_eq!(
        modified.status, 200,
        "case a modify to content of exactly {bound} characters: expected status 200, got {} with body {}",
        modified.status, modified.body
    );
    assert_eq!(
        stored_content(&emulator, "atbound").as_deref(),
        Some(replaced.as_str()),
        "case a modify to content of exactly {bound} characters: expected the store to hold it verbatim, got {:?} characters",
        stored_content(&emulator, "atbound").map(|held| held.chars().count())
    );
}
