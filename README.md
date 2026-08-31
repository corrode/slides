# Slides

A server-rendered interactive presentation app built with Rust, Axum, SQLite, and HTMX 4.

The current vertical slice supports:

- Markdown decks separated by `---`
- Highlighted fenced code blocks, including Rust
- Unsaved preview, saved drafts, and immutable published versions
- Named shortlinks and six-digit live-session codes
- Presenter-controlled slide navigation and audience locking
- Anonymous polls, word clouds, quizzes, and rate-limited reactions
- Live horizontal and vertical result charts over Server-Sent Events
- Responsive presenter and audience views
- Structured font and color themes

## Run it

```sh
SLIDES_ADMIN_PASSWORD=change-me cargo run
```

Open <http://127.0.0.1:3000>. The SQLite database is created as `slides.db` by default.

Configuration:

- `SLIDES_ADMIN_PASSWORD`: required presenter password
- `SLIDES_DATABASE_URL`: defaults to `sqlite://slides.db`
- `SLIDES_BIND`: defaults to `127.0.0.1:3000`
- `SLIDES_SECURE_COOKIES`: set to `true` behind HTTPS
- `RUST_LOG`: standard tracing filter

## Authoring syntax

Separate slides with a line containing only `---`.

````markdown
# Ownership in Rust

```rust
fn consume(value: String) {
    println!("{value}");
}
```

---

# Ask the audience

:::poll question="Which language do you use most?" multiple orientation="horizontal"
- Rust
- Python
- TypeScript
- Go
:::

---

# One word

:::wordcloud prompt="Describe Rust in one word" max="80"
:::

---

# Quick quiz

:::quiz question="Which type owns its text?"
- [x] String
- [ ] &str
:::
````

Poll orientation may be `horizontal` or `vertical`. A slide may contain one interactive block in addition to Markdown content and reactions.

HTMX 4.0.0 and its `hx-sse` extension are vendored under `assets/`; the app has no frontend build step. The files come from the official `htmx.org@4.0.0` jsDelivr package. Their SHA-256 checksums are `e484d9171a9db30a39c8f16e3d709d4137f3211c659f8e6125816635033d593f` and `8a834680c4000a9034d79228872372a92e140c810a075cb6d4a76690dfc13085`, respectively.

Live updates use an in-process broadcast hub, so the current version must run as a single application process. SQLite remains the durable source of truth.
