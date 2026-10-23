// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Allowed AMIs slice: `GetAllowedImagesSettings`,
//! `EnableAllowedImagesSettings`, `DisableAllowedImagesSettings` and
//! `ReplaceImageCriteriaInAllowedImagesSettings`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the Allowed AMIs setting is ACCOUNT-AND-REGION STATE: `GetAllowedImagesSettings`
//!    reports what the writers stored for the Region the request was signed for, and a
//!    Region nothing has configured is reported `disabled` with no criteria, without the
//!    read storing anything;
//! 2. the STATE and the CRITERIA move INDEPENDENTLY: an enable or a disable moves the state
//!    and leaves the criteria, a replace replaces the criteria and leaves the state, and
//!    every member a replace names round-trips through the get;
//! 3. a value outside what the model and the member docs allow is refused with EC2's own
//!    error code, and a refused request stores nothing;
//! 4. `DryRun` answers `DryRunOperation` on each of the four operations and stores nothing.
//!
//! Each table row is a distinct input along what the rule generalises over — which state is
//! asked for, which members a criterion names, how many criteria the request carries, how
//! many values each member carries at its published maximum and one past it, a day count at
//! each published bound, which Region the call is signed for — so no row is the
//! specification.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::{
    ALLOWED_IMAGES_SETTINGS_AUDIT_MODE, ALLOWED_IMAGES_SETTINGS_DISABLED,
    ALLOWED_IMAGES_SETTINGS_ENABLED, ComAmazonawsEc2Amazonec2State as Ec2State,
    IMAGE_PROVIDER_ALIASES, IMAGE_PROVIDER_NONE, MANAGED_BY_ACCOUNT, MAX_IMAGE_CRITERIA,
    MAX_IMAGE_NAME_CRITERION_CHARS, MAX_IMAGE_NAMES_PER_CRITERION,
    MAX_IMAGE_PROVIDERS_PER_CRITERION, MAX_IMAGE_WATERMARK_PATTERN_CHARS,
    MAX_IMAGE_WATERMARKS_PER_CRITERION, MAX_MARKETPLACE_PRODUCT_CODE_CHARS,
    MAX_MARKETPLACE_PRODUCT_CODES_PER_CRITERION,
};

use super::ec2_query::{call, element, elements, error_code, unescaped};

/// The Region most calls here are signed for. The setting is per Region, so the Region a
/// call carries is which setting it reads or writes.
const REGION: &str = "us-west-2";

/// A second Region, to pin that one Region's setting is not another's.
const OTHER_REGION: &str = "eu-west-1";

/// A twelve-digit account id, which `ImageProviders` accepts beside its documented words.
const PARTNER_ACCOUNT: &str = "111122223333";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One ec2Query call signed for `region`.
async fn api_in(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, region, action, params).await
}

/// One ec2Query call signed for [`REGION`].
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    api_in(router, REGION, action, params).await
}

/// One `GetAllowedImagesSettings` call signed for `region`.
async fn get_in(router: &MultiServiceRouter, region: &str) -> (u16, String) {
    api_in(router, region, "GetAllowedImagesSettings", &[]).await
}

/// One `GetAllowedImagesSettings` call signed for [`REGION`].
async fn get(router: &MultiServiceRouter) -> (u16, String) {
    get_in(router, REGION).await
}

/// How many Regions the store holds a setting for.
fn stored_regions(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .allowed_images_settings
        .len()
}

/// One watermark filter a criterion may carry, as the request spells it.
#[derive(Clone, Debug, Default)]
struct WatermarkSpec {
    key: Option<String>,
    source_image_region: Option<String>,
    days_since_source_image: Option<i64>,
    days_since_watermark: Option<i64>,
}

/// One `ImageCriterion` a replace asks for. The ROW is the request, and the expected get is
/// derived from it — the rule under test is that the two agree — so nothing here is written
/// twice.
#[derive(Clone, Debug, Default)]
struct CriterionSpec {
    providers: Vec<String>,
    names: Vec<String>,
    product_codes: Vec<String>,
    maximum_days_since_created: Option<i64>,
    maximum_days_since_deprecated: Option<i64>,
    watermarks: Vec<WatermarkSpec>,
}

impl CriterionSpec {
    /// A criterion naming image providers only.
    fn providers(values: &[&str]) -> Self {
        Self {
            providers: values.iter().map(|value| (*value).to_string()).collect(),
            ..Self::default()
        }
    }

    /// A criterion naming image names only.
    fn names(values: &[&str]) -> Self {
        Self {
            names: values.iter().map(|value| (*value).to_string()).collect(),
            ..Self::default()
        }
    }

    /// A criterion naming Marketplace product codes only.
    fn product_codes(values: &[&str]) -> Self {
        Self {
            product_codes: values.iter().map(|value| (*value).to_string()).collect(),
            ..Self::default()
        }
    }
}

/// The form fields one criterion contributes, at `position` (counting from 1).
///
/// The keys are the ec2Query flattening of the request shape: the member's name and the
/// 1-based index of each value, the same shape `TagSpecification.1.Tag.1.Key` takes.
fn criterion_params(position: usize, spec: &CriterionSpec) -> Vec<(String, String)> {
    let prefix = format!("ImageCriterion.{position}");
    let mut params = Vec::new();
    for (index, provider) in spec.providers.iter().enumerate() {
        params.push(p(&format!("{prefix}.ImageProvider.{}", index + 1), provider));
    }
    for (index, name) in spec.names.iter().enumerate() {
        params.push(p(&format!("{prefix}.ImageName.{}", index + 1), name));
    }
    for (index, code) in spec.product_codes.iter().enumerate() {
        params.push(p(
            &format!("{prefix}.MarketplaceProductCode.{}", index + 1),
            code,
        ));
    }
    if let Some(days) = spec.maximum_days_since_created {
        params.push(p(
            &format!("{prefix}.CreationDateCondition.MaximumDaysSinceCreated"),
            days.to_string(),
        ));
    }
    if let Some(days) = spec.maximum_days_since_deprecated {
        params.push(p(
            &format!("{prefix}.DeprecationTimeCondition.MaximumDaysSinceDeprecated"),
            days.to_string(),
        ));
    }
    for (index, watermark) in spec.watermarks.iter().enumerate() {
        let watermark_prefix = format!("{prefix}.ImageWatermark.{}", index + 1);
        if let Some(key) = &watermark.key {
            params.push(p(&format!("{watermark_prefix}.WatermarkKey"), key.clone()));
        }
        if let Some(region) = &watermark.source_image_region {
            params.push(p(
                &format!("{watermark_prefix}.SourceImageRegion"),
                region.clone(),
            ));
        }
        if let Some(days) = watermark.days_since_source_image {
            params.push(p(
                &format!("{watermark_prefix}.MaximumDaysSinceSourceImageCreated"),
                days.to_string(),
            ));
        }
        if let Some(days) = watermark.days_since_watermark {
            params.push(p(
                &format!("{watermark_prefix}.MaximumDaysSinceWatermarkCreated"),
                days.to_string(),
            ));
        }
    }
    params
}

/// The form fields a whole `ImageCriteria` list contributes.
fn criteria_params(criteria: &[CriterionSpec]) -> Vec<(String, String)> {
    criteria
        .iter()
        .enumerate()
        .flat_map(|(index, spec)| criterion_params(index + 1, spec))
        .collect()
}

/// The `<item>` values of every `<set_name>` list in `body`, one entry per list — so
/// `value_lists(body, "imageProviderSet")` is the providers of each criterion that names
/// any, in the order the criteria are reported.
fn value_lists(body: &str, set_name: &str) -> Vec<Vec<String>> {
    elements(body, set_name)
        .iter()
        .map(|list| elements(list, "item").iter().map(|item| unescaped(item)).collect())
        .collect()
}

/// Every `<member>` value in `body`, as the service wrote it — an image name carrying an
/// apostrophe reaches the wire as `&apos;`.
fn member_values(body: &str, member: &str) -> Vec<String> {
    elements(body, member)
        .iter()
        .map(|value| unescaped(value))
        .collect()
}

/// The lists `criteria` asks for, of the member `select` reads off one criterion, leaving
/// out the criteria that name none of it.
fn expected_lists(criteria: &[CriterionSpec], select: fn(&CriterionSpec) -> &[String]) -> Vec<Vec<String>> {
    criteria
        .iter()
        .map(select)
        .filter(|values| !values.is_empty())
        .map(<[String]>::to_vec)
        .collect()
}

/// Assert the get reports exactly the criteria `criteria` asked for.
fn assert_criteria_reported(case: &str, body: &str, criteria: &[CriterionSpec]) {
    for (set_name, expected) in [
        (
            "imageProviderSet",
            expected_lists(criteria, |spec| &spec.providers),
        ),
        ("imageNameSet", expected_lists(criteria, |spec| &spec.names)),
        (
            "marketplaceProductCodeSet",
            expected_lists(criteria, |spec| &spec.product_codes),
        ),
    ] {
        let reported = value_lists(body, set_name);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {set_name} {expected:?}, got {reported:?} in body {body}"
        );
    }
    for (member, expected) in [
        (
            "maximumDaysSinceCreated",
            criteria
                .iter()
                .filter_map(|spec| spec.maximum_days_since_created)
                .map(|days| days.to_string())
                .collect::<Vec<_>>(),
        ),
        (
            "maximumDaysSinceDeprecated",
            criteria
                .iter()
                .filter_map(|spec| spec.maximum_days_since_deprecated)
                .map(|days| days.to_string())
                .collect(),
        ),
        (
            "watermarkKey",
            criteria
                .iter()
                .flat_map(|spec| spec.watermarks.iter())
                .filter_map(|watermark| watermark.key.clone())
                .collect(),
        ),
        (
            "sourceImageRegion",
            criteria
                .iter()
                .flat_map(|spec| spec.watermarks.iter())
                .filter_map(|watermark| watermark.source_image_region.clone())
                .collect(),
        ),
        (
            "maximumDaysSinceSourceImageCreated",
            criteria
                .iter()
                .flat_map(|spec| spec.watermarks.iter())
                .filter_map(|watermark| watermark.days_since_source_image)
                .map(|days| days.to_string())
                .collect(),
        ),
        (
            "maximumDaysSinceWatermarkCreated",
            criteria
                .iter()
                .flat_map(|spec| spec.watermarks.iter())
                .filter_map(|watermark| watermark.days_since_watermark)
                .map(|days| days.to_string())
                .collect(),
        ),
    ] {
        let reported = member_values(body, member);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {member} {expected:?}, got {reported:?} in body {body}"
        );
    }
}

/// One `ReplaceImageCriteriaInAllowedImagesSettings` call that must succeed.
async fn replace(case: &str, router: &MultiServiceRouter, criteria: &[CriterionSpec]) {
    let (status, body) = api(
        router,
        "ReplaceImageCriteriaInAllowedImagesSettings",
        &criteria_params(criteria),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the replace expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "return").unwrap_or_default(),
        "true",
        "case {case:?}: the replace returns true, got body {body}"
    );
}

/// RULE 1: a Region nothing has configured is reported `disabled`, with no criteria and
/// managed by the account — and the read stores NOTHING, so two reads agree and a later
/// caller sees what the first one saw.
#[tokio::test]
async fn an_unconfigured_region_reports_the_disabled_setting_and_the_read_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    for case in ["the first read", "the second read"] {
        let (status, body) = get(&router).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "state").unwrap_or_default(),
            ALLOWED_IMAGES_SETTINGS_DISABLED,
            "case {case:?}: an account that never enabled Allowed AMIs is disabled, got body {body}"
        );
        assert_eq!(
            element(&body, "managedBy").unwrap_or_default(),
            MANAGED_BY_ACCOUNT,
            "case {case:?}: the settings are managed by the account, got body {body}"
        );
        assert_criteria_reported(case, &body, &[]);
        assert_eq!(
            stored_regions(&engine),
            0,
            "case {case:?}: a read stores no setting, got {} in the store",
            stored_regions(&engine)
        );
    }
}

/// A state-move row: the case, the action that makes the move, its form fields, and the
/// state the setting must hold after it.
type StateRow = (&'static str, &'static str, Vec<(String, String)>, &'static str);

/// RULE 1 and RULE 2: each state a writer can ask for is answered by that writer AND
/// reported by the get, whatever state the setting was in before.
#[tokio::test]
async fn every_state_a_writer_asks_for_is_what_the_get_reports() {
    let (router, _engine) = super::ec2_query::emulator();

    // Each row moves the setting from wherever the row before left it, so the table covers
    // every move between the three states a setting can hold — including a repeat of the
    // state it is already in, which is idempotent rather than refused.
    let rows: Vec<StateRow> = vec![
        (
            "enable, from the unconfigured setting",
            "EnableAllowedImagesSettings",
            vec![p("AllowedImagesSettingsState", ALLOWED_IMAGES_SETTINGS_ENABLED)],
            ALLOWED_IMAGES_SETTINGS_ENABLED,
        ),
        (
            "audit-mode, from enabled",
            "EnableAllowedImagesSettings",
            vec![p(
                "AllowedImagesSettingsState",
                ALLOWED_IMAGES_SETTINGS_AUDIT_MODE,
            )],
            ALLOWED_IMAGES_SETTINGS_AUDIT_MODE,
        ),
        (
            "audit-mode again",
            "EnableAllowedImagesSettings",
            vec![p(
                "AllowedImagesSettingsState",
                ALLOWED_IMAGES_SETTINGS_AUDIT_MODE,
            )],
            ALLOWED_IMAGES_SETTINGS_AUDIT_MODE,
        ),
        (
            "disable, from audit-mode",
            "DisableAllowedImagesSettings",
            Vec::new(),
            ALLOWED_IMAGES_SETTINGS_DISABLED,
        ),
        (
            "disable again",
            "DisableAllowedImagesSettings",
            Vec::new(),
            ALLOWED_IMAGES_SETTINGS_DISABLED,
        ),
        (
            "enable, from disabled",
            "EnableAllowedImagesSettings",
            vec![p("AllowedImagesSettingsState", ALLOWED_IMAGES_SETTINGS_ENABLED)],
            ALLOWED_IMAGES_SETTINGS_ENABLED,
        ),
    ];

    for (case, action, params, expected) in rows {
        let (status, body) = api(&router, action, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "allowedImagesSettingsState").unwrap_or_default(),
            expected,
            "case {case:?}: the writer answers the state it reached, got body {body}"
        );

        let (status, body) = get(&router).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the get expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "state").unwrap_or_default(),
            expected,
            "case {case:?}: the get reports the state the writer stored, got body {body}"
        );
    }
}

/// RULE 2: every member a replace names round-trips through the get, in the order the
/// request gave — and a replace that names no criteria at all clears them.
#[tokio::test]
async fn every_criterion_a_replace_names_round_trips_through_the_get() {
    let (router, _engine) = super::ec2_query::emulator();

    // The published maxima of the member docs, exercised at the bound: 200 providers, 50
    // names, 50 product codes, 50 watermark filters and 10 criteria. The day conditions are
    // exercised at each published bound of their modelled range (0..=2147483647).
    let longest_product_code = "a".repeat(MAX_MARKETPLACE_PRODUCT_CODE_CHARS);
    let longest_image_name = "n".repeat(MAX_IMAGE_NAME_CRITERION_CHARS);
    let longest_watermark_key = "k".repeat(MAX_IMAGE_WATERMARK_PATTERN_CHARS);
    let every_provider: Vec<String> = (0..MAX_IMAGE_PROVIDERS_PER_CRITERION)
        .map(|index| format!("{:012}", index + 1))
        .collect();
    let every_name: Vec<String> = (0..MAX_IMAGE_NAMES_PER_CRITERION)
        .map(|index| format!("image-{index}-*"))
        .collect();
    let every_product_code: Vec<String> = (0..MAX_MARKETPLACE_PRODUCT_CODES_PER_CRITERION)
        .map(|index| format!("code{index}"))
        .collect();
    let every_watermark: Vec<WatermarkSpec> = (0..MAX_IMAGE_WATERMARKS_PER_CRITERION)
        .map(|index| WatermarkSpec {
            key: Some(format!("{PARTNER_ACCOUNT}:watermark-{index}")),
            ..WatermarkSpec::default()
        })
        .collect();

    let rows: Vec<(&str, Vec<CriterionSpec>)> = vec![
        ("one documented provider word", vec![CriterionSpec::providers(&["amazon"])]),
        (
            "every documented provider word and an account id",
            vec![CriterionSpec::providers(&[
                IMAGE_PROVIDER_ALIASES[0],
                IMAGE_PROVIDER_ALIASES[1],
                IMAGE_PROVIDER_ALIASES[2],
                PARTNER_ACCOUNT,
            ])],
        ),
        (
            "the account-only provider word, which stands alone",
            vec![CriterionSpec::providers(&[IMAGE_PROVIDER_NONE])],
        ),
        (
            "image names with each documented wildcard and special character",
            vec![CriterionSpec::names(&[
                "ubuntu/images/*",
                "amzn2-ami-?????",
                "My Image (1) [beta].v1_2-3'x@y",
            ])],
        ),
        (
            "an image name at the longest length the docs allow",
            vec![CriterionSpec::names(&[longest_image_name.as_str()])],
        ),
        (
            "product codes, the longest one the docs allow included",
            vec![CriterionSpec::product_codes(&[
                "aw0evgkw8e5c1q413zgy5pjce",
                longest_product_code.as_str(),
            ])],
        ),
        (
            "the day conditions at the floor of their range",
            vec![CriterionSpec {
                maximum_days_since_created: Some(0),
                maximum_days_since_deprecated: Some(0),
                ..CriterionSpec::default()
            }],
        ),
        (
            "the day conditions at the ceiling of their range",
            vec![CriterionSpec {
                maximum_days_since_created: Some(2_147_483_647),
                maximum_days_since_deprecated: Some(2_147_483_647),
                ..CriterionSpec::default()
            }],
        ),
        (
            "one watermark filter naming every field",
            vec![CriterionSpec {
                watermarks: vec![WatermarkSpec {
                    key: Some(format!("{PARTNER_ACCOUNT}:golden")),
                    source_image_region: Some("us-east-1".to_string()),
                    days_since_source_image: Some(30),
                    days_since_watermark: Some(7),
                }],
                ..CriterionSpec::default()
            }],
        ),
        (
            "a watermark key at the longest pattern this emulator stores",
            vec![CriterionSpec {
                watermarks: vec![WatermarkSpec {
                    key: Some(longest_watermark_key.clone()),
                    ..WatermarkSpec::default()
                }],
                ..CriterionSpec::default()
            }],
        ),
        (
            "one criterion naming several members at once",
            vec![CriterionSpec {
                providers: vec!["amazon".to_string()],
                names: vec!["al2023-ami-*".to_string()],
                product_codes: vec!["abc123".to_string()],
                maximum_days_since_created: Some(300),
                maximum_days_since_deprecated: Some(10),
                watermarks: vec![WatermarkSpec {
                    source_image_region: Some("eu-*".to_string()),
                    ..WatermarkSpec::default()
                }],
            }],
        ),
        (
            "three criteria, each naming a different member",
            vec![
                CriterionSpec::providers(&["aws-marketplace"]),
                CriterionSpec::names(&["windows-*"]),
                CriterionSpec::product_codes(&["xyz789"]),
            ],
        ),
        (
            "every member at its published maximum, in one criterion",
            vec![CriterionSpec {
                providers: every_provider.clone(),
                names: every_name.clone(),
                product_codes: every_product_code.clone(),
                watermarks: every_watermark.clone(),
                ..CriterionSpec::default()
            }],
        ),
        (
            "as many criteria as the docs allow",
            (0..MAX_IMAGE_CRITERIA)
                .map(|index| CriterionSpec {
                    names: vec![format!("criterion-{index}-*")],
                    ..CriterionSpec::default()
                })
                .collect(),
        ),
        ("no criteria at all, which clears them", Vec::new()),
    ];

    for (case, criteria) in rows {
        replace(case, &router, &criteria).await;
        let (status, body) = get(&router).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the get expected HTTP 200, got {status} with body {body}"
        );
        assert_criteria_reported(case, &body, &criteria);
    }
}

/// RULE 2: the state and the criteria are independent — an enable and a disable leave the
/// criteria exactly as they stood, and a replace leaves the state.
#[tokio::test]
async fn the_state_and_the_criteria_do_not_disturb_each_other() {
    let (router, _engine) = super::ec2_query::emulator();
    let criteria = vec![
        CriterionSpec::providers(&["amazon", PARTNER_ACCOUNT]),
        CriterionSpec::names(&["al2023-ami-*"]),
    ];
    replace("the criteria set before any enable", &router, &criteria).await;

    for (case, action, params, expected_state) in [
        (
            "the criteria set while the feature is disabled",
            None,
            Vec::new(),
            ALLOWED_IMAGES_SETTINGS_DISABLED,
        ),
        (
            "an enable keeps the criteria",
            Some("EnableAllowedImagesSettings"),
            vec![p("AllowedImagesSettingsState", ALLOWED_IMAGES_SETTINGS_ENABLED)],
            ALLOWED_IMAGES_SETTINGS_ENABLED,
        ),
        (
            "a move to audit-mode keeps the criteria",
            Some("EnableAllowedImagesSettings"),
            vec![p(
                "AllowedImagesSettingsState",
                ALLOWED_IMAGES_SETTINGS_AUDIT_MODE,
            )],
            ALLOWED_IMAGES_SETTINGS_AUDIT_MODE,
        ),
        (
            "a disable keeps the criteria",
            Some("DisableAllowedImagesSettings"),
            Vec::new(),
            ALLOWED_IMAGES_SETTINGS_DISABLED,
        ),
    ] {
        if let Some(action) = action {
            let (status, body) = api(&router, action, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
        }
        let (_, body) = get(&router).await;
        assert_eq!(
            element(&body, "state").unwrap_or_default(),
            expected_state,
            "case {case:?}: expected the state {expected_state}, got body {body}"
        );
        assert_criteria_reported(case, &body, &criteria);
    }

    // And the other way round: a replace while the feature is enabled changes the criteria
    // and leaves the state where the enable put it.
    let (status, body) = api(
        &router,
        "EnableAllowedImagesSettings",
        &[p(
            "AllowedImagesSettingsState",
            ALLOWED_IMAGES_SETTINGS_AUDIT_MODE,
        )],
    )
    .await;
    assert_eq!(
        status, 200,
        "the enable before the second replace: got {status} with body {body}"
    );
    let replaced = vec![CriterionSpec::product_codes(&["abc123"])];
    replace("a replace while audit-mode is on", &router, &replaced).await;
    let (_, body) = get(&router).await;
    assert_eq!(
        element(&body, "state").unwrap_or_default(),
        ALLOWED_IMAGES_SETTINGS_AUDIT_MODE,
        "a replace leaves the state where the enable put it, got body {body}"
    );
    assert_criteria_reported("a replace while audit-mode is on", &body, &replaced);
}

/// RULE 1: the setting of one Region is not the setting of another — each Region reports
/// what was written for it, and the Region nothing was written for stays disabled with no
/// criteria.
#[tokio::test]
async fn one_regions_setting_is_not_another_regions() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = api_in(
        &router,
        REGION,
        "EnableAllowedImagesSettings",
        &[p("AllowedImagesSettingsState", ALLOWED_IMAGES_SETTINGS_ENABLED)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the enable in {REGION}: got {status} with body {body}"
    );
    let criteria = vec![CriterionSpec::providers(&["amazon"])];
    replace("the criteria of the signed Region", &router, &criteria).await;

    let (_, body) = get_in(&router, OTHER_REGION).await;
    assert_eq!(
        element(&body, "state").unwrap_or_default(),
        ALLOWED_IMAGES_SETTINGS_DISABLED,
        "a Region nothing was written for is still disabled, got body {body}"
    );
    assert_criteria_reported("the other Region before any write", &body, &[]);

    let other_criteria = vec![CriterionSpec::names(&["windows-*"])];
    let (status, body) = api_in(
        &router,
        OTHER_REGION,
        "ReplaceImageCriteriaInAllowedImagesSettings",
        &criteria_params(&other_criteria),
    )
    .await;
    assert_eq!(
        status, 200,
        "the replace in {OTHER_REGION}: got {status} with body {body}"
    );

    let (_, body) = get_in(&router, OTHER_REGION).await;
    assert_criteria_reported("the other Region after its own write", &body, &other_criteria);
    let (_, body) = get(&router).await;
    assert_eq!(
        element(&body, "state").unwrap_or_default(),
        ALLOWED_IMAGES_SETTINGS_ENABLED,
        "the first Region keeps its own state, got body {body}"
    );
    assert_criteria_reported("the signed Region after the other was written", &body, &criteria);
    assert_eq!(
        stored_regions(&engine),
        2,
        "two Regions were written, so the store holds two settings"
    );
}

/// A refusal row: the case, the action, the form fields, and the HTTP status and EC2 error
/// code the request must be refused with.
type RefusalRow = (&'static str, &'static str, Vec<(String, String)>, u16, &'static str);

/// RULE 3: a value outside what the model and the member docs allow is refused with EC2's
/// own code, and the refused request leaves the stored setting exactly as it stood.
#[tokio::test]
async fn a_value_the_docs_do_not_allow_is_refused_and_stores_nothing() {
    let (router, _engine) = super::ec2_query::emulator();

    // The setting the refusals must not disturb.
    let held = vec![CriterionSpec::providers(&["amazon"])];
    replace("the setting a refusal must not disturb", &router, &held).await;
    let (status, body) = api(
        &router,
        "EnableAllowedImagesSettings",
        &[p(
            "AllowedImagesSettingsState",
            ALLOWED_IMAGES_SETTINGS_AUDIT_MODE,
        )],
    )
    .await;
    assert_eq!(
        status, 200,
        "the enable before the refusal table: got {status} with body {body}"
    );

    let replace_action = "ReplaceImageCriteriaInAllowedImagesSettings";
    let one_too_many_criteria: Vec<CriterionSpec> = (0..=MAX_IMAGE_CRITERIA)
        .map(|index| CriterionSpec {
            names: vec![format!("criterion-{index}-*")],
            ..CriterionSpec::default()
        })
        .collect();
    let one_too_many_providers = CriterionSpec {
        providers: (0..=MAX_IMAGE_PROVIDERS_PER_CRITERION)
            .map(|index| format!("{:012}", index + 1))
            .collect(),
        ..CriterionSpec::default()
    };
    let one_too_many_names = CriterionSpec {
        names: (0..=MAX_IMAGE_NAMES_PER_CRITERION)
            .map(|index| format!("image-{index}-*"))
            .collect(),
        ..CriterionSpec::default()
    };
    let one_too_many_product_codes = CriterionSpec {
        product_codes: (0..=MAX_MARKETPLACE_PRODUCT_CODES_PER_CRITERION)
            .map(|index| format!("code{index}"))
            .collect(),
        ..CriterionSpec::default()
    };
    let one_too_many_watermarks = CriterionSpec {
        watermarks: (0..=MAX_IMAGE_WATERMARKS_PER_CRITERION)
            .map(|index| WatermarkSpec {
                key: Some(format!("{PARTNER_ACCOUNT}:watermark-{index}")),
                ..WatermarkSpec::default()
            })
            .collect(),
        ..CriterionSpec::default()
    };
    let over_long_image_name =
        CriterionSpec::names(&[&"n".repeat(MAX_IMAGE_NAME_CRITERION_CHARS + 1)]);
    let over_long_product_code = CriterionSpec::product_codes(&[&"a"
        .repeat(MAX_MARKETPLACE_PRODUCT_CODE_CHARS + 1)]);
    let over_long_watermark_key = CriterionSpec {
        watermarks: vec![WatermarkSpec {
            key: Some("k".repeat(MAX_IMAGE_WATERMARK_PATTERN_CHARS + 1)),
            ..WatermarkSpec::default()
        }],
        ..CriterionSpec::default()
    };

    let rows: Vec<RefusalRow> = vec![
        // `AllowedImagesSettingsEnabledState` is a modelled enum, so the request walk
        // refuses a value outside it BEFORE the handler runs and the refusal is the
        // boundary validator's `ValidationError`. The handler keeps its own
        // `InvalidParameterValue` path for the same case, so no unknown state can ever be
        // stored even if the boundary stops checking.
        (
            "a state only a disable can reach",
            "EnableAllowedImagesSettings",
            vec![p(
                "AllowedImagesSettingsState",
                ALLOWED_IMAGES_SETTINGS_DISABLED,
            )],
            400,
            "ValidationError",
        ),
        (
            "a state the enum does not have",
            "EnableAllowedImagesSettings",
            vec![p("AllowedImagesSettingsState", "audit")],
            400,
            "ValidationError",
        ),
        (
            "one criterion past the documented maximum",
            replace_action,
            criteria_params(&one_too_many_criteria),
            400,
            "InvalidParameterValue",
        ),
        (
            "one provider past the documented maximum",
            replace_action,
            criteria_params(&[one_too_many_providers]),
            400,
            "InvalidParameterValue",
        ),
        (
            "one image name past the documented maximum",
            replace_action,
            criteria_params(&[one_too_many_names]),
            400,
            "InvalidParameterValue",
        ),
        (
            "one product code past the documented maximum",
            replace_action,
            criteria_params(&[one_too_many_product_codes]),
            400,
            "InvalidParameterValue",
        ),
        (
            "one watermark filter past the documented maximum",
            replace_action,
            criteria_params(&[one_too_many_watermarks]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a provider that is neither a documented word nor an account id",
            replace_action,
            criteria_params(&[CriterionSpec::providers(&["nobody"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "an account id of the wrong length",
            replace_action,
            criteria_params(&[CriterionSpec::providers(&["12345"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "the account-only word beside another value",
            replace_action,
            criteria_params(&[CriterionSpec::providers(&[IMAGE_PROVIDER_NONE, "amazon"])]),
            400,
            "InvalidParameterCombination",
        ),
        (
            "an image name with a wildcard below the documented floor",
            replace_action,
            criteria_params(&[CriterionSpec::names(&["a?"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "an empty image name",
            replace_action,
            criteria_params(&[CriterionSpec::names(&[""])]),
            400,
            "InvalidParameterValue",
        ),
        // The 128-character ceiling is also modelled, as a `@length` on the member of
        // `ImageNameCriteriaRequestList`, so the request walk refuses an over-long name
        // before the handler runs and the refusal is the boundary validator's
        // `ValidationError`. The handler keeps its own `InvalidParameterValue` ceiling for
        // the same case, so no caller-sized name can ever be stored even if the boundary
        // stops checking.
        (
            "an image name one character past the documented maximum",
            replace_action,
            criteria_params(&[over_long_image_name]),
            400,
            "ValidationError",
        ),
        (
            "an image name carrying a character the docs exclude",
            replace_action,
            criteria_params(&[CriterionSpec::names(&["al2023#ami"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a product code carrying a character the docs exclude",
            replace_action,
            criteria_params(&[CriterionSpec::product_codes(&["abc-123"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a product code one character past the documented maximum",
            replace_action,
            criteria_params(&[over_long_product_code]),
            400,
            "InvalidParameterValue",
        ),
        (
            "an empty product code",
            replace_action,
            criteria_params(&[CriterionSpec::product_codes(&[""])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a watermark key one character past what this emulator stores",
            replace_action,
            criteria_params(&[over_long_watermark_key]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a watermark key with no pattern in it",
            replace_action,
            criteria_params(&[CriterionSpec {
                watermarks: vec![WatermarkSpec {
                    key: Some(String::new()),
                    ..WatermarkSpec::default()
                }],
                ..CriterionSpec::default()
            }]),
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, action, params, expected_status, expected_code) in rows {
        let (status, body) = api(&router, action, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?} in body {body}"
        );

        let (_, body) = get(&router).await;
        assert_eq!(
            element(&body, "state").unwrap_or_default(),
            ALLOWED_IMAGES_SETTINGS_AUDIT_MODE,
            "case {case:?}: a refused request leaves the state alone, got body {body}"
        );
        assert_criteria_reported(case, &body, &held);
    }
}

/// RULE 4: `DryRun` is answered `DryRunOperation` by each of the four operations, and the
/// stored setting is left exactly as it stood.
#[tokio::test]
async fn a_dry_run_is_refused_and_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let held = vec![CriterionSpec::names(&["al2023-ami-*"])];
    replace("the setting a dry run must not disturb", &router, &held).await;
    let regions_before = stored_regions(&engine);

    for (case, action, mut params) in [
        ("a dry run of the get", "GetAllowedImagesSettings", Vec::new()),
        (
            "a dry run of the enable",
            "EnableAllowedImagesSettings",
            vec![p("AllowedImagesSettingsState", ALLOWED_IMAGES_SETTINGS_ENABLED)],
        ),
        (
            "a dry run of the disable",
            "DisableAllowedImagesSettings",
            Vec::new(),
        ),
        (
            "a dry run of the replace",
            "ReplaceImageCriteriaInAllowedImagesSettings",
            criteria_params(&[CriterionSpec::providers(&["aws-marketplace"])]),
        ),
    ] {
        params.push(p("DryRun", "true"));
        let (status, body) = api(&router, action, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {code:?} in body {body}"
        );

        let (_, body) = get(&router).await;
        assert_eq!(
            element(&body, "state").unwrap_or_default(),
            ALLOWED_IMAGES_SETTINGS_DISABLED,
            "case {case:?}: a dry run leaves the state alone, got body {body}"
        );
        assert_criteria_reported(case, &body, &held);
        assert_eq!(
            stored_regions(&engine),
            regions_before,
            "case {case:?}: a dry run stores no setting"
        );
    }
}
