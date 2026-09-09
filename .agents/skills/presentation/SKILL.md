---
name: presentation
description: Create or revise a presentation for this Slides app as a complete ZIP bundle containing slides.md and its code, images, and HTML demos. Use for talks, workshops, lessons, pitches, and slide decks, including packaging or uploading a presentation draft.
---

# Create a presentation bundle

Deliver an editable source directory and an upload-ready ZIP for one presentation. Markdown is the entry point, not the entire deliverable. A Markdown-only presentation is still a bundle containing `slides.md`. ZIP bundles are import packaging, not a separate deck type: all drafts are browser editable, including their Markdown, inlined code, title, and theme.

When working in the Slides repository, use `docs/slide-format.md` as the syntax reference. Outside that repository, use `references/slide-format.md` relative to this skill's directory; the global installation includes a snapshot. References below to `docs/slide-format.md` mean whichever copy applies. If neither is available, obtain the reference from the user rather than guessing. Read its bundle rules and the sections relevant to the deck before authoring. Do not invent layout directives, metadata fields, API endpoints, or validation commands.

## 1. Establish the task

Use the supplied audience, topic, source material, duration, and desired outcome. Ask only for missing information that would materially change the presentation. Otherwise choose reasonable defaults and proceed.

Distinguish the requested action:

- **Create or revise:** write local source files and build a ZIP. No network upload.
- **Upload:** create or replace a server draft using the bundle API.
- **Publish or present:** upload if authorized, then explain the separate presenter-UI action. The bundle API does not publish or start a session.

Saving files locally does not authorize uploading. Do not deploy the application, modify CI, or change server configuration as part of making a deck.

For local bundle revisions, preserve the existing files and authoring paths. Change the requested content, then rebuild the complete archive. Do not return a patch as the presentation or upload only the changed files. The API's `source` field contains the current draft's resolved Markdown, including browser edits, not a recoverable copy of the original multi-file bundle; prefer the local authoring directory or original ZIP for bundle authoring. Before replacing a draft, reconcile any browser edits the user wants to keep into the local source: a full ZIP replaces draft content and title rather than merging them. Browser edits do not update the original bundle files.

## 2. Plan the presentation

- Give the audience a clear problem, explanation, and takeaway.
- Give each slide one purpose. Prefer concrete examples and short text to paragraphs.
- Budget roughly one to two minutes per substantive slide unless the format calls for another pace.
- Put delivery cues and supporting detail in presenter notes, not audience-facing prose.
- Use interactions when they help the audience learn or make a decision, not to demonstrate every feature.
- Verify factual claims and cite sources where useful. Do not invent quotations, results, or metrics.
- Keep diagrams and code readable at presentation distance. Do not add HTML demos when ordinary Markdown or Mermaid would be clearer.

### Style guide

- Do not use em dashes. Choose a period, comma, colon, semicolon, or parentheses to fit the sentence, or rewrite it.
- Write for the audience, not the authoring conversation. Leave out references to prior prompts, revision requests, and private discussions. Include any context the audience needs to understand the point.
- Work toward a clear outcome: what should the audience understand, decide, or do? Keep each slide focused on that outcome and cut material that does not help.
- Be brief without becoming cryptic. Use plain, natural language and concrete details. Cut filler, not meaning; prefer a clear sentence to a puzzling fragment.
- Optimize for reading speed throughout the presentation. The audience should grasp on-screen text quickly while following the speaker. Use familiar words, short sentences, and concise labels; move detail that requires sustained reading into presenter notes or supporting material.
- Use a question as a slide heading when it helps establish the problem or explain why the content matters. Let the slide answer it; do not force every heading into a question.
- Avoid ASCII art. Use Mermaid for diagrams when it makes relationships or steps easier to understand; use prose or a list when that is clearer.

### Slide composition

- Design a slide to support a spoken explanation, not to serve as a reference page. Choose one main element: a code example, diagram, comparison, statement, or audience question. Add only the context needed to understand it.
- Make the first glance useful: the audience should see the topic and know where to look. A heading plus one main element is a good starting point, not a mandatory template. Do not stack a diagram, table, bold takeaway, blockquote, and source link just because each fits.
- Write simple, easy-to-understand headings that get straight to the point and name the specific question or conclusion. The audience should understand them at a glance, without decoding clever wording or jargon. Keep them short enough to leave room for the content. Avoid vague exhortations such as “Choose one boundary to improve” when a concrete decision would tell the audience more.
- Let the example carry the explanation. Do not repeat the same point in the heading, body, bold text, and blockquote. Reserve blockquotes for actual quotations, not visual emphasis around ordinary instructions.
- Use empty space to separate and emphasize content, not as a reason to add filler. Large unused margins around an unreadably small diagram mean the diagram needs a different layout, not more surrounding text.
- Stay within the app's supported Markdown and theme controls. Do not invent columns, sizing attributes, or custom HTML to rescue an overloaded slide. Simplify or split it instead.

### Diagrams, code, and comparisons

- A diagram must be readable at presentation distance, including node and edge labels. Mermaid rendering successfully is not proof that it works as a slide. Never accept a miniature diagram surrounded by readable body text.
- Match the diagram's direction to the slide's available space. For a short pipeline on a wide slide, try `flowchart LR` before `TD`. Keep labels short, remove unnecessary nodes and edge captions, and check the rendered result. If it still needs tiny text, split the diagram or give it a slide of its own.
- Use a diagram to explain a relationship, transition, or dependency. If it only restates a nearby table or sentence, choose the clearer representation and remove the other. A three-step process may need only three short lines, not a graph.
- Use tables when the audience needs to compare entries along the same dimensions. Keep cells brief and rows few. Put detailed responsibility inventories in supporting material when the slide's real purpose is to explain one distinction.
- Show only the code needed to establish the point, with enough context to make it understandable. Put the full implementation behind an IDE source link or in supporting files. Never shrink code to fit a whole implementation.
- Keep references available without making them compete with the explanation. Prefer the snippet's IDE link for code navigation. Use brief, descriptive Markdown links for other sources and explain worksheets or homework on a dedicated next-step slide.

### Questions and audience participation

- Give a poll or exercise its own slide when it asks the audience to act. Use one question and concise, parallel choices; move the rationale, homework instructions, and other calls to action elsewhere.
- Ask about a concrete choice the audience is equipped to make. For example, “Which change should we try first?” could offer “Separate persistence from live state,” “Separate metadata from open files,” and “Represent compaction as a plan,” after those alternatives have been explained. Preserve the technical distinction without turning each option into a paragraph.
- Separate teaching, discussion, and follow-up. Do not end with a general instruction, a second instruction in a blockquote, a worksheet link, and a poll all competing for attention.

### Check the rendered slides

Inspect every slide at the intended presentation size when a preview is available. Check the rendered output, not just the Markdown:

- Is there one obvious focus, with a simple, direct heading that is understandable at a glance?
- Can the audience read and grasp the slide quickly while still following the speaker?
- Can the audience read every diagram label, code line, table cell, and poll choice without zooming?
- Does every visible element add something, rather than repeat or distract?
- Does the slide fit comfortably without clipping, awkward wrapping, or tiny content?
- Can a viewer understand the slide without knowing the authoring conversation?

Revise slides that fail these checks. Prefer cutting, splitting, or changing the representation over reducing text size. If visual inspection is unavailable, say so and flag dense or diagram-heavy slides as unverified rather than claiming they look good.

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
- Do not invent columns, reveal markers, slide classes, or background directives. New imports use the app's default theme. Every draft's theme is browser editable; replacement preserves the deck's existing theme.
- Use at most one `:::notes` block and one interaction per slide. Close directive blocks with `:::` and consult the format reference for exact attributes.
- Polls and ordering exercises need at least two items. Quizzes need at least two options and one marked correct answer. Word clouds have no body. Attribute values use double quotes, with no embedded quote escaping.
- Keep reference-style link definitions on the slide using them; reference labels are slide-local.

### Code

Use inline fenced code for short examples. Put reusable or longer examples in real UTF-8 files under `code/`, included through an otherwise empty fence:

````markdown
```rust code/example.rs
```
````

Includes expand the whole file into inline code at upload. That code is browser editable without changing the imported source file. Do not use line ranges, regions, placeholders, or extra fence arguments other than one trailing `ide="URL"` attribute. Use a longer fence if included content could close it.

#### Link a snippet to its source in an IDE

Add one trailing `ide="URL"` attribute to the opening code fence to link a snippet to its source file in Zed or another supported IDE. This works for both inline code and file includes:

````markdown
```rust ide="zed://file/Users/example/project/src/main.rs:12:3"
fn main() {}
```

```rust code/example.rs ide="vscode://file/Users/example/project/src/main.rs:12:3"
```
````

These example URLs point to line 12, column 3. Replace them with user-supplied editor URLs and paths in actual decks. The bundle-relative include path selects the code to display; the IDE URL independently selects the file to open on the viewer's machine. If none is supplied, omit `ide` rather than inventing a destination. Metadata survives includes and is not displayed or copied as code. The native **Open in IDE** link icon sits beside **Copy**, appears on hover or keyboard focus, stays visible on touch devices, and has an accessible label. Opening is explicit, requires the viewer's installed URL handler, and may prompt for permission; Slides never launches automatically, fetches the IDE URL, or checks its target file. Bundle code includes still must exist.

Use exactly one lowercase `ide` attribute, double-quoted, after the language and optional include path; percent-encode spaces as `%20`. Raw whitespace, controls, quotes, backslashes, backticks, and angle brackets are rejected, as are malformed/duplicate attributes and other arguments alongside IDE metadata. Allowed schemes (case-insensitive): `zed`, `vscode`, `vscode-insiders`, `idea`, `pycharm`, `clion`, `goland`, `rustrover`, `webstorm`, `phpstorm`, `rider`, `rubymine`, `datagrip`, `jetbrains`. Other schemes are rejected. No `ide` on `mermaid` fences. This allowlist is only for code fence metadata; ordinary Markdown link schemes remain HTTP(S), `mailto:`, and `zed:`.

For a GitHub or other HTTPS source URL, add an ordinary Markdown link next to the snippet instead. The `ide` attribute accepts editor schemes, not web URLs; there is no `source="URL"` fence attribute.

Test examples when practical. Only Rust blocks (`rust`/`rs`) can offer an explicit Run action backed by the public Rust Playground; do not include secrets or send confidential code there without authorization. Uploading a bundle itself never executes its code.

### Images, diagrams, and HTML

Reference existing bundle-relative paths:

```markdown
![Diagram description](images/diagram.svg)

[Example source](code/example.rs)

:::iframe src="demo/index.html" title="Interactive demonstration"
:::
```

- No external images or iframe URLs. Ordinary navigation links may use HTTP(S), mailto, `zed:`, or fragments. Keep supplied Zed URLs as clickable Markdown links, including in presenter notes; do not downgrade them to copyable text. For example: `[Open in Zed](zed://file/Users/example/project/main.rs:12:3)`. Targets refer to the viewer's Zed installation, are not bundled files, and may require browser permission to open. Preserve supplied paths rather than inventing local file locations.
- In ZIP source files, do not author `/assets/...` paths or fabricate generation URLs; the importer rewrites asset references to immutable generation URLs. Keep those generated URLs when editing the imported draft in the browser. Browser edits do not modify imported assets; changing asset files requires a complete replacement ZIP. Do not link to `slides.md`, which is private source and may contain notes.
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

### Server and credentials

- Default server: **`https://slides.corrode.dev`**. Use this unless the user explicitly selects another server; `SLIDES_URL` may also supply an override. Do not ask for the default server URL again.
- The user has stored the production bearer token in **macOS Keychain**, with service **`slides.corrode.dev.api-token`** and account **`slides`**. Use this entry for `https://slides.corrode.dev`; no environment variable setup is needed. Do not use this credential for an alternate server.
- Retrieve it inside the upload process by running `/usr/bin/security find-generic-password -s slides.corrode.dev.api-token -a slides -w` with stdout captured (for example, Python `subprocess.run(..., check=True, capture_output=True, text=True, timeout=30)`). Strip the trailing newline and construct the Authorization header in memory. **Do not run this command directly in a visible terminal: `-w` prints the secret.**
- If the item is missing, access is denied, or retrieval times out, ask the user to unlock Keychain or approve access. Do not create or rotate credentials automatically. An explicitly configured `SLIDES_API_TOKEN` may be used as an alternative for the selected server.
- Never paste the token into this skill, presentation files, ZIPs, source control, command arguments, logs, or responses. Do not use shell tracing or verbose HTTP diagnostics.
- Send credentials only to the selected, trusted server over HTTPS. Do not forward Authorization headers across redirects; treat an upload redirect as something to inspect, not automatically follow.

Do not search unrelated files or databases for credentials or ask the user to paste the secret into the conversation. Token management is at `https://slides.corrode.dev/admin/settings`, but the existing Keychain entry is the configured source.

The API bearer token is separate from `ADMIN_PASSWORD`, which is for presenter login. Do not use the password as a bearer token or change authentication settings.

1. Choose a slug with the user’s intent. Slugs use 1–48 lowercase ASCII letters, digits, or hyphens, without leading/trailing hyphens; reserved routes are rejected. Check for an existing presentation before a new upload. Ask before replacing an unrelated deck.
2. Send `POST /api/v1/presentations/{slug}/bundle` with `Authorization: Bearer <token>`, `Content-Type: application/zip`, and the ZIP as the raw body. No JSON payload, multipart form, separate embed upload, or PATCH request.
3. Expect `201 Created` for creation or `200 OK` for replacement. Read the response rather than assuming success. `413` indicates size limits, `415` a content-type error, and `422` invalid bundle contents. Correct the underlying files/archive before retrying; never suppress validation failures.
4. A successful upload installs immutable assets and replaces the draft content and title, including browser edits. It preserves the existing theme, published versions and their assets, and running sessions. The imported draft remains browser editable; another complete ZIP is needed only when replacing the import, not for every edit.
5. Provide `<server>/admin/decks/<slug>/edit` for authenticated editing, preview, and publishing. `<server>/<slug>` is the audience shortlink, not evidence that the new draft is published. The `Location` header points to the API resource, not the presenter UI.

If the user asks to publish, explain that they must choose Publish in the presenter UI; do not report an upload as publication. Never generate or rotate credentials automatically.

## Deliver the result

Default to a short handoff with clickable paths to the **source directory**, **`slides.md`**, and **ZIP**, plus the checks actually performed and any remaining limitations. Do not dump the whole Markdown into the response unless requested.

For a successful upload, also include the presenter link, optionally the audience shortlink, and an explicit statement that the draft was uploaded but not published.

If the user explicitly wants Markdown text only, provide it without creating or uploading an archive, and explain any supporting files it needs. If file tools are unavailable, state the limitation and provide the complete file contents and packaging instructions; never claim to have created a ZIP that does not exist.
