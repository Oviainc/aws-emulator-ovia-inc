Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 780a41c8ff20fd069d5e365e10fd3b0dbb5b515c

# The test harness endpoint_lifecycle.rs declares itself the single home for is re-implemented in three suites, and the 404 assertion copies have already drifted

_Filed: 2026-10-28T21:58:45Z_
_Operation: DeleteEndpoint | Class: maintainability_

> _Note: this issue was identified against the code base at commit `780a41c8ff20fd069d5e365e10fd3b0dbb5b515c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`tests/all/endpoint_lifecycle.rs` states that it is "the crate's single home for the test harness every other suite drives the service through: the emulator fixtures, the request helpers, the [`Answer`] reader ... so one listing rule is written down once and no suite carries a second copy of it", and exports exactly those helpers as `pub(crate)`.

Three sibling suites nevertheless carry their own second copies of helpers that module already exports:

1. The "modelled 404 with the one fixed not-found text" assertion rule is implemented twice — `not_found_messages.rs::assert_not_found` and `one_outpost_resolution_rule.rs::assert_not_found` — and the two copies HAVE ALREADY DRIFTED: the second checks only the status/code and the Message, dropping the "must not report an authorization failure" and "must not disclose the requested id" checks the first makes. Both copies are applied to `DeleteEndpoint`'s 404 (`not_found_messages.rs:191-192`, and `one_outpost_resolution_rule.rs` Door::Delete at :102 asserted at :315/:337), so the same operation's refusal is asserted by two different rules depending on which suite runs it, and a regression that leaked the requested `endpointId`/`outpostId` into the Message would still pass one of them.
2. `arn_partitions.rs` re-defines `create_in` and `endpoint_with_arn` with bodies character-identical to the harness's `pub(crate)` versions (it already imports `call_in`, `string_at`, `Answer`, `minimal_create` from the same module), plus its own `list_endpoints_in` / `list_shared_in` / `list_outposts_in` wrappers over the harness's.
3. `arn_patterns.rs` re-defines `emulator_holding` (provision Outposts into the store, then build a router with authentication disabled and the service registered — the same construction as the harness's, with the same `48 * TEBIBYTE` capacity) plus its own `create_in` / `list_endpoints_in` / `list_shared_in`.

This is the same shape as the already-fixed issue "The endpoint-listing paging test harness is implemented twice (and the two copies have already drifted)": every copy is one more place a rule can be changed in one suite and not the other. The fix is to move the one 404 assertion into `endpoint_lifecycle.rs` beside the helpers it already owns and have both suites call it, and to delete the re-defined `create_in` / `endpoint_with_arn` / `emulator_holding` / listing wrappers in favour of the exported ones. No handler, no response and no assertion is weakened by that move.

## Evidence

The single-home rule the harness states — services/service-com-amazonaws-s3outposts-s3outposts/tests/all/endpoint_lifecycle.rs:12-15:

    //! It is also the crate's single home for the test harness every other suite drives the
    //! service through: the emulator fixtures, the request helpers, the [`Answer`] reader and
    //! the [`Listing`] harness every listing suite pages a listing with — so one listing rule
    //! is written down once and no suite carries a second copy of it.

Copy 1 of the 404 rule — tests/all/not_found_messages.rs:64-93:

    /// The modelled 404 with exactly the one fixed text as its Message, with nothing of the
    /// request in it and no authorization wording.
    fn assert_not_found(case: &str, answer: &Answer, request_ids: &[&str]) {
        assert_eq!((answer.status, answer.error_code()), (404, "ResourceNotFoundException"), ...);
        assert_eq!(answer.message(), OUTPOST_MISSING, ...);
        assert!(!answer.message().contains(AUTHORIZATION_TEXT), ...);
        for id in request_ids {
            assert!(!answer.message().contains(id), ...);
        }
    }

Copy 2, drifted (no authorization check, no id-disclosure check) — tests/all/one_outpost_resolution_rule.rs:246-262:

    /// The modelled 404 with exactly the one fixed text, carrying nothing of the request.
    fn assert_not_found(case: &str, answer: &Answer) {
        assert_eq!((answer.status, answer.error_code()), (404, "ResourceNotFoundException"), ...);
        assert_eq!(answer.message(), OUTPOST_MISSING, ...);
    }

Both are applied to DeleteEndpoint's refusal — not_found_messages.rs:191-192:

            let answer = delete_endpoint(&router, outpost_id, endpoint_id).await;
            assert_not_found(&case, &answer, &[outpost_id, endpoint_id]);

and one_outpost_resolution_rule.rs:102 / :315:

                Door::Delete => delete_endpoint_in(router, region, outpost_id, endpoint_id).await,
    ...
                    assert_not_found(&case, &answer);

The harness's exported request helper — endpoint_lifecycle.rs:270-279:

    pub(crate) async fn create_in(router: &MultiServiceRouter, region: &str, request: Value) -> Answer {
        call_in(router, region, "POST", "/S3Outposts/CreateEndpoint", Some(request)).await
    }

its second copy — tests/all/arn_partitions.rs:105-114:

    async fn create_in(router: &MultiServiceRouter, region: &str, request: Value) -> Answer {
        call_in(router, region, "POST", "/S3Outposts/CreateEndpoint", Some(request)).await
    }

and the character-identical `endpoint_with_arn` — endpoint_lifecycle.rs:450-458 versus arn_partitions.rs:162-170:

    fn endpoint_with_arn(answer: &Answer, arn: &str) -> Value {
        answer.endpoints().iter()
            .find(|endpoint| endpoint["EndpointArn"].as_str() == Some(arn))
            .unwrap_or_else(|| panic!("no endpoint with ARN {arn} in {:?}", answer.body))
            .clone()
    }

the fixture builder, twice — endpoint_lifecycle.rs:71-87 `pub(crate) fn emulator_holding(outposts: &[(String, &str)])` versus arn_patterns.rs:156-176 `fn emulator_holding(row: &Row, region: &str)`, both doing `store.provision_outpost(OutpostRecord::provisioned(.., 48 * TEBIBYTE, region)...)` then `router.set_request_authentication(false); router.register_service(EmulatedService::new(engine));`.

Four further wrapper copies: arn_partitions.rs:116-147 (`list_endpoints_in`, `list_shared_in`, `list_outposts_in`) and arn_patterns.rs:241-261 (`list_endpoints_in`, `list_shared_in`), over endpoint_lifecycle.rs:285-336's exported versions.

Test coverage of the duplicated code: arn_partitions.rs::every_arn_names_the_partition_of_the_region_the_request_was_signed_for and ::one_store_reports_each_region_s_own_partition_consistently; arn_patterns.rs::an_outpost_reports_only_the_arns_its_members_patterns_admit and ::an_endpoint_reports_only_an_arn_its_pattern_admits; not_found_messages.rs::an_unresolvable_outpost_answers_one_fixed_text_for_every_id_form and ::an_unresolvable_endpoint_under_a_resolvable_outpost_carries_the_same_text; one_outpost_resolution_rule.rs::every_door_refuses_an_unresolvable_outpost_identically. All keep passing against the single shared helpers.

## Steps to Reproduce

1. grep -n "fn assert_not_found" services/service-com-amazonaws-s3outposts-s3outposts/tests/all/*.rs — two definitions (not_found_messages.rs:66, one_outpost_resolution_rule.rs:247); diff their bodies and see the second omits the AUTHORIZATION_TEXT and request-id disclosure assertions the first makes, while both are applied to DeleteEndpoint's 404.
2. grep -n "async fn create_in" services/service-com-amazonaws-s3outposts-s3outposts/tests/all/*.rs — three definitions (endpoint_lifecycle.rs:270 exported `pub(crate)`, arn_partitions.rs:105, arn_patterns.rs:230), the first two with identical bodies.
3. grep -n "fn endpoint_with_arn\|fn emulator_holding" services/service-com-amazonaws-s3outposts-s3outposts/tests/all/*.rs — endpoint_with_arn at endpoint_lifecycle.rs:451 and arn_partitions.rs:163 (character-identical); emulator_holding at endpoint_lifecycle.rs:71 and arn_patterns.rs:156.
4. Read endpoint_lifecycle.rs:12-15 and confirm the module declares itself the crate's single home for exactly those helpers, so each copy above contradicts the rule the harness states.
5. Fix check: move the 404 assertion into endpoint_lifecycle.rs, delete the re-defined helpers and import the exported ones, then `cargo test -p service-com-amazonaws-s3outposts-s3outposts` — every suite passes with no assertion or response changed.
