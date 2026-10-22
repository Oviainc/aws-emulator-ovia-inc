// SPDX-License-Identifier: CC0-1.0

//! The user store's commit-and-resolve contract, over the whole wire path.
//!
//! RULE: a user `CreateUser` accepts is committed to its server's user collection
//! under the `(ServerId, UserName)` key the request names, so EVERY later call
//! addressed at that key resolves the stored record — `DescribeUser` answers 200 with
//! it, `ListUsers` enumerates it (paging with a `NextToken` while users remain), and
//! `DeleteUser` answers 200 with an empty body and removes it — while no path ever
//! resolves a user that was never created or was already deleted.
//!
//! Each suite asserts that rule over a TABLE whose rows differ along what the rule
//! generalises over: how many users a server holds, which optional members the create
//! supplied, the shape of the user name, and the page bound a listing uses. The
//! recorded `(one server, one user, HomeDirectory '/probe')` case is one row of it.
//!
//! The not-found refusals are asserted as "a 4xx carrying `ResourceNotFoundException`"
//! rather than one status: which 4xx the Transfer wire answers is the subject of its
//! own open issue, and this rule is about which records resolve, not how a refusal is
//! worded.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; read back out of the ARNs the store built.
const REGION: &str = "us-east-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A role ARN satisfying `Role`'s modelled `len 20..2048` and `^arn:.*role/\S+$`.
const ROLE: &str = "arn:aws:iam::000000000000:role/transfer-s3";

/// A key body satisfying `SshPublicKeyBody`'s modelled pattern
/// `^\s*(ssh|ecdsa)-[a-z0-9-]+[ \t]+(([A-Za-z0-9+/]{4})*...)$`.
const SSH_KEY: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==";

/// The JSON null a missing member reads as.
static NULL: Value = Value::Null;

/// One emulator: the shared engine plus the transfer service mounted on a router.
struct Emulator {
    router: MultiServiceRouter,
}

/// One decoded HTTP answer.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The awsJson error code from `__type`, without any shape-id prefix.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .map(|t| t.rsplit('#').next().unwrap_or(t).to_string())
            .unwrap_or_default()
    }

    fn get(&self, member: &str) -> &Value {
        self.body.get(member).unwrap_or(&NULL)
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_transfer_transferservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn call(&self, op: &str, body: Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("TransferService.{op}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{REGION}/transfer/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the test request body serializes"),
            ))
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "op {op}: response body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer { status, body }
    }

    /// Create a server and return its minted `ServerId`.
    async fn create_server(&self, case: &str, request: Value) -> String {
        let answer = self.call("CreateServer", request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateServer 200, got {} with body {}",
            answer.status, answer.body
        );
        answer
            .get("ServerId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateServer returned no ServerId"))
            .to_string()
    }

    /// `ListUsers` once, asserting a 200, and return `(names, next_token)`.
    async fn list_page(
        &self,
        case: &str,
        server_id: &str,
        max_results: Option<i64>,
        next_token: Option<&str>,
    ) -> (Vec<Value>, Option<String>) {
        let mut request = json!({"ServerId": server_id});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = next_token {
            request["NextToken"] = json!(token);
        }
        let answer = self.call("ListUsers", request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListUsers 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.get("ServerId").as_str(),
            Some(server_id),
            "case {case:?}: expected ListUsers to answer for the requested server, got {:?}",
            answer.get("ServerId")
        );
        let users = answer
            .get("Users")
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: ListUsers answered no Users list: {}", answer.body));
        let token = answer.get("NextToken").as_str().map(str::to_string);
        (users, token)
    }

    /// Every user `ListUsers` reports for `server_id`, walked to exhaustion.
    async fn list_all(&self, case: &str, server_id: &str, max_results: Option<i64>) -> Vec<Value> {
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        // One more walk than there could be pages, so a token that never clears is a
        // failed assertion rather than a hung test.
        for _ in 0..64 {
            let (page, next) = self
                .list_page(case, server_id, max_results, token.as_deref())
                .await;
            all.extend(page);
            match next {
                Some(next) => token = Some(next),
                None => return all,
            }
        }
        panic!("case {case:?}: ListUsers never stopped handing out a NextToken");
    }
}

/// The member a listed/described user reports, as a string.
fn member<'a>(user: &'a Value, name: &str) -> Option<&'a str> {
    user.get(name).and_then(Value::as_str)
}

/// One row's user: the name to create and the extra create members to send.
fn user_row(name: &'static str, extra: Value) -> (&'static str, Value) {
    (name, extra)
}

// ---------------------------------------------------------------------------
// Create -> read back through every path.
// ---------------------------------------------------------------------------

/// RULE: every created user resolves through `DescribeUser`, is enumerated by
/// `ListUsers`, and is removed by `DeleteUser` answering an empty 200 — whatever the
/// server holds and whichever optional members the create supplied.
///
/// The rows differ along the SIZE of the server's user set (one, two, several), the
/// members supplied (none beyond the required pair, a PATH home directory, a LOGICAL
/// one with mappings, tags, a session policy, an SSH key) and the user-name shapes
/// `^[\w][\w@.-]{2,99}$` admits.
#[tokio::test]
async fn every_created_user_resolves_through_describe_list_and_delete() {
    let cases: Vec<(&str, Vec<(&str, Value)>)> = vec![
        (
            "one user, only the required members",
            vec![user_row("abc", json!({}))],
        ),
        (
            // The recorded instance this rule was filed from: a single SFTP user with a
            // PATH home directory of '/probe'.
            "one user with a PATH home directory",
            vec![user_row(
                "sim2realprobetransfersftpuser0c82ee17",
                json!({"HomeDirectory": "/probe", "HomeDirectoryType": "PATH"}),
            )],
        ),
        (
            "two users, one of them tagged",
            vec![
                user_row(
                    "sim2real-probe-transfer-alice-444b3424",
                    json!({"HomeDirectory": "/", "HomeDirectoryType": "PATH"}),
                ),
                user_row(
                    "bob.builder-2",
                    json!({"Tags": [{"Key": "env", "Value": "dev"}]}),
                ),
            ],
        ),
        (
            "several users spanning every optional member",
            vec![
                user_row("carol@example.com", json!({"SshPublicKeyBody": SSH_KEY})),
                user_row(
                    "dave_1",
                    json!({
                        "HomeDirectoryType": "LOGICAL",
                        "HomeDirectoryMappings": [{"Entry": "/", "Target": "/bucket/dave"}],
                    }),
                ),
                user_row(
                    "erin.5",
                    json!({"Policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}"}),
                ),
                user_row("frank", json!({"HomeDirectory": "", "HomeDirectoryType": "PATH"})),
            ],
        ),
    ];

    for (case, rows) in cases {
        let emulator = Emulator::new();
        // The server shape the recorded fixtures create: an S3-domain, PUBLIC,
        // SERVICE_MANAGED SFTP server.
        let server_id = emulator
            .create_server(
                case,
                json!({
                    "Domain": "S3",
                    "EndpointType": "PUBLIC",
                    "Protocols": ["SFTP"],
                    "IdentityProviderType": "SERVICE_MANAGED",
                }),
            )
            .await;

        // CREATE — each row is accepted and answers with the key it was filed under.
        for (name, extra) in &rows {
            let mut request = extra.clone();
            request["ServerId"] = json!(&server_id);
            request["UserName"] = json!(name);
            request["Role"] = json!(ROLE);
            let created = emulator.call("CreateUser", request).await;
            assert_eq!(
                created.status, 200,
                "case {case:?} user {name:?}: expected CreateUser 200, got {} with body {}",
                created.status, created.body
            );
            assert_eq!(
                (
                    created.get("ServerId").as_str(),
                    created.get("UserName").as_str()
                ),
                (Some(server_id.as_str()), Some(*name)),
                "case {case:?} user {name:?}: expected CreateUser to answer its own key, got {}",
                created.body
            );
        }

        // DESCRIBE — the stored record, not an echo of the describe request.
        for (name, extra) in &rows {
            let answer = emulator
                .call(
                    "DescribeUser",
                    json!({"ServerId": &server_id, "UserName": name}),
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} user {name:?}: expected DescribeUser 200, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.get("ServerId").as_str(),
                Some(server_id.as_str()),
                "case {case:?} user {name:?}: expected the described ServerId, got {:?}",
                answer.get("ServerId")
            );
            let user = answer.get("User").clone();
            assert_eq!(
                member(&user, "UserName"),
                Some(*name),
                "case {case:?} user {name:?}: expected the stored UserName, got {user}"
            );
            assert_eq!(
                member(&user, "Role"),
                Some(ROLE),
                "case {case:?} user {name:?}: expected the stored Role, got {user}"
            );
            assert_eq!(
                member(&user, "Arn"),
                Some(
                    format!("arn:aws:transfer:{REGION}:{ACCOUNT}:user/{server_id}/{name}").as_str()
                ),
                "case {case:?} user {name:?}: expected the stored user ARN, got {user}"
            );
            assert_eq!(
                user.get("HomeDirectory"),
                extra.get("HomeDirectory"),
                "case {case:?} user {name:?}: expected the HomeDirectory the create \
                 supplied, got {user}"
            );
            let keys = user
                .get("SshPublicKeys")
                .and_then(Value::as_array)
                .map(Vec::len);
            let supplied = usize::from(extra.get("SshPublicKeyBody").is_some());
            assert_eq!(
                keys,
                Some(supplied),
                "case {case:?} user {name:?}: expected {supplied} stored SSH key(s), got {user}"
            );
            // `Tags` and `SshPublicKeys` report the stored lists, EMPTY rather than
            // absent for a user that has none — the shape the recorded `DescribeUser`
            // response carries.
            let want_tags = extra.get("Tags").cloned().unwrap_or_else(|| json!([]));
            assert_eq!(
                user.get("Tags").cloned().unwrap_or(Value::Null),
                want_tags,
                "case {case:?} user {name:?}: expected Tags to report the stored list \
                 {want_tags}, got {user}"
            );
        }

        // LIST — exactly the created set, each entry rebuilt from the stored record.
        let listed = emulator.list_all(case, &server_id, None).await;
        let listed_names: Vec<&str> = listed.iter().filter_map(|u| member(u, "UserName")).collect();
        let mut want: Vec<&str> = rows.iter().map(|(name, _)| *name).collect();
        want.sort_unstable();
        assert_eq!(
            listed_names, want,
            "case {case:?}: expected ListUsers to enumerate {want:?}, got {listed_names:?}"
        );
        for user in &listed {
            let name = member(user, "UserName").unwrap_or_default();
            let extra = &rows
                .iter()
                .find(|(row, _)| *row == name)
                .unwrap_or_else(|| panic!("case {case:?}: ListUsers invented the user {name:?}"))
                .1;
            assert_eq!(
                member(user, "Arn"),
                Some(
                    format!("arn:aws:transfer:{REGION}:{ACCOUNT}:user/{server_id}/{name}").as_str()
                ),
                "case {case:?} user {name:?}: expected the stored ARN in the listing, got {user}"
            );
            assert_eq!(
                member(user, "Role"),
                Some(ROLE),
                "case {case:?} user {name:?}: expected the stored Role in the listing, got {user}"
            );
            assert_eq!(
                user.get("SshPublicKeyCount").and_then(Value::as_i64),
                Some(i64::from(extra.get("SshPublicKeyBody").is_some())),
                "case {case:?} user {name:?}: expected the live SshPublicKeyCount, got {user}"
            );
        }

        // DELETE — an empty 200, and the record is gone from every path afterwards.
        let mut remaining: Vec<&str> = want.clone();
        for (name, _) in &rows {
            let deleted = emulator
                .call(
                    "DeleteUser",
                    json!({"ServerId": &server_id, "UserName": name}),
                )
                .await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?} user {name:?}: expected DeleteUser 200, got {} with body {}",
                deleted.status, deleted.body
            );
            assert_eq!(
                deleted.body,
                json!({}),
                "case {case:?} user {name:?}: expected DeleteUser to answer an empty body, got {}",
                deleted.body
            );

            let gone = emulator
                .call(
                    "DescribeUser",
                    json!({"ServerId": &server_id, "UserName": name}),
                )
                .await;
            assert!(
                (400..500).contains(&gone.status) && gone.error_code() == "ResourceNotFoundException",
                "case {case:?} user {name:?}: expected DescribeUser to refuse a deleted user \
                 with a 4xx ResourceNotFoundException, got {} with body {}",
                gone.status,
                gone.body
            );
            let twice = emulator
                .call(
                    "DeleteUser",
                    json!({"ServerId": &server_id, "UserName": name}),
                )
                .await;
            assert!(
                (400..500).contains(&twice.status)
                    && twice.error_code() == "ResourceNotFoundException",
                "case {case:?} user {name:?}: expected a second DeleteUser to refuse with a 4xx \
                 ResourceNotFoundException, got {} with body {}",
                twice.status,
                twice.body
            );

            remaining.retain(|candidate| candidate != name);
            let still = emulator.list_all(case, &server_id, None).await;
            let still_names: Vec<&str> =
                still.iter().filter_map(|u| member(u, "UserName")).collect();
            assert_eq!(
                still_names, remaining,
                "case {case:?} user {name:?}: expected ListUsers to enumerate {remaining:?} after \
                 the delete, got {still_names:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination over the stored set.
// ---------------------------------------------------------------------------

/// RULE: `ListUsers` pages the STORED user set — a bounded call answers at most
/// `MaxResults` users and a `NextToken` while users remain, the token resumes exactly
/// after the last one rendered, and the walk over any bound yields the whole set once.
///
/// The rows are the page bounds: both published bounds of `MaxResults`' modelled
/// `range 1..1000`, bounds below, equal to and above the stored count, and no bound at
/// all.
#[tokio::test]
async fn list_users_pages_the_stored_set_and_emits_a_token_while_more_remain() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server("pagination", json!({})).await;

    let all: Vec<String> = (1..=5).map(|n| format!("u{n:02}")).collect();
    for name in &all {
        let created = emulator
            .call(
                "CreateUser",
                json!({"ServerId": &server_id, "UserName": name, "Role": ROLE}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "pagination setup {name:?}: expected CreateUser 200, got {} with body {}",
            created.status, created.body
        );
    }

    // `MaxResults` is modelled `@range(min: 1, max: 1000)`: both bounds are rows, as
    // are a bound below, at and above the stored count, and no bound at all.
    let bounds: Vec<Option<i64>> = vec![Some(1), Some(2), Some(4), Some(5), Some(6), Some(1000), None];
    for bound in bounds {
        let case = format!("MaxResults={bound:?}");
        let (first, token) = emulator
            .list_page(&case, &server_id, bound, None)
            .await;
        let want_first = bound.map_or(all.len(), |b| usize::try_from(b).unwrap_or(all.len()))
            .min(all.len());
        assert_eq!(
            first.len(),
            want_first,
            "case {case:?}: expected a first page of {want_first} user(s), got {first:?}"
        );
        let more_remain = want_first < all.len();
        assert_eq!(
            token.is_some(),
            more_remain,
            "case {case:?}: expected a NextToken to be {} after a page of {want_first}, got {token:?}",
            if more_remain { "present" } else { "absent" }
        );

        let walked = emulator.list_all(&case, &server_id, bound).await;
        let walked_names: Vec<&str> = walked.iter().filter_map(|u| member(u, "UserName")).collect();
        assert_eq!(
            walked_names, all,
            "case {case:?}: expected the paged walk to yield the stored set once, got \
             {walked_names:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Nothing is invented for a user that was never created.
// ---------------------------------------------------------------------------

/// RULE (the other half): a user name that was never created on a server resolves
/// NOWHERE — the read paths answer `ResourceNotFoundException` rather than
/// materialising a record, and a server with no users lists none.
///
/// The rows are names never created on this server: a plain one, one created on a
/// DIFFERENT server, and one whose name differs only in case from a created user.
#[tokio::test]
async fn an_uncreated_user_is_resolved_by_no_path() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server("uncreated", json!({})).await;
    let other_server = emulator.create_server("uncreated-other", json!({})).await;

    // A server with no users lists none — and listing does not create one.
    let empty = emulator.list_all("empty server", &server_id, None).await;
    assert!(
        empty.is_empty(),
        "case \"empty server\": expected ListUsers to enumerate nothing, got {empty:?}"
    );

    let created = emulator
        .call(
            "CreateUser",
            json!({"ServerId": &other_server, "UserName": "elsewhere", "Role": ROLE}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "setup: expected CreateUser 200 on the other server, got {} with body {}",
        created.status, created.body
    );
    let mine = emulator
        .call(
            "CreateUser",
            json!({"ServerId": &server_id, "UserName": "lower_case", "Role": ROLE}),
        )
        .await;
    assert_eq!(
        mine.status, 200,
        "setup: expected CreateUser 200, got {} with body {}",
        mine.status, mine.body
    );

    for name in ["never_created", "elsewhere", "LOWER_CASE"] {
        for op in ["DescribeUser", "DeleteUser"] {
            let answer = emulator
                .call(op, json!({"ServerId": &server_id, "UserName": name}))
                .await;
            assert!(
                (400..500).contains(&answer.status)
                    && answer.error_code() == "ResourceNotFoundException",
                "case {name:?}: expected {op} to refuse an uncreated user with a 4xx \
                 ResourceNotFoundException, got {} with body {}",
                answer.status,
                answer.body
            );
        }
    }

    // The refusals changed nothing: the one real user is still the whole set.
    let listed = emulator.list_all("after the refusals", &server_id, None).await;
    let names: Vec<&str> = listed.iter().filter_map(|u| member(u, "UserName")).collect();
    assert_eq!(
        names,
        vec!["lower_case"],
        "case \"after the refusals\": expected the stored set to be unchanged, got {names:?}"
    );
}
