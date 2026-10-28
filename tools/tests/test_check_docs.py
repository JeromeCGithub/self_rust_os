"""Regression tests for the repository documentation checker."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

MODULE = Path(__file__).resolve().parents[1] / "check_docs.py"
spec = importlib.util.spec_from_file_location("check_docs", MODULE)
check_docs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check_docs)


class DocumentationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def write(self, name, text):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        return path

    def test_valid_relative_file_and_heading_links(self):
        self.write("docs/guide.md", "# Boot & memory\n")
        page = self.write("README.md", "[Guide](docs/guide.md#boot--memory)\n")
        self.assertEqual(check_docs.check_markdown(self.root, [page]), [])

    def test_missing_file_reports_source_and_target(self):
        page = self.write("README.md", "[Guide](docs/missing.md)")
        errors = check_docs.check_markdown(self.root, [page])
        self.assertEqual(len(errors), 1)
        self.assertIn("README.md", errors[0])
        self.assertIn("docs/missing.md", errors[0])

    def test_missing_anchor_is_rejected(self):
        page = self.write("README.md", "# Start\n[Missing](#absent)")
        self.assertIn("absent", check_docs.check_markdown(self.root, [page])[0])

    def test_external_urls_and_fenced_examples_are_not_local_files(self):
        page = self.write("README.md", "[Web](https://example.com/a#b)\n"
                          "[Email](mailto:dev@example.com)\n"
                          "```md\n[Example](missing.md)\n```\n")
        self.assertEqual(check_docs.check_markdown(self.root, [page]), [])

    def test_encoded_paths_and_duplicate_headings(self):
        self.write("docs/a guide.md", "# Heading\n# Heading\n")
        page = self.write("README.md", "[Guide](docs/a%20guide.md#heading-1)")
        self.assertEqual(check_docs.check_markdown(self.root, [page]), [])

    def test_paths_outside_repository_are_rejected(self):
        page = self.write("README.md", "[Outside](../outside.md)")
        self.assertIn("outside repository", check_docs.check_markdown(self.root, [page])[0])

    def test_required_ai_entry_points_are_checked(self):
        errors = check_docs.check_repository(self.root)
        self.assertTrue(any("AGENTS.md" in error for error in errors))
        self.assertTrue(any("docs/testing.md" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
