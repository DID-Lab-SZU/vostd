#!/usr/bin/env python3
"""Generate a standalone verification-progress dashboard."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent
DEFAULT_INPUT = ROOT.parent / "target" / "verification-progress" / "progress.json"
DEFAULT_OUTPUT = ROOT / "index.html"
TEMPLATE_PATH = ROOT / "template.html"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, default=DEFAULT_INPUT)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument(
        "--check",
        action="store_true",
        help="validate the report and generated HTML without changing the output",
    )
    return parser.parse_args()


def load_report(path: Path) -> dict[str, Any]:
    try:
        report = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        raise SystemExit(f"progress report does not exist: {path}") from error
    except json.JSONDecodeError as error:
        raise SystemExit(f"invalid progress report JSON: {error}") from error

    if report.get("schema_version") != 1:
        raise SystemExit(
            "unsupported progress report schema_version: "
            f"{report.get('schema_version')!r}; expected 1"
        )

    required = ("project", "packages", "architectures", "verification", "repository")
    missing = [key for key in required if key not in report]
    if missing:
        raise SystemExit(f"progress report is missing fields: {', '.join(missing)}")

    if not isinstance(report["packages"], list) or not isinstance(
        report["architectures"], list
    ):
        raise SystemExit("packages and architectures must be JSON arrays")

    return report


def render(template: str, report: dict[str, Any]) -> str:
    if template.count("{{DATA_PLACEHOLDER}}") != 1:
        raise SystemExit("template must contain exactly one {{DATA_PLACEHOLDER}}")
    if template.count("{{BUILD_TIMESTAMP}}") != 1:
        raise SystemExit("template must contain exactly one {{BUILD_TIMESTAMP}}")

    data_json = json.dumps(report, ensure_ascii=False, separators=(",", ":"))
    data_json = data_json.replace("</", "<\\/")
    html = template.replace(
        "{{DATA_PLACEHOLDER}}", f"const PROGRESS_DATA = {data_json};"
    )
    return html.replace("{{BUILD_TIMESTAMP}}", str(report.get("generated_at", "unknown")))


def main() -> None:
    args = parse_args()
    report = load_report(args.input.resolve())
    template = TEMPLATE_PATH.read_text(encoding="utf-8")
    html = render(template, report)

    if args.check:
        if not args.output.exists():
            raise SystemExit(f"generated dashboard does not exist: {args.output}")
        current = args.output.read_text(encoding="utf-8")
        if current != html:
            raise SystemExit(
                "generated dashboard is stale; run `make` in "
                "vostd-verification-progress-dashboard"
            )
        print(f"dashboard is up to date: {args.output}")
        return

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(html, encoding="utf-8")
    print(f"generated {args.output} from {args.input}")


if __name__ == "__main__":
    main()
