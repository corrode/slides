# Slides: the kitchen sink

Everything this presentation app can do, in one live deck.

![A live presentation with charts and audience reactions](/assets/kitchen-sink.svg)

---

# Markdown, naturally

Write familiar Markdown instead of designing every slide by hand.

- **Bold ideas**, *quiet emphasis*, and ~~last week's plan~~
- `inline code` alongside [useful links](https://corrode.dev)
- Clear headings and readable lists

> The source stays useful even outside the presentation app.

---

# Lists and progress

1. Create a deck
2. Publish an immutable version
3. Start a live session
4. Invite the audience

- [x] Server-rendered slides
- [x] Live audience updates
- [x] Anonymous participation
- [ ] Add your own great content

---

# Tables work too

| Feature | Presenter | Audience |
| --- | --- | --- |
| Slide navigation | Controls | Follows live |
| Poll results | Live chart | Reveal on cue |
| Reactions | Live totals | One-tap feedback |
| History | Always current | Browse past slides |

Compact data, without leaving Markdown.

---

# Highlighted code

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app = build_router().await?;
    let listener = TcpListener::bind("0.0.0.0:3000").await?;

    axum::serve(listener, app).await?;
    Ok(())
}
```

Fenced code blocks use language-aware syntax highlighting.

---

# Predictable and safe

Presentation markers inside code stay code:

```markdown
---
:::poll question="Not an actual poll"
- One
- Two
:::
```

Raw HTML such as <button onclick="alert('nope')">this button</button> is shown as text, never executed.

---

# Start with a quick vote

Answers update the presenter's horizontal bar chart live.

:::poll question="What makes a presentation memorable?" orientation="horizontal"
- A clear story
- Useful examples
- Audience participation
- Excellent timing
:::

---

# Turn the chart around

The same poll can use vertical result bars.

:::poll question="How are you joining today?" orientation="vertical"
- Laptop
- Phone
- Tablet
- Other
:::

---

# Choose more than one

The `multiple` flag turns each option into an independent toggle.

:::poll question="Which features would you use?" multiple orientation="horizontal"
- Live polls
- Word clouds
- Quizzes
- Reactions
:::

---

# Build a word cloud

Repeated answers grow larger when results are revealed.

:::wordcloud prompt="Describe a great presentation in one word" max="32"
:::

---

# Check understanding

Close responses when time is up, then reveal the answer and distribution.

:::quiz question="What carries live slide updates to connected clients?"
- [ ] WebSockets
- [x] Server-Sent Events
- [ ] Long polling
- [ ] Carrier pigeons
:::

---

# Presenter controls

During a session you can:

- move everyone to the previous or next slide;
- allow free navigation or lock future slides;
- open and close an interaction;
- reveal results when you're ready;
- end the session for every participant.

Audience members can browse completed slides until you move again.

---

# Reactions are always available

Use the reaction bar now: **heart**, **thumbs up**, **applause**, **laugh**, or **question**.

Every reaction updates live without refreshing the page.

> Try opening the audience view in a second browser and watch both screens stay in sync.

---

# That's the whole kitchen sink

Markdown authoring. Live control. Audience participation. Animated results.

**Now replace this deck with your story.**
