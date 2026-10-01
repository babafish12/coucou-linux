#!/usr/bin/env python3
"""Launch the selected local build, replacing only verified older Coucou processes."""

import fcntl
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import stat
import sys
import tempfile


def owned_regular(metadata):
    return (stat.S_ISREG(metadata.st_mode) and metadata.st_uid == os.getuid()
            and not metadata.st_mode & 0o6022)


def fingerprint(fd):
    before = os.fstat(fd)
    if not owned_regular(before):
        raise RuntimeError("Coucou executable must be owned by you and not writable by other users.")
    os.lseek(fd, 0, os.SEEK_SET)
    if os.read(fd, 4) != b"\x7fELF":
        raise RuntimeError("The selected Coucou build is not a Linux executable.")
    os.lseek(fd, 0, os.SEEK_SET)
    digest = hashlib.sha256()
    while chunk := os.read(fd, 1024 * 1024):
        digest.update(chunk)
    after = os.fstat(fd)
    if (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (
            after.st_size, after.st_mtime_ns, after.st_ctime_ns):
        raise RuntimeError("Coucou changed while it was being checked. Wait for the build and retry.")
    return digest.hexdigest()


def executable_path(value):
    path = Path(value)
    if not path.is_absolute() or "\0" in value:
        raise RuntimeError("Coucou launcher contains an invalid executable path.")
    return path


def selected_binary(config, install_dir):
    if config["mode"] == "link-build":
        candidate = executable_path(config["buildPath"])
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate
    return install_dir / "coucou"


def load_config(path):
    metadata = path.lstat()
    if not owned_regular(metadata):
        raise RuntimeError("Coucou launcher settings must be a private, user-owned regular file.")
    config = json.loads(path.read_text())
    if config.get("version") != 1 or config.get("mode") not in ("link-build", "copy-build"):
        raise RuntimeError("Unsupported Coucou launcher settings. Run the installer again.")
    if not isinstance(config.get("knownBuilds"), list) or not all(
            isinstance(value, str) and len(value) == 64 and all(c in "0123456789abcdef" for c in value)
            for value in config["knownBuilds"]):
        raise RuntimeError("Invalid Coucou build identities. Run the installer again.")
    config["managedPaths"] = [str(executable_path(value)) for value in config["managedPaths"]]
    return config


def save_config(path, config):
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, prefix=".launcher-", delete=False) as stream:
            temporary = Path(stream.name)
            json.dump(config, stream, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def process_identity(pid, proc=Path("/proc")):
    directory = proc / str(pid)
    if directory.stat().st_uid != os.getuid():
        return None
    status = (directory / "status").read_text()
    uids = next((line.split()[1:] for line in status.splitlines() if line.startswith("Uid:")), [])
    if len(uids) != 4 or any(int(uid) != os.getuid() for uid in uids):
        return None
    if (directory / "comm").read_text().strip() != "coucou":
        return None
    raw_path = os.readlink(directory / "exe")
    path = raw_path.removesuffix(" (deleted)")
    # /proc/stat field 22 is starttime; comm may contain spaces and parentheses.
    started = (directory / "stat").read_text().rsplit(")", 1)[1].split()[19]
    metadata = (directory / "exe").stat()
    return path, started, metadata.st_dev, metadata.st_ino


def running_builds(config, opened, proc=Path("/proc")):
    allowed = set(config["managedPaths"])
    processes = []
    for directory in proc.iterdir():
        if not directory.name.isdigit() or int(directory.name) == os.getpid():
            continue
        pid = int(directory.name)
        try:
            identity = process_identity(pid, proc)
            if identity is None or identity[0] not in allowed:
                continue
            fd = os.open(directory / "exe", os.O_RDONLY | os.O_CLOEXEC)
            opened.append(fd)
            digest = fingerprint(fd)
            if process_identity(pid, proc) != identity:
                continue
            if digest not in config["knownBuilds"]:
                raise RuntimeError(f"Coucou PID {pid} uses an unrecognized build. Quit that instance before launching the new build.")
            processes.append((pid, identity, digest, fd))
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
    return processes


def stop_verified(pid, identity):
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        raise RuntimeError("Replacing a running build needs Python with pidfd support. Quit Coucou and retry.")
    try:
        handle = os.pidfd_open(pid)
    except ProcessLookupError:
        return
    try:
        try:
            current = process_identity(pid)
        except (FileNotFoundError, ProcessLookupError):
            return
        if current != identity:
            raise RuntimeError("The running Coucou process changed during launch. Retry.")
        try:
            signal.pidfd_send_signal(handle, signal.SIGTERM)
        except ProcessLookupError:
            return
        poller = select.poll()
        poller.register(handle, select.POLLIN)
        if not poller.poll(8_000):
            raise RuntimeError("The older Coucou build did not exit. Quit it before launching the new build.")
    finally:
        os.close(handle)


def launch(install_dir, arguments):
    config_path = install_dir / "launcher.json"
    lock = os.open(install_dir / "launcher.lock", os.O_RDWR | os.O_CREAT | os.O_CLOEXEC | os.O_NOFOLLOW, 0o600)
    opened = [lock]
    try:
        if not owned_regular(os.fstat(lock)):
            raise RuntimeError("Unsafe Coucou launcher lock file.")
        fcntl.flock(lock, fcntl.LOCK_EX)
        config = load_config(config_path)
        target = selected_binary(config, install_dir)
        fd = os.open(target, os.O_RDONLY | os.O_CLOEXEC)
        opened.append(fd)
        if not os.access(target, os.X_OK):
            raise RuntimeError(f"Coucou is not executable: {target}")
        digest = fingerprint(fd)
        path = str(target.resolve())
        if path not in config["managedPaths"]:
            config["managedPaths"].append(path)
        if digest not in config["knownBuilds"]:
            config["knownBuilds"].append(digest)
        save_config(config_path, config)
        processes = running_builds(config, opened)
        # If the linked build disappeared, do not downgrade a newer running build
        # to the installed fallback. Forward this invocation to that exact image.
        if config["mode"] == "link-build" and target == install_dir / "coucou":
            for _, identity, running_digest, running_fd in processes:
                if config["knownBuilds"].index(running_digest) > config["knownBuilds"].index(digest):
                    digest, fd, path = running_digest, running_fd, identity[0]
        for pid, identity, running_digest, _ in processes:
            if running_digest != digest:
                stop_verified(pid, identity)
        # Execute the checked inode, even if Cargo atomically replaces the path.
        os.execve(fd, [path, *arguments], os.environ.copy())
    finally:
        for fd in opened:
            os.close(fd)


if __name__ == "__main__":
    try:
        launch(Path(__file__).resolve().parent, sys.argv[1:])
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        print(f"Coucou launcher: {error}", file=sys.stderr)
        sys.exit(1)
