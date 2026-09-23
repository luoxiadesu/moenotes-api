#!/usr/bin/env python3
"""Test a built container with synthetic config; no game endpoint is called."""
import io
import json
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.request
import uuid


def run(*args, **kwargs):
    return subprocess.check_output(args, **kwargs)


def main(image, version):
    assert run("docker", "run", "--rm", "--network", "none", image, "--version", text=True).strip() == f"moenotes-server {version}"
    assert run("docker", "image", "inspect", "--format", "{{.Config.User}}", image, text=True).strip() == "65532:65532"
    config = b'''listen = "0.0.0.0:8080"
api_key_file = "key"
[session]
region = "synthetic"
origin = "https://game.example.invalid"
allowed_origins = ["https://game.example.invalid"]
platform = "android"
client_version = "1.0.1"
'''
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode="w") as tar:
        directory = tarfile.TarInfo("etc/moenotes")
        directory.type, directory.mode = tarfile.DIRTYPE, 0o700
        directory.uid = directory.gid = 65532
        tar.addfile(directory)
        for name, data in {"config.toml": config, "key": b"synthetic-smoke-key-not-for-production-123456"}.items():
            entry = tarfile.TarInfo("etc/moenotes/" + name)
            entry.uid = entry.gid = 65532
            entry.size, entry.mode = len(data), 0o600
            tar.addfile(entry, io.BytesIO(data))
    name = "moenotes-smoke-" + uuid.uuid4().hex
    run("docker", "create", "--name", name, "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
        "-p", "127.0.0.1::8080", image)
    try:
        subprocess.run(["docker", "cp", "-a", "-", f"{name}:/"], input=archive.getvalue(), check=True)
        run("docker", "start", name)
        port = run("docker", "port", name, "8080/tcp", text=True).strip()
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        for attempt in range(50):
            try:
                with opener.open(f"http://{port}/healthz", timeout=1) as response:
                    assert json.load(response) == {"status": "ok"}
                break
            except (urllib.error.URLError, TimeoutError):
                if attempt == 49:
                    raise
                time.sleep(0.2)
        for path, expected in (("/openapi.json", 401), ("/experimental/v1/announcements/list", 404)):
            try:
                opener.open(f"http://{port}{path}", timeout=2)
                raise AssertionError("unexpected success")
            except urllib.error.HTTPError as error:
                assert error.code == expected
        run("docker", "exec", name, "moenotes-server", "check-config", "/etc/moenotes/config.toml")
        for path in ("LICENSE", "PROTOCOL-NOTICE.md"):
            run("docker", "exec", name, "test", "-s", f"/usr/share/doc/moenotes-api/{path}")
        run("docker", "stop", "--time", "5", name)
        assert run("docker", "inspect", "--format", "{{.State.ExitCode}}", name, text=True).strip() == "0"
    finally:
        subprocess.run(["docker", "rm", "--force", name], check=False, stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    main(*sys.argv[1:])
