"""Regenerate only the bundled Shell CLI data from the Rust CLI and TOML catalogs."""
import argparse
import json
from pathlib import Path
import subprocess
import tomllib


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    binary = str(args.binary.resolve())
    actions = json.loads(subprocess.check_output(
        [binary, "--lang", "en-US", "list", "--json"], cwd=root, text=True, encoding="utf-8"))
    version = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    catalogs = {file.stem: tomllib.loads(file.read_text(encoding="utf-8"))
                for file in sorted((root / "locales").glob("*.toml"))}
    data = json.dumps(dict(version=version, actions=actions, catalogs=catalogs), ensure_ascii=False, indent=2)
    script = root / "cqs.sh"
    text = script.read_text(encoding="utf-8")
    begin = "    cat <<'CQS_METADATA_JSON'\n"
    end = "\nCQS_METADATA_JSON\n"
    prefix, rest = text.split(begin, 1)
    _, suffix = rest.split(end, 1)
    script.write_text(prefix + begin + data + end + suffix, encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
