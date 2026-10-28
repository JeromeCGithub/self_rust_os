#!/usr/bin/env python3
"""Check AI guide presence and local Markdown links using only Python's stdlib.

Covers inline links/images and ATX heading anchors, not remote URL availability,
reference-style links, or the correctness of technical claims.
"""
from pathlib import Path
import re
import sys
from urllib.parse import unquote, urlsplit

REQUIRED = (
    "AGENTS.md", "CLAUDE.md", ".github/copilot-instructions.md",
    "src/AGENTS.md", "user_programs/AGENTS.md",
    "tests/AGENTS.md", "e2e/AGENTS.md", "README.md", "docs/README.md",
    "docs/architecture.md", "docs/development.md", "docs/testing.md",
    "docs/syscalls.md", "docs/grub-boot.md",
)
LINK = re.compile(r"!?\[[^\]\n]*\]\(\s*(<[^>]+>|[^\s)]+)(?:\s+\"[^\"]*\")?\s*\)")
FENCE = re.compile(r"^\s*(`{3,}|~{3,}).*?$.*?^\s*\1\s*$", re.M | re.S)


def headings(text):
    """Return GitHub-style slugs for plain ATX headings, including duplicates."""
    counts = {}
    result = set()
    for heading in re.findall(r"^ {0,3}#{1,6}\s+(.+?)\s*#*\s*$", FENCE.sub("", text), re.M):
        slug = re.sub(r"[^\w\- ]", "", heading.lower()).replace(" ", "-")
        count = counts.get(slug, 0)
        counts[slug] = count + 1
        result.add(f"{slug}-{count}" if count else slug)
    return result


def check_markdown(root, files):
    root = root.resolve()
    errors = []
    for source in files:
        text = FENCE.sub("", source.read_text())
        for match in LINK.finditer(text):
            target = match.group(1).strip("<>")
            parsed = urlsplit(target)
            if parsed.scheme or parsed.netloc:
                continue
            path = unquote(parsed.path)
            destination = ((root / path.lstrip("/")) if path.startswith("/")
                           else (source.parent / path if path else source)).resolve()
            prefix = f"{source.relative_to(root)}: {target}: "
            if not destination.is_relative_to(root):
                errors.append(prefix + "outside repository")
            elif not destination.exists():
                errors.append(prefix + "missing file")
            elif parsed.fragment and destination.suffix == ".md":
                if unquote(parsed.fragment) not in headings(destination.read_text()):
                    errors.append(prefix + "missing heading")
    return errors


def check_repository(root):
    root = root.resolve()
    errors = [f"missing required guide: {name}" for name in REQUIRED
              if not (root / name).is_file()]
    pages = {root / name for name in REQUIRED if (root / name).is_file()}
    pages.update((root / "docs").rglob("*.md"))
    return errors + check_markdown(root, sorted(pages))


def main():
    errors = check_repository(Path(__file__).resolve().parents[1])
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("Documentation entry points and local Markdown links: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
