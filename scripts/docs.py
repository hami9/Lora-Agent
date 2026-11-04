"""Validate and package the Lora Agent architecture using Python 3.10+."""

import argparse
import hashlib
from pathlib import Path
import re
import sys
from urllib.parse import unquote, urlsplit
from zipfile import ZipFile, ZipInfo


ROOT = Path(__file__).resolve().parents[1]
TOPICS = """
ARCHITECTURE SYSTEM_OVERVIEW COMPETITIVE_RESEARCH TECH_STACK INFERENCE_ENGINE
HARDWARE_INTELLIGENCE AGENT_RUNTIME OS_INTEGRATION SECURITY_ARCHITECTURE
PLUGIN_SYSTEM MODEL_MANAGEMENT MEMORY_AND_RAG TRAINING_SYSTEM DESKTOP_UX
API_AND_SDK PLATFORM_SUPPORT PERFORMANCE_STRATEGY BENCHMARK_PLAN
REPOSITORY_STRUCTURE DEVELOPMENT_ROADMAP MVP_SPECIFICATION RISK_REGISTER CI_CD
""".split()
DECISIONS = """
language-strategy inference-backends process-isolation desktop-framework storage
agent-runtime security-model extension-runtime platform-integration training-backend
""".split()
DIAGRAMS = """
ARCHITECTURE INFERENCE_ENGINE AGENT_RUNTIME HARDWARE_INTELLIGENCE
SECURITY_ARCHITECTURE PLUGIN_SYSTEM MODEL_MANAGEMENT TRAINING_SYSTEM
""".split()
REQUIRED = (
    [f"docs/{topic}.md" for topic in TOPICS]
    + [f"docs/adr/{i:04}-{name}.md" for i, name in enumerate(DECISIONS, 1)]
    + ["README.md", "CONTRIBUTING.md", "CODE_OF_CONDUCT.md", "SECURITY.md"]
)
# The authoring convention uses inline links or reference definitions on one line.
LINK = re.compile(r"\]\(\s*(?:<([^>]+)>|((?:[^\s()]|\([^()\s]*\))+))")
REFERENCE = re.compile(r"^ {0,3}\[[^\]]+\]:\s*(?:<([^>]+)>|(\S+))")
FENCE = re.compile(r"^ {0,3}(\x60{3,}|~{3,})(.*)$")
TAG = re.compile(r"docs-v(?:0|[1-9][0-9]*)(?:\.(?:0|[1-9][0-9]*)){2}")
TEXT = {".md", ".yml", ".yaml", ".py"}
IMAGES = {".png", ".jpg", ".jpeg", ".webp", ".svg"}


def payload(root):
    """Only release reviewed documentation, its assets and contributor tooling."""
    names = set(REQUIRED)
    for pattern in (
        "docs/**/*.md", "docs/assets/*", ".github/**/*.md", ".github/**/*.yml",
        ".github/**/*.yaml", "scripts/*.py",
    ):
        for path in root.glob(pattern):
            if pattern == "docs/assets/*" and path.suffix.lower() not in IMAGES:
                continue
            if path.is_file():
                names.add(path.relative_to(root).as_posix())
    return sorted(names)


def parse_markdown(content):
    headings, links, errors = set(), [], []
    fence = None
    diagram = False
    mermaid_lines = None
    for number, line in enumerate(content.splitlines(), 1):
        mark = FENCE.match(line)
        if fence:
            if mark and mark[1][0] == fence[0] and len(mark[1]) >= len(fence) and not mark[2].strip():
                if mermaid_lines is not None:
                    body = [s for s in mermaid_lines if s and not s.startswith("%%")]
                    if len(body) >= 2 and re.match(r"^(?:flowchart|graph|sequenceDiagram|stateDiagram-v2)\b", body[0]):
                        diagram = True
                    else:
                        errors.append(f"{number}: empty or unrecognized Mermaid diagram")
                fence, mermaid_lines = None, None
            elif mermaid_lines is not None:
                mermaid_lines.append(line.strip())
            continue
        if mark:
            fence = mark[1]
            mermaid_lines = [] if mark[2].strip() == "mermaid" else None
            continue
        if heading := re.match(r"^ {0,3}#{1,6}\s+(.+)$", line):
            title = heading[1].strip().rstrip("#").strip().lower()
            slug = re.sub(r"[^\w -]", "", title).replace(" ", "-")
            key, index = slug, 0
            while key in headings:
                index += 1
                key = f"{slug}-{index}"
            headings.add(key)
        line = re.sub(r"(\x60+).*?\1", "", line)
        for match in LINK.finditer(line):
            links.append((number, match[1] or match[2]))
        if match := REFERENCE.match(line):
            links.append((number, match[1] or match[2]))
    if fence:
        errors.append("unclosed code fence")
    return headings, links, diagram, errors


def validate(root):
    names = payload(root)
    documents, errors = {}, []
    for name in names:
        path = root / name
        if not path.is_file():
            errors.append(f"{name}: required file missing")
            continue
        if path.resolve() != root.resolve() / name:
            errors.append(f"{name}: symbolic links cannot be distributed")
            continue
        if path.suffix != ".md":
            continue
        content = path.read_text(encoding="utf-8-sig")
        headings, links, diagram, problems = parse_markdown(content)
        if name in REQUIRED and not headings:
            problems.append("missing Markdown heading")
        if name in [f"docs/{topic}.md" for topic in DIAGRAMS] and not diagram:
            problems.append("required Mermaid diagram missing")
        documents[name] = (headings, links)
        errors.extend(f"{name}: {problem}" for problem in problems)
    for name, (_, links) in documents.items():
        for number, dest in links:
            uri = urlsplit(dest)
            if uri.scheme in ("http", "https", "mailto"):
                continue
            location = unquote(uri.path)
            target = (root / name).parent / location if location else root / name
            target = target.resolve()
            if uri.scheme or uri.netloc or location.startswith("/") or not target.is_relative_to(root):
                errors.append(f"{name}:{number}: non-portable link {dest}")
                continue
            if target.is_dir():
                target /= "README.md"
            key = target.relative_to(root).as_posix()
            if key not in names or not target.is_file():
                errors.append(f"{name}:{number}: missing package target {dest}")
            elif uri.fragment and key in documents and unquote(uri.fragment) not in documents[key][0]:
                errors.append(f"{name}:{number}: missing heading {dest}")
    if errors:
        raise ValueError("\n".join(errors))
    print(f"Verified {len(documents)} Markdown files and {len(DIAGRAMS)} diagram locations.")
    return names


def package(root, names, tag, output):
    if not TAG.fullmatch(tag):
        raise ValueError("Expected docs-vMAJOR.MINOR.PATCH without leading zeroes.")
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"lora-agent-{tag}.zip"
    with ZipFile(archive, "w") as bundle:
        for name in names:
            data = (root / name).read_bytes()
            if Path(name).suffix in TEXT:
                data = data.replace(b"\r\n", b"\n")
            entry = ZipInfo(name, (1980, 1, 1, 0, 0, 0))
            entry.create_system = 3
            entry.external_attr = 0o100644 << 16
            bundle.writestr(entry, data)
    with ZipFile(archive) as bundle:
        if bundle.namelist() != names or bundle.testzip():
            raise ValueError("Archive members or CRCs differ from the validated package.")
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    output.joinpath("SHA256SUMS").write_bytes(f"{digest}  {archive.name}\n".encode("ascii"))
    print(f"{digest}  {archive.name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check")
    build = commands.add_parser("package")
    build.add_argument("tag")
    build.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        names = validate(ROOT)
        if args.command == "package":
            package(ROOT, names, args.tag, args.output)
    except (OSError, ValueError) as error:
        sys.exit(str(error))


if __name__ == "__main__":
    main()
