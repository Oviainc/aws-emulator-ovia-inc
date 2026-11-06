// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the attribute mapping rules a profile enforces
//! (`PutAttributeMapping`, `DeleteAttributeMapping`, read back through `GetProfile`).
//!
//! The rule under test: one profile holds at most one mapping per certificate field, and a
//! write names ONE field and changes only the specifiers mapped for it — a put adds the
//! submitted specifiers to the ones that field already maps (a specifier already mapped
//! keeps its single entry, which is what makes the write `@idempotent` as the model
//! declares), a delete removes the specifiers it names, or every specifier of the field
//! when it names none, and a field left mapping nothing keeps no entry, because a mapping
//! "specifies a certificate field and one or more specifiers"
//! (`smithy_query(query="doc", shape="PutAttributeMapping")`). Both writes answer with the
//! stored profile, so the next read agrees with them, and both refuse an id that resolves
//! to no profile with this service's ARN-keyed 404 rather than writing anything.
//!
//! Every expectation below is the rule applied to the mappings the profile held BEFORE the
//! call, read back over the wire — never a literal list of mappings — so the suite states
//! the rule rather than one store's starting point.
//!
//! The tables differ along what the rule generalises over: WHICH certificate field is
//! written (every value the modelled enum accepts), how many specifiers one call carries,
//! whether the call repeats or overlaps specifiers the field already maps, which specifiers
//! a delete names (some, all, none, one that was never mapped), and how long a specifier is
//! (the modelled bound and one past it).
//!
//! The harness below is local to this suite, as in each sibling suite.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the suite signs for unless a case varies it.
const REGION: &str = "us-west-2";

/// The account every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// Every value the modelled `CertificateField` enum accepts
/// (`smithy_query(query="shape", shape="CertificateField")` reports
/// `["x509Subject", "x509Issuer", "x509SAN"]`).
const CERTIFICATE_FIELDS: [&str; 3] = ["x509Subject", "x509Issuer", "x509SAN"];

/// The longest specifier the model accepts: `MappingRule$specifier` is `@length(max 60)`
/// (`smithy_query(query="shape", shape="MappingRule")`).
const MAX_SPECIFIER_LEN: usize = 60;

/// Ids shaped like the modelled `Uuid` (`@length(36, 36)`,
/// `@pattern("[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}")`) that no profile is created
/// under, so the boundary validator accepts them and the handler has to resolve them.
const UNMINTED_IDS: [(&str, &str); 3] = [
    ("all-zero-id", "00000000-0000-0000-0000-000000000000"),
    ("hex-words-id", "deadbeef-dead-beef-dead-beefdeadbeef"),
    ("all-f-id", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
];

/// One profile's attribute mappings as `(certificateField, [specifier])` pairs, in the
/// order the response lists them.
type Mappings = Vec<(String, Vec<String>)>;

/// A client over one emulator: one engine, one mounted service, so every request in a test
/// observes the state the previous one wrote.
struct Client {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status, the parsed JSON body and the wire error code.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
    error_code: String,
}

impl Answer {
    fn message(&self) -> Option<&str> {
        self.body.get("message").and_then(Value::as_str)
    }

    /// The `profile` member of a response.
    fn profile(&self) -> &Value {
        self.body.get("profile").unwrap_or(&Value::Null)
    }
}

impl Client {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send_in_region(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
        region: &str,
    ) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/rolesanywhere/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if header_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        self.send_in_region(method, uri, body, REGION).await
    }

    async fn create_profile(&self, name: &str) -> Answer {
        self.send(
            "POST",
            "/profiles",
            Some(json!({
                "name": name,
                "roleArns": [format!("arn:aws:iam::{ACCOUNT}:role/DemoRole")],
            })),
        )
        .await
    }

    /// `PUT /profiles/{profileId}/mappings` — `certificateField` and `mappingRules` travel
    /// in the body (neither member carries an HTTP binding).
    async fn put_mapping_in_region(
        &self,
        profile_id: &str,
        certificate_field: &str,
        specifiers: &[String],
        region: &str,
    ) -> Answer {
        let rules: Vec<Value> = specifiers
            .iter()
            .map(|specifier| json!({"specifier": specifier}))
            .collect();
        self.send_in_region(
            "PUT",
            &format!("/profiles/{profile_id}/mappings"),
            Some(json!({"certificateField": certificate_field, "mappingRules": rules})),
            region,
        )
        .await
    }

    async fn put_mapping(
        &self,
        profile_id: &str,
        certificate_field: &str,
        specifiers: &[String],
    ) -> Answer {
        self.put_mapping_in_region(profile_id, certificate_field, specifiers, REGION)
            .await
    }

    /// `DELETE /profiles/{profileId}/mappings` — `certificateField` and `specifiers` are
    /// `@httpQuery` members, and a list-typed one repeats its name once per element.
    async fn delete_mapping_in_region(
        &self,
        profile_id: &str,
        certificate_field: &str,
        specifiers: Option<&[String]>,
        region: &str,
    ) -> Answer {
        let mut uri =
            format!("/profiles/{profile_id}/mappings?certificateField={certificate_field}");
        for specifier in specifiers.unwrap_or_default() {
            uri.push_str("&specifiers=");
            uri.push_str(specifier);
        }
        self.send_in_region("DELETE", &uri, None, region).await
    }

    async fn delete_mapping(
        &self,
        profile_id: &str,
        certificate_field: &str,
        specifiers: Option<&[String]>,
    ) -> Answer {
        self.delete_mapping_in_region(profile_id, certificate_field, specifiers, REGION)
            .await
    }

    async fn get_profile_in_region(&self, profile_id: &str, region: &str) -> Answer {
        self.send_in_region("GET", &format!("/profile/{profile_id}"), None, region)
            .await
    }

    async fn get_profile(&self, profile_id: &str) -> Answer {
        self.get_profile_in_region(profile_id, REGION).await
    }

    async fn delete_profile(&self, profile_id: &str) -> Answer {
        self.send("DELETE", &format!("/profile/{profile_id}"), None)
            .await
    }

    /// The mappings a stored profile holds right now, read over the wire.
    async fn mappings(&self, case: &str, profile_id: &str) -> Mappings {
        let read = self.get_profile(profile_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetProfile 200, got {read:?}"
        );
        mappings_of(read.profile())
    }
}

/// Create a profile and answer its id.
async fn profile_id(client: &Client, name: &str) -> String {
    let created = client.create_profile(name).await;
    assert_eq!(
        created.status, 201,
        "profile {name:?}: expected CreateProfile 201, got {created:?}"
    );
    created
        .body
        .pointer("/profile/profileId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("profile {name:?}: no profileId in {created:?}"))
        .to_string()
}

/// The ARN a profile id carries in `region`, which is the ARN a refusal naming that id
/// words: `ProfileArn` is
/// `@pattern("^arn:aws(-[^:]+)?:rolesanywhere(:.*){2}(:profile.*)$")`
/// (`smithy_query(query="shape", shape="ProfileDetail")`, member `profileArn`).
fn profile_arn(region: &str, id: &str) -> String {
    format!("arn:aws:rolesanywhere:{region}:{ACCOUNT}:profile/{id}")
}

/// The wording an ARN-keyed miss carries.
fn arn_not_found_message(resource_arn: &str) -> String {
    format!("The item doesn't exist. arn: {resource_arn} ; aborting.")
}

/// One profile's attribute mappings, out of a `ProfileDetail` on the wire.
fn mappings_of(profile: &Value) -> Mappings {
    profile
        .get("attributeMappings")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    let field = entry
                        .get("certificateField")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let specifiers = entry
                        .get("mappingRules")
                        .and_then(Value::as_array)
                        .map(|rules| {
                            rules
                                .iter()
                                .map(|rule| {
                                    rule.get("specifier")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string()
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (field, specifiers)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The specifiers one field maps, or `None` when the profile holds no entry for it.
fn specifiers_in(mappings: &Mappings, certificate_field: &str) -> Option<Vec<String>> {
    mappings
        .iter()
        .find(|(field, _)| field == certificate_field)
        .map(|(_, specifiers)| specifiers.clone())
}

/// The mappings every field OTHER than `certificate_field` holds: what a write naming that
/// one field may not change.
fn other_fields(mappings: &Mappings, certificate_field: &str) -> Mappings {
    mappings
        .iter()
        .filter(|(field, _)| field != certificate_field)
        .cloned()
        .collect()
}

/// The specifiers the rule says a field maps after a put: the ones it already mapped, in
/// the order it mapped them, then each submitted specifier that was not mapped yet, in the
/// order the request carried them, counted once however often it is repeated.
fn specifiers_after_put(before: Option<&Vec<String>>, submitted: &[String]) -> Vec<String> {
    let mut after = before.cloned().unwrap_or_default();
    for specifier in submitted {
        if !after.contains(specifier) {
            after.push(specifier.clone());
        }
    }
    after
}

/// The specifiers the rule says a field maps after a delete, or `None` when the field is
/// left mapping nothing and so keeps no entry: a request that names specifiers removes
/// exactly those, one that names none removes every specifier of the field.
fn specifiers_after_delete(
    before: Option<&Vec<String>>,
    named: Option<&[String]>,
) -> Option<Vec<String>> {
    let named = named.filter(|named| !named.is_empty())?;
    let after: Vec<String> = before
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|specifier| !named.contains(specifier))
        .collect();
    (!after.is_empty()).then_some(after)
}

/// Assert a write's own answer and the next read report the same mappings, and answer them.
async fn written_mappings(
    case: &str,
    client: &Client,
    profile_id: &str,
    written: &Answer,
) -> Mappings {
    assert_eq!(
        written.status, 200,
        "case {case:?}: expected status 200, got {written:?}"
    );
    let from_write = mappings_of(written.profile());
    let from_read = client.mappings(case, profile_id).await;
    assert_eq!(
        from_write, from_read,
        "case {case:?}: the write answered {from_write:?} but the read reports {from_read:?}"
    );
    from_write
}

/// One date-time member of a `ProfileDetail`, as nanoseconds since the epoch.
///
/// `ProfileDetail$createdAt`/`updatedAt` carry `@timestampFormat("date-time")`, so each is
/// an ISO-8601 string on this service's restJson1 wire.
fn instant(case: &str, profile: &Value, member: &str) -> i128 {
    let text = profile
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no string {member} in {profile:?}"));
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {member} {text:?} is not an ISO-8601 date-time: {error}")
        })
        .as_nanos()
}

/// Owned specifiers, so a table row can hold them.
fn owned(specifiers: &[&str]) -> Vec<String> {
    specifiers.iter().map(|s| (*s).to_string()).collect()
}

/// A put adds the specifiers it carries to the field it names, counting a specifier the
/// field already maps once, and changes no other field.
///
/// The table spans every value the modelled `CertificateField` enum accepts, the number of
/// specifiers one call carries, a call that repeats a specifier, specifiers that name a
/// sub-field path, and the longest specifier the model accepts.
#[tokio::test]
async fn put_adds_the_specifiers_it_carries_to_the_field_it_names() {
    let longest = "s".repeat(MAX_SPECIFIER_LEN);
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        ("one-specifier-on-subject", "x509Subject", owned(&["CN"])),
        (
            "three-specifiers-on-subject",
            "x509Subject",
            owned(&["CN", "OU", "UID"]),
        ),
        (
            "repeated-specifier-on-issuer",
            "x509Issuer",
            owned(&["CN", "CN", "O"]),
        ),
        (
            "sub-field-paths-on-san",
            "x509SAN",
            owned(&["SAN/DNS", "SAN/URI"]),
        ),
        (
            "longest-modelled-specifier-on-san",
            "x509SAN",
            vec![longest.clone()],
        ),
    ];

    let client = Client::new();
    for (case, field, submitted) in cases {
        let id = profile_id(&client, case).await;
        let before = client.mappings(case, &id).await;

        let written = client.put_mapping(&id, field, &submitted).await;
        let after = written_mappings(case, &client, &id, &written).await;

        let expected = specifiers_after_put(specifiers_in(&before, field).as_ref(), &submitted);
        assert_eq!(
            specifiers_in(&after, field).as_ref(),
            Some(&expected),
            "case {case:?}: expected {field:?} to map {expected:?}, got {after:?}"
        );
        // Each submitted specifier is mapped exactly once, however often it was carried.
        for specifier in &submitted {
            let occurrences = expected.iter().filter(|held| *held == specifier).count();
            assert_eq!(
                occurrences, 1,
                "case {case:?}: expected {specifier:?} to be mapped once, got {occurrences} in \
                 {expected:?}"
            );
        }
        // A put names one field, so no other field's mapping changed.
        assert_eq!(
            other_fields(&after, field),
            other_fields(&before, field),
            "case {case:?}: a put on {field:?} changed another field, before {before:?}, after \
             {after:?}"
        );
        for other in CERTIFICATE_FIELDS.iter().filter(|other| **other != field) {
            assert_eq!(
                specifiers_in(&after, other),
                specifiers_in(&before, other),
                "case {case:?}: {other:?} was not named by the put, so it must be unchanged, got \
                 {after:?}"
            );
        }
    }
}

/// Putting a field again ADDS to the specifiers it already maps, so a disjoint put grows
/// the field, an overlapping put adds only what was missing, and an identical put changes
/// nothing — one profile walked through a sequence of puts.
#[tokio::test]
async fn putting_a_field_again_adds_to_it_and_repeats_change_nothing() {
    let client = Client::new();
    let id = profile_id(&client, "union").await;

    // (case, field, the specifiers this put carries)
    let steps: Vec<(&str, &str, Vec<String>)> = vec![
        ("first-put", "x509Subject", owned(&["CN"])),
        ("disjoint-put-adds", "x509Subject", owned(&["OU"])),
        (
            "overlapping-put-adds-only-the-new-one",
            "x509Subject",
            owned(&["CN", "UID"]),
        ),
        (
            "identical-put-is-idempotent",
            "x509Subject",
            owned(&["CN", "UID"]),
        ),
        (
            "another-field-is-its-own-entry",
            "x509Issuer",
            owned(&["O", "C"]),
        ),
        (
            "a-put-of-what-is-mapped-changes-nothing",
            "x509Subject",
            owned(&["CN"]),
        ),
    ];

    for (case, field, submitted) in steps {
        let before = client.mappings(case, &id).await;
        let written = client.put_mapping(&id, field, &submitted).await;
        let after = written_mappings(case, &client, &id, &written).await;

        let expected = specifiers_after_put(specifiers_in(&before, field).as_ref(), &submitted);
        assert_eq!(
            specifiers_in(&after, field).as_ref(),
            Some(&expected),
            "case {case:?}: expected {field:?} to map {expected:?}, got {after:?}"
        );
        // Nothing the field mapped before is dropped by a put.
        for specifier in specifiers_in(&before, field).unwrap_or_default() {
            assert!(
                expected.contains(&specifier),
                "case {case:?}: a put must not drop {specifier:?}, got {expected:?}"
            );
        }
        assert_eq!(
            other_fields(&after, field),
            other_fields(&before, field),
            "case {case:?}: a put on {field:?} changed another field, got {after:?}"
        );
    }

    // One entry per written field, never two for one field.
    let held = client.mappings("one-entry-per-field", &id).await;
    let subject_entries = held
        .iter()
        .filter(|(field, _)| field == "x509Subject")
        .count();
    assert_eq!(
        subject_entries, 1,
        "case \"one-entry-per-field\": expected a single x509Subject entry, got {held:?}"
    );
}

/// One row of the delete table: the case name, the certificate field the delete names, and
/// the specifiers it names (`None` when it names none at all).
type DeleteCase = (&'static str, &'static str, Option<Vec<String>>);

/// A delete unmaps exactly the specifiers it names — every specifier of the field when it
/// names none — leaves the specifiers it does not name mapped, and drops the field's entry
/// once the field maps nothing.
///
/// Each row starts from a profile with two fields written, so the table also pins that a
/// delete touches only the field it names, and each delete is then repeated to pin that the
/// `@idempotent` operation changes nothing the second time.
#[tokio::test]
async fn delete_unmaps_only_the_specifiers_it_names() {
    let cases: Vec<DeleteCase> = vec![
        ("one-of-three", "x509Subject", Some(owned(&["OU"]))),
        ("two-of-three", "x509Subject", Some(owned(&["CN", "UID"]))),
        (
            "every-written-specifier",
            "x509Subject",
            Some(owned(&["CN", "OU", "UID"])),
        ),
        ("no-specifier-named", "x509Subject", None),
        (
            "a-specifier-the-field-never-mapped",
            "x509Subject",
            Some(owned(&["DC"])),
        ),
        (
            "another-field-entirely",
            "x509SAN",
            Some(owned(&["SAN/DNS"])),
        ),
    ];

    let client = Client::new();
    for (case, field, named) in cases {
        let id = profile_id(&client, case).await;
        for (written_field, specifiers) in [
            ("x509Subject", owned(&["CN", "OU", "UID"])),
            ("x509SAN", owned(&["SAN/DNS"])),
        ] {
            let seeded = client.put_mapping(&id, written_field, &specifiers).await;
            assert_eq!(
                seeded.status, 200,
                "case {case:?}: expected the seeding put for {written_field:?} to answer 200, \
                 got {seeded:?}"
            );
        }
        let before = client.mappings(case, &id).await;

        let deleted = client.delete_mapping(&id, field, named.as_deref()).await;
        let after = written_mappings(case, &client, &id, &deleted).await;

        let expected = specifiers_after_delete(
            specifiers_in(&before, field).as_ref(),
            named.as_deref(),
        );
        assert_eq!(
            specifiers_in(&after, field),
            expected,
            "case {case:?}: expected {field:?} to map {expected:?}, got {after:?}"
        );
        // Every specifier the request named is gone — every specifier of the field when it
        // named none — and every one it did not name is still mapped.
        let deletes_every_specifier = named.as_deref().is_none_or(|named| named.is_empty());
        let held = specifiers_in(&after, field).unwrap_or_default();
        for specifier in named.clone().unwrap_or_default() {
            assert!(
                !held.contains(&specifier),
                "case {case:?}: {specifier:?} was named for deletion, got {held:?}"
            );
        }
        for specifier in specifiers_in(&before, field).unwrap_or_default() {
            let deleted_now = deletes_every_specifier
                || named
                    .as_deref()
                    .is_some_and(|named| named.contains(&specifier));
            assert_eq!(
                held.contains(&specifier),
                !deleted_now,
                "case {case:?}: {specifier:?} was {}deleted by this call, got {held:?}",
                if deleted_now { "" } else { "not " }
            );
        }
        // The field the delete did not name keeps everything it mapped.
        assert_eq!(
            other_fields(&after, field),
            other_fields(&before, field),
            "case {case:?}: a delete on {field:?} changed another field, before {before:?}, \
             after {after:?}"
        );

        // Repeating the delete changes nothing: the operation is `@idempotent`.
        let repeated = client.delete_mapping(&id, field, named.as_deref()).await;
        let twice = written_mappings(&format!("{case}/repeated"), &client, &id, &repeated).await;
        assert_eq!(
            twice, after,
            "case {case:?}: repeating the delete changed the mappings from {after:?} to {twice:?}"
        );
    }
}

/// A specifier longer than the modelled bound is refused and stores nothing; one at the
/// bound is stored. The bound is `MappingRule$specifier`'s `@length(max 60)`.
#[tokio::test]
async fn a_specifier_past_the_modelled_length_bound_is_refused() {
    let client = Client::new();
    let id = profile_id(&client, "specifier-bound").await;

    for (case, len, expected_status) in [
        ("one-character", 1, 200),
        ("at-the-bound", MAX_SPECIFIER_LEN, 200),
        ("one-past-the-bound", MAX_SPECIFIER_LEN + 1, 400),
    ] {
        let specifier = "s".repeat(len);
        let answer = client
            .put_mapping(&id, "x509Subject", std::slice::from_ref(&specifier))
            .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected status {expected_status}, got {answer:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.error_code
            );
        }

        let held = client.mappings(case, &id).await;
        let mapped = specifiers_in(&held, "x509Subject").unwrap_or_default();
        assert_eq!(
            mapped.contains(&specifier),
            expected_status == 200,
            "case {case:?}: a refused put must store nothing and an accepted one must store its \
             specifier, got {mapped:?}"
        );
    }
}

/// A put bumps the profile's `updatedAt` — "The ISO-8601 timestamp when the profile was
/// last updated" — and leaves `createdAt` as the create set it.
#[tokio::test]
async fn a_put_bumps_the_profiles_updated_at() {
    let client = Client::new();
    let created = client.create_profile("updated-at").await;
    assert_eq!(
        created.status, 201,
        "case \"create\": expected CreateProfile 201, got {created:?}"
    );
    let id = created
        .body
        .pointer("/profile/profileId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case \"create\": no profileId in {created:?}"))
        .to_string();
    let created_at = instant("create", created.profile(), "createdAt");
    let created_updated_at = instant("create", created.profile(), "updatedAt");

    let written = client
        .put_mapping(&id, "x509Subject", &owned(&["CN"]))
        .await;
    assert_eq!(
        written.status, 200,
        "case \"put\": expected 200, got {written:?}"
    );
    let put_updated_at = instant("put", written.profile(), "updatedAt");
    assert!(
        put_updated_at > created_updated_at,
        "case \"put\": expected updatedAt to advance past {created_updated_at}, got \
         {put_updated_at}"
    );
    assert_eq!(
        instant("put", written.profile(), "createdAt"),
        created_at,
        "case \"put\": createdAt must not move, got {:?}",
        written.profile()
    );

    // The read agrees with the write, so the bump is stored rather than only echoed.
    let read = client.get_profile(&id).await;
    assert_eq!(
        instant("read-back", read.profile(), "updatedAt"),
        put_updated_at,
        "case \"read-back\": expected the stored updatedAt {put_updated_at}, got {:?}",
        read.profile()
    );
}

/// One field maps at most `MAX_MAPPING_RULES_PER_CERTIFICATE_FIELD` specifiers: a put that
/// would carry it past that is refused with the operation's modelled `ValidationException`
/// and stores nothing, while a put that fills the field exactly to the bound is stored.
#[tokio::test]
async fn a_put_past_the_specifier_ceiling_is_refused_and_stores_nothing() {
    let ceiling = service_com_amazonaws_rolesanywhere_rolesanywhere::state::MAX_MAPPING_RULES_PER_CERTIFICATE_FIELD;
    let client = Client::new();
    let id = profile_id(&client, "ceiling").await;

    // Distinct specifiers, each within `MappingRule$specifier`'s `@length(max 60)`.
    let to_the_bound: Vec<String> = (0..ceiling).map(|index| format!("s{index}")).collect();
    let filled = client.put_mapping(&id, "x509Subject", &to_the_bound).await;
    assert_eq!(
        filled.status, 200,
        "case \"fill-to-the-bound\": expected 200, got status {}",
        filled.status
    );
    let held = client.mappings("fill-to-the-bound", &id).await;
    let mapped = specifiers_in(&held, "x509Subject").unwrap_or_default();
    assert_eq!(
        mapped.len(),
        ceiling,
        "case \"fill-to-the-bound\": expected {ceiling} specifiers mapped, got {}",
        mapped.len()
    );

    // One specifier past it: refused, and the field still maps exactly what it did.
    let past = "one-too-many".to_string();
    let refused = client
        .put_mapping(&id, "x509Subject", std::slice::from_ref(&past))
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"one-past-the-bound\": expected 400, got status {}",
        refused.status
    );
    assert_eq!(
        refused.error_code, "ValidationException",
        "case \"one-past-the-bound\": expected ValidationException, got {:?}",
        refused.error_code
    );
    let after = client.mappings("one-past-the-bound", &id).await;
    assert_eq!(
        after, held,
        "case \"one-past-the-bound\": a refused put must store nothing"
    );

    // A put of specifiers the field ALREADY maps stays at the bound rather than being
    // refused: the ceiling counts the distinct specifiers the field would map, not the
    // ones the request carries.
    let repeat = client.put_mapping(&id, "x509Subject", &to_the_bound).await;
    assert_eq!(
        repeat.status, 200,
        "case \"repeat-at-the-bound\": expected 200, got status {}",
        repeat.status
    );
}

/// Both writes refuse an id that resolves to no stored profile with this service's
/// ARN-keyed 404, and nothing in the store changes.
///
/// The table spans why the id does not resolve (never minted, minted for a profile that was
/// deleted) and the Region the request is signed for, because the ARN in the wording follows
/// the request.
#[tokio::test]
async fn attribute_mapping_writes_refuse_an_unknown_profile() {
    let client = Client::new();

    let sibling = profile_id(&client, "sibling").await;
    let seeded = client
        .put_mapping(&sibling, "x509Subject", &owned(&["CN"]))
        .await;
    assert_eq!(
        seeded.status, 200,
        "case \"seed-the-sibling\": expected 200, got {seeded:?}"
    );
    let untouched = client.mappings("seed-the-sibling", &sibling).await;

    let gone = profile_id(&client, "deleted").await;
    let removed = client.delete_profile(&gone).await;
    assert_eq!(
        removed.status, 200,
        "case \"delete-the-fixture\": expected DeleteProfile 200, got {removed:?}"
    );

    let mut cases: Vec<(String, String, &str)> = Vec::new();
    for (label, id) in UNMINTED_IDS {
        cases.push((format!("unminted-{label}"), id.to_string(), REGION));
    }
    cases.push(("deleted-profile-id".to_string(), gone.clone(), REGION));
    for region in ["eu-central-1", "ap-southeast-2", "us-east-1"] {
        cases.push((
            format!("unminted-in-{region}"),
            UNMINTED_IDS[1].1.to_string(),
            region,
        ));
    }

    for (case, id, region) in cases {
        let expected = arn_not_found_message(&profile_arn(region, &id));
        for (path, answer) in [
            (
                "PutAttributeMapping",
                client
                    .put_mapping_in_region(&id, "x509Subject", &owned(&["CN"]), region)
                    .await,
            ),
            (
                "DeleteAttributeMapping",
                client
                    .delete_mapping_in_region(&id, "x509Subject", Some(&owned(&["CN"])), region)
                    .await,
            ),
        ] {
            let labelled = format!("{path}/{case}");
            assert_eq!(
                answer.status, 404,
                "case {labelled:?}: expected status 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {labelled:?}: expected ResourceNotFoundException, got {:?}",
                answer.error_code
            );
            assert_eq!(
                answer.message(),
                Some(expected.as_str()),
                "case {labelled:?}: expected message {expected:?}, got {:?}",
                answer.message()
            );
        }

        // The refusals wrote nothing: the sibling maps exactly what it mapped before them,
        // and the deleted profile is still gone.
        let still_there = client.mappings(&case, &sibling).await;
        assert_eq!(
            still_there, untouched,
            "case {case:?}: the sibling's mappings changed from {untouched:?} to {still_there:?}"
        );
        let missing = client.get_profile_in_region(&gone, region).await;
        assert_eq!(
            missing.status, 404,
            "case {case:?}: a refused write must not re-create the deleted profile, got \
             {missing:?}"
        );
    }
}
