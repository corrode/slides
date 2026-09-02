def count_words(filename):
    try:
        with open(filename, encoding="utf-8") as file:
            text = file.read()
        return len(text.split())
    except (
        FileNotFoundError,
        IsADirectoryError,
        PermissionError,
        UnicodeDecodeError,
    ) as error:
        print(f"Could not read {filename}: {error}")
        return 0
