Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 77b7d3dfd1a46160280007e44fe4e2e402c212f4

# Codegen-job tags are unreachable through the tagging surface: TaggedKind claims only three kinds hold a tag set, while StoredCodegenJob holds one

_Filed: 2026-10-30T04:26:41Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `77b7d3dfd1a46160280007e44fe4e2e402c212f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The tagging surface of this crate resolves a `resourceArn` through `crate::state::parse_resource_arn`, whose `<collection>` element is matched against `TaggedKind::ALL` — `[Component, Form, Theme]`. The doc above that enum asserts that "these three are the kinds of this service whose own records hold a tag set". That assertion is false inside the same file: `StoredCodegenJob` carries `pub tags: HashMap<String, String>` (state.rs:310), `StartCodegenJob` stores the tags the request supplied (`tags: data.tags.unwrap_or_default()`, start_codegen_job.rs:215) and `StoredCodegenJob::to_codegen_job` reports them (`tags: Some(self.tags.clone())`, state.rs:376). So this service holds a FOURTH record with its own tag set, and the tagging surface cannot see it.

Consequence, for an ARN of exactly the form `parse_resource_arn` parses and the form the model declares for the codegen job resource (`app/{appId}/environment/{environmentName}/codegen-jobs/{id}`):

* `ListTagsForResource` answers 404 `ResourceNotFoundException` ("The item specified was not found") for a live codegen job whose tags the store holds and whose `GetCodegenJob` read reports them — `TaggedKind::from_collection("codegen-jobs")` is `None`, so `resource_tags` returns `None` before any lookup runs.
* `TagResource` and `UntagResource` on that same ARN answer 404 as well, so the tag set a `StartCodegenJob` wrote can never be read, added to or removed through the operations modelled for exactly that purpose; the two reads of one resource (`GetCodegenJob` and `ListTagsForResource`) disagree about whether the resource exists at all.

The root cause is one missing `TaggedKind` arm (plus its two arms in `resource_tags` / `with_resource_tags_mut`, which are the crate's single read and single write over a tag set); all three tagging operations are the same symptom, filed once here against the reporting one.

## Evidence

The crate's claim, contradicted by its own record — state.rs:1239-1258:

    /// Amplify UI Builder names a component, a form and a theme with an ARN of the
    /// documented form
    /// `arn:<partition>:amplifyuibuilder:<region>:<account>:app/<appId>/environment/<environmentName>/<collection>/<id>`,
    /// whose `<collection>` is [`TaggedKind::collection`]. ...
    /// — so the form is the documented one, and these three are the kinds of this
    /// service whose own records hold a tag set.
    pub enum TaggedKind { Component, Form, Theme }
    impl TaggedKind {
        pub const ALL: [Self; 3] = [Self::Component, Self::Form, Self::Theme];

The record that also holds a tag set — state.rs:307-310:

    /// `CodegenJob$tags` is reported for every job, so the tags a start supplied
    /// are held as a map rather than as an absence: a job started with no tags
    /// carries an empty one and reports `{}`.
    pub tags: HashMap<String, String>,

and the module doc that states the same (false) three-kind scope — state.rs:38-40: "A component, a form and a theme each carry the tag set their create stored, so the tagging operations hold no store of their own".

Where the job's tags are written and reported:

* src/ops/start_codegen_job.rs:215 — `tags: data.tags.unwrap_or_default(),`
* src/state.rs:376 — `tags: Some(self.tags.clone()),` inside `to_codegen_job()`

Where the lookup gives up — src/state.rs:1315 `let kind = TaggedKind::from_collection(collection)?;` (so `codegen-jobs` yields `None`), reached from src/ops/list_tags_for_resource.rs:163-172:

    let Some(tags) = crate::state::resource_tags(&store, &input.resource_arn) else {
        return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException(
            crate::state::item_not_found(),
        ) .into());

The model declares the codegen job resource with exactly that ARN shape — `smithy_query(query="shape", shape="CodegenJobResource")`:

    "CodegenJobResource": {"type": "resource", "arn": {"template": "app/{appId}/environment/{environmentName}/codegen-jobs/{id}"}, ... "create": "StartCodegenJob"}

and `StartCodegenJobData$tags` is documented "One or more key-value pairs to use when tagging the code generation job data.", `TagResourceRequest$resourceArn` "The Amazon Resource Name (ARN) to use to tag a resource." (no kind restriction).

Test coverage today: tests/all/resource_tagging.rs:60 pins the surface to `const COLLECTIONS: [&str; 3] = ["components", "forms", "themes"];` under a module doc that claims rule 1 holds "whichever of this service's resource kinds that is" (:13-17), and tests/all/resource_tagging.rs:583-586 asserts 404 for

    ("a collection this service holds no resource of",
     arn("codegen-jobs", APP_ID, ENVIRONMENT, &live_component)),

whose label contradicts `state.codegen_jobs` (state.rs:513) holding resources of that collection. That row passes unchanged after a fix: the id it builds the ARN from is a COMPONENT's uuid, which names no codegen job. No test presents a live codegen job's own ARN to any tagging operation.

## Steps to Reproduce

1. POST /app/d1234abcd/environment/staging/codegen-jobs with codegenJobToCreate = {"renderConfig": {"react": {}}, "tags": {"team": "ui"}} — 200, and the returned CodegenJob carries id=<jobId> and tags {"team":"ui"}.
2. GET /app/d1234abcd/environment/staging/codegen-jobs/<jobId> — 200, tags {"team":"ui"}: the store holds that tag set.
3. GET /tags/arn%3Aaws%3Aamplifyuibuilder%3Aeu-west-1%3A000000000000%3Aapp%2Fd1234abcd%2Fenvironment%2Fstaging%2Fcodegen-jobs%2F<jobId> — observed HTTP 404 {"message":"The item specified was not found"} for a resource the previous step just reported, because TaggedKind::from_collection("codegen-jobs") is None.
4. POST (TagResource) and DELETE (UntagResource) on the same /tags/{arn} — both 404, so the job's stored tags can never be changed through the tagging operations.
5. Compare with the component ARN (collection `components`), which the same three operations serve from the same (appId, environmentName, id) key.
