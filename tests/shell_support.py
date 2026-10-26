"""Load the production shell entrypoint without its interactive startup."""

from pathlib import Path
import shlex
import subprocess


SCRIPT = Path(__file__).resolve().parents[1] / "cqs.sh"
SOURCE = SCRIPT.read_text(encoding="utf-8")


def load_script():
    return f"source {shlex.quote(str(SCRIPT))}\n"


def function_source(name):
    result = subprocess.run(
        ["bash", "-c", load_script() + 'declare -f "$1"', "_", name],
        text=True, capture_output=True, check=True, timeout=10,
    )
    return result.stdout
