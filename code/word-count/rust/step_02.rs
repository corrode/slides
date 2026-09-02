use std::fs;

fn count_words(filename: &str) -> usize {
    let text = fs::read_to_string(filename);
    text.split_whitespace().count()
}
