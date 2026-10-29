// SPDX-License-Identifier: CC0-1.0

//! `ListImports` read back one bounded page at a time, end to end over the awsQuery wire.
//!
//! The rule under test: `ListImports` reports the live stacks of the request's Region whose
//! deployed template resolved an import for the named export, and — being
//! `@paginated(inputToken: NextToken, outputToken: NextToken, items: Imports)` — it answers
//! ONE bounded page per call, with a `NextToken` that resumes the read just past that page
//! and no token on the last one, so walking the tokens reports every importer exactly once
//! whatever the size of the index. A token this collection never issued is a bad request,
//! not an empty page, and the read itself changes nothing a later caller sees.
//!
//! `cross_stack_imports.rs` is the suite that pins how a stack ENTERS and LEAVES that index
//! (a deployed `Fn::ImportValue`, and the delete that drops it), and `unimported_exports.rs`
//! the two refusals an export with no importers to report answers with. This one spans the
//! sizes the paging rule generalises over, which a wire-created stack per importer could not
//! reach inside a test: the rows are seeded straight into the store, exactly as
//! `stack_lifecycle.rs` seeds past a bound.

use std::time::SystemTime;

use service_com_amazonaws_cloudformation_cloudformation::state::{
    self, export_is_not_imported, status, StoredOutput, StoredStack, PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    list_imports_ok, list_imports_page, list_imports_refusal, router_with_engine, store, xml_blocks,
    xml_text, REGION,
};
use crate::stack_lifecycle::ACCOUNT_ID;

/// The export name the seeded importers of these cases consume.
const EXPORT: &str = "paged-shared-export";

/// An export name seeded beside it that the cases must never confuse with [`EXPORT`].
const OTHER_EXPORT: &str = "paged-other-export";

/// An export name the seeded exporter publishes and no seeded stack imports.
const UNIMPORTED_EXPORT: &str = "paged-unimported-export";

/// The stack names one `ListImports` page of this Region reports, with the page's
/// `NextToken`. The 200 every well-formed read answers is asserted by the shared reader.
async fn page(
    case: &str,
    router: &MultiServiceRouter,
    export_name: &str,
    token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    list_imports_ok(case, router, REGION, export_name, token).await
}

/// The `Message` of a `ListImports` read refused because the export has no importers to
/// report: the code is `ValidationError` on both of this operation's refusals, which the
/// shared reader asserts, so a row here names which message was due.
async fn refused_message(case: &str, router: &MultiServiceRouter, export_name: &str) -> String {
    let (code, message) = list_imports_refusal(case, router, REGION, export_name).await;
    assert_eq!(
        code, "ValidationError",
        "case {case:?}: expected the code CloudFormation refuses a bad request with"
    );
    message
}

/// Seed `importers` live stacks importing [`EXPORT`], in creation order, beside records the
/// index must leave out: a live stack importing another export, and a stack that imported
/// [`EXPORT`] before its deletion completed.
///
/// A live stack PUBLISHING each of those export names is seeded first, because an export name
/// this Region publishes nothing under is a refusal of its own (`unimported_exports.rs`):
/// what these rows are about is the PAGING of an index that exists.
fn seed(engine: &std::sync::Arc<engine::InMemoryEngine>, importers: usize) -> Vec<String> {
    let now = SystemTime::now();
    let state = store(engine);
    let mut held = state.lock();
    let mut expected = Vec::with_capacity(importers);

    let mut exporter = StoredStack::new(
        format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/paged-exporter/99997"),
        "paged-exporter".to_string(),
        Some("{}".to_string()),
        now,
    );
    exporter.status = status::CREATE_COMPLETE.to_string();
    for (index, export_name) in [EXPORT, OTHER_EXPORT, UNIMPORTED_EXPORT].iter().enumerate() {
        exporter.outputs.push(StoredOutput {
            key: format!("O{index}"),
            value: Some("exported-value".to_string()),
            description: None,
            export_name: Some((*export_name).to_string()),
        });
    }
    held.stacks.push(exporter);

    for index in 0..importers {
        let name = format!("importer-{index:05}");
        let mut stack = StoredStack::new(
            format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/{name}/{index:05}"),
            name.clone(),
            Some("{}".to_string()),
            now,
        );
        stack.status = status::CREATE_COMPLETE.to_string();
        stack
            .imports
            .insert(EXPORT.to_string(), "imported-value".to_string());
        held.stacks.push(stack);
        expected.push(name);
    }

    let mut other = StoredStack::new(
        format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/other-importer/99998"),
        "other-importer".to_string(),
        Some("{}".to_string()),
        now,
    );
    other.status = status::CREATE_COMPLETE.to_string();
    other
        .imports
        .insert(OTHER_EXPORT.to_string(), "other-value".to_string());
    held.stacks.push(other);

    let mut gone = StoredStack::new(
        format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/gone-importer/99999"),
        "gone-importer".to_string(),
        Some("{}".to_string()),
        now,
    );
    gone.status = status::DELETE_COMPLETE.to_string();
    gone.deletion_time = Some(now);
    gone.imports
        .insert(EXPORT.to_string(), "imported-value".to_string());
    held.stacks.push(gone);

    expected
}

/// Walk every page of one export's index, asserting each page is bounded and that the pages
/// neither repeat nor skip an importer. Answers the names in wire order.
async fn walk(case: &str, router: &MultiServiceRouter, export_name: &str) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One more read than any row needs, so a token that never ran out fails the case instead
    // of looping.
    for _ in 0..8 {
        let (names, next) = page(case, router, export_name, token.as_deref()).await;
        assert!(
            names.len() <= PAGE_SIZE,
            "case {case:?}: expected at most {PAGE_SIZE} importers in one page, got {}",
            names.len()
        );
        if next.is_some() {
            assert_eq!(
                names.len(),
                PAGE_SIZE,
                "case {case:?}: expected a full page wherever the read continues, got {}",
                names.len()
            );
        }
        for name in &names {
            assert!(
                !collected.contains(name),
                "case {case:?}: expected the pages not to repeat {name:?}, got {collected:?}"
            );
        }
        collected.extend(names);
        match next {
            None => return collected,
            Some(next) => token = Some(next),
        }
    }
    panic!("case {case:?}: the read never reached a last page, got {collected:?}");
}

#[tokio::test]
async fn the_import_index_is_reported_one_bounded_page_at_a_time() {
    // The sizes the paging rule generalises over: none, one, just under a page, exactly a
    // page (where the token stops), just over, and several pages.
    for importers in [0, 1, PAGE_SIZE - 1, PAGE_SIZE, PAGE_SIZE + 1, 2 * PAGE_SIZE + 7] {
        let case = format!("{importers} importing stacks");
        let (router, engine) = router_with_engine();
        let expected = seed(&engine, importers);

        // No importers at all is not a page: the published export reports none, which is its
        // own refusal, and there is nothing to walk.
        if importers == 0 {
            assert_eq!(
                refused_message(&case, &router, EXPORT).await,
                export_is_not_imported(EXPORT),
                "case {case:?}: expected the unimported export's own refusal"
            );
            continue;
        }

        let (first, next) = page(&case, &router, EXPORT, None).await;
        assert_eq!(
            next.is_some(),
            importers > PAGE_SIZE,
            "case {case:?}: expected a NextToken only where importers remain, got {next:?}"
        );
        assert_eq!(
            first.len(),
            importers.min(PAGE_SIZE),
            "case {case:?}: expected a bounded first page, got {first:?}"
        );

        let walked = walk(&case, &router, EXPORT).await;
        assert_eq!(
            walked, expected,
            "case {case:?}: expected the walk to report every importer once, got {} names",
            walked.len()
        );

        // Reading the index reports, so it leaves the store as the next caller finds it: the
        // same request answers the same page.
        let (again, next_again) = page(&case, &router, EXPORT, None).await;
        assert_eq!(
            (again, next_again.is_some()),
            (first, importers > PAGE_SIZE),
            "case {case:?}: expected a repeated read to answer the same page"
        );
    }
}

#[tokio::test]
async fn each_export_name_is_reported_its_own_importers_on_one_page() {
    let (router, engine) = router_with_engine();
    seed(&engine, 3);

    let case = "an export another stack imports";
    let (names, next) = page(case, &router, OTHER_EXPORT, None).await;
    assert_eq!(
        names,
        vec!["other-importer".to_string()],
        "case {case:?}: expected exactly that export's importers, got {names:?}"
    );
    assert!(
        next.is_none(),
        "case {case:?}: expected no NextToken on a single page, got {next:?}"
    );

    // An export the same account publishes and nothing imports has no page of its own: it is
    // the refusal that says so, not an empty list that reads like a safe-to-change export.
    let case = "an export name nothing imports";
    assert_eq!(
        refused_message(case, &router, UNIMPORTED_EXPORT).await,
        export_is_not_imported(UNIMPORTED_EXPORT),
        "case {case:?}: expected the unimported export's own refusal"
    );
}

#[tokio::test]
async fn a_next_token_this_collection_never_issued_is_refused() {
    let (router, engine) = router_with_engine();
    let expected = seed(&engine, PAGE_SIZE + 5);

    // A cursor past the end of the collection, one issued for ANOTHER collection of this
    // service, and one that is not a cursor at all: each is a bad request rather than an
    // empty page.
    for (case, token) in [
        (
            "a cursor past the end",
            state::encode_next_token(state::PageOf::Imports, expected.len() + 1),
        ),
        (
            "a cursor issued for another collection",
            state::encode_next_token(state::PageOf::Exports, 0),
        ),
        ("a token this service never issued", "not-a-cursor".to_string()),
        // A token carrying XML metacharacters: the refusal names it, so the envelope has to
        // ESCAPE what it reflects rather than letting a caller's value close an element.
        (
            "a token carrying xml metacharacters",
            "</Message><Injected>&\"'".to_string(),
        ),
    ] {
        let (http_status, body, _, _) =
            list_imports_page(&router, REGION, EXPORT, Some(&token)).await;
        assert_eq!(
            http_status, 400,
            "case {case:?}: expected the read to be refused, got {http_status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
        // The refusal is built from the REQUEST: it names the token it rejected, the same
        // sentence every paginated read of this service words it with.
        assert_eq!(
            xml_text(&body, "Message"),
            Some(format!("{token} is not a valid NextToken")),
            "case {case:?}: expected the refusal to name the rejected token, got {body}"
        );
        // One envelope, whatever the token carried: a reflected value that was not escaped
        // would split or close these elements.
        assert_eq!(
            (
                xml_blocks(&body, "Message").len(),
                xml_blocks(&body, "RequestId").len()
            ),
            (1, 1),
            "case {case:?}: expected one Message and one RequestId in the envelope, got {body}"
        );
    }

    // The cursor the read ITSELF issued still resumes it, so the refusals above are about
    // the token and not about the collection.
    let (first, next) = page("the token the read issued", &router, EXPORT, None).await;
    let token = next.unwrap_or_else(|| panic!("expected a NextToken past {} importers", first.len()));
    let (second, next) = page("the token the read issued", &router, EXPORT, Some(&token)).await;
    assert_eq!(
        second,
        expected[PAGE_SIZE..].to_vec(),
        "case {:?}: expected the token to resume just past the first page, got {second:?}",
        "the token the read issued"
    );
    assert!(
        next.is_none(),
        "case {:?}: expected the last page to carry no NextToken, got {next:?}",
        "the token the read issued"
    );
}
