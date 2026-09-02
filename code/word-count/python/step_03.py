def count_words(filename):
    file = open(filename)
    text = file.read()
    file.close()
    return len(text.split())
