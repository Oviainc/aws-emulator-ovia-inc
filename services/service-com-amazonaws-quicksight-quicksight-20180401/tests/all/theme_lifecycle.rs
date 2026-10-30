// SPDX-License-Identifier: CC0-1.0

//! Themes: the create and the read that reports it, over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: a theme is identified by `(AwsAccountId, ThemeId)`; `CreateTheme`
//! persists the submitted display configuration as that theme's first version
//! (`VersionNumber` 1), reports the theme's ARN, the version's ARN and the version's
//! non-terminal creation status, and `DescribeTheme` answers from that stored record,
//! reporting the terminal status once the clock owes it. So the rows below vary the
//! inputs the rule generalises over — the id's form, which parts of the configuration
//! the theme carries, the base theme, the presence of a version description, permissions
//! and tags, and the account — rather than re-running one recorded call.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// The region the test client signs for; it is the region in every ARN below.
const REGION: &str = "us-west-2";

fn theme_arn(account: &str, theme_id: &str) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:theme/{theme_id}")
}

fn theme_version_arn(account: &str, theme_id: &str, version_number: i64) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:theme/{theme_id}/version/{version_number}")
}

async fn create_theme(harness: &Harness, account: &str, theme_id: &str, body: Value) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/themes/{theme_id}"),
            Some(body),
        )
        .await
}

async fn describe_theme(
    harness: &Harness,
    account: &str,
    theme_id: &str,
    query: &str,
) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!("/accounts/{account}/themes/{theme_id}{query}"),
            None,
        )
        .await
}

/// Bring the creation transition the clock owes the theme's version `version_number`
/// forward, so a case can observe the terminal status without sleeping for the span.
fn settle_creation(harness: &Harness, account: &str, theme_id: &str, version_number: i64) {
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let mut store = state.lock();
    let record = store
        .themes
        .get_mut(&(account.to_string(), theme_id.to_string()))
        .unwrap_or_else(|| panic!("case {theme_id:?}: the created theme is not stored"));
    let version = record
        .versions
        .iter_mut()
        .find(|version| version.version_number == version_number)
        .unwrap_or_else(|| panic!("case {theme_id:?}: version {version_number} is not stored"));
    version.creation_settles_at = SystemTime::now() - Duration::from_millis(1);
}

/// One row of the create-then-read table.
struct Row {
    /// The case label every assertion names.
    case: &'static str,
    account: &'static str,
    theme_id: &'static str,
    name: &'static str,
    base_theme_id: &'static str,
    /// The `ThemeConfiguration` the create submits and the read must report back.
    configuration: Value,
    version_description: Option<&'static str>,
    permissions: Option<Value>,
    tags: Option<Value>,
}

/// The rows: each varies what the rule generalises over, not just the resource's name.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "data-colors-only",
            account: ACCOUNT,
            theme_id: "sales-theme",
            name: "Sales",
            base_theme_id: "MIDNIGHT",
            configuration: json!({
                "DataColorPalette": {
                    "Colors": ["#37BFF5", "#8E6E9B"],
                    "MinMaxGradient": ["#37BFF5", "#8E6E9B"],
                    "EmptyFillColor": "#CCCCCC",
                }
            }),
            version_description: None,
            permissions: None,
            tags: None,
        },
        Row {
            case: "ui-palette-and-description",
            account: ACCOUNT,
            theme_id: "theme_with_underscores",
            name: "Finance review",
            base_theme_id: "SEASIDE",
            configuration: json!({
                "UIColorPalette": {
                    "PrimaryForeground": "#333333",
                    "PrimaryBackground": "#FFFFFF",
                    "Accent": "#1A8CFF",
                }
            }),
            version_description: Some("the first cut"),
            permissions: None,
            tags: None,
        },
        Row {
            case: "sheet-typography-permissions-and-tags",
            account: ACCOUNT,
            theme_id: "Theme-3",
            name: "Operations",
            base_theme_id: "CLASSIC",
            configuration: json!({
                "Sheet": {
                    "Tile": { "BackgroundColor": "#FFFFFF", "BorderRadius": "8px", "Padding": "4px" },
                    "TileLayout": { "Gutter": { "Show": true } },
                },
                "Typography": { "FontFamilies": [{ "FontFamily": "Amazon Ember" }] },
            }),
            version_description: Some("ops colours"),
            permissions: Some(json!([{
                "Principal": "arn:aws:quicksight:us-west-2:000000000000:user/default/analyst",
                "Actions": ["quicksight:DescribeTheme"],
            }])),
            tags: Some(json!([{ "Key": "team", "Value": "ops" }])),
        },
        Row {
            // The configuration is a structure with no required member, so a theme
            // that overrides nothing of its base theme is a well-formed one; and the
            // shortest id the model admits is one character.
            case: "empty-configuration-other-account",
            account: OTHER_ACCOUNT,
            theme_id: "t",
            name: "Inherited",
            base_theme_id: "RAINIER",
            configuration: json!({}),
            version_description: None,
            permissions: None,
            tags: None,
        },
    ]
}

impl Row {
    /// The request body this row submits.
    fn body(&self) -> Value {
        let mut body = json!({
            "Name": self.name,
            "BaseThemeId": self.base_theme_id,
            "Configuration": self.configuration,
        });
        if let Some(description) = self.version_description {
            body["VersionDescription"] = json!(description);
        }
        if let Some(permissions) = &self.permissions {
            body["Permissions"] = permissions.clone();
        }
        if let Some(tags) = &self.tags {
            body["Tags"] = tags.clone();
        }
        body
    }
}

#[tokio::test]
async fn create_theme_persists_version_one_and_describe_reports_it() {
    let harness = Harness::new();

    for row in rows() {
        let case = row.case;
        let (status, body) = create_theme(&harness, row.account, row.theme_id, row.body()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create expected 200, got {status} with body {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(theme_arn(row.account, row.theme_id)),
            "case {case:?}: create reported the wrong Arn: {body}"
        );
        assert_eq!(
            body["VersionArn"],
            json!(theme_version_arn(row.account, row.theme_id, 1)),
            "case {case:?}: create reported the wrong VersionArn: {body}"
        );
        assert_eq!(
            body["ThemeId"],
            json!(row.theme_id),
            "case {case:?}: create reported the wrong ThemeId: {body}"
        );
        assert_eq!(
            body["CreationStatus"],
            json!("CREATION_IN_PROGRESS"),
            "case {case:?}: a version whose creation is still in flight reports \
             CREATION_IN_PROGRESS: {body}"
        );
        let request_id = body["RequestId"].as_str().unwrap_or_default().to_string();
        assert!(
            !request_id.is_empty(),
            "case {case:?}: create reported no RequestId: {body}"
        );

        // The read answers from the stored record, while the creation is still in
        // flight: the status the create reported is the one a caller acting straight
        // after it sees.
        let (status, body) = describe_theme(&harness, row.account, row.theme_id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with body {body}"
        );
        let theme = &body["Theme"];
        assert_eq!(
            theme["Arn"],
            json!(theme_arn(row.account, row.theme_id)),
            "case {case:?}: describe reported the wrong Arn: {body}"
        );
        assert_eq!(
            theme["ThemeId"],
            json!(row.theme_id),
            "case {case:?}: describe reported the wrong ThemeId: {body}"
        );
        assert_eq!(
            theme["Name"],
            json!(row.name),
            "case {case:?}: describe lost the submitted Name: {body}"
        );
        assert_eq!(
            theme["Type"],
            json!("CUSTOM"),
            "case {case:?}: a theme a caller created is CUSTOM: {body}"
        );
        let version = &theme["Version"];
        assert_eq!(
            version["VersionNumber"],
            json!(1),
            "case {case:?}: a create mints version 1: {body}"
        );
        assert_eq!(
            version["Arn"],
            json!(theme_version_arn(row.account, row.theme_id, 1)),
            "case {case:?}: describe reported the wrong version Arn: {body}"
        );
        assert_eq!(
            version["BaseThemeId"],
            json!(row.base_theme_id),
            "case {case:?}: describe lost the submitted BaseThemeId: {body}"
        );
        assert_eq!(
            version["Configuration"],
            row.configuration,
            "case {case:?}: describe must report the submitted Configuration: {body}"
        );
        assert_eq!(
            version["Description"],
            match row.version_description {
                Some(description) => json!(description),
                None => Value::Null,
            },
            "case {case:?}: describe reported the wrong version Description: {body}"
        );
        assert_eq!(
            version["Status"],
            json!("CREATION_IN_PROGRESS"),
            "case {case:?}: before the transition is due the version is still being \
             created: {body}"
        );
        assert!(
            theme["CreatedTime"].is_number(),
            "case {case:?}: describe reported no CreatedTime: {body}"
        );

        // What the create submitted beside the version is stored too, so the reads
        // that report it have something to report.
        {
            let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
            let store = state.lock();
            let record = store
                .themes
                .get(&(row.account.to_string(), row.theme_id.to_string()))
                .unwrap_or_else(|| panic!("case {case:?}: the created theme is not stored"));
            assert_eq!(
                record.permissions.len(),
                row.permissions
                    .as_ref()
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len),
                "case {case:?}: the submitted Permissions are not stored as given"
            );
            assert_eq!(
                record.tags.len(),
                row.tags.as_ref().and_then(Value::as_array).map_or(0, Vec::len),
                "case {case:?}: the submitted Tags are not stored as given"
            );
            let version = record
                .latest_version()
                .unwrap_or_else(|| panic!("case {case:?}: the theme holds no version"));
            // The transition the clock owes is materialised by time, not by a sleep,
            // and its span is never zero.
            let created_at = version.creation_settles_at - qs_state::THEME_CREATION_SPAN;
            assert_eq!(
                version.status_at(created_at),
                "CREATION_IN_PROGRESS",
                "case {case:?}: at the creating instant the version is not yet created"
            );
            assert_eq!(
                version.status_at(version.creation_settles_at),
                "CREATION_SUCCESSFUL",
                "case {case:?}: once due, the version reports CREATION_SUCCESSFUL"
            );
            assert!(
                version.creation_settles_at > created_at,
                "case {case:?}: the transition span must not be zero"
            );
        }

        // Once the clock owes the transition, every read reports the terminal status.
        settle_creation(&harness, row.account, row.theme_id, 1);
        let (status, body) = describe_theme(&harness, row.account, row.theme_id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe after the span expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Theme"]["Version"]["Status"],
            json!("CREATION_SUCCESSFUL"),
            "case {case:?}: a settled version reports CREATION_SUCCESSFUL: {body}"
        );

        // A second create under the same identity is a conflict, whatever it submits.
        let (status, body) = create_theme(&harness, row.account, row.theme_id, row.body()).await;
        assert_eq!(
            status, 409,
            "case {case:?}: re-creating the same theme expected 409, got {status} with {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!("The theme with ID {} already exists.", row.theme_id)),
            "case {case:?}: the conflict must name the theme: {body}"
        );
    }
}

#[tokio::test]
async fn theme_identity_is_the_account_and_the_theme_id() {
    let harness = Harness::new();
    let shared_id = "shared-theme";

    for (case, account, name) in [
        ("first-account", ACCOUNT, "Owned by the first account"),
        ("second-account", OTHER_ACCOUNT, "Owned by the second account"),
    ] {
        let (status, body) = create_theme(
            &harness,
            account,
            shared_id,
            json!({
                "Name": name,
                "BaseThemeId": "MIDNIGHT",
                "Configuration": { "DataColorPalette": { "Colors": ["#37BFF5"] } },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: one theme id per account, so this create expected 200, got \
             {status} with {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(theme_arn(account, shared_id)),
            "case {case:?}: the ARN must carry the owning account: {body}"
        );
    }

    // Each account's read reports its own theme, not the other account's.
    for (case, account, name) in [
        ("first-account", ACCOUNT, "Owned by the first account"),
        ("second-account", OTHER_ACCOUNT, "Owned by the second account"),
    ] {
        let (status, body) = describe_theme(&harness, account, shared_id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Theme"]["Name"],
            json!(name),
            "case {case:?}: describe must report that account's own theme: {body}"
        );
    }

    // A third account that created nothing holds no theme under that id.
    let (status, body) = describe_theme(&harness, "123456789012", shared_id, "").await;
    assert_eq!(
        status, 404,
        "an account that created no theme has none to describe, got {status} with {body}"
    );
    assert_eq!(
        body["Message"],
        json!(format!("Theme with ID {shared_id} not found.")),
        "the refusal must name the theme that was not found: {body}"
    );
}

#[tokio::test]
async fn describe_theme_selects_the_version_the_request_names() {
    let harness = Harness::new();
    let theme_id = "versioned-theme";
    let (status, body) = create_theme(
        &harness,
        ACCOUNT,
        theme_id,
        json!({
            "Name": "Versioned",
            "BaseThemeId": "MIDNIGHT",
            "Configuration": { "UIColorPalette": { "Accent": "#1A8CFF" } },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture create expected 200, got {status} with {body}"
    );

    // `VersionNumber` is a range 1.. member, so 1 is the lowest version a request can
    // name and the only one this theme has; the next number past it names none.
    // Source: smithy_query(query="operation", operation="DescribeTheme").
    for (case, query, expected_status) in [
        ("no-selector", "", 200u16),
        ("version-1", "?version-number=1", 200),
        ("version-2", "?version-number=2", 404),
        ("version-99", "?version-number=99", 404),
        ("alias-latest", "?alias-name=%24LATEST", 200),
        ("alias-published", "?alias-name=%24PUBLISHED", 400),
        ("alias-unminted", "?alias-name=prod", 404),
    ] {
        let (status, body) = describe_theme(&harness, ACCOUNT, theme_id, query).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {body}"
        );
        if status == 200 {
            assert_eq!(
                body["Theme"]["Version"]["VersionNumber"],
                json!(1),
                "case {case:?}: the theme's only version is 1: {body}"
            );
        } else {
            assert!(
                body["Message"].as_str().is_some_and(|message| !message.is_empty()),
                "case {case:?}: a refusal must carry a message: {body}"
            );
        }
    }
}
