
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

![Survey results for current Rust experience](/assets/survey-rust-experience.png)

---

![Survey results for main programming languages](/assets/survey-programming-languages.png)

---

![Survey results for IDE usage](/assets/survey-ides.png)

---

![Survey results for Rust ecosystem interests](/assets/survey-ecosystem-interests.png)

---

# This is interactive!

Ask questions and use the reactions whenever something is:

- 👏 useful
- 💡 surprising
- ❓ unclear

---

# Coffee or beer?

:::poll
- Coffee
- Beer
:::

---

# Cats or dogs?

:::poll
- Cats 🐈
- Dogs 🐕
:::

---

![My cat Oskar](/assets/oskar.jpg)

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

# Rust, the pitch

A systems programming language focused on:

- **reliability**
- **performance**
- **control**

...without a garbage collector.

---

# Why Rust is different

- Code is compiled and statically checked
- Ownership means **values** have **lifetimes**
- `Option` represents *possible absence* of values
- `Result` represents *possible failure*
- **Errors are values** and impossible to ignore

---

# cargo: one tool for everything

```sh
cargo new       # create a project
cargo add       # add a new dependency
cargo build     # compile it
cargo run       # run it
cargo test      # test it
cargo fmt       # format it
cargo clippy    # catch suspicious code
```

It's a bit like `uv`.

---

# Complexity exists either way

- Computers are complicated.
- Other languages often try to *hide complexity*.
- That doesn't make it go away!

Rust doesn't pretend systems are simple.
It gives you tools to deal with complexity.

---

# Example: Word Counter

Given a filename, return its **total word count**.

For today, a “word” is a non-empty sequence separated by whitespace.

```text
Rust makes systems programming
safer and more approachable.
```

→ 8 words

---

# Python interface

```python code/word-count/python/step_01.py
```

---

# Read, split, count

```python code/word-count/python/step_02.py
```

Simple and correct (for the happy path).  
...but what happens if we don't close the file?

---

# We should close the file

```python code/word-count/python/step_03.py
```

- If we don't, we leak file handles
- ...but what if `read()` raises an exception?

---

# Let the context manager handle it

```python code/word-count/python/step_04.py
```

Now the file closes when the block ends; even on failure.

---

# What happens if files are missing?

- Exception!
- Who catches that?
- Our entire program could crash.

---

# Handle missing files

```python code/word-count/python/step_05.py
```

---

# More problems

What if the path is…

- a directory?
- unreadable?
- not valid UTF-8?
- much larger than memory?

**The complexity was always there but Python hides it.**

---

# Make the assumptions explicit

```python code/word-count/python/step_06.py
```

---

# Wait, what does `0` mean?

- The file was empty
- The file did not exist
- The path was a directory
- Permission was denied
- The bytes were not valid UTF-8

We handled the errors, but we erased the difference between them.

---

# And what about large files?

```python
text = file.read()
```

- This leads the entire file into memory
- Can you handle a 1 petabyte file?

---

# Don't load the entire file

```python code/word-count/python/step_07.py
```

---

# One last question

What counts as one word?

```text
don't        state-of-the-art        🦀

中文没有空格
```

How does `split()` handle these? 

---

# Now let's try Rust

```rust code/word-count/rust/step_01.rs
```

```sh
thread 'main' panicked at 'not yet implemented'
```

- `todo!()` compiles, but fails loudly if we reach it.
- It's a bit like `pass`, but terminates the program.

---

# Read, split, count?

```rust code/word-count/rust/step_02.rs
```

---

# The compiler stops us

```sh
error[E0599]: no method named `split_whitespace`
found for enum `Result` in the current scope
```

`read_to_string` did not return text!

It returned either text **or an I/O error**.

---

# We can force the happy path

```rust code/word-count/rust/step_03.rs
```

This works, but a missing or invalid file now panics (i.e., halt and catch fire).

---

# Handle errors or return them

```rust code/word-count/rust/step_04.rs
```

`?` means: return the error to the caller if reading failed.

---

# The return type tells the truth

```rust
Ok(8)                 // eight words
Ok(0)                 // an empty file
Err(NotFound)         // no file there
Err(PermissionDenied) // cannot read it
Err(InvalidData)      // not valid UTF-8
```

No fake `0` value. The caller can decide how to handle each scenario. 

---

# Where is `close()`?

There isn't one!

`File` **owns** the operating-system file handle. When the value leaves scope, Rust drops it and closes the handle. **Even when `?` returns early.**

No leaking memory, no dangling pointers.

This is why Rust is so powerful.

---

# Rust didn't remove the complexity

It just made each problem visible:

- Failure → `Result`
- Early return → `?`
- Cleanup → ownership and `Drop`
- Text decoding → an explicit error
- “Word” → still our policy decision

---

# Rust's bargain

Systems are complex.

Related concepts have subtle differences. A path is not a string, for example.

In the face of ambiguity, the compiler asks for specificity.

**But the compiler is your friend. Read the error messages. 🤗**

---

# Where we go from here

If you like:

- Regular sessions around your current challenges
- Questions collected ahead of time
- Examples prepared for each session
- Active participation from the team

**I'm not a magician,** but I can help you understand Rust and apply it with confidence. 🧙‍♂️🪄

---

# Questions?

What felt useful, surprising, unclear?

---

# Where to go from here?

1. Add a function that counts lines in a file.
2. Add a function that counts characters in a file.
3. Change all filename parameters from `&str` to `&Path`.
4. Add a function that counts files in a directory.
5. Add a function that counts characters in all files in a directory.

**Bonus:**

6. Only count `.txt` files.
7. Print the filename with the most characters.
8. Create a `FileStats` struct containing words, lines, and characters.

[Open the self-directed exercise in the Rust Playground](https://play.rust-lang.org/?version=stable&mode=debug&edition=2024&gist=afa67b8068a6efcaa587734ad83aedfe)
