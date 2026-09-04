# Local HTML embeds

Place each embedded page and all of its dependencies in a separate bundle directory:

```text
assets/embeds/demo/index.html
assets/embeds/demo/app.css
assets/embeds/demo/app.js
assets/embeds/demo/image.png
```

Reference dependencies with relative URLs so the bundle also works in downloaded session archives. Slides copies the complete bundle into the archive. Embed pages run in a restricted iframe and cannot submit forms, open popups, or access the parent page. Cross-origin resources and APIs such as `fetch` and WebSocket are blocked, but a page can navigate its own frame, so bundles must be trusted local content.
