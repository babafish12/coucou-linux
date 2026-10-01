#!/usr/bin/env python3
"""No desktop processes are signalled; installation tests use temporary homes."""

import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parent.parent
sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("launcher", ROOT / "scripts/coucou-launcher.py")
launcher = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(launcher)


class Executed(Exception):
    pass


class LauncherTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="coucou-launcher-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.install = self.root / "installed"
        self.install.mkdir()
        self.old = self.binary(self.install / "coucou", b"old")
        self.build = self.root / "repo with spaces ;$'" / "coucou"
        self.new = self.binary(self.build, b"new")
        self.config = {
            "version": 1, "mode": "link-build", "buildPath": str(self.build),
            "managedPaths": [str(self.install / "coucou"), str(self.build)],
            "knownBuilds": [self.old, self.new],
        }

    def binary(self, path, payload):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"\x7fELF" + payload)
        path.chmod(0o755)
        with path.open("rb") as stream:
            return launcher.fingerprint(stream.fileno())

    def invoke(self, processes, arguments=("--settings",)):
        launcher.save_config(self.install / "launcher.json", self.config)
        captured = []

        def execute(fd, argv, env):
            captured.append((launcher.fingerprint(fd), argv, env))
            raise Executed()

        with patch.object(launcher, "running_builds", return_value=processes), \
                patch.object(launcher, "stop_verified") as stop, \
                patch.object(launcher.os, "execve", side_effect=execute):
            with self.assertRaises(Executed):
                launcher.launch(self.install, list(arguments))
        return captured[0], stop

    def test_same_build_forwards_arguments_without_stopping(self):
        arguments = ("--settings", "--telegram", "a b;$(false)")
        result, stop = self.invoke([(42, (str(self.build), "1", 2, 3), self.new, -1)], arguments)
        self.assertEqual(result[0], self.new)
        self.assertEqual(result[1], [str(self.build), *arguments])
        stop.assert_not_called()

    def test_known_old_build_is_replaced_before_new_image_executes(self):
        identity = (str(self.install / "coucou"), "1", 2, 3)
        result, stop = self.invoke([(42, identity, self.old, -1)])
        stop.assert_called_once_with(42, identity)
        self.assertEqual(result[0], self.new)

    def test_explicit_local_build_wins_even_when_its_hash_was_seen_earlier(self):
        self.config["knownBuilds"] = [self.new, self.old]
        identity = (str(self.install / "coucou"), "1", 2, 3)
        result, stop = self.invoke([(42, identity, self.old, -1)])
        self.assertEqual(result[0], self.new)
        stop.assert_called_once_with(42, identity)

    def test_missing_local_build_keeps_a_newer_running_image(self):
        running_fd = os.open(self.build, os.O_RDONLY)
        self.addCleanup(os.close, running_fd)
        self.build.unlink()
        result, stop = self.invoke([(42, (str(self.build), "1", 2, 3), self.new, running_fd)])
        self.assertEqual(result[0], self.new)
        stop.assert_not_called()

    def test_reused_pid_is_never_signalled(self):
        with patch.object(launcher.os, "pidfd_open", return_value=100), \
                patch.object(launcher.os, "close"), \
                patch.object(launcher, "process_identity", return_value=("different", "2", 4, 5)), \
                patch.object(launcher.signal, "pidfd_send_signal") as send:
            with self.assertRaisesRegex(RuntimeError, "changed"):
                launcher.stop_verified(42, ("expected", "1", 2, 3))
            send.assert_not_called()

    def test_verified_signal_uses_pidfd_and_bounded_wait(self):
        identity = (str(self.build), "1", 2, 3)
        poller = Mock()
        poller.poll.return_value = [(100, 1)]
        with patch.object(launcher.os, "pidfd_open", return_value=100), \
                patch.object(launcher.os, "close"), \
                patch.object(launcher, "process_identity", return_value=identity), \
                patch.object(launcher.select, "poll", return_value=poller), \
                patch.object(launcher.signal, "pidfd_send_signal") as send:
            launcher.stop_verified(42, identity)
            send.assert_called_once_with(100, launcher.signal.SIGTERM)
            poller.poll.assert_called_once_with(8000)

    def fake_process(self, digest_path, uid=None):
        proc = self.root / "proc"
        process = proc / "42"
        process.mkdir(parents=True)
        uid = os.getuid() if uid is None else uid
        (process / "status").write_text(f"Uid:\t{uid}\t{uid}\t{uid}\t{uid}\n")
        (process / "comm").write_text("coucou\n")
        (process / "stat").write_text("42 (coucou) " + " ".join(["S"] + ["0"] * 18 + ["123"]))
        (process / "exe").symlink_to(digest_path)
        return proc

    def test_process_selection_checks_uid_path_and_known_digest(self):
        proc = self.fake_process(self.build)
        opened = []
        try:
            processes = launcher.running_builds(self.config, opened, proc)
            self.assertEqual([item[0] for item in processes], [42])
            self.config["knownBuilds"] = [self.old]
            with self.assertRaisesRegex(RuntimeError, "unrecognized"):
                launcher.running_builds(self.config, opened, proc)
            (proc / "42/status").write_text("Uid:\t999999\t999999\t999999\t999999\n")
            self.assertEqual(launcher.running_builds(self.config, opened, proc), [])
        finally:
            for fd in opened:
                os.close(fd)

    def test_group_writable_executable_and_symlink_config_are_rejected(self):
        self.build.chmod(0o775)
        with self.build.open("rb") as stream:
            with self.assertRaisesRegex(RuntimeError, "owned"):
                launcher.fingerprint(stream.fileno())
        config = self.root / "config"
        config.write_text(json.dumps(self.config))
        (self.install / "launcher.json").symlink_to(config)
        with self.assertRaises(RuntimeError):
            launcher.load_config(self.install / "launcher.json")

    def test_installer_preserves_link_mode_and_quotes_wrapper_paths(self):
        repo = self.root / "repo ' spaces $"
        (repo / "scripts").mkdir(parents=True)
        for name in ("install-linux.sh", "coucou-launcher.py"):
            shutil.copy2(ROOT / "scripts" / name, repo / "scripts" / name)
        build = repo / "linux/target/release/coucou"
        first = self.binary(build, b"first")
        icon = repo / "linux/src-tauri/icons/128x128.png"
        icon.parent.mkdir(parents=True)
        icon.write_bytes(b"test icon")
        home = self.root / "home ' spaces $"
        env = {**os.environ, "HOME": str(home), "XDG_DATA_HOME": str(home / "data"), "XDG_CONFIG_HOME": str(home / "config")}
        script = repo / "scripts/install-linux.sh"
        self.assertEqual(subprocess.run([script, "--help"], env=env, capture_output=True).returncode, 0)
        self.assertEqual(subprocess.run([script, "--unknown"], env=env, capture_output=True).returncode, 2)

        def install(*args):
            subprocess.run([script, "--skip-build", *args], env=env, check=True, capture_output=True)
            return json.loads((home / ".local/lib/coucou/launcher.json").read_text())

        self.assertEqual(install("--link-build")["mode"], "link-build")
        second = self.binary(build, b"second")
        config = install()
        self.assertEqual(config["mode"], "link-build")
        self.assertEqual(config["knownBuilds"], [first, second])
        self.assertEqual(install("--copy-build")["mode"], "copy-build")
        runtime = home / ".local/lib/coucou/launcher.py"
        runtime.write_text("import json,sys; print(json.dumps(sys.argv[1:]))\n")
        arguments = ["--settings", "a b;$(false)", "--telegram"]
        result = subprocess.run([home / ".local/bin/coucou", *arguments], env=env, check=True, capture_output=True, text=True)
        self.assertEqual(json.loads(result.stdout), arguments)


if __name__ == "__main__":
    unittest.main()
