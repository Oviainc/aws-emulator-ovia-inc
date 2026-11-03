// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of what `GetMembers` answers for an account id the
//! calling hub holds no member association for: `EnableSecurityHub` →
//! `CreateMembers` → `DeleteMembers` → `GetMembers`.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized result that is wrong on the wire fails here.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region most cases sign for; the handler reads it out of the SigV4 credential
/// scope, exactly as a signed SDK request carries it.
const REGION: &str = "us-east-1";

/// A second Region, to pin that an association belongs to the hub of ONE Region.
const OTHER_REGION: &str = "us-west-2";

/// The account every emulated caller presents — the administrator account of every
/// association these cases create.
const ADMINISTRATOR_ID: &str = "000000000000";

/// The batch bound every member operation holds: the length the model puts on
/// `CreateMembers`' own `AccountDetailsList` ("len 1..50",
/// `smithy_query(query="operation", operation="CreateMembers")`).
const MAX_BATCH: usize = 50;

/// One emulator with the Security Hub service mounted, driven over HTTP.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_securityhub_securityhubapiservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch one request signed for `region` and return `(status, parsed body)`.
    async fn call(
        &self,
        method: &str,
        path: &str,
        region: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
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

    async fn enable(&self, region: &str) -> (u16, Value) {
        self.call("POST", "/accounts", region, Some(json!({}))).await
    }

    async fn create_members(&self, region: &str, account_ids: &[&str]) -> (u16, Value) {
        let details: Vec<Value> = account_ids
            .iter()
            .map(|id| json!({ "AccountId": id, "Email": format!("{id}@example.com") }))
            .collect();
        self.call(
            "POST",
            "/members",
            region,
            Some(json!({ "AccountDetails": details })),
        )
        .await
    }

    async fn delete_members(&self, region: &str, account_ids: &[&str]) -> (u16, Value) {
        self.call(
            "POST",
            "/members/delete",
            region,
            Some(json!({ "AccountIds": account_ids })),
        )
        .await
    }

    async fn get_members(&self, region: &str, account_ids: &[&str]) -> (u16, Value) {
        self.call(
            "POST",
            "/members/get",
            region,
            Some(json!({ "AccountIds": account_ids })),
        )
        .await
    }

    /// `ListMembers`; every parameter is `@httpQuery`-bound, and this suite sends none.
    async fn list_members(&self, region: &str) -> (u16, Value) {
        self.call("GET", "/members", region, None).await
    }

    /// A harness whose caller is subscribed in `REGION`.
    async fn enabled() -> Self {
        let harness = Self::new();
        let (status, body) = harness.enable(REGION).await;
        assert_eq!(
            status, 200,
            "EnableSecurityHub must succeed, got {status} with {body}"
        );
        harness
    }
}

/// A 12-digit account id built from `n`, so a case can span batch sizes without a
/// hand-written list.
fn account_id(n: usize) -> String {
    format!("{:012}", 100_000_000_000u64 + n as u64)
}

/// The account ids a `Members` list reports, in response order.
fn member_ids(body: &Value) -> Vec<&str> {
    body["Members"]
        .as_array()
        .unwrap_or_else(|| panic!("GetMembers must answer a Members list, got {body}"))
        .iter()
        .map(|member| {
            member["AccountId"]
                .as_str()
                .unwrap_or_else(|| panic!("every Member must carry an AccountId, got {member}"))
        })
        .collect()
}

/// The `UnprocessedAccounts` list of a response, which must always be carried.
fn unprocessed_of(body: &Value) -> &Vec<Value> {
    body["UnprocessedAccounts"]
        .as_array()
        .unwrap_or_else(|| panic!("response must carry UnprocessedAccounts, got {body}"))
}

/// RULE: `GetMembers` reports stored member associations — it answers a `Members` entry
/// for every requested account id the calling hub holds an association for, in the
/// order asked, and reports an id it holds NO association for in neither list:
/// `UnprocessedAccounts` carries only accounts whose association the hub resolved but
/// could not process, never a synthesized "not a member" result.
///
/// The table spans what the rule generalises over: WHY the hub holds no association for
/// an id (never created, created then deleted, the administrator's own account, a
/// member of another Region's hub only), how many unknown ids one call carries, whether
/// an id repeats (known or unknown — a requested id is answered once per occurrence,
/// in request order), where the unknown ids sit among known ones, and the size of the
/// batch up to the bound every member operation holds.
#[tokio::test]
async fn get_members_reports_an_unknown_account_in_neither_list() {
    // The hub these cases read: three live associations, plus one that was created and
    // then deleted, plus one that is a member in `OTHER_REGION` only.
    let live: Vec<String> = (0..3).map(account_id).collect();
    let deleted = account_id(3);
    let never_created = account_id(4);
    let other_region_only = account_id(5);
    let filler: Vec<String> = (10..10 + MAX_BATCH).map(account_id).collect();

    // (case, the ids one GetMembers call asks about, the member ids it must answer)
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "an id that was never a member",
            vec![never_created.as_str()],
            vec![],
        ),
        (
            "an id that was created and then deleted",
            vec![deleted.as_str()],
            vec![],
        ),
        (
            "the administrator's own account id",
            vec![ADMINISTRATOR_ID],
            vec![],
        ),
        (
            "an id that is a member of another Region's hub only",
            vec![other_region_only.as_str()],
            vec![],
        ),
        (
            "the same unknown id twice in one call",
            vec![never_created.as_str(), never_created.as_str()],
            vec![],
        ),
        (
            "the same KNOWN id twice in one call, beside an unknown one",
            vec![live[0].as_str(), never_created.as_str(), live[0].as_str()],
            vec![live[0].as_str(), live[0].as_str()],
        ),
        (
            "every unknown id at once",
            vec![
                never_created.as_str(),
                deleted.as_str(),
                ADMINISTRATOR_ID,
                other_region_only.as_str(),
            ],
            vec![],
        ),
        (
            "unknown ids around the known ones, in request order",
            vec![
                never_created.as_str(),
                live[0].as_str(),
                deleted.as_str(),
                live[1].as_str(),
                ADMINISTRATOR_ID,
            ],
            vec![live[0].as_str(), live[1].as_str()],
        ),
        (
            "known ids in reverse order with an unknown one between them",
            vec![live[2].as_str(), never_created.as_str(), live[0].as_str()],
            vec![live[2].as_str(), live[0].as_str()],
        ),
        (
            "only known ids: nothing is unprocessed either",
            vec![live[0].as_str(), live[1].as_str(), live[2].as_str()],
            vec![live[0].as_str(), live[1].as_str(), live[2].as_str()],
        ),
        (
            "the maximum batch, all of it unknown",
            filler.iter().map(String::as_str).collect(),
            vec![],
        ),
        (
            "the maximum batch, one known id among unknown ones",
            filler
                .iter()
                .map(String::as_str)
                .take(MAX_BATCH - 1)
                .chain(std::iter::once(live[1].as_str()))
                .collect(),
            vec![live[1].as_str()],
        ),
    ];

    for (case, requested, expected) in cases {
        let harness = Harness::enabled().await;
        let (status, body) = harness.enable(OTHER_REGION).await;
        assert_eq!(
            status, 200,
            "case {case:?}: EnableSecurityHub in {OTHER_REGION} expected 200, got {status} \
             with {body}"
        );
        let created: Vec<&str> = live
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(deleted.as_str()))
            .collect();
        let (status, body) = harness.create_members(REGION, &created).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateMembers expected 200, got {status} with {body}"
        );
        let (status, body) = harness
            .create_members(OTHER_REGION, &[other_region_only.as_str()])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateMembers in {OTHER_REGION} expected 200, got {status} \
             with {body}"
        );
        let (status, body) = harness.delete_members(REGION, &[deleted.as_str()]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteMembers expected 200, got {status} with {body}"
        );

        let (status, body) = harness.get_members(REGION, &requested).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetMembers expected 200, got {status} with {body}"
        );
        assert_eq!(
            member_ids(&body),
            expected,
            "case {case:?}: expected the members {expected:?}, got {body}"
        );
        assert!(
            unprocessed_of(&body).is_empty(),
            "case {case:?}: an id the hub holds no association for must be reported in \
             neither list, got {body}"
        );
    }
}

/// RULE (the same rule as an invariant over the association lifecycle): reading an
/// account back through `GetMembers` after `DeleteMembers` removed its association is
/// stable and empty — `Members` and `UnprocessedAccounts` are both empty, every time it
/// is read, until the association is created again, at which point the account resolves
/// through `Members` again. A read never changes what a later read sees.
///
/// The rows differ along how many associations the hub holds when the delete happens,
/// so the invariant is pinned for a single association and for one deleted out of a
/// batch.
#[tokio::test]
async fn a_deleted_member_reads_back_empty_on_every_read() {
    // (case, the ids the hub starts with, the index of the one to delete)
    let rows: Vec<(&str, Vec<String>, usize)> = vec![
        ("the hub's only association", vec![account_id(31)], 0),
        (
            "one of three associations",
            (41..44).map(account_id).collect(),
            1,
        ),
        (
            "one of the maximum batch",
            (100..100 + MAX_BATCH).map(account_id).collect(),
            MAX_BATCH - 1,
        ),
    ];

    for (case, held, index) in rows {
        let harness = Harness::enabled().await;
        let ids: Vec<&str> = held.iter().map(String::as_str).collect();
        let (status, body) = harness.create_members(REGION, &ids).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateMembers expected 200, got {status} with {body}"
        );
        assert!(
            unprocessed_of(&body).is_empty(),
            "case {case:?}: every account must be created, got {body}"
        );
        let target = ids[index];

        // Before the delete the account resolves through `Members`.
        let (status, body) = harness.get_members(REGION, &[target]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetMembers expected 200, got {status} with {body}"
        );
        assert_eq!(
            member_ids(&body),
            vec![target],
            "case {case:?}: {target} is a member before the delete, got {body}"
        );

        let (status, body) = harness.delete_members(REGION, &[target]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteMembers expected 200, got {status} with {body}"
        );

        // `ListMembers` no longer lists it, and neither `GetMembers` list mentions it —
        // on this read or any later one.
        let (status, body) = harness.list_members(REGION).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListMembers expected 200, got {status} with {body}"
        );
        assert!(
            !member_ids(&body).contains(&target),
            "case {case:?}: ListMembers must not list the deleted {target}, got {body}"
        );
        for read in 1..=3 {
            let (status, body) = harness.get_members(REGION, &[target]).await;
            assert_eq!(
                status, 200,
                "case {case:?} read {read}: GetMembers expected 200, got {status} with {body}"
            );
            assert!(
                member_ids(&body).is_empty(),
                "case {case:?} read {read}: the deleted {target} must be no member, got {body}"
            );
            assert!(
                unprocessed_of(&body).is_empty(),
                "case {case:?} read {read}: the deleted {target} must not be reported \
                 unprocessed, got {body}"
            );
        }

        // Creating it again brings it back through `Members`, so the empty answer was
        // the absence of the association and nothing else.
        let (status, body) = harness.create_members(REGION, &[target]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: re-creating {target} expected 200, got {status} with {body}"
        );
        let (status, body) = harness.get_members(REGION, &[target]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetMembers expected 200, got {status} with {body}"
        );
        assert_eq!(
            member_ids(&body),
            vec![target],
            "case {case:?}: the re-created {target} must resolve again, got {body}"
        );
        assert!(
            unprocessed_of(&body).is_empty(),
            "case {case:?}: the re-created {target} must not be unprocessed, got {body}"
        );
    }
}
