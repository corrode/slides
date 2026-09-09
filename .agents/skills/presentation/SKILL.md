---
name: presentation
description: Create or revise a presentation for this Slides app as a complete ZIP bundle containing slides.md and its code, images, and HTML demos. Use for talks, workshops, lessons, pitches, and slide decks, including packaging or uploading a presentation draft.
---

# Create a presentation bundle

Deliver an editable source directory and an upload-ready ZIP for one presentation. Markdown is the entry point, not the entire deliverable. A Markdown-only presentation is still a bundle containing `slides.md`.

When working in the Slides repository, use `docs/slide-format.md` as the syntax reference. Outside that repository, use `references/slide-format.md` relative to this skill's directory; the global installation includes a snapshot. References below to `docs/slide-format.md` mean whichever copy applies. If neither is available, obtain the reference from the user rather than guessing. Read its bundle rules and the sections relevant to the deck before authoring. Do not invent layout directives, metadata fields, API endpoints, or validation commands.

## 1. Establish the task

Use the supplied audience, topic, source material, duration, and desired outcome. Ask only for missing information that would materially change the presentation. Otherwise choose reasonable defaults and proceed.

Distinguish the requested action:

- **Create or revise:** write local source files and build a ZIP. No network upload.
- **Upload:** create or replace a server draft using the bundle API.
- **Publish or present:** upload if authorized, then explain the separate presenter-UI action. The bundle API does not publish or start a session.

Saving files locally does not authorize uploading. Do not deploy the application, modify CI, or change server configuration as part of making a deck.

For revisions, preserve the existing bundle's files and authoring paths. Change the requested content, then rebuild the complete archive. Do not return a patch as the presentation or upload only the changed files. The API's `source` field contains resolved Markdown, not a recoverable copy of the original multi-file bundle; prefer the local authoring directory or original ZIP.

## 2. Plan the presentation

- Give the audience a clear problem, explanation, and takeaway.
- Give each slide one purpose. Prefer concrete examples and short text to paragraphs.
- Budget roughly one to two minutes per substantive slide unless the format calls for another pace.
- Put delivery cues and supporting detail in presenter notes, not audience-facing prose.
- Use interactions when they help the audience learn or make a decision, not to demonstrate every feature.
- Verify factual claims and cite sources where useful. Do not invent quotations, results, or metrics.
- Keep diagrams and code readable at presentation distance. Do not add HTML demos when ordinary Markdown or Mermaid would be clearer.

## 3. Write the source directory

Use the user's requested location or an appropriate project-local directory. Inspect the parent before creating files. Keep the ZIP outside the source directory so it cannot include itself. Do not overwrite another presentation or an existing archive without authorization.

Example layout; create only the supporting files the deck needs:

```text
my-talk/
  slides.md
  code/example.rs
  images/diagram.svg
  demo/index.html
  demo/app.js
  demo/style.css
my-talk.zip
```

The archive contains the **contents** of `my-talk/`, not the parent directory.

### Entry point and Markdown

- Require the exact root filename `slides.md`, encoded as UTF-8.
- Begin with a nonempty H1 of at most 120 characters. Its text supplies the presentation title.
- Use `---` on its own line between slides. Do not add leading or trailing separators.
- No frontmatter, `bundle.json`, theme metadata, or multi-file deck composition. Additional Markdown files are supporting material, not automatically included slides.
- Use CommonMark with tables, strikethrough, task lists, and fenced code. Raw HTML in Markdown is not a layout mechanism.
- Do not invent columns, reveal markers, slide classes, or background directives. New bundles use the app's default theme; replacement preserves an existing deck's theme.
- Use at most one `:::notes` block and one interaction per slide. Close directive blocks with `:::` and consult the format reference for exact attributes.
- Polls and ordering exercises need at least two items. Quizzes need at least two options and one marked correct answer. Word clouds have no body. Attribute values use double quotes, with no embedded quote escaping.
- Keep reference-style link definitions on the slide using them; reference labels are slide-local.

### Code

Use inline fenced code for short examples. Put reusable or longer examples in real UTF-8 files under `code/`, included through an otherwise empty fence:

````markdown
```rust code/example.rs
```
````

Includes expand the whole file at upload. Do not use line ranges, regions, placeholders, or extra fence arguments. Use a longer fence if included content could close it.

Test examples when practical. Rust blocks can offer an explicit Run action backed by the public Rust Playground; do not include secrets or send confidential code there without authorization. Uploading a bundle itself never executes its code.

### Images, diagrams, and HTML

Reference existing bundle-relative paths:

```markdown
![Diagram description](images/diagram.svg)

[Example source](code/example.rs)

:::iframe src="demo/index.html" title="Interactive demonstration"
:::
```

- No external images or iframe URLs. Ordinary navigation links may use HTTP(S), mailto, or fragments.
- Do not author `/assets/...` paths or fabricate generation URLs; the importer creates those. Do not link to `slides.md`, which is private source and may contain notes.
- Prefer a small fenced `mermaid` diagram when appropriate. Include accessibility text using `accTitle` and `accDescr` where supported; avoid custom scripts, initialization directives, and raw HTML.
- HTML demos must be self-contained and trusted. Bundle their JavaScript, CSS, images, and fonts; resolve dependencies relative to the HTML/CSS file. No CDN dependencies, external fetches, or build/install steps on the server.
- HTML runs in a sandbox without parent-page access, same-origin privileges, forms, popups, or fetch/WebSocket connections. Do not weaken that sandbox to make a demo work. A frame can navigate itself; sandboxing is not proof that arbitrary content is safe.
- Presenter notes are excluded from audience rendering. Do not put private material in supporting files: assets can be served or included in audience archives.

## 4. Package and check

Create a **fresh** ZIP with Stored or Deflated entries, using explicit source paths. For example, from the verified source directory:

```sh
python3 -m zipfile -c ../my-talk.zip slides.md code images demo
```

Replace names with the actual paths and omit nonexistent directories. For a Markdown-only deck, include just `slides.md`. Never recursively archive the whole repository or include `.git`, credentials, dependencies, build output, editor debris, or another archive.

Before handing off or uploading, inspect the actual ZIP, not just the source tree:

- Exact root `slides.md`; no wrapper directory or manifest.
- At most **20 MiB compressed**, **100 MiB extracted**, and **512 entries**, counting directories. Root Markdown at most **2 MiB**; each HTML file at most **4 MiB**.
- Use the extension allowlist in `docs/slide-format.md`. Path segments contain only ASCII letters, digits, dots, hyphens, and underscores. No spaces, trailing dots, absolute paths, traversal, or backslashes.
- No symlinks, special files, encryption, duplicate paths, case-only collisions, or file/directory collisions.
- Every explicit local image, link, iframe, and code reference exists. Check static HTML/CSS dependencies too; exercise interactive demos where possible.
- All fences close; slide separators are outside code blocks. Interactions satisfy their semantic rules.
- List the archive entries, check CRCs, and calculate compressed/extracted sizes. Confirm the source directory remains usable for the next revision.

Be precise about validation. `slides validate <FILE>` is a Markdown CLI, **not a bundle validator**. It uses legacy code-reference and iframe-path handling, so bundle-relative source can fail there even when valid for upload. Do not change correct authoring paths to satisfy it, and do not claim a CLI pass validates the archive. There is currently no bundle dry-run endpoint or dedicated ZIP validation command. The upload handler performs authoritative bundle validation before accepting a draft.

If a local preview is available, check slide density, code readability, diagrams, and HTML behavior. Otherwise state that visual validation was not performed.

## 5. Upload only when requested

Use the server URL and API token supplied by the user or available through the established secret mechanism. Conventional client variables are `SLIDES_URL` and `SLIDES_API_TOKEN`. Never print token values, put them in files or URLs, or expose them through shell tracing. If unavailable, ask for the server location or direct the user to `/admin/settings` to create an API token.

The API bearer token is separate from `ADMIN_PASSWORD`, which is for presenter login. Do not use the password as a bearer token or change authentication settings.

1. Choose a slug with the user’s intent. Slugs use 1–48 lowercase ASCII letters, digits, or hyphens, without leading/trailing hyphens; reserved routes are rejected. Check for an existing presentation before a new upload. Ask before replacing an unrelated deck.
2. Send `POST /api/v1/presentations/{slug}/bundle` with `Authorization: Bearer <token>`, `Content-Type: application/zip`, and the ZIP as the raw body. No JSON payload, multipart form, separate embed upload, or PATCH request.
3. Expect `201 Created` for creation or `200 OK` for replacement. Read the response rather than assuming success. `413` indicates size limits, `415` a content-type error, and `422` invalid bundle contents. Correct the underlying files/archive before retrying; never suppress validation failures.
4. A successful upload replaces only the draft and installs immutable assets. Published versions and running sessions retain their previous contents. Further edits require another complete bundle upload.
5. Provide `<server>/admin/decks/<slug>/edit` for authenticated preview and publishing. `<server>/<slug>` is the audience shortlink, not evidence that the new draft is published. The `Location` header points to the API resource, not the presenter UI.

If the user asks to publish, explain that they must choose Publish in the presenter UI; do not report an upload as publication. Never generate or rotate credentials automatically.

## Deliver the result

Default to a short handoff with clickable paths to the **source directory**, **`slides.md`**, and **ZIP**, plus the checks actually performed and any remaining limitations. Do not dump the whole Markdown into the response unless requested.

For a successful upload, also include the presenter link, optionally the audience shortlink, and an explicit statement that the draft was uploaded but not published.

If the user explicitly wants Markdown text only, provide it without creating or uploading an archive, and explain any supporting files it needs. If file tools are unavailable, state the limitation and provide the complete file contents and packaging instructions; never claim to have created a ZIP that does not exist.
