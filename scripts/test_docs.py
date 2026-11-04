"""Regression checks for the document release boundary."""

from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from zipfile import ZipFile

from docs import DIAGRAMS, REQUIRED, package, validate


class DocsTests(unittest.TestCase):
    def setUp(self):
        temporary = TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        for name in REQUIRED:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            content = "# Test heading\n"
            if path.stem in DIAGRAMS:
                content += "\n```mermaid\nflowchart LR\n A --> B\n```\n"
            path.write_bytes(content.encode())

    def check(self):
        with redirect_stdout(StringIO()):
            return validate(self.root)

    def test_offline_bundle(self):
        readme = self.root / "README.md"
        readme.write_bytes(b"# Entry\n[Design](docs/ARCHITECTURE.md#test-heading)\n")
        (self.root / "private.txt").write_text("must stay local")
        names = self.check()
        out = self.root / "out"
        with redirect_stdout(StringIO()):
            package(self.root, names, "docs-v1.2.0", out)
            first = (out / "lora-agent-docs-v1.2.0.zip").read_bytes()
            readme.write_bytes(readme.read_bytes().replace(b"\n", b"\r\n"))
            package(self.root, names, "docs-v1.2.0", out)
        archive = out / "lora-agent-docs-v1.2.0.zip"
        self.assertEqual(first, archive.read_bytes())
        with ZipFile(archive) as bundle:
            self.assertNotIn("private.txt", bundle.namelist())
            unpacked = self.root / "unpacked"
            bundle.extractall(unpacked)
        with redirect_stdout(StringIO()):
            self.assertEqual(names, validate(unpacked))

    def test_required_file(self):
        (self.root / "docs/adr/0010-training-backend.md").unlink()
        with self.assertRaisesRegex(ValueError, "required file missing"):
            self.check()

    def test_mermaid(self):
        path = self.root / "docs/INFERENCE_ENGINE.md"
        path.write_text("# Engine\n```mermaid\nflowchart LR\n```\n")
        with self.assertRaisesRegex(ValueError, "required Mermaid diagram missing"):
            self.check()

    def test_broken_links(self):
        for target, error in [
            ("docs/no.md", "missing package target"),
            ("docs/ARCHITECTURE.md#nope", "missing heading"),
            ("../secrets", "non-portable link"),
        ]:
            with self.subTest(target=target):
                (self.root / "README.md").write_text(f"# Index\n[Read]({target})\n")
                with self.assertRaisesRegex(ValueError, error):
                    self.check()

    def test_invalid_version(self):
        for tag in ["v1.0.0", "docs-v01.0.0", "docs-v1.0.0/../../out", "docs-v1.0"]:
            with self.subTest(tag=tag), self.assertRaisesRegex(ValueError, "Expected docs-v"):
                package(self.root, [], tag, self.root / "out")
