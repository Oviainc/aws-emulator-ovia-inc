// SPDX-License-Identifier: CC0-1.0

//! The mgn application lifecycle, end to end over HTTP: `InitializeService`,
//! `CreateApplication`, `ListApplications`, `UpdateApplication`, `DeleteApplication`.
//!
//! Every assertion here goes through the router's `dispatch`, so it exercises the same
//! restJson1 deserialize → handle → serialize path a client drives, and reads the
//! service's state only through its own operations: what a create returns must be what
//! a later list, update or delete sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mgn_applicationmigrationservice::state::{
    service_state, AggregatedStatus, Application, ACCOUNT_QUOTA, APPLICATION_QUOTA,
};
use service_com_amazonaws_mgn_applicationmigrationservice::{
    ComAmazonawsMgnApplicationmigrationserviceState as MgnState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test requests sign for; it is the region every ARN must carry.
pub(crate) const REGION: &str = "us-west-2";

/// The account every ARN carries: the emulator's canonical placeholder account.
pub(crate) const EMULATOR_ACCOUNT: &str = "000000000000";

/// A 12-digit account id the test never initializes, for the "uninitialized" rule.
pub(crate) const OTHER_ACCOUNT: &str = "123456789012";

/// One emulator with only this service mounted, driven through the router.
pub(crate) struct Mgn {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Mgn {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The service's own store, for seeding a precondition no sequence of calls can
    /// reach (another account's records, a full store) and for reading state back.
    pub(crate) fn store(&self) -> engine::ServiceState<MgnState> {
        service_state(&*self.engine).expect("this service has a store in this engine")
    }

    /// Put an application straight into the store, as if `CreateApplication` had been
    /// called by `account`. Answers its `applicationID`.
    fn seed(&self, account: &str, application_id: &str, name: &str) -> String {
        let now = "2026-10-21T21:48:13.000000000Z".to_string();
        let stored = self.store().lock().insert_application(Application {
            application_id: application_id.to_string(),
            arn: format!("arn:aws:mgn:{REGION}:{account}:application/{application_id}"),
            name: name.to_string(),
            aggregated_status: AggregatedStatus {
                last_update_date_time: now.clone(),
                ..AggregatedStatus::default()
            },
            creation_date_time: now.clone(),
            last_modified_date_time: now,
            account_id: account.to_string(),
            ..Application::default()
        });
        assert!(
            stored,
            "seeding {application_id:?} must store it (the store bound was not reached)"
        );
        application_id.to_string()
    }

    /// POST `/<operation>` with `body`, answering the status and the parsed JSON body
    /// (`Value::Null` for an empty one).
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let (status, _, parsed) = self.call_reporting_error_type(operation, body).await;
        (status, parsed)
    }

    /// As [`Mgn::call`], also answering the `x-amzn-errortype` header — the header
    /// restJson1 resolves an error SHAPE from, which a refusal must carry the same code
    /// in as the body's `__type`.
    pub(crate) async fn call_reporting_error_type(
        &self,
        operation: &str,
        body: &Value,
    ) -> (u16, Option<String>, Value) {
        self.call_http("POST", &format!("/{operation}"), Some(body))
            .await
    }

    /// One request at `method` and `path`, for an operation whose modelled HTTP binding
    /// is not a POST to `/<Operation>` — the `/tags/{resourceArn}` family is a GET, a
    /// POST and a DELETE of ONE route. Answers the status, the `x-amzn-errortype` header
    /// and the parsed body (`Value::Null` for an empty one).
    pub(crate) async fn call_http(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> (u16, Option<String>, Value) {
        let (status, error_type, raw) = self.call_http_raw(method, path, body).await;
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|error| {
                panic!("{method} {path} answered a body that is not JSON: {error}")
            })
        };
        (status, error_type, parsed)
    }

    /// As [`Mgn::call_http`], answering the response body as the RAW bytes it arrived in,
    /// for a rule about the body's presence rather than its contents — an answer that
    /// carries no entity must carry no bytes either.
    pub(crate) async fn call_http_raw(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> (u16, Option<String>, Vec<u8>) {
        let (status, headers, raw) = self.call_http_stating_headers(method, path, body).await;
        let error_type = headers
            .iter()
            .find(|(name, _)| name == "x-amzn-errortype")
            .map(|(_, value)| value.clone());
        (status, error_type, raw)
    }

    /// As [`Mgn::call_http_raw`], answering every response header as a lowercase
    /// name/value pair, for a rule about what an answer STATES about itself — the
    /// framing of an answer that carries no content, say.
    pub(crate) async fn call_http_stating_headers(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> (u16, Vec<(String, String)>, Vec<u8>) {
        let payload = body
            .map(|body| serde_json::to_vec(body).expect("the request body serializes"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://mgn.{REGION}.amazonaws.com{path}"))
            .header("host", format!("mgn.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLEEXAMPLE/20260101/{REGION}/mgn/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
            })
            .collect();
        let raw = response.into_body();
        (status, headers, raw.to_vec())
    }

    /// `InitializeService` for the caller's own account.
    pub(crate) async fn initialize(&self) {
        let (status, body) = self.call("InitializeService", &json!({})).await;
        assert_eq!(
            status, 204,
            "InitializeService: expected 204, got {status} with {body:?}"
        );
    }

    /// `CreateApplication`, asserting 201 and answering the created record.
    async fn create(&self, case: &str, request: &Value) -> Value {
        let (status, body) = self.call("CreateApplication", request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateApplication expected 201, got {status} with {body:?}"
        );
        body
    }

    /// `ListApplications`, asserting 200 and answering the response.
    async fn list(&self, case: &str, request: &Value) -> Value {
        let (status, body) = self.call("ListApplications", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListApplications expected 200, got {status} with {body:?}"
        );
        body
    }
}

/// `^app-[0-9a-zA-Z]{17}$` — the model's `ApplicationID` pattern (length 21..21).
fn is_application_id(value: &str) -> bool {
    value.len() == 21
        && value.starts_with("app-")
        && value[4..].chars().all(|c| c.is_ascii_alphanumeric())
}

/// The model's `ISO8601DatetimeString`: `YYYY-MM-DDTHH:MM:SS(.<digits>)?Z`, 19..32 chars.
fn is_iso8601_z(value: &str) -> bool {
    if !(19..=32).contains(&value.len()) || !value.ends_with('Z') {
        return false;
    }
    let separators_in_place = value
        .char_indices()
        .take(19)
        .all(|(index, c)| match index {
            4 | 7 => c == '-',
            10 => c == 'T',
            13 | 16 => c == ':',
            _ => c.is_ascii_digit(),
        });
    let fraction = &value[19..value.len() - 1];
    let fraction_well_formed = fraction.is_empty()
        || (fraction.starts_with('.')
            && fraction.len() > 1
            && fraction[1..].chars().all(|c| c.is_ascii_digit()));
    separators_in_place && fraction_well_formed
}

fn application_id(application: &Value) -> String {
    application["applicationID"]
        .as_str()
        .unwrap_or_else(|| panic!("no applicationID in {application:?}"))
        .to_string()
}

fn ids_of(list: &Value) -> Vec<String> {
    list["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no items array in {list:?}"))
        .iter()
        .map(application_id)
        .collect()
}

pub(crate) fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("<no __type>")
}

/// The class a `CreateApplication` request spans: the name's length and shape, the
/// description (absent, empty, long) and the tag count (0, 1, several). The recorded
/// `{"name":"billing-app","description":"demo","tags":{"env":"dev"}}` is one row.
fn creation_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "name only",
            json!({"name": "billing-app"}),
        ),
        (
            "name, description and one tag",
            json!({"name": "billing-app", "description": "demo", "tags": {"env": "dev"}}),
        ),
        (
            "single-character name, empty description",
            json!({"name": "a", "description": ""}),
        ),
        (
            "interior spaces and punctuation in the name",
            json!({"name": "SAP ERP  prod (wave 1)", "description": "line one, line two"}),
        ),
        (
            "256-character name — the modelled maximum",
            json!({"name": "n".repeat(256), "description": "d".repeat(600)}),
        ),
        (
            "several tags",
            json!({
                "name": "inventory",
                "tags": {"env": "prod", "team": "migration", "cost-center": "4711"}
            }),
        ),
        (
            "explicit accountID — the caller's own account",
            json!({"name": "explicit-account", "accountID": EMULATOR_ACCOUNT}),
        ),
    ]
}

/// Every created application is minted, stored and reported from state: the record a
/// create answers is byte-for-byte the record a later list reports, an update mutates
/// only the members it names and bumps `lastModifiedDateTime`, and a delete removes it.
#[tokio::test]
async fn an_application_is_created_listed_updated_and_deleted_through_state() {
    for (case, request) in creation_cases() {
        let mgn = Mgn::new();
        mgn.initialize().await;

        let created = mgn.create(case, &request).await;
        let id = application_id(&created);
        assert!(
            is_application_id(&id),
            "case {case:?}: applicationID must match ^app-[0-9a-zA-Z]{{17}}$, got {id:?}"
        );
        assert_eq!(
            created["arn"].as_str(),
            Some(format!("arn:aws:mgn:{REGION}:{EMULATOR_ACCOUNT}:application/{id}").as_str()),
            "case {case:?}: arn must name this region, the emulator account and the minted id, got {:?}",
            created["arn"]
        );
        assert_eq!(
            created["name"], request["name"],
            "case {case:?}: name must be the stored name, got {:?}",
            created["name"]
        );
        assert_eq!(
            created["description"], request["description"],
            "case {case:?}: description must be the stored description, got {:?}",
            created["description"]
        );
        assert_eq!(
            created["isArchived"],
            json!(false),
            "case {case:?}: a new application is not archived, got {:?}",
            created["isArchived"]
        );
        let expected_tags = request
            .get("tags")
            .cloned()
            .filter(|tags| !tags.as_object().expect("tags is an object").is_empty());
        assert_eq!(
            created.get("tags").cloned(),
            expected_tags,
            "case {case:?}: tags must round-trip, got {:?}",
            created.get("tags")
        );
        let status = &created["applicationAggregatedStatus"];
        assert_eq!(
            status["totalSourceServers"],
            json!(0),
            "case {case:?}: a new application has no source servers, got {status:?}"
        );
        assert_eq!(
            status["progressStatus"],
            json!("NOT_STARTED"),
            "case {case:?}: a new application has not started migrating, got {status:?}"
        );
        for field in [
            "creationDateTime",
            "lastModifiedDateTime",
        ] {
            let value = created[field].as_str().unwrap_or("");
            assert!(
                is_iso8601_z(value),
                "case {case:?}: {field} must be an ISO8601 ...Z string of 19..32 chars, got {value:?}"
            );
        }
        let created_at = created["creationDateTime"].as_str().expect("a timestamp");
        assert_eq!(
            created["lastModifiedDateTime"].as_str(),
            Some(created_at),
            "case {case:?}: an untouched application was last modified when it was created"
        );
        assert!(
            is_iso8601_z(status["lastUpdateDateTime"].as_str().unwrap_or("")),
            "case {case:?}: applicationAggregatedStatus.lastUpdateDateTime must be ISO8601, got {status:?}"
        );

        // The listed record is the stored record, not a fresh one.
        let listed = mgn
            .list(case, &json!({"filters": {"applicationIDs": [id]}}))
            .await;
        assert_eq!(
            listed["items"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: exactly the created application must be listed, got {listed:?}"
        );
        assert_eq!(
            listed["items"][0], created,
            "case {case:?}: the listed application must equal the created one"
        );

        // An update touches only what it names.
        let (status_code, updated) = mgn
            .call(
                "UpdateApplication",
                &json!({"applicationID": id, "description": "updated"}),
            )
            .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: UpdateApplication expected 200, got {status_code} with {updated:?}"
        );
        assert_eq!(
            updated["description"],
            json!("updated"),
            "case {case:?}: the description must be the updated one, got {:?}",
            updated["description"]
        );
        assert_eq!(
            updated["name"], created["name"],
            "case {case:?}: an absent name must leave the stored name alone"
        );
        assert_eq!(
            updated["creationDateTime"], created["creationDateTime"],
            "case {case:?}: an update must not move creationDateTime"
        );
        let modified_at = updated["lastModifiedDateTime"].as_str().expect("a timestamp");
        assert!(
            modified_at >= created_at,
            "case {case:?}: lastModifiedDateTime {modified_at:?} must not be older than creationDateTime {created_at:?}"
        );

        // And the update was persisted, not echoed.
        let after_update = mgn
            .list(case, &json!({"filters": {"applicationIDs": [id]}}))
            .await;
        assert_eq!(
            after_update["items"][0], updated,
            "case {case:?}: the listed application must equal the updated one"
        );

        // An update that names no mutable member changes nothing at all.
        let (status_code, untouched) = mgn
            .call("UpdateApplication", &json!({"applicationID": id}))
            .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: an update naming no member is still accepted, got {untouched:?}"
        );
        assert_eq!(
            untouched, updated,
            "case {case:?}: an update naming no member must leave the record — including lastModifiedDateTime — as it was"
        );

        // A delete removes the record from state.
        let (delete_status, delete_body) = mgn
            .call("DeleteApplication", &json!({"applicationID": id}))
            .await;
        assert_eq!(
            delete_status, 204,
            "case {case:?}: DeleteApplication expected 204, got {delete_status} with {delete_body:?}"
        );
        let after_delete = mgn.list(case, &json!({})).await;
        assert_eq!(
            after_delete["items"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a deleted application must not be listed, got {after_delete:?}"
        );
        for operation in ["UpdateApplication", "DeleteApplication"] {
            let (status, body) = mgn
                .call(operation, &json!({"applicationID": id}))
                .await;
            assert_eq!(
                (status, error_code(&body)),
                (404, "ResourceNotFoundException"),
                "case {case:?}: {operation} on a deleted application must be 404 ResourceNotFoundException, got {status} with {body:?}"
            );
        }
    }
}

/// Paging returns every application exactly once, in creation order, for any page size:
/// a `nextToken` resumes where the previous page stopped and is absent on the last page.
#[tokio::test]
async fn list_applications_pages_without_repeating_or_dropping_items() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    let mut created = Vec::new();
    for index in 0..7 {
        let application = mgn
            .create("pagination", &json!({"name": format!("app-{index}")}))
            .await;
        created.push(application_id(&application));
    }

    // Rows differ along the page size: below, equal to and above the item count.
    for page_size in [1usize, 2, 3, 6, 7, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = token.as_ref() {
                request["nextToken"] = json!(token);
            }
            let page = mgn.list("pagination", &request).await;
            let ids = ids_of(&page);
            assert!(
                ids.len() <= page_size,
                "page size {page_size}: a page returned {} items, more than maxResults",
                ids.len()
            );
            seen.extend(ids);
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "page size {page_size}: paging did not terminate after {pages} pages"
            );
            match page["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
            assert_eq!(
                seen.len() % page_size,
                0,
                "page size {page_size}: a non-final page was short ({} items so far)",
                seen.len()
            );
        }
        assert_eq!(
            seen, created,
            "page size {page_size}: paging must yield every application exactly once in creation order"
        );
    }
}

/// `filters` selects from the stored applications; each filter is tested on a value the
/// issue did not cite, and an absent or empty filter selects everything.
#[tokio::test]
async fn list_applications_honours_every_modelled_filter() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    let first = application_id(&mgn.create("filters", &json!({"name": "alpha"})).await);
    let second = application_id(&mgn.create("filters", &json!({"name": "beta"})).await);
    let third = application_id(&mgn.create("filters", &json!({"name": "gamma"})).await);

    // (case, filters, expected ids) — `waveIDs` selects nothing while no application
    // is associated with a wave, which is the state this slice leaves them in.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filters", json!({}), vec![first.clone(), second.clone(), third.clone()]),
        (
            "empty filter structure",
            json!({"filters": {}}),
            vec![first.clone(), second.clone(), third.clone()],
        ),
        (
            "one applicationID",
            json!({"filters": {"applicationIDs": [second.clone()]}}),
            vec![second.clone()],
        ),
        (
            "two applicationIDs",
            json!({"filters": {"applicationIDs": [third.clone(), first.clone()]}}),
            vec![first.clone(), third.clone()],
        ),
        (
            "an applicationID no application holds",
            json!({"filters": {"applicationIDs": ["app-00000000000000000"]}}),
            vec![],
        ),
        (
            "empty applicationIDs selects everything",
            json!({"filters": {"applicationIDs": []}}),
            vec![first.clone(), second.clone(), third.clone()],
        ),
        (
            "isArchived false",
            json!({"filters": {"isArchived": false}}),
            vec![first.clone(), second.clone(), third.clone()],
        ),
        ("isArchived true", json!({"filters": {"isArchived": true}}), vec![]),
        (
            "waveIDs",
            json!({"filters": {"waveIDs": ["wave-0000000000000000a"]}}),
            vec![],
        ),
        (
            "applicationIDs and isArchived together",
            json!({"filters": {"applicationIDs": [first.clone()], "isArchived": false}}),
            vec![first.clone()],
        ),
    ];

    for (case, request, expected) in cases {
        let listed = mgn.list(case, &request).await;
        assert_eq!(
            ids_of(&listed),
            expected,
            "case {case:?}: ListApplications selected the wrong applications ({listed:?})"
        );
    }
}

/// A `nextToken` this service never issued is refused, not served as page one or as
/// some mid-list page.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let mgn = Mgn::new();
    mgn.initialize().await;
    mgn.create("token", &json!({"name": "only-app"})).await;

    for token in [
        "not-a-token",
        "-1",
        "12x",
        // A bare offset carries no marker, so it is not one of ours.
        "1",
        // Shaped like ours, but naming an application this selection does not hold.
        "mgn-applications-500",
        "mgn-applications-app-00000000000000000",
    ] {
        let (status, body) = mgn
            .call("ListApplications", &json!({"nextToken": token}))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "token {token:?}: expected 400 ValidationException, got {status} with {body:?}"
        );
    }
}

/// `CreateApplication` is `@idempotent`: an identical repeat is one application. A
/// request whose name is taken but whose other members differ is the modelled
/// `ConflictException`, and a request with a free name is another application.
#[tokio::test]
async fn create_application_is_idempotent_and_names_are_unique_per_account() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    let request = json!({"name": "billing-app", "description": "demo", "tags": {"env": "dev"}});
    let first = mgn.create("repeat", &request).await;
    let repeat = mgn.create("repeat", &request).await;
    assert_eq!(
        first, repeat,
        "a repeat of an identical CreateApplication must answer the record the first call created"
    );

    // Rows differ along which member changes: a changed name is a new application,
    // anything else under a taken name is a conflict.
    let conflicts = [
        ("another description", json!({"name": "billing-app", "description": "other", "tags": {"env": "dev"}})),
        ("another tag value", json!({"name": "billing-app", "description": "demo", "tags": {"env": "prod"}})),
        ("no tags", json!({"name": "billing-app", "description": "demo"})),
        ("no description", json!({"name": "billing-app", "tags": {"env": "dev"}})),
    ];
    for (case, variant) in conflicts {
        let (status, body) = mgn.call("CreateApplication", &variant).await;
        assert_eq!(
            (status, error_code(&body)),
            (409, "ConflictException"),
            "case {case:?}: a taken name with different members must be 409 ConflictException, got {status} with {body:?}"
        );
    }

    let fresh_names = [
        ("another name", json!({"name": "payments-app", "description": "demo", "tags": {"env": "dev"}})),
        ("a third name", json!({"name": "billing-app-2"})),
    ];
    let mut expected_ids = vec![application_id(&first)];
    for (case, variant) in fresh_names {
        let created = mgn.create(case, &variant).await;
        let id = application_id(&created);
        assert!(
            !expected_ids.contains(&id),
            "case {case:?}: a free name must mint a new id, got {id:?}"
        );
        expected_ids.push(id);
    }

    let listed = mgn.list("variants", &json!({})).await;
    assert_eq!(
        ids_of(&listed),
        expected_ids,
        "exactly the accepted CreateApplication calls must have stored an application ({listed:?})"
    );

    // Renaming onto a taken name is the same conflict, and an archived application
    // cannot be edited at all.
    let (status, body) = mgn
        .call(
            "UpdateApplication",
            &json!({"applicationID": expected_ids[1], "name": "billing-app"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (409, "ConflictException"),
        "renaming onto a taken name must be 409 ConflictException, got {status} with {body:?}"
    );

    let archived = mgn.seed(EMULATOR_ACCOUNT, "app-bbbbbbbbbbbbbbbbb", "archived-app");
    mgn.store()
        .lock()
        .application_of_mut(EMULATOR_ACCOUNT, &archived)
        .expect("the seeded application")
        .is_archived = true;
    let (status, body) = mgn
        .call(
            "UpdateApplication",
            &json!({"applicationID": archived, "description": "x"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (409, "ConflictException"),
        "updating an archived application must be 409 ConflictException, got {status} with {body:?}"
    );
}

/// The modelled constraints are refused at the boundary, on each published bound and
/// one past it, rather than silently coerced by a handler.
#[tokio::test]
async fn modelled_input_constraints_are_refused() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    // (case, operation, body) — `maxResults` range 1..1000, `name` length 1..256,
    // `applicationID` pattern/length 21, `accountID` length 12, all from the model.
    let accepted: Vec<(&str, &str, Value)> = vec![
        ("maxResults at the minimum", "ListApplications", json!({"maxResults": 1})),
        ("maxResults at the maximum", "ListApplications", json!({"maxResults": 1000})),
        ("name at the minimum length", "CreateApplication", json!({"name": "x"})),
        (
            "name at the maximum length",
            "CreateApplication",
            json!({"name": "y".repeat(256)}),
        ),
    ];
    for (case, operation, body) in accepted {
        let (status, answer) = mgn.call(operation, &body).await;
        assert!(
            (200..300).contains(&status),
            "case {case:?}: a value on a published bound must be accepted, got {status} with {answer:?}"
        );
    }

    let refused: Vec<(&str, &str, Value)> = vec![
        ("maxResults below the minimum", "ListApplications", json!({"maxResults": 0})),
        ("maxResults above the maximum", "ListApplications", json!({"maxResults": 1001})),
        ("negative maxResults", "ListApplications", json!({"maxResults": -1})),
        (
            "name above the maximum length",
            "CreateApplication",
            json!({"name": "y".repeat(257)}),
        ),
        ("empty name", "CreateApplication", json!({"name": ""})),
        ("absent required name", "CreateApplication", json!({})),
        (
            "applicationID of the wrong shape",
            "UpdateApplication",
            json!({"applicationID": "not-an-app-id"}),
        ),
        (
            "accountID that is not 12 digits",
            "ListApplications",
            json!({"accountID": "1234"}),
        ),
    ];
    for (case, operation, body) in refused {
        let (status, answer) = mgn.call(operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value outside a modelled constraint must be refused with 400, got {status} with {answer:?}"
        );
    }
}

/// Until `InitializeService` has run for the account a request acts on, every other
/// application operation answers the modelled `UninitializedAccountException` (400) —
/// and initializing one account does not initialize another.
#[tokio::test]
async fn application_operations_refuse_an_uninitialized_account() {
    let mgn = Mgn::new();

    // (operation, body) — one row per operation of this slice.
    let operations = |account: Option<&str>| -> Vec<(&'static str, Value)> {
        let with_account = |mut body: Value| -> Value {
            if let Some(account) = account {
                body["accountID"] = json!(account);
            }
            body
        };
        vec![
            ("CreateApplication", with_account(json!({"name": "billing-app"}))),
            ("ListApplications", with_account(json!({}))),
            (
                "UpdateApplication",
                with_account(json!({"applicationID": "app-0000000000000000a", "name": "x"})),
            ),
            (
                "DeleteApplication",
                with_account(json!({"applicationID": "app-0000000000000000a"})),
            ),
        ]
    };

    for (operation, body) in operations(None) {
        let (status, answer) = mgn.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&answer)),
            (400, "UninitializedAccountException"),
            "{operation} before InitializeService must be 400 UninitializedAccountException, got {status} with {answer:?}"
        );
    }

    mgn.initialize().await;

    // The caller's own account now serves; an unknown id is a 404, not a 400.
    let (create_status, _) = mgn
        .call("CreateApplication", &json!({"name": "billing-app"}))
        .await;
    assert_eq!(
        create_status, 201,
        "CreateApplication must succeed once the account is initialized"
    );
    for operation in ["UpdateApplication", "DeleteApplication"] {
        let (status, answer) = mgn
            .call(operation, &json!({"applicationID": "app-0000000000000000a"}))
            .await;
        assert_eq!(
            (status, error_code(&answer)),
            (404, "ResourceNotFoundException"),
            "{operation} on an initialized account must be 404 for an unknown id, got {status} with {answer:?}"
        );
    }

    // Another account is still uninitialized: the flag is per account, not global.
    for (operation, body) in operations(Some(OTHER_ACCOUNT)) {
        let (status, answer) = mgn.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&answer)),
            (400, "UninitializedAccountException"),
            "{operation} for an uninitialized account must be 400 UninitializedAccountException, got {status} with {answer:?}"
        );
    }

    // And InitializeService is idempotent.
    mgn.initialize().await;
    let listed = mgn.list("after re-initialize", &json!({})).await;
    assert_eq!(
        listed["items"].as_array().map(Vec::len),
        Some(1),
        "InitializeService must not disturb the stored applications, got {listed:?}"
    );
}

/// An operation reaches only the records of the account it acts on: an `applicationID`
/// taken off the request can neither read nor DESTROY another account's application.
#[tokio::test]
async fn one_accounts_operations_cannot_touch_another_accounts_application() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    // Owned by another account, so unreachable from the caller's account.
    let foreign = mgn.seed(OTHER_ACCOUNT, "app-aaaaaaaaaaaaaaaaa", "foreign-app");
    let own = application_id(&mgn.create("own", &json!({"name": "own-app"})).await);

    for operation in ["UpdateApplication", "DeleteApplication"] {
        let (status, body) = mgn
            .call(operation, &json!({"applicationID": foreign}))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "{operation} on another account's application must be 404, got {status} with {body:?}"
        );
    }
    assert!(
        mgn.store()
            .lock()
            .application_of(OTHER_ACCOUNT, &foreign)
            .is_some(),
        "another account's application must survive a {foreign:?} request from this account"
    );

    let listed = mgn.list("own account only", &json!({})).await;
    assert_eq!(
        ids_of(&listed),
        vec![own.clone()],
        "ListApplications must report only this account's applications, got {listed:?}"
    );

    // And the caller's own delete still works, keyed by the same request member.
    let (status, _) = mgn
        .call("DeleteApplication", &json!({"applicationID": own}))
        .await;
    assert_eq!(status, 204, "DeleteApplication on an own application is 204");
    assert!(
        mgn.store()
            .lock()
            .application_of(OTHER_ACCOUNT, &foreign)
            .is_some(),
        "deleting an own application must not disturb another account's records"
    );
}

/// The store is bounded: once an account holds `APPLICATION_QUOTA` applications,
/// `CreateApplication` answers the modelled `ServiceQuotaExceededException` (402)
/// instead of growing the store, and releasing one record makes room again.
#[tokio::test]
async fn create_application_refuses_past_the_store_bound() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    for index in 0..APPLICATION_QUOTA {
        mgn.seed(
            EMULATOR_ACCOUNT,
            &format!("app-{index:017}"),
            &format!("seeded-{index}"),
        );
    }

    let (status, body) = mgn
        .call("CreateApplication", &json!({"name": "one-too-many"}))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (402, "ServiceQuotaExceededException"),
        "a full store must be refused with the modelled quota error, got {status} with {body:?}"
    );
    assert_eq!(
        mgn.store().lock().applications.len(),
        APPLICATION_QUOTA,
        "the refused CreateApplication must not have stored anything"
    );

    // The bound is a live count, not a latch: free one slot and a create succeeds.
    let (delete_status, _) = mgn
        .call(
            "DeleteApplication",
            &json!({"applicationID": format!("app-{:017}", 0)}),
        )
        .await;
    assert_eq!(delete_status, 204, "DeleteApplication must release a record");
    let (create_status, created) = mgn
        .call("CreateApplication", &json!({"name": "one-too-many"}))
        .await;
    assert_eq!(
        create_status, 201,
        "a create below the bound must succeed, got {create_status} with {created:?}"
    );
    assert_eq!(
        mgn.store().lock().applications.len(),
        APPLICATION_QUOTA,
        "the store must hold exactly the bound after replacing one record"
    );

    // And the bound is on the WHOLE store, not just one account: naming another
    // account cannot buy a second allowance.
    let (status, body) = mgn
        .call(
            "CreateApplication",
            &json!({"name": "other-account-app", "accountID": OTHER_ACCOUNT}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "UninitializedAccountException"),
        "an uninitialized account is refused before the store is even consulted, got {status} with {body:?}"
    );
    mgn.store()
        .lock()
        .initialized_accounts
        .insert(OTHER_ACCOUNT.to_string());
    let (status, body) = mgn
        .call(
            "CreateApplication",
            &json!({"name": "other-account-app", "accountID": OTHER_ACCOUNT}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (402, "ServiceQuotaExceededException"),
        "a full store must refuse every account, got {status} with {body:?}"
    );
    assert_eq!(
        mgn.store().lock().applications.len(),
        APPLICATION_QUOTA,
        "no account may grow the store past the bound"
    );
}

/// The account keyspace is bounded too: `InitializeService` refuses with the modelled
/// `ValidationException` once the store tracks `ACCOUNT_QUOTA` accounts.
#[tokio::test]
async fn initialize_service_refuses_past_the_account_bound() {
    let mgn = Mgn::new();

    // Fill the account set with accounts other than the caller's own.
    for index in 0..ACCOUNT_QUOTA {
        mgn.store()
            .lock()
            .initialized_accounts
            .insert(format!("{:012}", index + 1));
    }

    let (status, body) = mgn.call("InitializeService", &json!({})).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "ValidationException"),
        "InitializeService past the account bound must be 400 ValidationException, got {status} with {body:?}"
    );
    assert_eq!(
        mgn.store().lock().initialized_accounts.len(),
        ACCOUNT_QUOTA,
        "the refused InitializeService must not have grown the account set"
    );

    // An account already in the set is still served: the bound refuses growth, not
    // the idempotent repeat.
    mgn.store()
        .lock()
        .initialized_accounts
        .insert(EMULATOR_ACCOUNT.to_string());
    mgn.store()
        .lock()
        .initialized_accounts
        .remove(&format!("{:012}", 1));
    mgn.initialize().await;
}

