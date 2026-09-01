# Slides

A server-rendered interactive presentation app built with Rust, Axum, SQLite, and HTMX 4.

The current vertical slice supports:

- Markdown decks separated by `---`
- Highlighted fenced code blocks, including Rust
- Unsaved preview, saved drafts, and immutable published versions
- Named shortlinks, automatically derived from deck titles when omitted, and six-digit live-session codes
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

`GET /healthz` returns `200 OK` when the process can query SQLite and `503 Service Unavailable` otherwise.

## Docker

Build and run the production image with a persistent data directory:

```sh
docker build -t slides .
docker volume create slides-data
docker run --rm \
  --name slides \
  -p 3000:3000 \
  -e SLIDES_ADMIN_PASSWORD=change-me \
  -e SLIDES_SECURE_COOKIES=false \
  -v slides-data:/app/data \
  slides
```

In production, provide `SLIDES_ADMIN_PASSWORD` through the deployment platform's secret store and set `SLIDES_SECURE_COOKIES=true` behind HTTPS. The container runs as UID/GID `10001`; `/app/data` must be writable by that user on Linux hosts.

The current live-update hub is process-local. Run exactly one application replica and mount persistent SQLite storage at `/app/data`.

## CI and deployment

`.github/workflows/ci.yml` formats, lints, and tests Rust; builds the Docker image for pull requests; and publishes `latest` plus commit-SHA tags to GHCR from `main`.

A push to `main` deploys through Coolify when these repository settings exist:

- variable `COOLIFY_RESOURCE_UUID`: the Slides application resource UUID;
- optional variable `COOLIFY_BASE_URL`: defaults to `https://admin.corrode.dev`;
- optional variable `DEPLOY_HEALTHCHECK_URL`: the public URL ending in `/healthz`;
- secret `COOLIFY_TOKEN`: an API token allowed to deploy that resource.

## Database migrations

SQLx verifies applied migrations by checksum. Once a migration has been run anywhere, do not edit or reformat it; add a new numbered file under `migrations/` instead. `.gitattributes` keeps migration line endings stable across platforms.

## Authoring syntax

The normative format specification and research notes are in [`docs/slide-format.md`](docs/slide-format.md). A complete, ready-to-present showcase is available at [`examples/kitchen-sink.md`](examples/kitchen-sink.md).

Decks use `---` separators, CommonMark content, fenced code blocks, and at most one poll, quiz, or word cloud per slide. Reactions are available on every slide without authoring syntax.

HTMX 4.0.0 and its `hx-sse` extension are vendored under `assets/`; the app has no frontend build step. The files come from the official `htmx.org@4.0.0` jsDelivr package. Their SHA-256 checksums are `e484d9171a9db30a39c8f16e3d709d4137f3211c659f8e6125816635033d593f` and `8a834680c4000a9034d79228872372a92e140c810a075cb6d4a76690dfc13085`, respectively.

Live updates use an in-process broadcast hub, so the current version must run as a single application process. SQLite remains the durable source of truth.
