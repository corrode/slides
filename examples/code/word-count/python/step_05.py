def count_words(filename):
    try:
        with open(filename) as file:
            text = file.read()
        return len(text.split())
    except FileNotFoundError:
        print(f"File not found: {filename}")
        return 0
