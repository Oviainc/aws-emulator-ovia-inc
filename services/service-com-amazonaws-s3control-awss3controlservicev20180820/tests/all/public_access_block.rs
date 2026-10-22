// SPDX-License-Identifier: CC0-1.0

//! The S3 Control account-level `PublicAccessBlock` configuration, driven end to
//! end over HTTP.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes it
//! per the Smithy model, runs the handler and serializes the restXml response.
//! Nothing calls a handler directly, so a passing row here is a claim about the
//! wire.
//!
//! The rule under test: an account has AT MOST ONE `PublicAccessBlock`
//! configuration; `PutPublicAccessBlock` creates or replaces it,
//! `GetPublicAccessBlock` reports exactly the settings last put (a member the
//! request omits reading `false`, the model's `@default`), and
//! `DeletePublicAccessBlock` returns the account to having none — which
//! `GetPublicAccessBlock` answers `NoSuchPublicAccessBlockConfiguration` (404)
//! for, a different answer from a configuration whose four settings are all
//! `false`. The table below spans that class: every one of the sixteen
//! combinations of the four settings, the subsets of them a request may carry at
//! all, and two accounts.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// The account whose configuration most rows below write and read.
const ACCOUNT: &str = "123456789012";

/// A second account, used to pin that one account's configuration is not
/// another's.
const OTHER_ACCOUNT: &str = "210987654321";

/// The XML namespace this service's model declares.
const XMLNS: &str = "http://awss3control.amazonaws.com/doc/2018-08-20/";

/// The URI both the account-level reads and writes are bound to
/// (`smithy_query(query="operation",operation="GetPublicAccessBlock")`).
const URI: &str = "/v20180820/configuration/publicAccessBlock";

/// The four members of `PublicAccessBlockConfiguration`, in the order the shape
/// declares them. SOURCE: the model's
/// `com.amazonaws.s3control#PublicAccessBlockConfiguration` — each member
/// targets `Setting` (boolean) and carries `@default: false`.
const SETTINGS: [&str; 4] = [
    "BlockPublicAcls",
    "IgnorePublicAcls",
    "BlockPublicPolicy",
    "RestrictPublicBuckets",
];

/// The wire error code `GetPublicAccessBlock` answers for an account that has no
/// configuration, with HTTP 404 (the model's
/// `NoSuchPublicAccessBlockConfiguration`, `@httpError(404)`).
const NO_SUCH_CONFIGURATION: &str = "NoSuchPublicAccessBlockConfiguration";

/// One response: the status and the body, with readers for its XML elements.
struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    /// The text of the first `<element>…</element>` in the body, or `None` when
    /// the body carries none.
    ///
    /// The emulator writes a flat document: no attributes on these elements, no
    /// CDATA, no layout whitespace between tags, and the values read here
    /// (`true`, `false`, an error code) carry no character needing escaping.
    fn text(&self, element: &str) -> Option<String> {
        let open = format!("<{element}>");
        let close = format!("</{element}>");
        let start = self.body.find(&open)? + open.len();
        let end = self.body[start..].find(&close)?;
        Some(self.body[start..start + end].to_string())
    }

    /// The S3-style error code (`<Code>`) of a failure response.
    fn code(&self) -> String {
        self.text("Code")
            .unwrap_or_else(|| format!("<no Code element> in {}", self.body))
    }

    /// The four settings this reply reports, as the text of each element, in the
    /// order [`SETTINGS`] declares them. An element the reply omits reads
    /// `None`.
    fn settings(&self) -> [Option<String>; 4] {
        [
            self.text(SETTINGS[0]),
            self.text(SETTINGS[1]),
            self.text(SETTINGS[2]),
            self.text(SETTINGS[3]),
        ]
    }
}

/// One emulator: a router with this service mounted on a single shared engine,
/// exactly as `core/router` assembles it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn start() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These requests carry an unsigned `Authorization` header, which is what
        // makes them route to the `s3` signing name; signature verification is
        // the transport's concern, not this suite's.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_s3control_awss3controlservicev20180820::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    async fn call(&self, method: &str, account_id: &str, body: &str) -> Reply {
        let request = ::http::Request::builder()
            .method(method)
            .uri(URI)
            .header(
                "host",
                format!("{account_id}.s3-control.{REGION}.amazonaws.com"),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/s3/aws4_request, \
                     SignedHeaders=host, Signature=abc"
                ),
            )
            .header("x-amz-account-id", account_id)
            .header("content-type", "application/xml")
            .body(::bytes::Bytes::from(body.to_owned()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        Reply {
            status: response.status().as_u16(),
            body: String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body"),
        }
    }

    /// `PutPublicAccessBlock` with all four settings spelled out.
    async fn put(&self, account_id: &str, settings: [bool; 4]) -> Reply {
        let members: String = SETTINGS
            .iter()
            .zip(settings)
            .map(|(member, value)| format!("<{member}>{value}</{member}>"))
            .collect();
        self.put_body(account_id, &members).await
    }

    /// `PutPublicAccessBlock` carrying exactly `members` inside the payload
    /// element, so a row can omit a setting the way a client that leaves it
    /// unset does.
    async fn put_body(&self, account_id: &str, members: &str) -> Reply {
        let body = format!(
            "<PublicAccessBlockConfiguration xmlns=\"{XMLNS}\">{members}\
             </PublicAccessBlockConfiguration>"
        );
        self.call("PUT", account_id, &body).await
    }

    async fn get(&self, account_id: &str) -> Reply {
        self.call("GET", account_id, "").await
    }

    async fn delete(&self, account_id: &str) -> Reply {
        self.call("DELETE", account_id, "").await
    }
}

/// Every combination of the four settings: sixteen rows, differing in which
/// settings are on and in how many of them are, with the all-`false` and
/// all-`true` corners among them.
fn every_combination() -> Vec<[bool; 4]> {
    (0..16u8)
        .map(|bits| {
            [
                bits & 1 != 0,
                bits & 2 != 0,
                bits & 4 != 0,
                bits & 8 != 0,
            ]
        })
        .collect()
}

/// The text a reply must carry for each setting of `settings`.
fn expected(settings: [bool; 4]) -> [Option<String>; 4] {
    [
        Some(settings[0].to_string()),
        Some(settings[1].to_string()),
        Some(settings[2].to_string()),
        Some(settings[3].to_string()),
    ]
}

#[tokio::test]
async fn put_then_get_round_trips_every_combination_of_the_four_settings() {
    let emulator = Emulator::start();

    for settings in every_combination() {
        let put = emulator.put(ACCOUNT, settings).await;
        assert_eq!(
            put.status, 200,
            "case {settings:?}: expected PutPublicAccessBlock to answer 200, got {} with body {}",
            put.status, put.body
        );

        let read = emulator.get(ACCOUNT).await;
        assert_eq!(
            read.status, 200,
            "case {settings:?}: expected GetPublicAccessBlock to answer 200 after the put, \
             got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.settings(),
            expected(settings),
            "case {settings:?}: expected the settings just put, got body {}",
            read.body
        );

        // A second read changes nothing a later caller sees: the report is the
        // same configuration.
        let again = emulator.get(ACCOUNT).await;
        assert_eq!(
            again.settings(),
            expected(settings),
            "case {settings:?}: expected a repeat read to report the same settings, got body {}",
            again.body
        );
    }

    // Sixteen puts against one account left one configuration, not sixteen: the
    // last one put is what is reported.
    let last = *every_combination().last().expect("sixteen rows");
    let read = emulator.get(ACCOUNT).await;
    assert_eq!(
        read.settings(),
        expected(last),
        "case {last:?}: expected the LAST put to win, got body {}",
        read.body
    );
}

#[tokio::test]
async fn absence_is_distinct_from_a_configuration_whose_settings_are_all_false() {
    let all_false = [false; 4];
    let emulator = Emulator::start();

    let before = emulator.get(ACCOUNT).await;
    assert_eq!(
        (before.status, before.code().as_str()),
        (404, NO_SUCH_CONFIGURATION),
        "case {:?}: expected a 404 {NO_SUCH_CONFIGURATION} before any put, got {} with body {}",
        "no configuration",
        before.status,
        before.body
    );

    assert_eq!(
        emulator.put(ACCOUNT, all_false).await.status,
        200,
        "case {:?}: expected the all-false put to be accepted",
        "all four settings false"
    );
    let read = emulator.get(ACCOUNT).await;
    assert_eq!(
        read.status, 200,
        "case {:?}: expected 200 rather than {NO_SUCH_CONFIGURATION} for a stored all-false \
         configuration, got {} with body {}",
        "all four settings false", read.status, read.body
    );
    assert_eq!(
        read.settings(),
        expected(all_false),
        "case {:?}: expected the four settings reported as false, got body {}",
        "all four settings false",
        read.body
    );

    let delete = emulator.delete(ACCOUNT).await;
    assert_eq!(
        delete.status, 200,
        "case {:?}: expected DeletePublicAccessBlock to answer 200, got {} with body {}",
        "delete a stored configuration", delete.status, delete.body
    );
    let after = emulator.get(ACCOUNT).await;
    assert_eq!(
        (after.status, after.code().as_str()),
        (404, NO_SUCH_CONFIGURATION),
        "case {:?}: expected the account to hold no configuration after the delete, got {} \
         with body {}",
        "read after delete",
        after.status,
        after.body
    );

    // The delete is idempotent: the model declares no error for it, and an
    // account holding none ends up holding none.
    let repeat = emulator.delete(ACCOUNT).await;
    assert_eq!(
        repeat.status, 200,
        "case {:?}: expected a repeat delete to succeed, got {} with body {}",
        "repeat delete", repeat.status, repeat.body
    );
    let still = emulator.get(ACCOUNT).await;
    assert_eq!(
        (still.status, still.code().as_str()),
        (404, NO_SUCH_CONFIGURATION),
        "case {:?}: expected the account to still hold no configuration, got {} with body {}",
        "read after repeat delete",
        still.status,
        still.body
    );
}

#[tokio::test]
async fn a_setting_the_request_omits_is_stored_and_reported_as_false() {
    let emulator = Emulator::start();

    // Each row carries a DIFFERENT subset of the four members: none of them, one
    // of them, and three of them. The members a row leaves out must read
    // `false`, which is the `@default` the model puts on every one of them.
    let mut rows: Vec<(String, String, [bool; 4])> = vec![(
        "no member at all".to_string(),
        String::new(),
        [false; 4],
    )];
    for (index, member) in SETTINGS.iter().enumerate() {
        let mut settings = [false; 4];
        settings[index] = true;
        rows.push((
            format!("only {member}"),
            format!("<{member}>true</{member}>"),
            settings,
        ));
    }
    // Three of the four, leaving the last one out.
    let three: String = SETTINGS[..3]
        .iter()
        .map(|member| format!("<{member}>true</{member}>"))
        .collect();
    rows.push((
        format!("every member but {}", SETTINGS[3]),
        three,
        [true, true, true, false],
    ));

    for (case, members, settings) in rows {
        let put = emulator.put_body(ACCOUNT, &members).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected PutPublicAccessBlock to accept the payload, got {} with \
             body {}",
            put.status, put.body
        );

        let read = emulator.get(ACCOUNT).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetPublicAccessBlock to answer 200, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.settings(),
            expected(settings),
            "case {case:?}: expected the omitted members to read false, got body {}",
            read.body
        );
    }
}

#[tokio::test]
async fn one_accounts_configuration_is_not_another_accounts() {
    let emulator = Emulator::start();
    let mine = [true, false, true, false];
    let theirs = [false, true, false, true];

    assert_eq!(emulator.put(ACCOUNT, mine).await.status, 200);
    let unset = emulator.get(OTHER_ACCOUNT).await;
    assert_eq!(
        (unset.status, unset.code().as_str()),
        (404, NO_SUCH_CONFIGURATION),
        "case {:?}: one account's put must not give another a configuration, got {} with body {}",
        "second account before its own put",
        unset.status,
        unset.body
    );

    assert_eq!(emulator.put(OTHER_ACCOUNT, theirs).await.status, 200);
    for (case, account, settings) in [
        ("first account", ACCOUNT, mine),
        ("second account", OTHER_ACCOUNT, theirs),
    ] {
        let read = emulator.get(account).await;
        assert_eq!(
            read.settings(),
            expected(settings),
            "case {case:?}: expected the settings this account put, got body {}",
            read.body
        );
    }

    assert_eq!(emulator.delete(OTHER_ACCOUNT).await.status, 200);
    let kept = emulator.get(ACCOUNT).await;
    assert_eq!(
        kept.settings(),
        expected(mine),
        "case {:?}: deleting one account's configuration must leave the other's alone, got \
         body {}",
        "first account after the second was deleted",
        kept.body
    );
    let gone = emulator.get(OTHER_ACCOUNT).await;
    assert_eq!(
        (gone.status, gone.code().as_str()),
        (404, NO_SUCH_CONFIGURATION),
        "case {:?}: expected the deleted account to hold no configuration, got {} with body {}",
        "second account after its delete",
        gone.status,
        gone.body
    );
}

/// A 12-digit account id, as `AccountId`'s `^\d{12}$` requires, distinct per
/// `index`.
fn account_id(index: usize) -> String {
    format!("{:012}", 100_000_000_000u64 + index as u64)
}

#[tokio::test]
async fn the_number_of_stored_configurations_is_bounded_and_a_delete_releases_the_bound() {
    let emulator = Emulator::start();
    // Read from the crate so this test cannot drift onto a stale number.
    let quota =
        service_com_amazonaws_s3control_awss3controlservicev20180820::state::PUBLIC_ACCESS_BLOCK_QUOTA;

    for index in 0..quota {
        let reply = emulator.put(&account_id(index), [true; 4]).await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: expected the put for account {} to be accepted, got {} with body {}",
            "filling the store to its bound",
            account_id(index),
            reply.status,
            reply.body
        );
    }

    // One account past the bound: refused rather than stored.
    let past = account_id(quota);
    let refused = emulator.put(&past, [true; 4]).await;
    assert_eq!(
        (refused.status, refused.code().as_str()),
        (403, "AccessDenied"),
        "case {:?}: expected a 403 AccessDenied past the bound, got {} with body {}",
        "one account past the bound",
        refused.status,
        refused.body
    );
    assert!(
        refused.body.contains(&quota.to_string()),
        "case {:?}: expected the refusal to name the bound {quota}, got body {}",
        "one account past the bound",
        refused.body
    );
    let unstored = emulator.get(&past).await;
    assert_eq!(
        (unstored.status, unstored.code().as_str()),
        (404, NO_SUCH_CONFIGURATION),
        "case {:?}: a refused put must store nothing, got {} with body {}",
        "read the refused account",
        unstored.status,
        unstored.body
    );

    // An account already in the store stays writable: replacing its
    // configuration adds no record.
    let replaced = emulator.put(&account_id(0), [false; 4]).await;
    assert_eq!(
        replaced.status, 200,
        "case {:?}: expected a replace at the bound to be accepted, got {} with body {}",
        "replace at the bound", replaced.status, replaced.body
    );
    assert_eq!(
        emulator.get(&account_id(0)).await.settings(),
        expected([false; 4]),
        "case {:?}: expected the replacing settings to be reported",
        "replace at the bound"
    );

    // A delete releases a slot, so the account refused above is accepted now.
    assert_eq!(emulator.delete(&account_id(1)).await.status, 200);
    let accepted = emulator.put(&past, [true; 4]).await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: expected the put to be accepted once a delete released a slot, got {} \
         with body {}",
        "one account past the bound, after a delete", accepted.status, accepted.body
    );
    assert_eq!(
        emulator.get(&past).await.settings(),
        expected([true; 4]),
        "case {:?}: expected the newly accepted configuration to be reported",
        "one account past the bound, after a delete"
    );
}
