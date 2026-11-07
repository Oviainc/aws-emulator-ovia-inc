// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListSecurityControlDefinitions`: the published
//! security-control catalog, the `StandardsArn` and `Providers` selections,
//! `MaxResults`/`NextToken` paging, and how a customizable control reports its
//! parameters.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized member that is wrong on the wire fails here.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_securityhub_securityhubapiservice::ops::types::{
    REGION_AVAILABILITY_STATUS_VALUES, SECURITY_CONTROLS_PROVIDER_VALUES,
    SECURITY_CONTROL_PROPERTY_VALUES, SEVERITY_RATING_VALUES,
};
use service_com_amazonaws_securityhub_securityhubapiservice::state::{
    standard_arn, standard_controls, standards_control_remediation_url, ParameterConfiguration,
    SecurityControlCatalogEntry, SECURITY_CONTROL_CATALOG, STANDARDS_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

/// One emulator with the Security Hub service mounted, driven over HTTP, signed for
/// one Region. The handler reads the Region out of the SigV4 credential scope,
/// exactly as a signed SDK request carries it.
struct Harness {
    router: MultiServiceRouter,
    region: String,
}

impl Harness {
    fn new(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_securityhub_securityhubapiservice::EmulatedService::new(engine),
        );
        Self {
            router,
            region: region.to_string(),
        }
    }

    /// An emulator whose hub is already subscribed in `region`.
    async fn subscribed(region: &str) -> Self {
        let harness = Self::new(region);
        let (status, body) = harness
            .call("POST", "/accounts", Some(json!({})))
            .await;
        assert_eq!(
            status, 200,
            "region {region:?}: EnableSecurityHub must succeed, got {status} with {body}"
        );
        harness
    }

    /// Dispatch one request signed for this harness' Region, and return
    /// `(status, parsed body)`.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let region = self.region.clone();
        let serialized = body.map(|b| serde_json::to_vec(&b).expect("serializable request body"));
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{region}/securityhub/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(serialized.unwrap_or_default()))
            .expect("valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `ListSecurityControlDefinitions` with `query` appended to the path (without the
    /// leading `?`, which this adds when the query is non-empty).
    async fn list(&self, query: &str) -> (u16, Value) {
        let path = if query.is_empty() {
            "/securityControls/definitions".to_string()
        } else {
            format!("/securityControls/definitions?{query}")
        };
        self.call("GET", &path, None).await
    }
}

/// The `SecurityControlDefinitions` entries of a response, in the order it listed them.
fn returned_definitions(body: &Value) -> Vec<Value> {
    body["SecurityControlDefinitions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `SecurityControlId`s of a response, in the order it listed them.
fn returned_ids(body: &Value) -> Vec<String> {
    returned_definitions(body)
        .iter()
        .map(|definition| {
            definition["SecurityControlId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every published control id, in catalog order.
fn catalog_ids() -> Vec<String> {
    SECURITY_CONTROL_CATALOG
        .iter()
        .map(|entry| entry.control.control_id.to_string())
        .collect()
}

/// The published controls whose `Provider` is one of `providers`, in catalog order.
fn catalog_ids_for_providers(providers: &[&str]) -> Vec<String> {
    SECURITY_CONTROL_CATALOG
        .iter()
        .filter(|entry| providers.contains(&entry.provider))
        .map(|entry| entry.control.control_id.to_string())
        .collect()
}

/// The response entry reported for `security_control_id`, if any.
fn entry_for(body: &Value, security_control_id: &str) -> Option<Value> {
    returned_definitions(body)
        .into_iter()
        .find(|definition| definition["SecurityControlId"].as_str() == Some(security_control_id))
}

/// The published catalog entry for `security_control_id`.
fn catalog_entry(security_control_id: &str) -> &'static SecurityControlCatalogEntry {
    SECURITY_CONTROL_CATALOG
        .iter()
        .find(|entry| entry.control.control_id == security_control_id)
        .expect("a published control id")
}

/// Walk every page of `query` and return the concatenated `SecurityControlId`s,
/// asserting that no page exceeds `page_size` and that only the final page omits
/// `NextToken`.
async fn page_through(harness: &Harness, query: &str, page_size: usize) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut full = format!("MaxResults={page_size}");
        if !query.is_empty() {
            full = format!("{query}&{full}");
        }
        if let Some(token) = token.as_ref() {
            full = format!("{full}&NextToken={token}");
        }
        let (status, body) = harness.list(&full).await;
        assert_eq!(
            status, 200,
            "query {full:?}: expected 200, got {status} with {body}"
        );
        let page = returned_ids(&body);
        assert!(
            page.len() <= page_size,
            "query {full:?}: a page must hold at most {page_size}, got {}",
            page.len()
        );
        seen.extend(page);
        pages += 1;
        assert!(
            pages <= SECURITY_CONTROL_CATALOG.len() + 1,
            "query {full:?}: paging did not terminate after {pages} pages"
        );
        match body["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    seen
}

/// RULE: `ListSecurityControlDefinitions` reports the security controls the service
/// publishes in the request's Region, each under its standard-agnostic id and with the
/// title, description, remediation link, severity rating, Region availability and
/// cloud provider the catalog records for it, in one stable published order.
///
/// The table spans Regions in three geographies: the control catalog is service
/// metadata, so it cannot vary with the Region the request was signed for.
#[tokio::test]
async fn the_published_security_control_catalog_is_reported_in_full() {
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let harness = Harness::subscribed(region).await;
        let (status, body) = harness.list("MaxResults=100").await;
        assert_eq!(
            status, 200,
            "region {region:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            returned_ids(&body),
            catalog_ids(),
            "region {region:?}: expected the whole published catalog in order, got {body}"
        );
        assert!(
            body["NextToken"].is_null(),
            "region {region:?}: a page holding the whole catalog must carry no NextToken, got {body}"
        );

        for entry in SECURITY_CONTROL_CATALOG.iter() {
            let id = entry.control.control_id;
            let reported = entry_for(&body, id)
                .unwrap_or_else(|| panic!("region {region:?}: control {id} must be reported"));
            assert_eq!(
                reported["Title"].as_str(),
                Some(entry.control.title),
                "control {id}: unexpected Title in {reported}"
            );
            assert_eq!(
                reported["Description"].as_str(),
                Some(entry.control.description),
                "control {id}: unexpected Description in {reported}"
            );
            assert_eq!(
                reported["RemediationUrl"].as_str(),
                Some(standards_control_remediation_url(id).as_str()),
                "control {id}: a remediation link must carry the control's own id, got {reported}"
            );
            assert_eq!(
                reported["SeverityRating"].as_str(),
                Some(entry.control.severity_rating),
                "control {id}: unexpected SeverityRating in {reported}"
            );
            assert_eq!(
                reported["Provider"].as_str(),
                Some(entry.provider),
                "control {id}: unexpected Provider in {reported}"
            );
            assert_eq!(
                reported["CurrentRegionAvailability"].as_str(),
                Some("AVAILABLE"),
                "control {id}: a published control is available in the Region it is asked for, got {reported}"
            );
        }
    }
}

/// RULE: every reported definition satisfies the members the model requires of a
/// `SecurityControlDefinition` — a non-blank id, title, description and remediation
/// link (`NonEmptyString`, `@pattern("\\S")`), a `SeverityRating`, a
/// `RegionAvailabilityStatus`, a `SecurityControlsProvider`, and an id unique across
/// the catalog, reported in ascending id order.
#[tokio::test]
async fn every_reported_definition_satisfies_the_modelled_constraints() {
    let harness = Harness::new("ca-central-1");
    let (status, body) = harness.list("MaxResults=100").await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");

    let definitions = returned_definitions(&body);
    assert!(
        !definitions.is_empty(),
        "the service must publish at least one security control, got {body}"
    );
    for definition in &definitions {
        for member in [
            "SecurityControlId",
            "Title",
            "Description",
            "RemediationUrl",
        ] {
            let value = definition[member].as_str().unwrap_or_default();
            assert!(
                value.chars().any(|c| !c.is_whitespace()),
                "member {member} must be a non-blank string, got {definition}"
            );
        }
        let severity = definition["SeverityRating"].as_str().unwrap_or_default();
        assert!(
            SEVERITY_RATING_VALUES.contains(&severity),
            "SeverityRating {severity:?} is not a modelled value in {definition}"
        );
        let availability = definition["CurrentRegionAvailability"]
            .as_str()
            .unwrap_or_default();
        assert!(
            REGION_AVAILABILITY_STATUS_VALUES.contains(&availability),
            "CurrentRegionAvailability {availability:?} is not a modelled value in {definition}"
        );
        let provider = definition["Provider"].as_str().unwrap_or_default();
        assert!(
            SECURITY_CONTROLS_PROVIDER_VALUES.contains(&provider),
            "Provider {provider:?} is not a modelled value in {definition}"
        );
    }

    let ids = returned_ids(&body);
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "a control id must name one definition, got {ids:?}"
    );
    let mut ascending = ids.clone();
    ascending.sort();
    assert_eq!(
        ids, ascending,
        "definitions must be reported in ascending SecurityControlId order, got {ids:?}"
    );
}

/// RULE: `Providers` selects the published controls whose `Provider` it names, and
/// naming no provider constrains nothing.
///
/// The table is driven by `SecurityControlsProvider`'s own value list, so every
/// modelled provider is exercised, together with the empty filter and the full list.
#[tokio::test]
async fn providers_selects_the_controls_of_the_providers_it_names() {
    let harness = Harness::subscribed("eu-central-1").await;

    // (case, query, expected ids)
    let mut cases: Vec<(String, String, Vec<String>)> = vec![
        (
            "omitted".to_string(),
            "MaxResults=100".to_string(),
            catalog_ids(),
        ),
        (
            "every provider".to_string(),
            format!(
                "{}&MaxResults=100",
                SECURITY_CONTROLS_PROVIDER_VALUES
                    .iter()
                    .map(|provider| format!("Providers={provider}"))
                    .collect::<Vec<_>>()
                    .join("&")
            ),
            catalog_ids_for_providers(SECURITY_CONTROLS_PROVIDER_VALUES),
        ),
    ];
    for provider in SECURITY_CONTROLS_PROVIDER_VALUES {
        cases.push((
            (*provider).to_string(),
            format!("Providers={provider}&MaxResults=100"),
            catalog_ids_for_providers(&[provider]),
        ));
    }

    for (case, query, expected) in cases {
        let (status, body) = harness.list(&query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            returned_ids(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
        assert!(
            !expected.is_empty(),
            "case {case:?}: every modelled provider must publish at least one control"
        );
    }

    // The unfiltered catalog is the union of the per-provider selections, so no
    // control is reachable only without a filter.
    let mut union: Vec<String> = SECURITY_CONTROLS_PROVIDER_VALUES
        .iter()
        .flat_map(|provider| catalog_ids_for_providers(&[provider]))
        .collect();
    union.sort();
    let mut all = catalog_ids();
    all.sort();
    assert_eq!(
        union, all,
        "every published control must be selectable by its own provider"
    );
}

/// RULE: a `StandardsArn` narrows the listing to the security controls that standard
/// applies — the controls its published control set addresses by the control's own id
/// — and the selection composes with `Providers`.
///
/// Three rows name the expected control ids outright, from the control sets
/// `STANDARDS_CONTROL_CATALOG` publishes for those standards (an AWS standard, the
/// tagging standard whose controls are the customizable ones, and an Azure standard
/// the issue did not cite); the loop then spans EVERY published standard, including
/// both legacy revisions that address their controls by their own requirement ids.
#[tokio::test]
async fn a_standards_arn_selects_the_controls_that_standard_applies() {
    let region = "us-west-2";
    let harness = Harness::subscribed(region).await;

    let named_rows: [(&str, &str, &[&str]); 3] = [
        (
            "aws-foundational-security-best-practices",
            "1.0.0",
            &[
                "ACM.1",
                "CloudTrail.1",
                "CloudTrail.2",
                "CloudWatch.1",
                "Config.1",
                "EC2.2",
                "GuardDuty.1",
                "IAM.4",
                "S3.1",
            ],
        ),
        (
            "aws-resource-tagging-standard",
            "1.0.0",
            &["EC2.52", "ECS.13"],
        ),
        (
            "azure-foundational-security-best-practices",
            "1.0.0",
            &[
                "AzureKeyVault.1",
                "AzureSQL.1",
                "AzureStorage.1",
                "AzureVirtualMachine.1",
            ],
        ),
    ];
    for (slug, version, expected) in named_rows {
        let definition = STANDARDS_CATALOG
            .iter()
            .find(|candidate| candidate.slug == slug && candidate.version == version)
            .expect("a published standard");
        let arn = standard_arn(region, definition);
        let (status, body) = harness
            .list(&format!("StandardsArn={arn}&MaxResults=100"))
            .await;
        assert_eq!(
            status, 200,
            "standard {slug}/{version}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            returned_ids(&body),
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "standard {slug}/{version}: expected {expected:?}, got {body}"
        );
    }

    for definition in STANDARDS_CATALOG.iter() {
        let arn = standard_arn(region, definition);
        let (status, body) = harness
            .list(&format!("StandardsArn={arn}&MaxResults=100"))
            .await;
        assert_eq!(
            status, 200,
            "standard {arn}: expected 200, got {status} with {body}"
        );
        let reported = returned_ids(&body);
        let published: Vec<String> = SECURITY_CONTROL_CATALOG
            .iter()
            .filter(|entry| {
                standard_controls(definition)
                    .iter()
                    .any(|control| control.control_id == entry.control.control_id)
            })
            .map(|entry| entry.control.control_id.to_string())
            .collect();
        assert_eq!(
            reported, published,
            "standard {arn}: expected the controls it applies, got {body}"
        );
        for id in &reported {
            assert!(
                catalog_ids().contains(id),
                "standard {arn}: {id} is not a published security control, got {body}"
            );
        }
    }

    // The selections compose: an Azure standard filtered to AWS controls selects
    // none, and the same standard filtered to its own provider selects all of its.
    let azure = STANDARDS_CATALOG
        .iter()
        .find(|candidate| candidate.slug == "azure-foundational-security-best-practices")
        .expect("a published Azure standard");
    let arn = standard_arn(region, azure);
    let (status, body) = harness
        .list(&format!("StandardsArn={arn}&Providers=AWS&MaxResults=100"))
        .await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");
    assert!(
        returned_ids(&body).is_empty(),
        "an Azure standard holds no AWS-provider control, got {body}"
    );
    let (status, body) = harness
        .list(&format!("StandardsArn={arn}&Providers=Azure&MaxResults=100"))
        .await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");
    assert_eq!(
        returned_ids(&body).len(),
        4,
        "the Azure standard's own provider selects all of its controls, got {body}"
    );
}

/// RULE: `MaxResults` bounds a page and `NextToken` resumes the listing where the
/// previous page stopped, so walking the pages yields every selected control exactly
/// once in catalog order; a request that names no `MaxResults` is answered with the
/// documented default page of 25.
///
/// The table spans page sizes that divide the catalog, that do not, that equal it and
/// that exceed it — plus the published `MaxResults` bounds.
#[tokio::test]
async fn paging_truncates_at_max_results_and_resumes_from_next_token() {
    let harness = Harness::subscribed("us-east-2").await;
    let expected = catalog_ids();
    let total = expected.len();

    for page_size in [1usize, 2, 3, total - 1, total, total + 1, 100] {
        let seen = page_through(&harness, "", page_size).await;
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "page size {page_size}: no control may repeat across pages, got {seen:?}"
        );
        assert_eq!(
            seen, expected,
            "page size {page_size}: expected the whole catalog in order, got {seen:?}"
        );
    }

    // Paging composes with the filter: the filtered listing pages over just its own
    // selection.
    let azure_only = catalog_ids_for_providers(&["Azure"]);
    let seen = page_through(&harness, "Providers=Azure", 2).await;
    assert_eq!(
        seen, azure_only,
        "a filtered listing must page over just its own selection, got {seen:?}"
    );

    // The documented default page: 25 controls, which is more than the catalog holds,
    // so the first page is the whole catalog and carries no cursor.
    let (status, body) = harness.list("").await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");
    assert!(
        total <= 25,
        "this case assumes a catalog no larger than the documented default page"
    );
    assert_eq!(
        returned_ids(&body),
        expected,
        "a request naming no MaxResults gets the first 25 controls, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "a default page holding every control must carry no NextToken, got {body}"
    );
}

/// RULE: `MaxResults` is `@range(min: 1, max: 100)`, so each published bound is
/// accepted and a value one past either bound is refused (400); a `NextToken` this
/// operation did not issue, or one pointing past the end of the selection, is a client
/// error rather than an empty page; a `Providers` value `SecurityControlsProvider`
/// does not define is refused; and a `StandardsArn` no published standard carries in
/// the request's Region names no standard to list.
#[tokio::test]
async fn out_of_range_and_unresolvable_inputs_are_refused() {
    let region = "us-east-1";
    let harness = Harness::subscribed(region).await;

    // A token this operation really issued, resuming after the first control.
    let (status, body) = harness.list("MaxResults=1").await;
    assert_eq!(status, 200, "first page: got {status} with {body}");
    let issued = body["NextToken"]
        .as_str()
        .expect("a catalog larger than one page must carry a NextToken")
        .to_string();

    // A token issued against the whole catalog whose offset is past the end of the
    // narrower Azure selection, so resuming the narrower listing from it points past
    // its end.
    let azure_total = catalog_ids_for_providers(&["Azure"]).len();
    let (status, body) = harness
        .list(&format!("MaxResults={}", azure_total + 1))
        .await;
    assert_eq!(status, 200, "wide page: got {status} with {body}");
    let past_the_end = body["NextToken"]
        .as_str()
        .expect("the catalog must be larger than the Azure selection")
        .to_string();

    // A standard of another Region: a standard ARN carries the Region it was asked
    // for, so one built for another Region resolves to no standard here.
    let other_region_arn = standard_arn(
        "eu-west-1",
        STANDARDS_CATALOG
            .iter()
            .find(|candidate| candidate.slug == "aws-foundational-security-best-practices")
            .expect("a published standard"),
    );

    let refused: [(&str, String); 6] = [
        ("MaxResults below the minimum", "MaxResults=0".to_string()),
        ("MaxResults above the maximum", "MaxResults=101".to_string()),
        (
            "a NextToken this operation never issued",
            "NextToken=not-a-token".to_string(),
        ),
        (
            "a NextToken pointing past the end of a narrower selection",
            format!("Providers=Azure&NextToken={past_the_end}&MaxResults=1"),
        ),
        (
            "a provider the enum does not define",
            "Providers=GCP".to_string(),
        ),
        (
            "a standard of another Region",
            format!("StandardsArn={other_region_arn}"),
        ),
    ];
    for (case, query) in refused {
        let (status, body) = harness.list(&query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
    }

    // The token this operation issued resumes the listing it was issued for.
    let (status, body) = harness.list(&format!("NextToken={issued}&MaxResults=1")).await;
    assert_eq!(status, 200, "resumed page: got {status} with {body}");
    assert_eq!(
        returned_ids(&body),
        vec![catalog_ids()[1].clone()],
        "a token resumes where the previous page stopped, got {body}"
    );

    // The published bounds themselves are accepted.
    for page_size in [1, 100] {
        let (status, body) = harness.list(&format!("MaxResults={page_size}")).await;
        assert_eq!(
            status, 200,
            "MaxResults={page_size}: expected 200, got {status} with {body}"
        );
    }

    // An ARN that names no standard at all is refused by naming the ARN back.
    let unknown = "arn:aws:securityhub:::standards/no-such-standard/v/1.0.0";
    let (status, body) = harness.list(&format!("StandardsArn={unknown}")).await;
    assert_eq!(status, 400, "expected 400, got {status} with {body}");
    let message = body["Message"].as_str().unwrap_or_default();
    assert!(
        message.contains(unknown),
        "the refusal must name the ARN the request carried, got {body}"
    );
}

/// RULE: a control that supports custom parameters reports `Parameters` as a
/// customizable property and one `ParameterDefinitions` entry per parameter, carrying
/// that parameter's description and the options a custom value must satisfy; a control
/// that supports none reports an EMPTY `CustomizableProperties` and no
/// `ParameterDefinitions` at all.
///
/// The case list is the whole published catalog, so both the customizable controls and
/// the ones that support nothing are covered, whichever side a control is on.
#[tokio::test]
async fn customizable_controls_report_their_parameter_definitions() {
    let harness = Harness::new("eu-west-2");
    let (status, body) = harness.list("MaxResults=100").await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");

    let mut customizable_seen = 0usize;
    let mut plain_seen = 0usize;
    for entry in SECURITY_CONTROL_CATALOG.iter() {
        let id = entry.control.control_id;
        let reported =
            entry_for(&body, id).unwrap_or_else(|| panic!("control {id} must be reported"));
        let properties = reported["CustomizableProperties"]
            .as_array()
            .unwrap_or_else(|| panic!("control {id}: CustomizableProperties must be an array"))
            .clone();
        if entry.parameters.is_empty() {
            plain_seen += 1;
            assert!(
                properties.is_empty(),
                "control {id}: a control with no custom property reports an empty array, got {reported}"
            );
            assert!(
                reported.get("ParameterDefinitions").is_none(),
                "control {id}: ParameterDefinitions is excluded for a control without parameters, got {reported}"
            );
            continue;
        }

        customizable_seen += 1;
        assert_eq!(
            properties,
            vec![Value::String(
                SECURITY_CONTROL_PROPERTY_VALUES[0].to_string()
            )],
            "control {id}: a customizable control reports its one customizable property, got {reported}"
        );
        let definitions = reported["ParameterDefinitions"]
            .as_object()
            .unwrap_or_else(|| panic!("control {id}: ParameterDefinitions must be a map"))
            .clone();
        assert_eq!(
            definitions.len(),
            entry.parameters.len(),
            "control {id}: expected one entry per parameter, got {reported}"
        );
        for parameter in entry.parameters {
            let reported_parameter = definitions
                .get(parameter.name)
                .unwrap_or_else(|| panic!("control {id}: parameter {} must be reported", parameter.name));
            assert_eq!(
                reported_parameter["Description"].as_str(),
                Some(parameter.description),
                "control {id}: unexpected description for {}, got {reported_parameter}",
                parameter.name
            );
            let options = &reported_parameter["ConfigurationOptions"];
            match parameter.configuration {
                ParameterConfiguration::Integer {
                    default_value,
                    min,
                    max,
                } => {
                    let integer = &options["Integer"];
                    assert_eq!(
                        (
                            integer["DefaultValue"].as_i64(),
                            integer["Min"].as_i64(),
                            integer["Max"].as_i64()
                        ),
                        (
                            Some(i64::from(default_value)),
                            Some(i64::from(min)),
                            Some(i64::from(max))
                        ),
                        "control {id}: unexpected integer options for {}, got {options}",
                        parameter.name
                    );
                }
                ParameterConfiguration::StringList {
                    max_items,
                    re2_expression,
                    expression_description,
                } => {
                    let list = &options["StringList"];
                    assert_eq!(
                        list["MaxItems"].as_i64(),
                        Some(i64::from(max_items)),
                        "control {id}: unexpected MaxItems for {}, got {options}",
                        parameter.name
                    );
                    assert_eq!(
                        list["Re2Expression"].as_str(),
                        Some(re2_expression),
                        "control {id}: unexpected Re2Expression for {}, got {options}",
                        parameter.name
                    );
                    assert_eq!(
                        list["ExpressionDescription"].as_str(),
                        Some(expression_description),
                        "control {id}: unexpected ExpressionDescription for {}, got {options}",
                        parameter.name
                    );
                    assert!(
                        list.get("DefaultValue").is_none(),
                        "control {id}: a parameter with no default reports none, got {options}"
                    );
                }
            }
        }
    }
    assert!(
        customizable_seen > 0 && plain_seen > 0,
        "the catalog must cover both customizable ({customizable_seen}) and plain ({plain_seen}) controls"
    );

    // The parameters a control publishes are the ones named for it, not a catalog-wide
    // set: a customizable control and a plain one differ in the response.
    let tagging = catalog_entry("EC2.52");
    assert!(
        !tagging.parameters.is_empty(),
        "a tagging control is customizable"
    );
    assert!(
        catalog_entry("IAM.4").parameters.is_empty(),
        "a control the reference publishes no parameter for carries none"
    );
}

/// RULE: the control catalog is the service's own published metadata, so it is served
/// to every caller — an account with no hub included — and enabling or disabling a hub
/// does not change it.
#[tokio::test]
async fn the_catalog_is_served_whatever_the_account_has_enabled() {
    let region = "ap-south-1";
    let queries = [
        "MaxResults=100",
        "Providers=AWS&MaxResults=100",
        "MaxResults=2",
    ];

    let unsubscribed = Harness::new(region);
    let mut before: Vec<(String, Value)> = Vec::new();
    for query in queries {
        let (status, body) = unsubscribed.list(query).await;
        assert_eq!(
            status, 200,
            "query {query:?} without a hub: expected 200, got {status} with {body}"
        );
        before.push((query.to_string(), body));
    }

    let subscribed = Harness::subscribed(region).await;
    for (query, expected) in &before {
        let (status, body) = subscribed.list(query).await;
        assert_eq!(
            status, 200,
            "query {query:?} with a hub: expected 200, got {status} with {body}"
        );
        assert_eq!(
            &body, expected,
            "query {query:?}: the published catalog must not depend on the hub, got {body}"
        );
    }

    // And after the hub is gone again.
    let (status, body) = subscribed.call("DELETE", "/accounts", None).await;
    assert_eq!(
        status, 200,
        "DisableSecurityHub must succeed, got {status} with {body}"
    );
    for (query, expected) in &before {
        let (status, body) = subscribed.list(query).await;
        assert_eq!(
            status, 200,
            "query {query:?} after disable: expected 200, got {status} with {body}"
        );
        assert_eq!(
            &body, expected,
            "query {query:?}: disabling the hub must not change the catalog, got {body}"
        );
    }
}
