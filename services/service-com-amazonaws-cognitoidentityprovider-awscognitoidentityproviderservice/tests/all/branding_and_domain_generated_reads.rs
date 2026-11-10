// SPDX-License-Identifier: CC0-1.0

//! The values the SERVICE generates for a managed login branding style and for a
//! user pool domain, driven through the router exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rules under test:
//!
//! * A branding style's `Assets` is the list of image files the style holds, and
//!   a style that holds none holds an EMPTY list of them: the member is reported
//!   by every operation that renders a style, whatever the style holds.
//! * "When true, returns values for branding options that are unchanged from
//!   Amazon Cognito defaults. When false or when you omit this parameter,
//!   returns only values that you customized in your branding style"
//!   (`ReturnMergedResources` on the two readers of a style), so the MERGED view
//!   reports the service's own settings document under the style's own options
//!   and the service's own image file for every role the style supplied none
//!   for, while the customized view reports the style's own alone.
//! * Every hosted user pool domain is served from a static-file bucket, through
//!   a CloudFront endpoint, at an app version — `DomainDescriptionType`'s
//!   `S3Bucket`, `CloudFrontDistribution` and `Version` — so a read of a domain
//!   reports all three, and the domain's `Status` is the status its own
//!   provisioning is in: a prefix domain is served by infrastructure that is
//!   already running and is `ACTIVE` from the create, while a custom domain's
//!   CloudFront distribution has to be provisioned first.
//!
//! Each table row names the case it asserts and every assertion reports the
//! value it actually saw.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use crate::managed_login_branding::{
    asset, call, client, ok, pool, router, CATEGORIES, COLOR_MODES, EXTENSIONS,
};
use cognito_idp::state::DOMAIN_ACTIVATION_SPAN;

/// Long enough that the clock owes a domain with provisioning to do its move to
/// `ACTIVE`.
const PAST_THE_ACTIVATION: Duration =
    DOMAIN_ACTIVATION_SPAN.saturating_add(Duration::from_millis(250));

/// An ACM certificate ARN in the Region `CreateUserPoolDomain` names for a
/// custom domain ("you must provide an SSL certificate in the US East (N.
/// Virginia) Amazon Web Services Region").
const CERTIFICATE: &str =
    "arn:aws:acm:us-east-1:000000000000:certificate/11111111-2222-3333-4444-555555555555";

/// The modelled length bounds of `DomainVersionType`
/// (`smithy_query(query="shape",shape="DomainVersionType")`).
const VERSION_LENGTH: (usize, usize) = (1, 20);

/// The modelled length bounds of `S3BucketType`
/// (`smithy_query(query="shape",shape="S3BucketType")`).
const BUCKET_LENGTH: (usize, usize) = (3, 1024);

/// The modelled length maximum of `AssetListType`
/// (`smithy_query(query="operation",operation="CreateManagedLoginBranding")`).
const MAX_ASSETS: usize = 40;

/// Create a style for a fresh pool and app client from `create`, and answer the
/// pool id, the app client id and the created style.
async fn style(
    router: &MultiServiceRouter,
    name: &str,
    create: &Value,
) -> (String, String, Value) {
    let user_pool_id = pool(router, &format!("{name}-pool")).await;
    let client_id = client(router, &user_pool_id, &format!("{name}-client")).await;
    let mut body = create
        .as_object()
        .cloned()
        .expect("the create body is an object");
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientId".to_string(), json!(client_id));
    let created = ok(router, "CreateManagedLoginBranding", &Value::Object(body)).await;
    (
        user_pool_id,
        client_id,
        created["ManagedLoginBranding"].clone(),
    )
}

/// The `Assets` of a rendered style, or a panic naming the operation that failed
/// to report the member at all.
fn assets<'a>(case: &str, where_from: &str, rendered: &'a Value) -> &'a Vec<Value> {
    rendered["Assets"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: {where_from} must report Assets as a list, got {rendered}")
    })
}

/// One row of the `Assets` table: the style a row creates, how many image files
/// it holds afterwards, and whether it is on the Cognito-provided values.
struct AssetsCase {
    case: &'static str,
    create: Value,
    /// The number of files the created style holds of its own.
    expected_files: usize,
    provided_values: bool,
}

#[tokio::test]
async fn every_read_of_a_style_reports_the_image_files_it_holds() {
    let router = router();
    // The rows differ along what the rule generalises over: the number of image
    // files the style holds of its own (none, one, three and the modelled
    // maximum 40) and whether it takes the Cognito-provided values.
    let rows = vec![
        AssetsCase {
            case: "a style that supplies no image files at all",
            create: json!({}),
            expected_files: 0,
            provided_values: false,
        },
        AssetsCase {
            case: "a style that supplies an empty list of image files",
            create: json!({ "Assets": [] }),
            expected_files: 0,
            provided_values: false,
        },
        AssetsCase {
            case: "a style that supplies one image file",
            create: json!({
                "Assets": [asset(CATEGORIES[13], COLOR_MODES[0], EXTENSIONS[2], b"logo", None)],
            }),
            expected_files: 1,
            provided_values: false,
        },
        AssetsCase {
            case: "a style that supplies three image files",
            create: json!({
                "Assets": [
                    asset(CATEGORIES[13], COLOR_MODES[0], EXTENSIONS[2], b"light logo", None),
                    asset(CATEGORIES[13], COLOR_MODES[1], EXTENSIONS[4], b"dark logo", None),
                    asset(CATEGORIES[11], COLOR_MODES[2], EXTENSIONS[3], b"<svg/>", None),
                ],
            }),
            expected_files: 3,
            provided_values: false,
        },
        AssetsCase {
            case: "a style that supplies the modelled maximum of image files",
            create: json!({
                "Assets": CATEGORIES
                    .iter()
                    .flat_map(|category| {
                        COLOR_MODES
                            .iter()
                            .map(move |mode| asset(category, mode, EXTENSIONS[2], b"bytes", None))
                    })
                    .take(MAX_ASSETS)
                    .collect::<Vec<Value>>(),
            }),
            expected_files: MAX_ASSETS,
            provided_values: false,
        },
        AssetsCase {
            case: "a style whose files are all for the browser-adaptive display mode",
            create: json!({
                "Assets": CATEGORIES
                    .iter()
                    .map(|category| asset(category, COLOR_MODES[2], EXTENSIONS[2], b"bytes", None))
                    .collect::<Vec<Value>>(),
            }),
            expected_files: CATEGORIES.len(),
            provided_values: false,
        },
        AssetsCase {
            case: "a style on the Cognito-provided values",
            create: json!({ "UseCognitoProvidedValues": true }),
            expected_files: 0,
            provided_values: true,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        let (user_pool_id, client_id, created) =
            style(&router, &format!("assets-{index}"), &row.create).await;
        let created_files = assets(case, "the create's answer", &created).clone();
        assert_eq!(
            created_files.len(),
            row.expected_files,
            "case {case:?}: the created style must hold the files it was given, got {created}"
        );

        // Every operation that renders the style reports that same list in the
        // CUSTOMIZED view: "returns only values that you customized".
        let branding_id = created["ManagedLoginBrandingId"]
            .as_str()
            .expect("a minted ManagedLoginBrandingId")
            .to_string();
        let read = ok(
            &router,
            "DescribeManagedLoginBranding",
            &json!({
                "UserPoolId": user_pool_id,
                "ManagedLoginBrandingId": branding_id,
                "ReturnMergedResources": false,
            }),
        )
        .await;
        assert_eq!(
            assets(case, "DescribeManagedLoginBranding", &read["ManagedLoginBranding"]),
            &created_files,
            "case {case:?}: the reader by id must report the style's own files, got {read}"
        );
        let by_client = ok(
            &router,
            "DescribeManagedLoginBrandingByClient",
            &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
        )
        .await;
        assert_eq!(
            assets(
                case,
                "DescribeManagedLoginBrandingByClient",
                &by_client["ManagedLoginBranding"]
            ),
            &created_files,
            "case {case:?}: the reader by app client must report the style's own files, got {by_client}"
        );

        // The MERGED view reports a file for every role, so the style's own
        // files are all in it and no role is left without one.
        let merged = ok(
            &router,
            "DescribeManagedLoginBranding",
            &json!({
                "UserPoolId": user_pool_id,
                "ManagedLoginBrandingId": branding_id,
                "ReturnMergedResources": true,
            }),
        )
        .await;
        let merged_files = assets(case, "the merged view", &merged["ManagedLoginBranding"]);
        for own in &created_files {
            assert!(
                merged_files.contains(own),
                "case {case:?}: the merged view must keep the style's own file {own}, got {merged}"
            );
        }
        assert!(
            merged_files.len() >= created_files.len(),
            "case {case:?}: the merged view reports a file for every role, got {merged}"
        );
        // Whatever the style holds, the merged list stays inside the modelled
        // maximum: a style may hold files for roles the service provides none
        // for, and what is then left out is the service's own.
        assert!(
            merged_files.len() <= MAX_ASSETS,
            "case {case:?}: the merged view must stay inside the modelled maximum {MAX_ASSETS}, \
             got {} files in {merged}",
            merged_files.len()
        );

        // An update that names no image file keeps the ones the style holds, and
        // reports them too.
        let updated = ok(
            &router,
            "UpdateManagedLoginBranding",
            &json!({
                "UserPoolId": user_pool_id,
                "ManagedLoginBrandingId": branding_id,
                "UseCognitoProvidedValues": row.provided_values,
            }),
        )
        .await;
        assert_eq!(
            assets(case, "UpdateManagedLoginBranding", &updated["ManagedLoginBranding"]),
            &created_files,
            "case {case:?}: the update must report the style's files, got {updated}"
        );
    }
}

#[tokio::test]
async fn the_merged_view_reports_the_services_own_settings_and_image_files() {
    let router = router();
    // A style that customizes nothing, so every option and every image file the
    // merged view reports is the service's own.
    let (user_pool_id, client_id, created) =
        style(&router, "provided", &json!({ "UseCognitoProvidedValues": true })).await;

    // The customized view of such a style reports only what the caller
    // customized: no settings document at all, and an empty list of files.
    assert!(
        created["Settings"].is_null(),
        "a style that customized nothing reports no settings of its own, got {created}"
    );
    assert!(
        assets("the provided values", "the create's answer", &created).is_empty(),
        "a style that supplied no image file holds an empty list of them, got {created}"
    );

    let merged = ok(
        &router,
        "DescribeManagedLoginBranding",
        &json!({
            "UserPoolId": user_pool_id,
            "ManagedLoginBrandingId": created["ManagedLoginBrandingId"],
            "ReturnMergedResources": true,
        }),
    )
    .await;
    let merged = merged["ManagedLoginBranding"].clone();

    // The settings document the service manages: the option groups the branding
    // editor manages, down to the options a caller can override. The paths below
    // are leaves of that document, picked across all three groups.
    let settings = merged["Settings"]
        .as_object()
        .unwrap_or_else(|| panic!("the merged view reports the service's options, got {merged}"));
    for group in ["categories", "componentClasses", "components"] {
        assert!(
            settings
                .get(group)
                .and_then(Value::as_object)
                .is_some_and(|options| !options.is_empty()),
            "case {group:?}: the merged settings must carry the option group, got {merged}"
        );
    }
    for path in [
        // Options no step of the recorded hosted-UI traces names, so the
        // document is asserted beyond the options that were reported missing.
        "categories.form.location.horizontal",
        "componentClasses.focusState.lightMode.borderColor",
        "components.phoneNumberSelector.displayType",
        // And the ones a client reads for each display mode of a component.
        "componentClasses.statusIndicator.darkMode.success",
        "components.pageText.lightMode.headingColor",
        "components.primaryButton.darkMode.defaults",
        "components.secondaryButton.lightMode.hover",
    ] {
        let mut here = &merged["Settings"];
        for member in path.split('.') {
            here = &here[member];
        }
        assert!(
            !here.is_null(),
            "case {path:?}: the merged settings must carry the option, got {merged}"
        );
    }

    // The image files the service provides: one SVG for each display mode of
    // every category it provides artwork for, inside the modelled maximum. The
    // artwork itself is served from the managed login pages, so no file claims
    // bytes a caller uploaded.
    let files = assets("the provided values", "the merged view", &merged);
    assert!(
        !files.is_empty() && files.len() <= MAX_ASSETS,
        "the provided files must be a non-empty list inside the modelled maximum {MAX_ASSETS}, \
         got {files:?}"
    );
    let mut slots = BTreeSet::new();
    for file in files {
        let category = file["Category"].as_str().unwrap_or_default();
        let color_mode = file["ColorMode"].as_str().unwrap_or_default();
        assert!(
            CATEGORIES.contains(&category),
            "a provided file must name a modelled category, got {file}"
        );
        assert!(
            ["LIGHT", "DARK"].contains(&color_mode),
            "a provided file must name the display mode it is for, got {file}"
        );
        assert_eq!(
            file["Extension"],
            json!("SVG"),
            "a provided file is an SVG, got {file}"
        );
        assert!(
            file.get("Bytes").is_none() && file.get("ResourceId").is_none(),
            "a provided file is the service's own, so it carries no uploaded file, got {file}"
        );
        assert!(
            slots.insert(format!("{category}/{color_mode}")),
            "the provided files hold one image per category and display mode, got {file} twice"
        );
    }
    for category in CATEGORIES {
        for color_mode in ["LIGHT", "DARK"] {
            assert!(
                slots.contains(&format!("{category}/{color_mode}")),
                "the provided files cover {category} in {color_mode}, got {slots:?}"
            );
        }
    }

    // The reader by app client serves the same merged view, and a merged read
    // changes nothing a later caller sees.
    let by_client = ok(
        &router,
        "DescribeManagedLoginBrandingByClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "ReturnMergedResources": true,
        }),
    )
    .await;
    assert_eq!(
        by_client["ManagedLoginBranding"], merged,
        "the reader by app client must serve the same merged view, got {by_client}"
    );
    let customized = ok(
        &router,
        "DescribeManagedLoginBranding",
        &json!({
            "UserPoolId": user_pool_id,
            "ManagedLoginBrandingId": created["ManagedLoginBrandingId"],
        }),
    )
    .await;
    assert_eq!(
        customized["ManagedLoginBranding"], created,
        "a merged read must not change the stored style, got {customized}"
    );

    // A style that supplies a file for one role keeps it in the merged view, and
    // takes the service's file for every role it left alone.
    let own_file = asset(CATEGORIES[13], COLOR_MODES[0], EXTENSIONS[2], b"logo", None);
    let (other_pool, _, customized) = style(
        &router,
        "partly-customized",
        &json!({ "Assets": [own_file.clone()] }),
    )
    .await;
    let partly_merged = ok(
        &router,
        "DescribeManagedLoginBranding",
        &json!({
            "UserPoolId": other_pool,
            "ManagedLoginBrandingId": customized["ManagedLoginBrandingId"],
            "ReturnMergedResources": true,
        }),
    )
    .await;
    let partly_merged = &partly_merged["ManagedLoginBranding"];
    let merged_files = assets("a partly customized style", "the merged view", partly_merged);
    assert_eq!(
        merged_files.len(),
        files.len(),
        "the merged view holds one file per role, whoever supplied it, got {partly_merged}"
    );
    let own_slot = format!(
        "{}/{}",
        own_file["Category"].as_str().unwrap_or_default(),
        own_file["ColorMode"].as_str().unwrap_or_default()
    );
    let held = merged_files
        .iter()
        .find(|file| {
            format!(
                "{}/{}",
                file["Category"].as_str().unwrap_or_default(),
                file["ColorMode"].as_str().unwrap_or_default()
            ) == own_slot
        })
        .unwrap_or_else(|| panic!("the merged view holds the role {own_slot}, got {partly_merged}"));
    assert_eq!(
        held, &own_file,
        "the merged view must hold the caller's own file for the role it filled, got {held}"
    );
}

/// One row of the domain table: the `CreateUserPoolDomain` members a row sends
/// and the status the created domain reports.
struct DomainCase {
    case: &'static str,
    domain: String,
    custom_domain_config: Option<Value>,
    routing: Option<Value>,
    managed_login_version: Option<i32>,
}

impl DomainCase {
    /// This row's create request for `user_pool_id`, with every member the row
    /// leaves unset left out of it.
    fn request(&self, user_pool_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("Domain".to_string(), json!(self.domain));
        if let Some(config) = &self.custom_domain_config {
            body.insert("CustomDomainConfig".to_string(), config.clone());
        }
        if let Some(routing) = &self.routing {
            body.insert("Routing".to_string(), routing.clone());
        }
        if let Some(version) = self.managed_login_version {
            body.insert("ManagedLoginVersion".to_string(), json!(version));
        }
        Value::Object(body)
    }

    /// Whether this row's domain is a CUSTOM one, which is exactly "the create
    /// carried a `CustomDomainConfig`".
    fn is_custom(&self) -> bool {
        self.custom_domain_config.is_some()
    }
}

/// The table. The rows differ along what the rule generalises over: the kind of
/// domain, the branding version it is on, the optional members the create sets,
/// and the length and form of the domain string (both ends of `DomainType`'s
/// modelled length 1..63). Members from
/// `smithy_query(query="operation",operation="CreateUserPoolDomain")`.
fn domain_cases() -> Vec<DomainCase> {
    vec![
        DomainCase {
            case: "a prefix domain with every optional member unset",
            domain: "generated-prefix".to_string(),
            custom_domain_config: None,
            routing: None,
            managed_login_version: None,
        },
        DomainCase {
            case: "a prefix domain on managed login branding",
            domain: "generated-managed".to_string(),
            custom_domain_config: None,
            routing: None,
            managed_login_version: Some(2),
        },
        DomainCase {
            case: "a prefix domain of one character, the length minimum",
            domain: "g".to_string(),
            custom_domain_config: None,
            routing: None,
            managed_login_version: Some(1),
        },
        DomainCase {
            case: "a prefix domain of 63 characters, the length maximum",
            domain: format!("g{}7", "e".repeat(61)),
            custom_domain_config: None,
            routing: None,
            managed_login_version: None,
        },
        DomainCase {
            case: "a custom domain with the certificate alone",
            domain: "generated-custom".to_string(),
            custom_domain_config: Some(json!({ "CertificateArn": CERTIFICATE })),
            routing: None,
            managed_login_version: None,
        },
        DomainCase {
            case: "a custom domain with a security policy and a failover routing",
            domain: "generated-custom-routed".to_string(),
            custom_domain_config: Some(json!({
                "CertificateArn": CERTIFICATE,
                "SecurityPolicy": "TLS_V1_2_2021",
            })),
            routing: Some(json!({
                "Failover": {
                    "SecondaryRegion": "us-west-2",
                    "PrimaryRoute53HealthCheckId": "abcdef01-2345-6789-abcd-ef0123456789",
                },
            })),
            managed_login_version: Some(2),
        },
    ]
}

#[tokio::test]
async fn every_hosted_domain_reports_the_values_the_service_generates_for_it() {
    let router = router();
    let mut buckets = BTreeSet::new();

    for (index, case) in domain_cases().into_iter().enumerate() {
        let label = case.case;
        // A pool per row: a pool holds at most one prefix and at most one custom
        // domain.
        let user_pool_id = pool(&router, &format!("generated-domain-{index}")).await;
        ok(
            &router,
            "CreateUserPoolDomain",
            &case.request(&user_pool_id),
        )
        .await;

        let read = ok(
            &router,
            "DescribeUserPoolDomain",
            &json!({ "Domain": case.domain }),
        )
        .await;
        let description = &read["DomainDescription"];

        let bucket = description["S3Bucket"].as_str().unwrap_or_else(|| {
            panic!("case {label:?}: a hosted domain reports its file bucket, got {read}")
        });
        assert!(
            (BUCKET_LENGTH.0..=BUCKET_LENGTH.1).contains(&bucket.len())
                && !bucket.ends_with('.')
                && bucket
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')),
            "case {label:?}: the bucket must be a bucket name, got {bucket:?}"
        );
        buckets.insert(bucket.to_string());

        let distribution = description["CloudFrontDistribution"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {label:?}: a hosted domain reports its CloudFront endpoint, got {read}")
            });
        assert!(
            distribution.ends_with(".cloudfront.net"),
            "case {label:?}: the endpoint must be a CloudFront FQDN, got {distribution:?}"
        );

        let version = description["Version"].as_str().unwrap_or_else(|| {
            panic!("case {label:?}: a hosted domain reports its app version, got {read}")
        });
        assert!(
            (VERSION_LENGTH.0..=VERSION_LENGTH.1).contains(&version.len())
                && !version.is_empty()
                && version.chars().all(char::is_numeric),
            "case {label:?}: the version must be a version stamp inside the modelled length, got \
             {version:?}"
        );

        // The status is the one the domain's OWN provisioning is in: a custom
        // domain has a distribution to provision, a prefix domain does not.
        let expected_status = if case.is_custom() { "CREATING" } else { "ACTIVE" };
        assert_eq!(
            description["Status"],
            json!(expected_status),
            "case {label:?}: a created domain reports the status its provisioning is in, got {read}"
        );

        // Reading the domain again answers the same generated values: a read
        // never mints a second set, and never moves what a later caller sees.
        let again = ok(
            &router,
            "DescribeUserPoolDomain",
            &json!({ "Domain": case.domain }),
        )
        .await;
        assert_eq!(
            again["DomainDescription"], *description,
            "case {label:?}: a read must answer the stored values, got {again}"
        );

        tokio::time::sleep(PAST_THE_ACTIVATION).await;
        let settled = ok(
            &router,
            "DescribeUserPoolDomain",
            &json!({ "Domain": case.domain }),
        )
        .await;
        let settled = &settled["DomainDescription"];
        assert_eq!(
            settled["Status"],
            json!("ACTIVE"),
            "case {label:?}: a domain is ACTIVE once the clock owes it, got {settled}"
        );
        for member in ["S3Bucket", "CloudFrontDistribution", "Version"] {
            assert_eq!(
                settled[member], description[member],
                "case {label:?}: reaching ACTIVE must not move the domain's {member}, got {settled}"
            );
        }
    }

    // The bucket belongs to the service rather than to one domain, so every
    // domain of the Region reports the same one.
    assert_eq!(
        buckets.len(),
        1,
        "every domain of one Region is served from the same bucket, got {buckets:?}"
    );
}

#[tokio::test]
async fn a_domain_the_service_holds_none_of_reports_nothing_generated() {
    let router = router();
    // The reader answers 200 with an empty description for a name no pool holds
    // (`DescribeUserPoolDomain` is answered for any name), so no generated value
    // is minted for a domain that does not exist.
    let (status, payload) = call(
        &router,
        "DescribeUserPoolDomain",
        &json!({ "Domain": "generated-absent" }),
    )
    .await;
    assert_eq!(
        (status, payload["DomainDescription"].clone()),
        (200, json!({})),
        "an absent domain reports no description at all, got {payload}"
    );
}
