use std::fs;

fn count_words(filename: &str) -> usize {
    let text = fs::read_to_string(filename).unwrap();
    text.split_whitespace().count()
}
