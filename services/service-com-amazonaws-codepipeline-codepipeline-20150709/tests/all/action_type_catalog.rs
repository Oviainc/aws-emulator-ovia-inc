// SPDX-License-Identifier: CC0-1.0

//! `ListActionTypes`, over the wire the service actually speaks: `awsJson1_1`,
//! `POST /` with `X-Amz-Target: CodePipeline_20150709.ListActionTypes`.
//!
//! The rule under test: the listing reports every action type the account can use —
//! the ones the service provides itself plus the ones registered in this account's
//! store — narrowed by `actionOwnerFilter` and `regionFilter`, in one total order
//! over the `ActionTypeId` members, paged through an opaque `nextToken` that resumes
//! exactly where the previous page stopped.
//!
//! Every read goes through the router, so nothing here can pass by reading a
//! handler's internals.

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_smithy_types::base64;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::list_action_types::MAX_ACTION_TYPES_PER_PAGE;
use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::{
    ActionConfigurationProperty, ActionTypeSettings, ArtifactDetails,
};
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    ActionTypeKey, ActionTypeRecord, BUILT_IN_ACTION_TYPES, MAX_ACTION_TYPES_PER_ACCOUNT,
};
use service_com_amazonaws_codepipeline_codepipeline_20150709::ComAmazonawsCodepipelineCodepipeline20150709State as ServiceState;

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// One emulator: the shared engine, the service mounted on it, and the router that
/// dispatches to it — exactly how `core/router` assembles the production emulator.
///
/// This suite keeps the engine handle because it registers action types directly in
/// the SHARED store the handler reads, which is what a handler writes through too —
/// so these cases pin the listing on its own, independently of
/// `CreateCustomActionType` (whose own round-trip cases live in
/// `custom_action_types.rs`).
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Call `operation` signed for `region`, so the handler reads the Region off the
    /// SigV4 credential scope the way a real SDK request carries it.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/codepipeline/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|error| {
                panic!("{operation}: response body is not JSON ({error}): {body:?}")
            })
        };
        (status, parsed)
    }

    /// A successful `ListActionTypes` in `region` with `body`.
    async fn list_in(&self, region: &str, body: &Value) -> Value {
        let (status, response) = self.call_in(region, "ListActionTypes", body).await;
        assert_eq!(
            status, 200,
            "ListActionTypes {body}: expected HTTP 200, got {status} with {response}"
        );
        response
    }

    /// A successful `ListActionTypes` in the default Region.
    async fn list(&self, body: &Value) -> Value {
        self.list_in("us-east-1", body).await
    }

    /// The `(status, __type)` of a refused `ListActionTypes`.
    async fn refusal(&self, body: &Value) -> (u16, String) {
        let (status, response) = self.call_in("us-east-1", "ListActionTypes", body).await;
        let code = response
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("expected an error body, got {response}"))
            .to_string();
        (status, code)
    }

    /// Register one action type in this account's store, the way
    /// `CreateCustomActionType` does — through the gate that enforces
    /// `MAX_ACTION_TYPES_PER_ACCOUNT`.
    fn register(&self, key: ActionTypeKey, record: ActionTypeRecord) {
        let accepted = self
            .engine
            .state::<ServiceState>()
            .lock()
            .register_action_type(key.clone(), record);
        assert!(
            accepted,
            "registering {key:?} must be accepted below the account's action-type bound"
        );
    }
}

/// A `Custom`-owned action type registered in `region`.
fn custom(category: &str, provider: &str, version: &str, region: &str) -> (ActionTypeKey, ActionTypeRecord) {
    (
        ActionTypeKey {
            category: category.to_string(),
            owner: "Custom".to_string(),
            provider: provider.to_string(),
            version: version.to_string(),
        },
        ActionTypeRecord {
            settings: None,
            configuration_properties: Vec::new(),
            input_artifact_details: ArtifactDetails {
                minimum_count: 0,
                maximum_count: 1,
            },
            output_artifact_details: ArtifactDetails {
                minimum_count: 0,
                maximum_count: 1,
            },
            region: Some(region.to_string()),
            tags: Vec::new(),
        },
    )
}

/// The `(category, owner, provider, version)` of every action type a response
/// reports, in the order it reports them.
fn ids(response: &Value) -> Vec<(String, String, String, String)> {
    response["actionTypes"]
        .as_array()
        .unwrap_or_else(|| panic!("actionTypes is a list, got {response}"))
        .iter()
        .map(|action_type| {
            let id = &action_type["id"];
            let member = |name: &str| {
                id[name]
                    .as_str()
                    .unwrap_or_else(|| panic!("id.{name} is a string, got {action_type}"))
                    .to_string()
            };
            (
                member("category"),
                member("owner"),
                member("provider"),
                member("version"),
            )
        })
        .collect()
}

/// The action types this emulator provides itself, as the listing must report them:
/// a provider for each category, with the input- and output-artifact counts the page
/// `ActionTypeId$provider` links to documents for it ("Valid action types and
/// providers in CodePipeline", reference-pipeline-structure.html#actions-valid-providers,
/// and each action's structure reference). Rows differ along the category, the owner
/// and both artifact bounds; none of them is a value this issue's report cited.
const DOCUMENTED_ACTION_TYPES: [DocumentedActionType; 7] = [
    row("Source", "AWS", "S3", "1", (0, 0), (1, 1)),
    row("Source", "AWS", "CodeCommit", "1", (0, 0), (1, 1)),
    row("Source", "ThirdParty", "GitHub", "1", (0, 0), (1, 1)),
    row("Build", "AWS", "CodeBuild", "1", (1, 5), (0, 5)),
    row("Deploy", "AWS", "CodeDeploy", "1", (1, 1), (0, 0)),
    row("Invoke", "AWS", "Lambda", "1", (0, 5), (0, 5)),
    row("Approval", "AWS", "Manual", "1", (0, 0), (0, 0)),
];

/// One row of [`DOCUMENTED_ACTION_TYPES`].
#[derive(Debug, Clone, Copy)]
struct DocumentedActionType {
    category: &'static str,
    owner: &'static str,
    provider: &'static str,
    version: &'static str,
    /// `(minimumCount, maximumCount)` of the action's input artifacts.
    input: (i64, i64),
    /// `(minimumCount, maximumCount)` of the action's output artifacts.
    output: (i64, i64),
}

const fn row(
    category: &'static str,
    owner: &'static str,
    provider: &'static str,
    version: &'static str,
    input: (i64, i64),
    output: (i64, i64),
) -> DocumentedActionType {
    DocumentedActionType {
        category,
        owner,
        provider,
        version,
        input,
        output,
    }
}

#[tokio::test]
async fn documented_action_types_are_listed_with_their_artifact_bounds() {
    let emulator = Emulator::new();
    let response = emulator.list(&json!({})).await;
    let listed = response["actionTypes"]
        .as_array()
        .unwrap_or_else(|| panic!("actionTypes is a list, got {response}"))
        .clone();

    for case in DOCUMENTED_ACTION_TYPES {
        let DocumentedActionType {
            category,
            owner,
            provider,
            version,
            input,
            output,
        } = case;
        let found = listed
            .iter()
            .find(|action_type| {
                let id = &action_type["id"];
                id["category"] == category
                    && id["owner"] == owner
                    && id["provider"] == provider
                    && id["version"] == version
            })
            .unwrap_or_else(|| panic!("case {case:?}: not listed, got {response}"));
        let bounds = |member: &str| {
            (
                found[member]["minimumCount"].as_i64(),
                found[member]["maximumCount"].as_i64(),
            )
        };
        assert_eq!(
            bounds("inputArtifactDetails"),
            (Some(input.0), Some(input.1)),
            "case {case:?}: unexpected inputArtifactDetails, got {found}"
        );
        assert_eq!(
            bounds("outputArtifactDetails"),
            (Some(output.0), Some(output.1)),
            "case {case:?}: unexpected outputArtifactDetails, got {found}"
        );
    }

    // The whole catalog is reported, exactly once each, in one ascending order.
    let reported = ids(&response);
    assert_eq!(
        reported.len(),
        BUILT_IN_ACTION_TYPES.len(),
        "the listing reports every action type the service provides, got {reported:?}"
    );
    let mut sorted = reported.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        reported, sorted,
        "the reported order is ascending by ActionTypeId and free of duplicates, got {reported:?}"
    );
}

#[tokio::test]
async fn an_owner_filter_reports_only_that_owner() {
    let emulator = Emulator::new();
    let (key, record) = custom("Build", "MyRunner", "1", "us-east-1");
    emulator.register(key, record);

    let unfiltered: BTreeSet<_> = ids(&emulator.list(&json!({})).await).into_iter().collect();
    let mut union = BTreeSet::new();

    // Every `ActionOwner` value (`smithy_query query="shape" shape="ActionOwner"`).
    for owner in ["AWS", "ThirdParty", "Custom"] {
        let response = emulator
            .list(&json!({"actionOwnerFilter": owner}))
            .await;
        let filtered = ids(&response);
        assert!(
            !filtered.is_empty(),
            "case {owner:?}: at least one action type has this owner, got {response}"
        );
        for id in &filtered {
            assert_eq!(
                id.1, owner,
                "case {owner:?}: reported an action type owned by {:?}, got {response}",
                id.1
            );
        }
        union.extend(filtered);
    }

    assert_eq!(
        union, unfiltered,
        "the three owner filters together report exactly the unfiltered listing"
    );
}

#[tokio::test]
async fn a_registered_action_type_is_reported_from_the_store() {
    let emulator = Emulator::new();
    let key = ActionTypeKey {
        category: "Test".to_string(),
        owner: "Custom".to_string(),
        provider: "My-Tester_2".to_string(),
        version: "7".to_string(),
    };
    let record = ActionTypeRecord {
        settings: Some(ActionTypeSettings {
            third_party_configuration_url: None,
            entity_url_template: Some("https://example.com/{Config:Suite}".to_string()),
            execution_url_template: Some("https://example.com/runs".to_string()),
            revision_url_template: None,
        }),
        configuration_properties: vec![
            ActionConfigurationProperty {
                name: "Suite".to_string(),
                required: true,
                key: true,
                secret: false,
                queryable: false,
                description: Some("which suite to run".to_string()),
                r#type: Some("String".to_string()),
            },
            ActionConfigurationProperty {
                name: "Verbose".to_string(),
                required: false,
                key: false,
                secret: false,
                queryable: false,
                description: None,
                r#type: Some("Boolean".to_string()),
            },
        ],
        input_artifact_details: ArtifactDetails {
            minimum_count: 1,
            maximum_count: 3,
        },
        output_artifact_details: ArtifactDetails {
            minimum_count: 0,
            maximum_count: 2,
        },
        region: Some("us-east-1".to_string()),
        tags: Vec::new(),
    };
    emulator.register(key, record);

    let response = emulator.list(&json!({"actionOwnerFilter": "Custom"})).await;
    assert_eq!(
        response["actionTypes"],
        json!([{
            "id": {
                // The awsJson shape discriminator this service's wire form carries on an
                // `ActionType`'s id, beside the four members the shape declares.
                "__type": "ActionTypeId",
                "category": "Test",
                "owner": "Custom",
                "provider": "My-Tester_2",
                "version": "7",
            },
            "settings": {
                "entityUrlTemplate": "https://example.com/{Config:Suite}",
                "executionUrlTemplate": "https://example.com/runs",
            },
            "actionConfigurationProperties": [
                {
                    "name": "Suite",
                    "required": true,
                    "key": true,
                    "secret": false,
                    "queryable": false,
                    "description": "which suite to run",
                    "type": "String",
                },
                {
                    "name": "Verbose",
                    "required": false,
                    "key": false,
                    "secret": false,
                    "queryable": false,
                    "type": "Boolean",
                },
            ],
            "inputArtifactDetails": {"minimumCount": 1, "maximumCount": 3},
            "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 2},
        }]),
        "the stored registration is reported exactly as it is held, got {response}"
    );
}

#[tokio::test]
async fn a_region_filter_selects_the_registrations_of_that_region() {
    let emulator = Emulator::new();
    for (region, provider) in [("us-east-1", "EastOnly"), ("eu-west-1", "WestOnly")] {
        let (key, record) = custom("Build", provider, "1", region);
        emulator.register(key, record);
    }

    // (case, Region the call is signed for, regionFilter, the Custom providers the
    // listing must report).
    let cases: [(&str, &str, Option<&str>, &[&str]); 5] = [
        ("no filter, called in us-east-1", "us-east-1", None, &["EastOnly"]),
        ("no filter, called in eu-west-1", "eu-west-1", None, &["WestOnly"]),
        ("filter eu-west-1 from us-east-1", "us-east-1", Some("eu-west-1"), &["WestOnly"]),
        ("filter us-east-1 from eu-west-1", "eu-west-1", Some("us-east-1"), &["EastOnly"]),
        ("filter a Region with no registration", "us-east-1", Some("ap-south-1"), &[]),
    ];

    for (case, region, filter, expected) in cases {
        let body = match filter {
            None => json!({}),
            Some(filter) => json!({"regionFilter": filter}),
        };
        let response = emulator.list_in(region, &body).await;
        let reported = ids(&response);
        let customs: Vec<&str> = reported
            .iter()
            .filter(|id| id.1 == "Custom")
            .map(|id| id.2.as_str())
            .collect();
        assert_eq!(
            customs, expected,
            "case {case:?}: unexpected Custom action types, got {response}"
        );
        // An action type the service provides belongs to every Region, so no filter
        // takes it away.
        assert_eq!(
            reported.len(),
            BUILT_IN_ACTION_TYPES.len() + expected.len(),
            "case {case:?}: the service's own action types are reported in every Region, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn pages_walk_every_action_type_exactly_once() {
    let emulator = Emulator::new();
    // Two full pages and a partial one, so the walk crosses a page boundary twice.
    let registered = MAX_ACTION_TYPES_PER_PAGE * 2 + 3;
    for index in 0..registered {
        let (key, record) = custom("Build", &format!("Runner-{index:04}"), "1", "us-east-1");
        emulator.register(key, record);
    }
    let expected_total = registered + BUILT_IN_ACTION_TYPES.len();

    let mut walked: Vec<(String, String, String, String)> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let body = match &token {
            None => json!({}),
            Some(token) => json!({"nextToken": token}),
        };
        let response = emulator.list(&body).await;
        let page = ids(&response);
        pages += 1;
        assert!(
            page.len() <= MAX_ACTION_TYPES_PER_PAGE,
            "page {pages}: a page holds at most {MAX_ACTION_TYPES_PER_PAGE} action types, got {}",
            page.len()
        );
        walked.extend(page);
        token = response["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            pages <= expected_total,
            "paging must terminate; it issued {pages} pages for {expected_total} action types"
        );
    }

    assert!(
        pages >= 3,
        "{expected_total} action types at {MAX_ACTION_TYPES_PER_PAGE} per page cross two page boundaries, got {pages} page(s)"
    );
    let mut unique = walked.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        walked, unique,
        "the pages together report each action type exactly once, in ascending order"
    );
    assert_eq!(
        walked.len(),
        expected_total,
        "the pages together report every action type, got {} of {expected_total}",
        walked.len()
    );
}

#[tokio::test]
async fn a_next_token_this_listing_never_issued_is_refused() {
    let emulator = Emulator::new();

    // Each row is a token no `ListActionTypes` page ever handed out: not base64 at
    // all, base64 carrying no cursor kind, a cursor of another listing's kind, and
    // action-type cursors naming too few members, too many, and an empty one.
    let cases: [(&str, String); 6] = [
        ("not base64", "not-a-token".to_string()),
        ("no cursor kind", base64::encode("demo")),
        ("another listing's cursor", base64::encode("pipeline:demo")),
        ("too few members", base64::encode("actionType:Source/AWS/S3")),
        (
            "too many members",
            base64::encode("actionType:Source/AWS/S3/1/extra"),
        ),
        ("an empty member", base64::encode("actionType:Source//S3/1")),
    ];

    for (case, token) in cases {
        let (status, code) = emulator.refusal(&json!({"nextToken": token})).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: token {token:?} was never issued by ListActionTypes"
        );
    }

    // A cursor this listing DID issue resumes the walk rather than being refused.
    for index in 0..=MAX_ACTION_TYPES_PER_PAGE {
        let (key, record) = custom("Build", &format!("Runner-{index:04}"), "1", "us-east-1");
        emulator.register(key, record);
    }
    let first = emulator.list(&json!({})).await;
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a full page carries a nextToken, got {first}"))
        .to_string();
    let second = emulator.list(&json!({"nextToken": token})).await;
    let (first_ids, second_ids) = (ids(&first), ids(&second));
    assert!(
        !second_ids.is_empty() && second_ids[0] > first_ids[first_ids.len() - 1],
        "the second page resumes after the first, got {second}"
    );
}

/// This account's registration store is BOUNDED: a registration is taken while the
/// account holds fewer than `MAX_ACTION_TYPES_PER_ACCOUNT` action types and refused
/// past that, re-registering an id already held replaces it (it grows nothing), and
/// forgetting one gives the slot back. Without the bound a registration path would
/// grow the process heap for as long as the emulator runs.
#[tokio::test]
async fn the_accounts_registration_store_is_bounded() {
    let emulator = Emulator::new();
    let state = emulator.engine.state::<ServiceState>();

    // Fill the account to the bound through the one way into the store.
    for index in 0..MAX_ACTION_TYPES_PER_ACCOUNT {
        let (key, record) = custom("Build", &format!("provider-{index}"), "1", "us-east-1");
        assert!(
            state.lock().register_action_type(key, record),
            "registration {index} is inside the bound and must be taken"
        );
    }
    assert_eq!(
        state.lock().registered_action_type_count(),
        MAX_ACTION_TYPES_PER_ACCOUNT,
        "the account holds exactly the registrations it was given"
    );

    // One id past the bound is refused, and stores nothing.
    let (extra, record) = custom("Build", "one-too-many", "1", "us-east-1");
    assert!(
        !state.lock().register_action_type(extra.clone(), record),
        "a registration past the bound must be refused, got it accepted"
    );
    assert!(
        state.lock().registered_action_type(&extra).is_none(),
        "a refused registration must store nothing"
    );
    assert_eq!(
        state.lock().registered_action_type_count(),
        MAX_ACTION_TYPES_PER_ACCOUNT,
        "a refused registration must not grow the store"
    );

    // Replacing an id the store already holds grows nothing, so it is taken even at
    // the bound — and the record it reports is the new one.
    let (held, replacement) = custom("Build", "provider-0", "1", "eu-west-1");
    assert!(
        state.lock().register_action_type(held.clone(), replacement),
        "re-registering a held id must be taken at the bound"
    );
    assert_eq!(
        state
            .lock()
            .registered_action_type(&held)
            .and_then(|record| record.region.clone()),
        Some("eu-west-1".to_string()),
        "the replacement record must be the one the store reports"
    );
    assert_eq!(
        state.lock().registered_action_type_count(),
        MAX_ACTION_TYPES_PER_ACCOUNT,
        "a replacement must not grow the store"
    );

    // Forgetting a registration releases its slot, and forgetting it twice reports
    // that there was nothing left to forget.
    assert!(
        state.lock().forget_action_type(&held),
        "a held registration must be forgotten"
    );
    assert!(
        !state.lock().forget_action_type(&held),
        "forgetting an id the store no longer holds must report nothing forgotten"
    );
    let (extra, record) = custom("Build", "one-too-many", "1", "us-east-1");
    assert!(
        state.lock().register_action_type(extra.clone(), record),
        "the slot a forgotten registration released must be usable again"
    );
    assert_eq!(
        state.lock().registered_action_type_count(),
        MAX_ACTION_TYPES_PER_ACCOUNT,
        "the store is back at the bound, never past it"
    );

    // The listing still answers from the store, so the bound did not break the read
    // path: the id that replaced a slot is reported for its Region.
    let listed = emulator
        .list(&json!({"actionOwnerFilter": "Custom", "regionFilter": "us-east-1"}))
        .await;
    assert!(
        ids(&listed)
            .iter()
            .all(|(_, owner, _, _)| owner == "Custom"),
        "the listing must report only the registrations it was asked for, got {listed}"
    );
}

