#!/usr/bin/env python3

import contextlib
import importlib.util
import io
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from types import ModuleType
from unittest.mock import patch

sys.dont_write_bytecode = True

ROOT = Path(__file__).resolve().parent
EXAMPLES_DIR = ROOT.parent.parent
PYTHON_DIR = ROOT / "python"
RUST_DIR = ROOT / "rust"
FIXTURES = ROOT / "fixtures"
DECK = EXAMPLES_DIR / "intro-to-rust.md"

PYTHON_STEPS = [PYTHON_DIR / f"step_{number:02}.py" for number in range(1, 8)]
RUST_STEPS = [RUST_DIR / f"step_{number:02}.rs" for number in range(1, 6)]


def load_module(path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(path.stem, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Could not load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def quietly(function, *args):
    with contextlib.redirect_stdout(io.StringIO()):
        return function(*args)


def verify_python():
    input_file = FIXTURES / "input.txt"
    empty_file = FIXTURES / "empty.txt"
    missing_file = FIXTURES / "missing.txt"
    invalid_file = FIXTURES / "invalid-utf8.txt"
    invalid_file.write_bytes(b"valid text\xffinvalid text")

    modules = [load_module(path) for path in PYTHON_STEPS]
    assert modules[0].count_words(input_file) is None

    for module in modules[1:4]:
        assert module.count_words(input_file) == 8

    assert quietly(modules[4].count_words, input_file) == 8
    assert quietly(modules[4].count_words, missing_file) == 0

    for module in modules[5:]:
        assert quietly(module.count_words, input_file) == 8
        assert quietly(module.count_words, empty_file) == 0
        assert quietly(module.count_words, missing_file) == 0
        assert quietly(module.count_words, FIXTURES) == 0
        assert quietly(module.count_words, invalid_file) == 0
        with patch("builtins.open", side_effect=PermissionError):
            assert quietly(module.count_words, input_file) == 0

    invalid_file.unlink()
    print("Python steps: verified")


def rust_string(path):
    return json.dumps(str(path))


def compile_rust(source, output):
    return subprocess.run(
        ["rustc", "--edition=2024", str(source), "-o", str(output)],
        capture_output=True,
        text=True,
        check=False,
    )


def verify_rust():
    input_file = rust_string(FIXTURES / "input.txt")
    empty_file = rust_string(FIXTURES / "empty.txt")
    missing_file = rust_string(FIXTURES / "missing.txt")
    invalid_file_path = FIXTURES / "invalid-utf8.txt"
    invalid_file_path.write_bytes(b"valid text\xffinvalid text")
    invalid_file = rust_string(invalid_file_path)
    directory = rust_string(FIXTURES)

    harnesses = {
        1: "fn main() { let _ = count_words(\"unused.txt\"); }\n",
        3: (
            "fn main() {\n"
            f"    assert_eq!(count_words({input_file}), 8);\n"
            f"    let missing = std::panic::catch_unwind(|| count_words({missing_file}));\n"
            "    assert!(missing.is_err());\n"
            "}\n"
        ),
        4: (
            "fn main() {\n"
            f"    assert_eq!(count_words({input_file}).unwrap(), 8);\n"
            f"    assert_eq!(count_words({empty_file}).unwrap(), 0);\n"
            f"    assert!(count_words({missing_file}).is_err());\n"
            f"    assert!(count_words({invalid_file}).is_err());\n"
            "}\n"
        ),
        5: (
            "fn main() {\n"
            f"    assert_eq!(count_words({input_file}).unwrap(), 8);\n"
            f"    assert_eq!(count_words({empty_file}).unwrap(), 0);\n"
            f"    assert!(count_words({missing_file}).is_err());\n"
            f"    assert!(count_words({invalid_file}).is_err());\n"
            f"    assert!(count_words({directory}).is_err());\n"
            "}\n"
        ),
    }

    with tempfile.TemporaryDirectory() as temporary_directory:
        temporary = Path(temporary_directory)

        for number in (1, 3, 4, 5):
            source = temporary / f"step_{number:02}.rs"
            source.write_text(
                RUST_STEPS[number - 1].read_text() + "\n" + harnesses[number]
            )
            binary = temporary / f"step_{number:02}"
            compiled = compile_rust(source, binary)
            assert compiled.returncode == 0, compiled.stderr
            result = subprocess.run(binary, capture_output=True, text=True, check=False)
            if number == 1:
                assert result.returncode != 0
                assert "not yet implemented" in result.stderr
            else:
                assert result.returncode == 0, result.stderr

        broken_binary = temporary / "step_02"
        broken = compile_rust(RUST_STEPS[1], broken_binary)
        assert broken.returncode != 0
        assert "Result" in broken.stderr
        assert "split_whitespace" in broken.stderr

    invalid_file_path.unlink()
    print("Rust steps: verified (including expected failures)")


def referenced_files(language, source):
    pattern = rf"```{language} (code/[^\s]+)\n\s*```"
    return re.findall(pattern, source)


def verify_deck_snippets():
    source = DECK.read_text()
    expected_python = [path.relative_to(EXAMPLES_DIR).as_posix() for path in PYTHON_STEPS]
    expected_rust = [path.relative_to(EXAMPLES_DIR).as_posix() for path in RUST_STEPS[:4]]
    assert referenced_files("python", source) == expected_python
    assert referenced_files("rust", source) == expected_rust
    print("Slide code references: synchronized with source files")


def main():
    verify_python()
    verify_rust()
    verify_deck_snippets()
    print("All word-count examples are valid.")


if __name__ == "__main__":
    main()
