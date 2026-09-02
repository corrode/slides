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
