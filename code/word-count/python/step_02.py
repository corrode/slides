def count_words(filename):
    file = open(filename)
    text = file.read()
    return len(text.split())
