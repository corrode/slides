use std::{fs, io};

fn count_words(filename: &str) -> io::Result<usize> {
    let text = fs::read_to_string(filename)?;
    Ok(text.split_whitespace().count())
}
