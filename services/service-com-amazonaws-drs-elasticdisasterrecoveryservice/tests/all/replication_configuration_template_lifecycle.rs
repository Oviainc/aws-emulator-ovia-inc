// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the DRS account-initialization +
//! ReplicationConfigurationTemplate lifecycle.
//!
//! Every test drives the service the way the router does — one buffered
//! `http::Request` per call through [`MultiServiceRouter::dispatch`] — so what is
//! asserted is the wire behaviour (status, error code, JSON body), not a handler's
//! Rust return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one
//! recorded request:
//!
//! 1. Every DRS operation whose model declares `UninitializedAccountException`
//!    refuses with it (HTTP 400) until `InitializeService` has run, and serves
//!    normally afterwards.
//! 2. A create persists every member the request supplied and reports the stored
//!    record, so a later read returns the same values; an update merges only the
//!    members the request supplied; a delete makes the resource unreadable.
//! 3. A `@paginated` read walks each stored item exactly once, in a stable order,
//!    for any `maxResults`, and changes nothing a later caller sees.
//! 4. The service bounds itself: the store holds at most its quota of templates
//!    (the next create is the modelled quota refusal) and one page carries at most
//!    the server-side page limit, whatever `maxResults` asks for.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_drs_elasticdisasterrecoveryservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for. The service reads it from the
/// SigV4 credential scope, so a template's ARN must carry THIS region and not the
/// emulator's `us-east-1` default.
const REGION: &str = "eu-west-1";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One answer from the emulator: the pieces a client branches on.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The wire error code (`x-amzn-errortype` header, else the body's `__type`),
    /// reduced to the bare shape name.
    error_code: Option<String>,
    body: Value,
}

impl Answer {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name)
    }
}

/// A router serving only DRS, over a single shared engine — the same assembly the
/// production router performs for this service.
fn drs() -> MultiServiceRouter {
    drs_with_engine().0
}

/// The same router, with the engine handed back so a test can seed state that no
/// implemented operation can create yet: a source server is registered by the
/// replication agent, which this slice does not model, so `DescribeSourceServers`'
/// filters can only be exercised over a store written directly.
fn drs_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are verified by the emulator, not by the service; this suite is
    // about the service's behaviour, so it does not sign.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// `POST /<operation>` with `body`, routed by the SigV4 credential scope exactly as
/// an AWS SDK client routes it.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let host = format!("drs.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/{operation}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/drs/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(bare_shape_name);
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    let error_code = header_code.or_else(|| {
        parsed
            .get("__type")
            .and_then(Value::as_str)
            .map(bare_shape_name)
    });

    Answer {
        status,
        error_code,
        body: parsed,
    }
}

/// `com.amazonaws.drs#ValidationException:http://...` -> `ValidationException`.
fn bare_shape_name(raw: &str) -> String {
    let after_namespace = raw.rsplit('#').next().unwrap_or(raw);
    after_namespace
        .split(':')
        .next()
        .unwrap_or(after_namespace)
        .to_string()
}

/// `^rct-[0-9a-zA-Z]{17}$`, the model's `ReplicationConfigurationTemplateID`
/// pattern (length 21..21), checked without pulling in a regex engine.
fn is_template_id(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("rct-") else {
        return false;
    };
    suffix.len() == 17 && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Initialize the account, asserting the documented 204 + empty body.
async fn initialize(router: &MultiServiceRouter) {
    let answer = call(router, "InitializeService", &json!({})).await;
    assert_eq!(
        answer.status, 204,
        "InitializeService: expected 204, got {answer:?}"
    );
}

/// A create request body: the required members, with `extra` merged over them.
fn create_body(subnet: &str, extra: Value) -> Value {
    let mut body = json!({
        "stagingAreaSubnetId": subnet,
        "replicationServersSecurityGroupsIDs": ["sg-0123456789abcdef0"],
        "ebsEncryption": "DEFAULT",
        "bandwidthThrottling": 0,
        "stagingAreaTags": {"env": "test"},
        // `pitPolicy` is modelled `length(min: 3, max: 3)`.
        "pitPolicy": [
            {"ruleID": 1, "units": "MINUTE", "interval": 10, "retentionDuration": 60, "enabled": true},
            {"ruleID": 2, "units": "HOUR", "interval": 1, "retentionDuration": 24, "enabled": true},
            {"ruleID": 3, "units": "DAY", "interval": 1, "retentionDuration": 7, "enabled": true},
        ],
    });
    merge(&mut body, &extra);
    body
}

/// Overlay every member of `patch` onto `target` (both JSON objects).
fn merge(target: &mut Value, patch: &Value) {
    let Some(patch) = patch.as_object() else {
        return;
    };
    let Some(target) = target.as_object_mut() else {
        return;
    };
    for (key, value) in patch {
        target.insert(key.clone(), value.clone());
    }
}

/// Create a template and return its id, failing the test on anything but a 201.
async fn create(router: &MultiServiceRouter, body: &Value) -> String {
    let answer = call(router, "CreateReplicationConfigurationTemplate", body).await;
    assert_eq!(
        answer.status, 201,
        "create {body}: expected 201, got {answer:?}"
    );
    answer
        .member("replicationConfigurationTemplateID")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("create {body}: no template id in {answer:?}"))
        .to_string()
}

/// The `items` of a Describe answer.
fn items(answer: &Answer) -> &Vec<Value> {
    answer
        .member("items")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an items list, got {answer:?}"))
}

// ---------------------------------------------------------------------------
// Rule 1 — the uninitialized-account gate
// ---------------------------------------------------------------------------

/// Every operation whose model declares `UninitializedAccountException` answers it
/// — 400, that code — on a fresh account, and stops doing so once the account is
/// initialized. The table spans the four template operations plus the source-server
/// read, i.e. every gated operation in this slice, with request shapes that differ
/// (no members, full create, id-only, filtered read).
#[tokio::test]
async fn gated_operations_refuse_until_the_account_is_initialized() {
    let absent_id = "rct-00000000000000000";
    let cases: Vec<(&str, Value)> = vec![
        (
            "DescribeReplicationConfigurationTemplates",
            json!({}),
        ),
        (
            "CreateReplicationConfigurationTemplate",
            create_body("subnet-0123456789abcdef0", json!({})),
        ),
        (
            "UpdateReplicationConfigurationTemplate",
            json!({"replicationConfigurationTemplateID": absent_id, "bandwidthThrottling": 10}),
        ),
        (
            "DeleteReplicationConfigurationTemplate",
            json!({"replicationConfigurationTemplateID": absent_id}),
        ),
        (
            "DescribeSourceServers",
            json!({"filters": {"sourceServerIDs": ["s-0123456789abcdef0"]}}),
        ),
    ];

    let router = drs();
    for (operation, body) in &cases {
        let answer = call(&router, operation, body).await;
        assert_eq!(
            answer.status, 400,
            "case {operation}: expected 400 before InitializeService, got {answer:?}"
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("UninitializedAccountException"),
            "case {operation}: expected UninitializedAccountException, got {answer:?}"
        );
    }

    initialize(&router).await;

    // After initialization the same requests are served: the two reads and the
    // create succeed, and the id-keyed mutations fail on the MISSING TEMPLATE
    // (404) rather than on the account gate.
    for (operation, body) in &cases {
        let answer = call(&router, operation, body).await;
        let expected: u16 = match *operation {
            "CreateReplicationConfigurationTemplate" => 201,
            "UpdateReplicationConfigurationTemplate"
            | "DeleteReplicationConfigurationTemplate" => 404,
            _ => 200,
        };
        assert_eq!(
            answer.status, expected,
            "case {operation}: expected {expected} after InitializeService, got {answer:?}"
        );
        assert_ne!(
            answer.error_code.as_deref(),
            Some("UninitializedAccountException"),
            "case {operation}: still gated after InitializeService: {answer:?}"
        );
    }
}

/// `InitializeService` is a no-op on an already-initialized account: 204 again, and
/// the templates created in between survive it.
#[tokio::test]
async fn initialize_service_is_idempotent() {
    let router = drs();
    initialize(&router).await;
    let id = create(&router, &create_body("subnet-0123456789abcdef0", json!({}))).await;
    initialize(&router).await;

    let answer = call(
        &router,
        "DescribeReplicationConfigurationTemplates",
        &json!({}),
    )
    .await;
    assert_eq!(answer.status, 200, "describe after re-initialize: {answer:?}");
    assert_eq!(
        items(&answer).len(),
        1,
        "case re-initialize: expected the existing template to survive, got {answer:?}"
    );
    assert_eq!(
        items(&answer)[0]["replicationConfigurationTemplateID"],
        json!(id),
        "case re-initialize: expected template {id}, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2 — create persists, read reports, update merges, delete removes
// ---------------------------------------------------------------------------

/// A create stores every member the request supplied and reports the stored record;
/// a Describe of that id reports the identical record. The rows differ along the
/// members the rule generalises over: how many optionals are set, the enum values
/// chosen (`ebsEncryption`, `dataPlaneRouting`, `defaultLargeStagingDiskType`,
/// `internetProtocol`), list size (`replicationServersSecurityGroupsIDs`), map size
/// (`stagingAreaTags`, `tags`), and the `pitPolicy` units/retention.
#[tokio::test]
async fn create_persists_every_supplied_member_and_describe_reports_it() {
    // The enum values below are the model's own, per
    // smithy_query(query="shape", shape="ReplicationConfigurationEbsEncryption,
    // ReplicationConfigurationDataPlaneRouting,
    // ReplicationConfigurationDefaultLargeStagingDiskType,InternetProtocol").
    let cases: Vec<(&str, Value)> = vec![
        // Only the required members: every optional must stay absent, not be
        // invented.
        ("minimal", json!({})),
        (
            "every optional set",
            json!({
                "associateDefaultSecurityGroup": true,
                "replicationServerInstanceType": "t3.small",
                "useDedicatedReplicationServer": false,
                "defaultLargeStagingDiskType": "GP3",
                "ebsEncryption": "CUSTOM",
                "ebsEncryptionKeyArn": "arn:aws:kms:eu-west-1:000000000000:key/abcd1234-1234-1234-1234-1234567890ab",
                "bandwidthThrottling": 10_000,
                "dataPlaneRouting": "PRIVATE_IP",
                "createPublicIP": true,
                "autoReplicateNewDisks": true,
                "internetProtocol": "IPV6",
                "tags": {"team": "dr", "tier": "gold"},
            }),
        ),
        (
            "other enum values, empty collections",
            json!({
                "ebsEncryption": "NONE",
                "defaultLargeStagingDiskType": "ST1",
                "dataPlaneRouting": "PUBLIC_IP",
                "internetProtocol": "IPV4",
                "replicationServersSecurityGroupsIDs": [],
                "stagingAreaTags": {},
                "tags": {},
            }),
        ),
        (
            "larger list and different pit policy",
            json!({
                "ebsEncryption": "DEFAULT",
                "replicationServersSecurityGroupsIDs": [
                    "sg-00000000000000001", "sg-00000000000000002", "sg-00000000000000003",
                ],
                "bandwidthThrottling": 1,
                "pitPolicy": [
                    {"ruleID": 7, "units": "MINUTE", "interval": 5, "retentionDuration": 1440, "enabled": false},
                    {"ruleID": 8, "units": "HOUR", "interval": 6, "retentionDuration": 168},
                    {"ruleID": 9, "units": "DAY", "interval": 2, "retentionDuration": 30, "enabled": true},
                ],
            }),
        ),
    ];

    let router = drs();
    initialize(&router).await;

    let mut ids = BTreeSet::new();
    for (case, extra) in &cases {
        let body = create_body("subnet-0123456789abcdef0", extra.clone());
        let answer = call(&router, "CreateReplicationConfigurationTemplate", &body).await;
        assert_eq!(answer.status, 201, "case {case}: expected 201, got {answer:?}");

        // The service assigns the id and the ARN.
        let id = answer
            .member("replicationConfigurationTemplateID")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: no template id in {answer:?}"))
            .to_string();
        assert!(
            is_template_id(&id),
            "case {case}: expected an id matching ^rct-[0-9a-zA-Z]{{17}}$, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case}: id {id} was already handed out"
        );
        // A later id sorts AFTER every earlier one: the store is keyed by id, so
        // that ordering is what makes a paginated read page in creation order.
        assert!(
            ids.iter().next_back() == Some(&id),
            "case {case}: id {id} does not sort after the ids already minted ({ids:?})"
        );
        assert_eq!(
            answer.member("arn").and_then(Value::as_str),
            Some(
                format!("arn:aws:drs:{REGION}:{ACCOUNT}:replication-configuration-template/{id}")
                    .as_str()
            ),
            "case {case}: unexpected ARN in {answer:?}"
        );

        // Every member the request supplied comes back with the value it supplied.
        for (member, sent) in body.as_object().expect("an object body") {
            assert_eq!(
                answer.member(member),
                Some(sent),
                "case {case}: member {member} was not stored as sent; got {answer:?}"
            );
        }

        // And a read of that id reports exactly the record the create reported.
        let described = call(
            &router,
            "DescribeReplicationConfigurationTemplates",
            &json!({"replicationConfigurationTemplateIDs": [id]}),
        )
        .await;
        assert_eq!(described.status, 200, "case {case}: describe {described:?}");
        assert_eq!(
            items(&described),
            &vec![answer.body.clone()],
            "case {case}: describe did not report the created record"
        );
    }

    // An optional member nobody sent must not appear at all.
    let minimal = call(
        &router,
        "DescribeReplicationConfigurationTemplates",
        &json!({"maxResults": 1}),
    )
    .await;
    assert!(
        items(&minimal)[0].get("tags").is_none(),
        "case minimal: tags were never supplied, so they must be absent: {minimal:?}"
    );
}

/// An update merges only the members it supplies: each one takes the new value and
/// every other member keeps the stored one, as a following Describe confirms. The
/// rows differ along which members are supplied and of what kind (scalar, enum,
/// list, map, structure list).
#[tokio::test]
async fn update_merges_only_the_supplied_members() {
    let cases: Vec<(&str, Value)> = vec![
        ("a single scalar", json!({"bandwidthThrottling": 500})),
        (
            "two scalars and an enum",
            json!({
                "bandwidthThrottling": 42,
                "defaultLargeStagingDiskType": "GP3",
                "createPublicIP": false,
            }),
        ),
        (
            "a replacement list",
            json!({"replicationServersSecurityGroupsIDs": ["sg-00000000000000009"]}),
        ),
        (
            "a replacement map",
            json!({"stagingAreaTags": {"env": "prod", "wave": "2"}}),
        ),
        (
            "a replacement structure list",
            json!({"pitPolicy": [
                {"ruleID": 4, "units": "MINUTE", "interval": 30, "retentionDuration": 120, "enabled": false},
                {"ruleID": 5, "units": "HOUR", "interval": 2, "retentionDuration": 48, "enabled": true},
                {"ruleID": 6, "units": "DAY", "interval": 3, "retentionDuration": 14, "enabled": true},
            ]}),
        ),
        (
            "the subnet and the encryption",
            json!({
                "stagingAreaSubnetId": "subnet-abcdefabcdefabcde",
                "ebsEncryption": "NONE",
                "internetProtocol": "IPV6",
            }),
        ),
    ];

    for (case, patch) in &cases {
        let router = drs();
        initialize(&router).await;
        let created = call(
            &router,
            "CreateReplicationConfigurationTemplate",
            &create_body(
                "subnet-0123456789abcdef0",
                json!({
                    "bandwidthThrottling": 7,
                    "tags": {"team": "dr"},
                    "createPublicIP": true,
                }),
            ),
        )
        .await;
        assert_eq!(created.status, 201, "case {case}: create {created:?}");
        let id = created.body["replicationConfigurationTemplateID"]
            .as_str()
            .expect("a template id")
            .to_string();

        // What the merged record must be: the created record with the patch laid
        // over it. `bandwidthThrottling` carries `@default(0)` in the model, so an
        // update that omits it cannot be told from one that sends 0 and the member
        // takes 0 — the one member a merge cannot preserve.
        let mut expected = created.body.clone();
        merge(&mut expected, patch);
        if patch.get("bandwidthThrottling").is_none() {
            expected["bandwidthThrottling"] = json!(0);
        }

        let mut request = json!({"replicationConfigurationTemplateID": id});
        merge(&mut request, patch);
        let updated = call(&router, "UpdateReplicationConfigurationTemplate", &request).await;
        assert_eq!(updated.status, 200, "case {case}: expected 200, got {updated:?}");
        assert_eq!(
            updated.body, expected,
            "case {case}: the merged record is wrong"
        );

        let described = call(
            &router,
            "DescribeReplicationConfigurationTemplates",
            &json!({"replicationConfigurationTemplateIDs": [id]}),
        )
        .await;
        assert_eq!(
            items(&described),
            &vec![expected],
            "case {case}: the merge was not visible to a following Describe"
        );
    }
}

/// A delete removes the template: the id becomes unreadable (404) and an unfiltered
/// read no longer lists it, while its siblings are untouched and keep their order.
#[tokio::test]
async fn delete_removes_only_that_template() {
    let router = drs();
    initialize(&router).await;

    let mut ids = Vec::new();
    for index in 0..4 {
        ids.push(
            create(
                &router,
                &create_body(
                    "subnet-0123456789abcdef0",
                    json!({"bandwidthThrottling": index}),
                ),
            )
            .await,
        );
    }

    // Deleting the middle one keeps the rest in their original relative order,
    // which is what the paginated read cuts pages on.
    let doomed = ids.remove(1);
    let deleted = call(
        &router,
        "DeleteReplicationConfigurationTemplate",
        &json!({"replicationConfigurationTemplateID": doomed}),
    )
    .await;
    assert_eq!(deleted.status, 204, "delete: expected 204, got {deleted:?}");
    assert!(
        deleted
            .body
            .as_object()
            .is_none_or(serde_json::Map::is_empty),
        "delete: expected a body with no members, got {deleted:?}"
    );

    let gone = call(
        &router,
        "DescribeReplicationConfigurationTemplates",
        &json!({"replicationConfigurationTemplateIDs": [doomed]}),
    )
    .await;
    assert_eq!(gone.status, 404, "describe deleted: {gone:?}");
    assert_eq!(
        gone.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "describe deleted: {gone:?}"
    );

    let listed = call(
        &router,
        "DescribeReplicationConfigurationTemplates",
        &json!({}),
    )
    .await;
    let remaining: Vec<String> = items(&listed)
        .iter()
        .map(|item| item["replicationConfigurationTemplateID"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        remaining, ids,
        "after delete: expected {ids:?} in order, got {remaining:?}"
    );

    // A second delete of the same id, and of an id never minted, are both 404.
    for (case, id) in [("already deleted", doomed.as_str()), ("never minted", "rct-00000000000000000")] {
        let answer = call(
            &router,
            "DeleteReplicationConfigurationTemplate",
            &json!({"replicationConfigurationTemplateID": id}),
        )
        .await;
        assert_eq!(answer.status, 404, "case {case}: expected 404, got {answer:?}");
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: {answer:?}"
        );
    }
}

/// An id-keyed operation on an id the store does not hold is a 404
/// `ResourceNotFoundException`, for each of the three operations that take one.
#[tokio::test]
async fn unknown_template_id_is_not_found() {
    let router = drs();
    initialize(&router).await;
    // Created so the store is non-empty: "not found" must be about the id, not
    // about the store being empty.
    create(&router, &create_body("subnet-0123456789abcdef0", json!({}))).await;

    let unknown = "rct-zzzzzzzzzzzzzzzzz";
    let cases: Vec<(&str, Value)> = vec![
        (
            "DescribeReplicationConfigurationTemplates",
            json!({"replicationConfigurationTemplateIDs": [unknown]}),
        ),
        (
            "UpdateReplicationConfigurationTemplate",
            json!({"replicationConfigurationTemplateID": unknown, "bandwidthThrottling": 1}),
        ),
        (
            "DeleteReplicationConfigurationTemplate",
            json!({"replicationConfigurationTemplateID": unknown}),
        ),
    ];

    for (operation, body) in &cases {
        let answer = call(&router, operation, body).await;
        assert_eq!(
            answer.status, 404,
            "case {operation}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {operation}: {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3 — pagination
// ---------------------------------------------------------------------------

/// For ANY page size, walking the pages reports every stored template exactly once,
/// in the store's order, and the last page is the one without a `nextToken`. The
/// rows are page sizes below, at and above the item count, plus one that divides it
/// exactly.
#[tokio::test]
async fn paginated_describe_walks_every_template_exactly_once() {
    const TEMPLATES: usize = 6;

    let router = drs();
    initialize(&router).await;
    let mut expected = Vec::new();
    for index in 0..TEMPLATES {
        expected.push(
            create(
                &router,
                &create_body(
                    "subnet-0123456789abcdef0",
                    json!({"bandwidthThrottling": index}),
                ),
            )
            .await,
        );
    }

    for page_size in [1_usize, 2, 4, 6, 11] {
        let mut seen = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &next {
                request["nextToken"] = json!(token);
            }
            let answer = call(
                &router,
                "DescribeReplicationConfigurationTemplates",
                &request,
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={page_size}: {answer:?}"
            );
            let page = items(&answer);
            assert!(
                page.len() <= page_size,
                "case maxResults={page_size}: page of {} exceeds the page size: {answer:?}",
                page.len()
            );
            for item in page {
                seen.push(
                    item["replicationConfigurationTemplateID"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= TEMPLATES + 1,
                "case maxResults={page_size}: pagination did not terminate"
            );
            match answer.member("nextToken").and_then(Value::as_str) {
                Some(token) => next = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case maxResults={page_size}: the walk did not report every template once, in order"
        );
        let expected_pages = TEMPLATES.div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case maxResults={page_size}: expected {expected_pages} pages, walked {pages}"
        );
    }

    // `@readonly`: paginating changed nothing a later caller sees.
    let after = call(
        &router,
        "DescribeReplicationConfigurationTemplates",
        &json!({}),
    )
    .await;
    let ids: Vec<String> = items(&after)
        .iter()
        .map(|item| item["replicationConfigurationTemplateID"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        ids, expected,
        "reading must not change what a later caller sees: {after:?}"
    );
}

/// A `nextToken` the service did not mint — or one minted for a different paginated
/// collection — is refused as a `ValidationException` (400) rather than silently
/// restarting the walk.
#[tokio::test]
async fn an_unusable_next_token_is_refused() {
    let router = drs();
    initialize(&router).await;
    create(&router, &create_body("subnet-0123456789abcdef0", json!({}))).await;
    create(&router, &create_body("subnet-0123456789abcdef1", json!({}))).await;

    // The token the source-server read hands out is for a different collection.
    let first_page = call(
        &router,
        "DescribeReplicationConfigurationTemplates",
        &json!({"maxResults": 1}),
    )
    .await;
    let template_token = first_page
        .member("nextToken")
        .and_then(Value::as_str)
        .expect("a nextToken on a partial page")
        .to_string();

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "not a token at all",
            "DescribeReplicationConfigurationTemplates",
            json!({"nextToken": "nonsense"}),
        ),
        (
            "a token for another collection",
            "DescribeSourceServers",
            json!({"nextToken": template_token.clone()}),
        ),
    ];

    for (case, operation, body) in &cases {
        let answer = call(&router, operation, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case}: {answer:?}"
        );
    }

    // The genuine token still resumes the walk.
    let second_page = call(
        &router,
        "DescribeReplicationConfigurationTemplates",
        &json!({"maxResults": 1, "nextToken": template_token}),
    )
    .await;
    assert_eq!(second_page.status, 200, "resume: {second_page:?}");
    assert_eq!(
        items(&second_page).len(),
        1,
        "resume: expected the second template, got {second_page:?}"
    );
    assert_ne!(
        items(&second_page)[0]["replicationConfigurationTemplateID"],
        items(&first_page)[0]["replicationConfigurationTemplateID"],
        "resume: the token replayed the first page"
    );
}

/// A repeated id in `replicationConfigurationTemplateIDs` names ONE template, so
/// it is reported once and the paginated walk still terminates. The model puts no
/// uniqueness constraint on the list (`length(0, 200)`), and a page token names the
/// id the next page starts at, so a selection that held an id twice would serve the
/// same page forever. The rows are a doubled id, a tripled one, and a list where a
/// duplicate is interleaved with another template.
#[tokio::test]
async fn a_repeated_template_id_is_reported_once() {
    let router = drs();
    initialize(&router).await;

    let first = create(&router, &create_body("subnet-0123456789abcdef0", json!({}))).await;
    let second = create(
        &router,
        &create_body("subnet-00112233445566778", json!({})),
    )
    .await;

    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "doubled",
            vec![first.as_str(), first.as_str()],
            vec![first.as_str()],
        ),
        (
            "tripled",
            vec![first.as_str(), first.as_str(), first.as_str()],
            vec![first.as_str()],
        ),
        (
            "interleaved with another template",
            vec![
                first.as_str(),
                second.as_str(),
                first.as_str(),
                second.as_str(),
            ],
            vec![first.as_str(), second.as_str()],
        ),
    ];

    for (case, requested, expected) in &cases {
        // Unpaginated: one item per distinct id, in the order first asked for.
        let answer = call(
            &router,
            "DescribeReplicationConfigurationTemplates",
            &json!({"replicationConfigurationTemplateIDs": requested}),
        )
        .await;
        assert_eq!(answer.status, 200, "case {case}: expected 200, got {answer:?}");
        let reported: Vec<String> = items(&answer)
            .iter()
            .map(|item| {
                item["replicationConfigurationTemplateID"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported, *expected,
            "case {case}: expected {expected:?} once each, got {reported:?}"
        );

        // One item per page: the walk must END, and report the same set.
        let mut walked = Vec::new();
        let mut next: Option<String> = None;
        for _ in 0..=requested.len() {
            let mut body = json!({
                "replicationConfigurationTemplateIDs": requested,
                "maxResults": 1,
            });
            if let Some(token) = &next {
                merge(&mut body, &json!({"nextToken": token}));
            }
            let page = call(&router, "DescribeReplicationConfigurationTemplates", &body).await;
            assert_eq!(page.status, 200, "case {case}: expected 200, got {page:?}");
            for item in items(&page) {
                walked.push(
                    item["replicationConfigurationTemplateID"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            match page.member("nextToken").and_then(Value::as_str) {
                Some(token) => next = Some(token.to_string()),
                None => {
                    next = None;
                    break;
                }
            }
        }
        assert!(
            next.is_none(),
            "case {case}: the paginated walk did not terminate; walked {walked:?}"
        );
        assert_eq!(
            walked, *expected,
            "case {case}: expected the walk to report {expected:?}, got {walked:?}"
        );
    }
}

/// `DescribeSourceServers` reads the source-server store: an initialized account
/// holds none (they are registered by the replication agent, which this slice does
/// not model), so it answers 200 with an empty `items` and no `nextToken` — for any
/// filter, and whether or not templates exist.
#[tokio::test]
async fn describe_source_servers_reports_the_empty_store() {
    let router = drs();
    initialize(&router).await;

    let cases: Vec<(&str, Value)> = vec![
        ("no filters", json!({})),
        (
            "by source server id",
            json!({"filters": {"sourceServerIDs": ["s-0123456789abcdef0"]}}),
        ),
        (
            "by hardware id",
            json!({"filters": {"hardwareId": "i-0123456789abcdef0"}}),
        ),
        (
            "by staging account",
            json!({"filters": {"stagingAccountIDs": ["000000000000"]}}),
        ),
        ("with a page size", json!({"maxResults": 5})),
    ];

    for (case, body) in &cases {
        let answer = call(&router, "DescribeSourceServers", body).await;
        assert_eq!(answer.status, 200, "case {case}: expected 200, got {answer:?}");
        assert!(
            items(&answer).is_empty(),
            "case {case}: expected no source servers, got {answer:?}"
        );
        assert!(
            answer.member("nextToken").is_none(),
            "case {case}: a complete page must carry no nextToken: {answer:?}"
        );
    }

    // Creating a template does not create a source server.
    create(&router, &create_body("subnet-0123456789abcdef0", json!({}))).await;
    let answer = call(&router, "DescribeSourceServers", &json!({})).await;
    assert!(
        items(&answer).is_empty(),
        "case after create: a template is not a source server: {answer:?}"
    );
}

/// Each `DescribeSourceServers` filter selects exactly the servers it names, over a
/// store seeded with servers that differ in HOW they are identifiable (EC2 instance
/// id, VMware UUID, NIC MAC address, nothing but an id) and in whether they are
/// extended from a staging account. The rows are the modelled filter members, alone
/// and combined, each with the servers it must select and the ones it must reject;
/// an empty list and an empty `hardwareId` are rows too, because those are the
/// boundary of the "absent means do not filter on this member" rule.
///
/// Source servers are registered by the replication agent, which this slice does not
/// model, so the store is seeded through the crate's public state module — the same
/// store the handler reads.
#[tokio::test]
async fn describe_source_servers_applies_every_modelled_filter() {
    use service_com_amazonaws_drs_elasticdisasterrecoveryservice::ops::types::{
        IdentificationHints, NetworkInterface,
    };
    use service_com_amazonaws_drs_elasticdisasterrecoveryservice::state::{
        self, SourceServerRecord,
    };

    // Ids chosen so the store's key order (and therefore the reported order) is the
    // order they are listed here.
    let by_instance = "s-00000000000000001";
    let by_vmware = "s-00000000000000002";
    let by_mac = "s-00000000000000003";
    let bare = "s-00000000000000004";

    let instance_id = "i-0123456789abcdef0";
    let vmware_uuid = "564d1234-5678-9abc-def0-123456789abc";
    let instance_mac = "00:11:22:33:44:55";
    let other_mac = "aa:bb:cc:dd:ee:ff";
    let account_one = "111111111111";
    let account_two = "222222222222";

    let (router, engine) = drs_with_engine();
    initialize(&router).await;

    {
        let store = state::service_state(&*engine).expect("the DRS store");
        let mut store = store.lock();
        for (id, hints, nics, account) in [
            (
                by_instance,
                Some(IdentificationHints {
                    aws_instance_id: Some(instance_id.to_string()),
                    ..IdentificationHints::default()
                }),
                vec![NetworkInterface {
                    mac_address: Some(instance_mac.to_string()),
                    ..NetworkInterface::default()
                }],
                Some(account_one.to_string()),
            ),
            (
                by_vmware,
                Some(IdentificationHints {
                    vm_ware_uuid: Some(vmware_uuid.to_string()),
                    ..IdentificationHints::default()
                }),
                Vec::new(),
                None,
            ),
            (
                by_mac,
                None,
                vec![NetworkInterface {
                    mac_address: Some(other_mac.to_string()),
                    ..NetworkInterface::default()
                }],
                Some(account_two.to_string()),
            ),
            (bare, None, Vec::new(), None),
        ] {
            store.source_servers.insert(
                id.to_string(),
                SourceServerRecord {
                    source_server_id: id.to_string(),
                    arn: engine::emulator_arn("drs", REGION, &format!("source-server/{id}")),
                    identification_hints: hints,
                    network_interfaces: nics,
                    staging_account_id: account,
                    ..SourceServerRecord::default()
                },
            );
        }
    }

    let everything = vec![by_instance, by_vmware, by_mac, bare];
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no filters at all", json!({}), everything.clone()),
        (
            "an empty filters structure",
            json!({"filters": {}}),
            everything.clone(),
        ),
        (
            "two ids",
            json!({"filters": {"sourceServerIDs": [by_vmware, bare]}}),
            vec![by_vmware, bare],
        ),
        (
            "one id",
            json!({"filters": {"sourceServerIDs": [by_mac]}}),
            vec![by_mac],
        ),
        (
            "an id the store does not hold",
            json!({"filters": {"sourceServerIDs": ["s-99999999999999999"]}}),
            Vec::new(),
        ),
        (
            "an empty id list",
            json!({"filters": {"sourceServerIDs": []}}),
            everything.clone(),
        ),
        (
            "hardwareId as an EC2 instance id",
            json!({"filters": {"hardwareId": instance_id}}),
            vec![by_instance],
        ),
        (
            "hardwareId as a VMware UUID",
            json!({"filters": {"hardwareId": vmware_uuid}}),
            vec![by_vmware],
        ),
        (
            "hardwareId as the MAC of a server known by its instance id too",
            json!({"filters": {"hardwareId": instance_mac}}),
            vec![by_instance],
        ),
        (
            "hardwareId as the MAC of a server known by nothing else",
            json!({"filters": {"hardwareId": other_mac}}),
            vec![by_mac],
        ),
        (
            "hardwareId nothing is known by",
            json!({"filters": {"hardwareId": "i-fffffffffffffffff"}}),
            Vec::new(),
        ),
        (
            "an empty hardwareId",
            json!({"filters": {"hardwareId": ""}}),
            everything.clone(),
        ),
        (
            "one staging account",
            json!({"filters": {"stagingAccountIDs": [account_one]}}),
            vec![by_instance],
        ),
        (
            "two staging accounts",
            json!({"filters": {"stagingAccountIDs": [account_one, account_two]}}),
            vec![by_instance, by_mac],
        ),
        (
            "a staging account no server was extended from",
            json!({"filters": {"stagingAccountIDs": ["999999999999"]}}),
            Vec::new(),
        ),
        (
            "an empty staging account list",
            json!({"filters": {"stagingAccountIDs": []}}),
            everything.clone(),
        ),
        (
            "ids AND a staging account, which must intersect",
            json!({"filters": {
                "sourceServerIDs": [by_instance, by_mac],
                "stagingAccountIDs": [account_two],
            }}),
            vec![by_mac],
        ),
        (
            "ids AND a hardwareId that select nothing together",
            json!({"filters": {
                "sourceServerIDs": [by_vmware],
                "hardwareId": instance_id,
            }}),
            Vec::new(),
        ),
    ];

    for (case, body, expected) in &cases {
        let answer = call(&router, "DescribeSourceServers", body).await;
        assert_eq!(answer.status, 200, "case {case}: expected 200, got {answer:?}");
        let reported: Vec<String> = items(&answer)
            .iter()
            .map(|item| item["sourceServerID"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported, *expected,
            "case {case}: expected {expected:?}, got {reported:?}"
        );
    }

    // The filtered read is still paginated, and still reports each match once.
    let mut walked = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut body = json!({
            "filters": {"stagingAccountIDs": [account_one, account_two]},
            "maxResults": 1,
        });
        if let Some(token) = &next {
            merge(&mut body, &json!({"nextToken": token}));
        }
        let answer = call(&router, "DescribeSourceServers", &body).await;
        assert_eq!(answer.status, 200, "paged filter: expected 200, got {answer:?}");
        assert!(
            items(&answer).len() <= 1,
            "paged filter: maxResults 1 must serve at most one item, got {answer:?}"
        );
        for item in items(&answer) {
            walked.push(item["sourceServerID"].as_str().unwrap_or_default().to_string());
        }
        match answer.member("nextToken").and_then(Value::as_str) {
            Some(token) => next = Some(token.to_string()),
            None => break,
        }
    }
    assert_eq!(
        walked,
        vec![by_instance.to_string(), by_mac.to_string()],
        "paged filter: expected the two extended servers once each, got {walked:?}"
    );
}

// ---------------------------------------------------------------------------
// Modelled input constraints
// ---------------------------------------------------------------------------

/// A request outside a modelled constraint is refused with 400
/// `ValidationException` — never stored, and never a 501. The rows walk each
/// constraint the create input declares: the `stagingAreaSubnetId` pattern, the
/// `pitPolicy` length(3,3) on both sides, the `bandwidthThrottling` range(min 0),
/// the `ebsEncryption` enumeration, and `maxResults`' range(min 1) on the read.
#[tokio::test]
async fn requests_outside_a_modelled_constraint_are_refused() {
    let router = drs();
    initialize(&router).await;

    let three = |n: i64| {
        json!([
            {"ruleID": n, "units": "MINUTE", "interval": 10, "retentionDuration": 60},
            {"ruleID": n + 1, "units": "HOUR", "interval": 1, "retentionDuration": 24},
            {"ruleID": n + 2, "units": "DAY", "interval": 1, "retentionDuration": 7},
        ])
    };

    let mut four_rules = three(1);
    four_rules
        .as_array_mut()
        .expect("an array of rules")
        .push(json!({"ruleID": 4, "units": "DAY", "interval": 1, "retentionDuration": 7}));

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "stagingAreaSubnetId off pattern",
            "CreateReplicationConfigurationTemplate",
            create_body("vpc-0123456789abcdef0", json!({})),
        ),
        (
            "pitPolicy below length 3",
            "CreateReplicationConfigurationTemplate",
            create_body(
                "subnet-0123456789abcdef0",
                json!({"pitPolicy": [
                    {"ruleID": 1, "units": "MINUTE", "interval": 10, "retentionDuration": 60},
                    {"ruleID": 2, "units": "HOUR", "interval": 1, "retentionDuration": 24},
                ]}),
            ),
        ),
        (
            "pitPolicy above length 3",
            "CreateReplicationConfigurationTemplate",
            create_body("subnet-0123456789abcdef0", json!({"pitPolicy": four_rules})),
        ),
        (
            "bandwidthThrottling below range",
            "CreateReplicationConfigurationTemplate",
            create_body("subnet-0123456789abcdef0", json!({"bandwidthThrottling": -1})),
        ),
        (
            "ebsEncryption off enumeration",
            "CreateReplicationConfigurationTemplate",
            create_body("subnet-0123456789abcdef0", json!({"ebsEncryption": "MAYBE"})),
        ),
        (
            "maxResults below range",
            "DescribeReplicationConfigurationTemplates",
            json!({"maxResults": 0}),
        ),
        (
            "template id off pattern",
            "UpdateReplicationConfigurationTemplate",
            json!({"replicationConfigurationTemplateID": "rct-short"}),
        ),
    ];

    for (case, operation, body) in &cases {
        let answer = call(&router, operation, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case}: {answer:?}"
        );
    }

    // The refusals stored nothing: the exact boundary value still creates, and it
    // is the only template in the store.
    let ok = call(
        &router,
        "CreateReplicationConfigurationTemplate",
        &create_body(
            "subnet-0123456789abcdef0",
            json!({"bandwidthThrottling": 0, "pitPolicy": three(10)}),
        ),
    )
    .await;
    assert_eq!(ok.status, 201, "boundary create: {ok:?}");
    let listed = call(
        &router,
        "DescribeReplicationConfigurationTemplates",
        &json!({}),
    )
    .await;
    assert_eq!(
        items(&listed).len(),
        1,
        "a refused request must store nothing: {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Server-side bounds
// ---------------------------------------------------------------------------

/// The service bounds what a caller can make it hold and what one response can
/// carry:
///
/// * the store accepts `REPLICATION_CONFIGURATION_TEMPLATE_QUOTA` templates and
///   refuses the next create with the modelled
///   `ServiceQuotaExceededException` (402) — the model gives
///   `CreateReplicationConfigurationTemplate` that error, and no quota of the
///   emulator's is reported as any other status;
/// * `maxResults` is modelled `@range(min: 1)` with NO maximum, so one page is
///   capped at `PAGE_LIMIT` server-side: whatever the caller asks for — nothing,
///   a value under the cap, the cap itself, or far above it — the page holds at
///   most `PAGE_LIMIT` items and the rest is reached through `nextToken`.
///
/// A refusal at the quota must also leave the store readable and unchanged.
#[tokio::test]
async fn the_store_and_every_page_are_bounded() {
    use service_com_amazonaws_drs_elasticdisasterrecoveryservice::state::{
        PAGE_LIMIT, REPLICATION_CONFIGURATION_TEMPLATE_QUOTA as QUOTA,
    };

    let router = drs();
    initialize(&router).await;

    for index in 0..QUOTA {
        let body = create_body(
            "subnet-0123456789abcdef0",
            json!({"bandwidthThrottling": index}),
        );
        let answer = call(&router, "CreateReplicationConfigurationTemplate", &body).await;
        assert_eq!(
            answer.status, 201,
            "case create #{index} (under the quota of {QUOTA}): expected 201, got {answer:?}"
        );
    }

    // One past the bound: the modelled quota error, not a 500 and not an endlessly
    // growing store.
    let refused = call(
        &router,
        "CreateReplicationConfigurationTemplate",
        &create_body("subnet-0123456789abcdef0", json!({})),
    )
    .await;
    assert_eq!(
        refused.status, 402,
        "case create #{QUOTA} (at the quota): expected 402, got {refused:?}"
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case create #{QUOTA} (at the quota): {refused:?}"
    );

    // Each row is what the caller asks for; the page is what the service serves.
    let page_limit = usize::try_from(PAGE_LIMIT).expect("the page limit fits in usize");
    let cases: Vec<(&str, Value, usize)> = vec![
        ("no maxResults", json!({}), page_limit),
        ("maxResults below the cap", json!({"maxResults": 7}), 7),
        (
            "maxResults at the cap",
            json!({"maxResults": PAGE_LIMIT}),
            page_limit,
        ),
        (
            "maxResults one past the cap",
            json!({"maxResults": PAGE_LIMIT + 1}),
            page_limit,
        ),
        (
            "maxResults far above the cap",
            json!({"maxResults": 1_000_000}),
            page_limit,
        ),
    ];

    for (case, body, expected) in &cases {
        for operation in [
            "DescribeReplicationConfigurationTemplates",
            "DescribeSourceServers",
        ] {
            let answer = call(&router, operation, body).await;
            assert_eq!(
                answer.status, 200,
                "case {operation} {case}: expected 200, got {answer:?}"
            );
            // The source-server store is empty, so its page is bounded by the store
            // rather than by the cap; the template store holds QUOTA items.
            let bound = if operation == "DescribeSourceServers" {
                0
            } else {
                *expected
            };
            assert_eq!(
                items(&answer).len(),
                bound,
                "case {operation} {case}: expected {bound} items, got {answer:?}"
            );
        }
    }

    // The store survived the refusal intact: walking the capped pages still reports
    // exactly QUOTA templates, each once.
    let mut walked = BTreeSet::new();
    let mut next: Option<String> = None;
    loop {
        let body = match &next {
            None => json!({}),
            Some(token) => json!({"nextToken": token}),
        };
        let answer = call(&router, "DescribeReplicationConfigurationTemplates", &body).await;
        assert!(
            items(&answer).len() <= page_limit,
            "walking the store: a page may hold at most {page_limit} items, got {answer:?}"
        );
        for item in items(&answer) {
            let id = item["replicationConfigurationTemplateID"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(walked.insert(id.clone()), "walking the store: {id} twice");
        }
        match answer.member("nextToken").and_then(Value::as_str) {
            Some(token) => next = Some(token.to_string()),
            None => break,
        }
    }
    assert_eq!(
        walked.len(),
        QUOTA,
        "walking the store: expected {QUOTA} templates, saw {}",
        walked.len()
    );
}
