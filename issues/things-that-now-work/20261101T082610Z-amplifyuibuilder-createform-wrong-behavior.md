Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2294bca91bc8bfde7308721c1d7853ded4942c2e

# A form created without tags is the only resource of this crate that reports no `tags` member, and its two readers disagree on the wire shape

_Filed: 2026-11-01T08:26:10Z_
_Operation: CreateForm | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2294bca91bc8bfde7308721c1d7853ded4942c2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Four of this crate's stored records carry an optional, non-`@required` `Tags` map (`Component$tags`, `Theme$tags`, `CodegenJob$tags`, `Form$tags`). Three of them are normalised the same way on the write path — the create stores `data.tags.unwrap_or_default()`, so a resource created without tags holds an empty map and reports `"tags": {}` — and the FORM is the one that is not: `CreateForm` stores the `Option` as it arrived, so `GetForm` and `ExportForms` report no `tags` member at all for a form created without tags.

Two consequences inside this crate:

1. The same modelled construct answers two different wire shapes depending on resource kind: `GET /app/{appId}/environment/{env}/themes/{id}` answers `"tags": {}` while `GET /app/{appId}/environment/{env}/forms/{id}` answers nothing at all, for the identical "created without tags" state. A client reading tags off this service has to handle both.
2. The two readers of a FORM's one tag set disagree: `ListTagsForResource` coerces the absence to the empty map (`resource_tags`, form branch), while the form's own reader reports the absence — against the rule the crate's own tagging test states ("ONE TAG SET PER RESOURCE … the resource's OWN reader reports the same set … because there is one stored tag set, not two").

The divergence also costs the shared tagging write path a form-only branch (`with_resource_tags_mut`) that exists solely to put the absence back; the component and theme branches need none.

The reason the form path gives for the divergence ("`Form$tags` is optional on both `CreateFormData` and `Form`") is equally true of `Theme$tags` and `Component$tags` — the model marks all three as plain optional `Tags` maps — so it does not distinguish the form from the siblings that do the opposite. This report makes no claim about what the real service returns; the defect is that one crate normalises one construct two opposite ways.

## Evidence

The model treats the three members identically — none carries `required: true` (smithy_query query="shape" shape="Form,Theme"):

  Form:  "tags": {"target": "Tags", "type": "map", "doc": "<p>One or more key-value pairs to use when tagging the form.</p>"}
  Theme: "tags": {"target": "Tags", "type": "map", "doc": "<p>One or more key-value pairs to use when tagging the theme.</p>"}

The three siblings hold it as a map (so they report `{}`):

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:240-243
    /// `Component$tags` is reported for every component, so the tags a create
    /// supplied are held as a map rather than as an absence: a component created
    /// with no tags carries an empty one and reports `{}`.
    pub tags: HashMap<String, String>,
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:458-461
    /// `Theme$tags` is reported for every theme, so the tags a create supplied
    /// are held as a map rather than as an absence: a theme created with no tags
    /// carries an empty one and reports `{}`, exactly as a component does.
    pub tags: HashMap<String, String>,
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:310-313
    /// `CodegenJob$tags` is reported for every job, so the tags a start supplied
    /// are held as a map rather than as an absence: a job started with no tags
    /// carries an empty one and reports `{}`.
    pub tags: HashMap<String, String>,

The form holds it as an absence:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:407-410
    /// `Form$tags` is optional on both `CreateFormData` and `Form`, so the
    /// absence of tags is held as an absence rather than as an empty map: a form
    /// created without them reports no `tags` member at all.
    pub tags: Option<HashMap<String, String>>,

The write paths, side by side:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/create_form.rs:210
            tags: data.tags,
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/create_theme.rs:206
            tags: data.tags.unwrap_or_default(),
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/create_component.rs:269
            tags: data.tags.unwrap_or_default(),

The two readers of a form's one tag set disagree, and the write helper carries a form-only branch to keep the absence:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1442-1446
        TaggedKind::Form => state
            .form(app, environment, id)
            // `Form$tags` is optional, so a form carrying none holds an absence
            // rather than an empty map; the tag SET it carries is empty either way.
            .map(|form| form.tags.clone().unwrap_or_default()),
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1475-1485
        TaggedKind::Form => state.form_mut(app, environment, id).map(|form| {
            // A form carrying no tags reports no `tags` member at all
            // ([`StoredForm::tags`]), so a write that leaves such a form with no
            // tag keeps that absence rather than turning it into an empty map;
            let was_absent = form.tags.is_none();
            ...
            form.tags = (!was_absent || !tags.is_empty()).then_some(tags);

The rule the crate's own tagging test states, which the form breaks on the wire:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/resource_tagging.rs:13-21
    //! 1. ONE TAG SET PER RESOURCE. `ListTagsForResource` reports exactly the tags stored on
    //!    the resource its `resourceArn` names, whichever of this service's resource kinds
    //!    that is, and the resource's OWN reader reports the same set: … a resource created
    //!    without any answers the empty map
    //! 2. … because there is one stored tag set, not two.

Test coverage today: the tagging test cannot see the divergence because its reader coerces the absence —

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/resource_tagging.rs:109-117
    /// The tag set the answer reports: the `tags` member, read as the empty map when the
    /// wire omits it — `Form$tags` is optional, so a form carrying no tag reports no
    /// member at all, which is the same SET as an empty map.
    fn tags(&self) -> Value { match self.body.get("tags") { Some(Value::Object(map)) => …, _ => json!({}) } }

and the two lifecycle tests pin the two opposite shapes with reasons that do not distinguish the kinds:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/form_lifecycle.rs:395-406
        // An optional member the create did not carry is reported as an absence, not as an
        // invented value: `CreateFormData`'s `cta`, `tags` and `labelDecorator` are each
        // optional on `Form` too.
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/theme_lifecycle.rs:339-345
        // `Theme$tags` is reported for every theme, so a create that supplied none reports
        // the empty map it was stored with — the same convention `Component$tags` follows.

## Steps to Reproduce

1. POST /app/d1234abcd/environment/staging/forms with formToCreate = {"name":"Contact","formActionType":"create","dataType":{"dataSourceType":"DataStore","dataTypeName":"Post"},"fields":{},"style":{},"sectionalElements":{},"schemaVersion":"1.0"} — no `tags` member. Note the returned id.
2. GET /app/d1234abcd/environment/staging/forms/{id}: the body carries NO `tags` member (create_form.rs:210 stored the `Option` unchanged).
3. GET /tags/arn:aws:amplifyuibuilder:eu-west-1:000000000000:app/d1234abcd/environment/staging/forms/{id}: ListTagsForResource answers {"tags":{}} for the SAME record (state.rs:1442-1446 coerces the absence), so the two readers of one tag set report two shapes.
4. POST /app/d1234abcd/environment/staging/themes with themeToCreate = {"name":"Studio","values":[]} — also no `tags` member — then GET /app/d1234abcd/environment/staging/themes/{id}: the body carries "tags": {} (create_theme.rs:206). Same for a component (create_component.rs:269) and a codegen job (state.rs:310-313).
5. Compare the four stored records: state.rs:240-243, 310-313 and 458-461 hold the map; state.rs:407-410 holds an absence, for a member the model marks optional in all four cases.
