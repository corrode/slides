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
