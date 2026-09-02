def count_words(filename):
    with open(filename) as file:
        text = file.read()
    return len(text.split())
