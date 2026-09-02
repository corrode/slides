
# Intro to Rust

A tour for Python and TypeScript developers

**Matthias Endler · corrode Rust Consulting**

---

![Düsseldorf from above](/assets/luftaufnahme-9231x4827-1200x627.jpg)

# Hi from Düsseldorf 👋

---

# Hi, I'm Matthias

I help engineering teams adopt and improve Rust.

**corrode Rust Consulting**

- Architecture and implementation
- Training and mentoring
- Practical help on real systems

---

# Today: a guided tour

- Start from what you already know
- See where Rust makes different trade-offs
- Build one tiny example together
- Leave plenty of room for questions
- **No expectation that you already know Rust.**

---

# What I heard from your survey

- Python is the dominant language
- TypeScript is common
- Most of you are new to Rust
- Almost nobody has used Rust in production

That's a good place to start!

---

# What you want to learn

- Project structure, modules, and visibility
- Error handling and testing
- Traits and generics
- Rust for web services

The clearest concern: **the learning curve**.

---

# This is interactive

Vote. Add your answers. Ask questions.

Use the reaction bar whenever something is:

- 👏 useful
- 💡 surprising
- ❓ unclear

---

# Coffee or beer?

:::poll question="" orientation="horizontal"
- Coffee
- Beer
:::

---

# Cats or dogs?

:::poll question="" orientation="horizontal"
- Cats 🐈
- Dogs 🐕
:::

---

# Outside of programming…

:::wordcloud prompt="Name one thing you enjoy besides programming" max="32"
:::

---

# What should we watch or read next?

One book, movie, or TV show title per response.

:::wordcloud prompt="" max="80"
:::

---

# Okay, so much for the warmup.

# Let's talk about Rust.

---

# Rust in one sentence

A systems programming language focused on:

- **reliability**
- **performance**
- **control**

...without a garbage collector.

---

# What feels different?

- Code is compiled and statically checked
- Ownership means **values** have **lifetimes**
- `Option` represents *possible absence* of values
- `Result` represents *possible failure*
- **Errors are values** and impossible to ignore

---

# Complexity exists either way

- Other languages often try to *hide complexity*
- That doesn't make it go away!

Rust doesn't pretend systems are simple.
It gives you tools to deal with complexity.

---

# cargo: one tool for everything

```text
cargo new       create a project
cargo build     compile it
cargo run       run it
cargo test      test it
cargo fmt       format it
cargo clippy    catch suspicious code
```

It's a bit like `uv`.

---

# Our tiny task

Given a filename, return its **total word count**.

For today, a “word” is a non-empty sequence separated by whitespace.

```text
Rust makes systems programming
safer and more approachable.

→ 8 words
```

---

# Python: start here

```python
def count_words(filename):
    pass
```

---

# Read, split, count

```python
def count_words(filename):
    file = open(filename)
    text = file.read()
    return len(text.split())
```

Simple—and correct for the happy path.

---

# We should close the file

```python
def count_words(filename):
    file = open(filename)
    text = file.read()
    file.close()
    return len(text.split())
```

But what if `read()` raises an exception?

---

# Let the context manager handle it

```python
def count_words(filename):
    with open(filename) as file:
        text = file.read()
    return len(text.split())
```

Now the file closes when the block ends—even on failure.

---

# Files can be missing

```python
def count_words(filename):
    try:
        with open(filename) as file:
            text = file.read()
        return len(text.split())
    except FileNotFoundError:
        print(f"File not found: {filename}")
        return 0
```

---

# Then reality arrives

What if the path is…

- a directory?
- unreadable?
- not valid UTF-8?
- much larger than memory?

None of this is Rust-specific. These were always part of the task.

---

# Make the assumptions explicit

```python
def count_words(filename):
    try:
        with open(filename, encoding="utf-8") as file:
            text = file.read()
        return len(text.split())
    except (
        FileNotFoundError,
        IsADirectoryError,
        PermissionError,
        UnicodeDecodeError,
    ) as error:
        print(f"Could not read {filename}: {error}")
        return 0
```

---

# What does `0` mean now?

- The file was empty
- The file did not exist
- The path was a directory
- Permission was denied
- The bytes were not valid UTF-8

We handled the errors—and erased the difference between them.

---

# Don't load the entire file

```python
def count_words(filename):
    try:
        with open(filename, encoding="utf-8") as file:
            return sum(len(line.split()) for line in file)
    except (
        FileNotFoundError,
        IsADirectoryError,
        PermissionError,
        UnicodeDecodeError,
    ) as error:
        print(f"Could not read {filename}: {error}")
        return 0
```

---

# One last question

What counts as one word?

```text
don't        state-of-the-art        🦀

中文没有空格
```

`split()` is a policy decision, not a universal definition.

---

# Now let's try Rust

```rust
fn count_words(_filename: &str) -> usize {
    todo!()
}
```

```text
thread 'main' panicked at 'not yet implemented'
```

`todo!()` compiles, but fails loudly if we reach it.

---

# Read, split, count?

```rust
use std::fs;

fn count_words(filename: &str) -> usize {
    let text = fs::read_to_string(filename);
    text.split_whitespace().count()
}
```

---

# The compiler stops us

```text
error[E0599]: no method named `split_whitespace`
found for enum `Result` in the current scope
```

`read_to_string` did not return text.

It returned either text **or an I/O error**.

---

# We can force the happy path

```rust
use std::fs;

fn count_words(filename: &str) -> usize {
    let text = fs::read_to_string(filename).unwrap();
    text.split_whitespace().count()
}
```

This runs—but a missing or invalid file now panics.

---

# Better: preserve failure

```rust
use std::{fs, io};

fn count_words(filename: &str) -> io::Result<usize> {
    let text = fs::read_to_string(filename)?;
    Ok(text.split_whitespace().count())
}
```

`?` means: return the error to the caller if reading failed.

---

# The return type tells the truth

```text
Ok(8)                 eight words
Ok(0)                 an empty file
Err(NotFound)         no file there
Err(PermissionDenied) cannot read it
Err(InvalidData)      not valid UTF-8
```

The caller decides what each failure should mean.

---

# Where is `close()`?

There isn't one.

`File` owns the operating-system file handle. When the value leaves scope, Rust drops it and closes the handle—even when `?` returns early.

This pattern is called **RAII**: resource acquisition is initialization.

---

# Stream the file

```rust
use std::{
    fs::File,
    io::{self, BufRead, BufReader},
};

fn count_words(filename: &str) -> io::Result<usize> {
    let file = File::open(filename)?;
    let reader = BufReader::new(file);

    reader.lines().try_fold(
        0,
        |count, line| Ok(count + line?.split_whitespace().count()),
    )
}
```

---

# Rust didn't remove the complexity

It gave each concern a visible home:

- Failure → `Result`
- Early return → `?`
- Cleanup → ownership and `Drop`
- Streaming → `BufReader`
- Text decoding → an explicit error
- “Word” → still our policy decision

---

# Rust's bargain

Systems are complex.

Related concepts have subtle differences.

In the face of ambiguity, the compiler asks for specificity.

> The compiler is your friend.

---

# Where we go from here

- Regular sessions around your current challenges
- Questions collected ahead of time
- Focused examples prepared for each session
- Active participation from the team

I'm not a magician—but I can help you understand Rust and apply it with confidence.

---

# Questions?

What felt useful, surprising, or unclear?
