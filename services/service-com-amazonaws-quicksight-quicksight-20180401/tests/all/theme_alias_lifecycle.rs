// SPDX-License-Identifier: CC0-1.0

//! Theme aliases: the create that mints one and the read that reports it, over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: a theme alias is a NAME for one version a theme already holds,
//! identified by `(AwsAccountId, ThemeId, AliasName)`. `CreateThemeAlias` stores that
//! name against that version and reports it; a version the theme's history does not
//! hold, a name already taken, and a name reserved by the service are each refused and
//! persist nothing; and every later read resolves the name to the version it points at.
//! So the rows below vary what the rule generalises over — the alias name's form and
//! length, which version of the history it points at, how far past the history a
//! refused version number sits, and the account — rather than re-running one recorded
//! call.

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// The region the test client signs for; it is the region in every ARN below.
const REGION: &str = "us-west-2";

fn theme_alias_arn(account: &str, theme_id: &str, alias_name: &str) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:theme/{theme_id}/alias/{alias_name}")
}

fn theme_version_arn(account: &str, theme_id: &str, version_number: i64) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:theme/{theme_id}/version/{version_number}")
}

fn theme_arn(account: &str, theme_id: &str) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:theme/{theme_id}")
}

async fn create_theme(harness: &Harness, account: &str, theme_id: &str) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/themes/{theme_id}"),
            Some(json!({
                "Name": "Aliased",
                "BaseThemeId": "MIDNIGHT",
                "Configuration": { "UIColorPalette": { "Accent": "#1A8CFF" } },
            })),
        )
        .await
}

async fn create_theme_alias(
    harness: &Harness,
    account: &str,
    theme_id: &str,
    alias_name: &str,
    version_number: i64,
) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/themes/{theme_id}/aliases/{alias_name}"),
            Some(json!({ "ThemeVersionNumber": version_number })),
        )
        .await
}

async fn describe_theme_alias(
    harness: &Harness,
    account: &str,
    theme_id: &str,
    alias_name: &str,
) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!("/accounts/{account}/themes/{theme_id}/aliases/{alias_name}"),
            None,
        )
        .await
}

async fn describe_theme_by_alias(
    harness: &Harness,
    account: &str,
    theme_id: &str,
    alias_name: &str,
) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!("/accounts/{account}/themes/{theme_id}?alias-name={alias_name}"),
            None,
        )
        .await
}

/// Append version `version_number` to a stored theme's history, so a case can point an
/// alias at a version other than the one the create minted. `UpdateTheme` — the
/// operation that mints further versions over the wire — is a separate stub, so the
/// history is extended through the store the handlers share.
fn add_theme_version(harness: &Harness, account: &str, theme_id: &str, version_number: i64) {
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let mut store = state.lock();
    let record = store
        .themes
        .get_mut(&(account.to_string(), theme_id.to_string()))
        .unwrap_or_else(|| panic!("case {theme_id:?}: the created theme is not stored"));
    let latest = record
        .latest_version()
        .unwrap_or_else(|| panic!("case {theme_id:?}: the theme holds no version"))
        .clone();
    record.versions.push(qs_state::ThemeVersionRecord {
        version_number,
        arn: qs_state::theme_version_arn(REGION, account, theme_id, version_number),
        ..latest
    });
}

/// A 2048-character alias name: the longest the model's `AliasName` admits.
/// Source: smithy_query(query="operation", operation="CreateThemeAlias").
fn longest_alias_name() -> String {
    "a".repeat(2048)
}

#[tokio::test]
async fn create_theme_alias_names_a_version_and_the_reads_resolve_it() {
    let harness = Harness::new();
    let theme_id = "aliased-theme";
    let (status, body) = create_theme(&harness, ACCOUNT, theme_id).await;
    assert_eq!(
        status, 200,
        "the fixture create expected 200, got {status} with {body}"
    );
    // A history of three versions, so an alias can point at the first, the last and
    // one in between.
    add_theme_version(&harness, ACCOUNT, theme_id, 2);
    add_theme_version(&harness, ACCOUNT, theme_id, 3);

    let longest = longest_alias_name();
    // (case, alias name, the version of the history the alias names)
    let rows: Vec<(&str, String, i64)> = vec![
        ("word-name-first-version", "prod".to_string(), 1),
        ("hyphenated-name-middle-version", "pre-prod".to_string(), 2),
        (
            "underscored-name-latest-version",
            "release_candidate".to_string(),
            3,
        ),
        // The shortest and the longest name the model's length constraint admits.
        ("single-character-name", "p".to_string(), 1),
        ("longest-name", longest.clone(), 3),
        // A digits-only name is still a `[\w\-]+` name, and it must not be read as a
        // version number.
        ("digits-name", "2024".to_string(), 1),
    ];

    for (case, alias_name, version_number) in &rows {
        let (status, body) =
            create_theme_alias(&harness, ACCOUNT, theme_id, alias_name, *version_number).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create expected 200, got {status} with body {body}"
        );
        let alias = &body["ThemeAlias"];
        assert_eq!(
            alias["AliasName"],
            json!(alias_name),
            "case {case:?}: create reported the wrong AliasName: {body}"
        );
        assert_eq!(
            alias["ThemeVersionNumber"],
            json!(version_number),
            "case {case:?}: create reported the wrong ThemeVersionNumber: {body}"
        );
        assert_eq!(
            alias["Arn"],
            json!(theme_alias_arn(ACCOUNT, theme_id, alias_name)),
            "case {case:?}: create reported the wrong Arn: {body}"
        );
        assert!(
            body["RequestId"]
                .as_str()
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {case:?}: create reported no RequestId: {body}"
        );
    }

    // Every alias is readable afterwards, each still naming its own version: the
    // creates did not overwrite one another.
    for (case, alias_name, version_number) in &rows {
        let (status, body) = describe_theme_alias(&harness, ACCOUNT, theme_id, alias_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with body {body}"
        );
        let alias = &body["ThemeAlias"];
        assert_eq!(
            alias["AliasName"],
            json!(alias_name),
            "case {case:?}: describe reported the wrong AliasName: {body}"
        );
        assert_eq!(
            alias["ThemeVersionNumber"],
            json!(version_number),
            "case {case:?}: describe must report the version the create named: {body}"
        );
        assert_eq!(
            alias["Arn"],
            json!(theme_alias_arn(ACCOUNT, theme_id, alias_name)),
            "case {case:?}: describe reported the wrong Arn: {body}"
        );

        // The theme read resolves the same name to the same version.
        let (status, body) = describe_theme_by_alias(&harness, ACCOUNT, theme_id, alias_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describing the theme by its alias expected 200, got {status} \
             with body {body}"
        );
        assert_eq!(
            body["Theme"]["Version"]["VersionNumber"],
            json!(version_number),
            "case {case:?}: the alias must select the version it points at: {body}"
        );
    }
}

#[tokio::test]
async fn create_theme_alias_refuses_a_version_the_theme_does_not_hold() {
    let harness = Harness::new();
    let theme_id = "two-version-theme";
    let (status, body) = create_theme(&harness, ACCOUNT, theme_id).await;
    assert_eq!(
        status, 200,
        "the fixture create expected 200, got {status} with {body}"
    );
    add_theme_version(&harness, ACCOUNT, theme_id, 2);

    // The history holds versions 1 and 2; `VersionNumber` is a range 1.. member, so
    // these are the numbers a request can name that this theme does not hold — the one
    // just past the history, and numbers far past it.
    // Source: smithy_query(query="operation", operation="CreateThemeAlias").
    for (case, version_number) in [
        ("one-past-the-history", 3_i64),
        ("far-past-the-history", 99),
        ("past-a-32-bit-version", 2_147_483_648),
        ("largest-version", i64::MAX),
    ] {
        let alias_name = format!("alias-{case}");
        let (status, body) =
            create_theme_alias(&harness, ACCOUNT, theme_id, &alias_name, version_number).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an alias for a version the theme does not hold expected 404, \
             got {status} with body {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!(
                "Theme {} is not found",
                theme_version_arn(ACCOUNT, theme_id, version_number)
            )),
            "case {case:?}: the refusal must name the version that was not found: {body}"
        );

        // A refusal persists nothing: the name is still free afterwards.
        let (status, body) = describe_theme_alias(&harness, ACCOUNT, theme_id, &alias_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the refused alias must not have been stored, got {status} with \
             body {body}"
        );
        let (status, body) = create_theme_alias(&harness, ACCOUNT, theme_id, &alias_name, 2).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the refused name is still free for a version that exists, got \
             {status} with body {body}"
        );
        assert_eq!(
            body["ThemeAlias"]["ThemeVersionNumber"],
            json!(2),
            "case {case:?}: the alias must name the version this create gave it: {body}"
        );
    }
}

#[tokio::test]
async fn create_theme_alias_refuses_a_reserved_name() {
    let harness = Harness::new();
    let theme_id = "reserved-name-theme";
    let (status, body) = create_theme(&harness, ACCOUNT, theme_id).await;
    assert_eq!(
        status, 200,
        "the fixture create expected 200, got {status} with {body}"
    );

    // `CreateThemeAlias` documents that an alias name can't begin with a `$`: the
    // service's own keywords are the names that do.
    // Source: smithy_query(query="doc", shape="CreateThemeAliasRequest",
    // member="AliasName").
    for (case, alias_name, encoded) in [
        ("latest-keyword", "$LATEST", "%24LATEST"),
        ("published-keyword", "$PUBLISHED", "%24PUBLISHED"),
    ] {
        let (status, body) = create_theme_alias(&harness, ACCOUNT, theme_id, encoded, 1).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a reserved alias name expected 400, got {status} with body {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!("The alias name {alias_name} can't begin with $.")),
            "case {case:?}: the refusal must name the rejected alias name: {body}"
        );
        let (status, body) = describe_theme_alias(&harness, ACCOUNT, theme_id, encoded).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused name must not have been stored, got {status} with \
             body {body}"
        );
    }
}

#[tokio::test]
async fn create_theme_alias_refuses_past_the_per_theme_ceiling() {
    let harness = Harness::new();
    let theme_id = "crowded-theme";
    let (status, body) = create_theme(&harness, ACCOUNT, theme_id).await;
    assert_eq!(
        status, 200,
        "the fixture create expected 200, got {status} with {body}"
    );

    // Fill the theme to one short of the ceiling. The names are the caller's to choose,
    // so this is the growth the ceiling bounds; the fill goes through the store because
    // the point of the case is the last two creates, not the first thousand.
    let ceiling = qs_state::MAX_THEME_ALIASES_PER_THEME;
    {
        let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
        let mut store = state.lock();
        let record = store
            .themes
            .get_mut(&(ACCOUNT.to_string(), theme_id.to_string()))
            .expect("the created theme is stored");
        for index in 0..ceiling - 1 {
            let alias_name = format!("filler-{index}");
            record.aliases.insert(
                alias_name.clone(),
                qs_state::ThemeAliasRecord {
                    arn: theme_alias_arn(ACCOUNT, theme_id, &alias_name),
                    alias_name,
                    theme_version_number: 1,
                },
            );
        }
    }

    // The one that reaches the ceiling is still stored.
    let (status, body) = create_theme_alias(&harness, ACCOUNT, theme_id, "at-the-ceiling", 1).await;
    assert_eq!(
        status, 200,
        "the alias that reaches the ceiling expected 200, got {status} with {body}"
    );

    // The next one is refused, and nothing is stored for it.
    let (status, body) = create_theme_alias(&harness, ACCOUNT, theme_id, "one-too-many", 1).await;
    assert_eq!(
        status, 409,
        "an alias past the ceiling expected 409, got {status} with {body}"
    );
    assert_eq!(
        body["Message"],
        json!(format!(
            "The theme with ID {theme_id} already holds the maximum of {ceiling} aliases."
        )),
        "the refusal must name the theme and the ceiling: {body}"
    );
    let (status, body) = describe_theme_alias(&harness, ACCOUNT, theme_id, "one-too-many").await;
    assert_eq!(
        status, 404,
        "the refused alias must not have been stored, got {status} with {body}"
    );
}

#[tokio::test]
async fn theme_alias_identity_is_the_account_the_theme_and_the_name() {
    let harness = Harness::new();
    let theme_id = "shared-alias-theme";
    let other_theme_id = "other-alias-theme";
    let alias_name = "prod";

    // A theme that was never created holds no alias to mint.
    let (status, body) = create_theme_alias(&harness, ACCOUNT, theme_id, alias_name, 1).await;
    assert_eq!(
        status, 404,
        "an alias on a theme that does not exist expected 404, got {status} with {body}"
    );
    assert_eq!(
        body["Message"],
        json!(format!("Theme {} is not found", theme_arn(ACCOUNT, theme_id))),
        "the refusal must name the theme that was not found: {body}"
    );

    // The same theme id under two accounts, and a second theme under one of them: each
    // carries its own alias of the same name, pointing at a version of its own.
    for (case, account, theme) in [
        ("first-account", ACCOUNT, theme_id),
        ("second-account", OTHER_ACCOUNT, theme_id),
        ("second-theme", ACCOUNT, other_theme_id),
    ] {
        let (status, body) = create_theme(&harness, account, theme).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture create expected 200, got {status} with {body}"
        );
        let (status, body) = create_theme_alias(&harness, account, theme, alias_name, 1).await;
        assert_eq!(
            status, 200,
            "case {case:?}: each theme carries its own alias namespace, so this create \
             expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["ThemeAlias"]["Arn"],
            json!(theme_alias_arn(account, theme, alias_name)),
            "case {case:?}: the ARN must carry the owning account and theme: {body}"
        );
    }

    // A name already taken on one theme is a conflict on that theme, whatever version
    // the second create names.
    add_theme_version(&harness, ACCOUNT, theme_id, 2);
    for (case, version_number) in [("same-version", 1_i64), ("other-version", 2)] {
        let (status, body) =
            create_theme_alias(&harness, ACCOUNT, theme_id, alias_name, version_number).await;
        assert_eq!(
            status, 409,
            "case {case:?}: re-minting the same alias expected 409, got {status} with {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!("Alias {alias_name} already exists")),
            "case {case:?}: the conflict must name the alias that already exists: {body}"
        );
    }

    // The conflict changed nothing: the alias still names the version it was created
    // with.
    let (status, body) = describe_theme_alias(&harness, ACCOUNT, theme_id, alias_name).await;
    assert_eq!(
        status, 200,
        "describe after the conflict expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["ThemeAlias"]["ThemeVersionNumber"],
        json!(1),
        "a refused create must not have repointed the stored alias: {body}"
    );

    // An account that minted nothing has no alias to read, and neither has a theme of
    // the same account that was never given one: each refusal names the resource that
    // is missing — the theme, or the alias.
    for (case, account, theme, name, expected_message) in [
        (
            "third-account",
            "123456789012",
            theme_id,
            alias_name,
            format!("Theme {} is not found", theme_arn("123456789012", theme_id)),
        ),
        (
            "unminted-name",
            ACCOUNT,
            theme_id,
            "staging",
            "Alias staging is not found".to_string(),
        ),
    ] {
        let (status, body) = describe_theme_alias(&harness, account, theme, name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            body["Message"],
            json!(expected_message),
            "case {case:?}: expected the message {expected_message:?}: {body}"
        );
    }
}
