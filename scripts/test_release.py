import tempfile
import unittest
from pathlib import Path
from release import metadata


class ReleaseTests(unittest.TestCase):
    def test_real_workspace(self):
        info = metadata()
        self.assertEqual(metadata(tag=info["tag"]), info)
        self.assertEqual(info["prerelease"], "-" in info["version"])

    def fixture(self, path, version="1.2.3", locked=None, notes=True):
        (path / "Cargo.toml").write_text(f'[workspace.package]\nversion = "{version}"\n')
        (path / "Cargo.lock").write_text("\n".join(
            f'[[package]]\nname = "moenotes-{name}"\nversion = "{locked or version}"\n'
            for name in ("proto", "client", "server")))
        (path / "CHANGELOG.md").write_text(f"# Changelog\n\n## {version} - 2026-09-23\n\n- Synthetic notes\n" if notes else "# Changelog\n")

    def test_stable_and_prerelease(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            for version in ("0.1.0", "1.2.3-alpha.1", "1.2.3-beta.2", "1.2.3-rc.1"):
                self.fixture(path, version)
                info = metadata(path, "v" + version, "Example/Project")
                self.assertEqual(info["prerelease"], "-" in version)
                self.assertEqual(info["image"], "ghcr.io/example/project")

    def test_rejects_mismatch_bad_tag_and_missing_notes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            for version, locked, notes, tag in (
                ("01.2.3", None, True, ""), ("1.2.3-dev", None, True, ""),
                ("1.2.3", "1.2.2", True, ""), ("1.2.3", None, False, ""),
                ("1.2.3", None, True, "v1.2.4"), ("1.2.3", None, True, "--help"),
            ):
                self.fixture(path, version, locked, notes)
                with self.assertRaises(ValueError):
                    metadata(path, tag)


if __name__ == "__main__":
    unittest.main()
