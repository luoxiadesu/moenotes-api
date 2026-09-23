import importlib.util
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("publish", Path(__file__).with_name("publish-release.py"))
publish = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publish)


class PublishTests(unittest.TestCase):
    def scenario(self, visibility="public", corrupt=False, prerelease=True):
        digests = {"amd64": "sha256:" + "a" * 64, "arm64": "sha256:" + "b" * 64}
        calls = []

        def run(*args):
            calls.append(args)
            if args[:2] == ("gh", "api"):
                return json.dumps({"visibility": visibility, "repository": {"full_name": "Example/Project"}})
            if "--raw" in args:
                return json.dumps({"manifests": [
                    {"platform": {"os": "linux", "architecture": arch},
                     "digest": "sha256:" + "0" * 64 if corrupt else digest}
                    for arch, digest in digests.items()]})
            if "--format" in args:
                return json.dumps({"digest": "sha256:" + "c" * 64})
            return ""

        info = {"version": "1.2.3", "tag": "v1.2.3", "prerelease": prerelease,
                "image": "ghcr.io/example/project", "notes": "Synthetic release"}
        env = {"GITHUB_REPOSITORY": "Example/Project", "RELEASE_TAG": "v1.2.3",
               "IMAGE": info["image"], "GITHUB_SHA": "d" * 40}
        old_cwd = Path.cwd()
        with tempfile.TemporaryDirectory() as directory:
            try:
                os.chdir(directory)
                Path("digests").mkdir()
                for arch, digest in digests.items():
                    Path(f"digests/{arch}.txt").write_text(digest)
                with patch.dict(os.environ, env, clear=True), patch.object(publish, "run", run), \
                        patch.object(publish, "metadata", return_value=info), \
                        patch.object(publish.subprocess, "run", return_value=SimpleNamespace(returncode=1)):
                    if visibility != "public" or corrupt:
                        with self.assertRaises(ValueError):
                            publish.main()
                        self.assertFalse(any(c[:3] == ("gh", "release", "create") for c in calls))
                    else:
                        publish.main()
                        result = json.loads(Path("image-digests.json").read_text())
                        self.assertEqual(result["platforms"], digests)
                        self.assertEqual(result["digest"], "sha256:" + "c" * 64)
                        self.assertIn(f"--prerelease={str(prerelease).lower()}", calls[-1])
                        self.assertIn("--latest=false", calls[-1])
            finally:
                os.chdir(old_cwd)

    def test_publish_prerelease(self):
        self.scenario()

    def test_publish_stable(self):
        self.scenario(prerelease=False)

    def test_reject_private_package(self):
        self.scenario(visibility="private")

    def test_reject_wrong_manifest(self):
        self.scenario(corrupt=True)


if __name__ == "__main__":
    unittest.main()
